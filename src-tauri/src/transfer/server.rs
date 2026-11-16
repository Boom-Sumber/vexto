//! lanflare-core 传输实现 + 服务生命周期 + 设备快照。
//!
//! **换传输实现只改这个文件**（ADR-010）：`backend.rs` 的 trait 是契约面，
//! 本文件是 lanflare-core 0.5 的实现体。全部 lanflare 依赖都关在这里，
//! 其余模块只看 `TransferBackend` 与 [`ServiceState`]。
//!
//! 会话 39（S4a）：绑定 53317（TCP+UDP，HTTPS + 持久化自签证书）、
//! 组播 announce、设备发现与快照。
//!
//! 会话 40（S4b）：接收确认（PrepareUpload → [`respond`]）、进度事件汇聚、
//! 落盘由 lanflare 的 `StreamingFileWriter` 完成（128 KB 分块、边写边算
//! SHA-256、同目录临时文件原子 rename、失败删 `.lanflare-part`），
//! 启动时清超过 24 小时的残留临时文件（任务书 §S4.10 坑 3）。

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use lanflare_core::config::AppConfig;
use lanflare_core::crypto::cert::{load_or_generate_identity, TlsIdentity};
use lanflare_core::discovery::interface::InterfaceFilter;
use lanflare_core::discovery::multicast::{
    start_multicast_engine, MulticastConfig, MulticastDiscoveryEvent, MulticastHandle, MulticastNodeInfo,
};
use lanflare_core::discovery::{DiscoveredPeer, PeerRegistry, PeerRegistryConfig};
use lanflare_core::server::{
    start_server, PrepareUploadDecision, ServerTransferEvent, SessionEndReason,
};
use lanflare_proto::discovery::{DeviceType, ProtocolType};
use tokio::sync::{mpsc, oneshot};

use super::backend::{
    ActiveFile, ActiveTransfer, IncomingFile, IncomingRequest, Peer, TransferBackend,
    TransferConfig, TransferHandle,
};
use super::events::{ProgressView, TransferEvent};
use crate::config;

/// 「刷新」时对单台设备做在线探测的超时。局域网内 TCP 握手正常在几十毫秒内，
/// 1.5 秒足够覆盖手机省电时的慢应答，又不至于让刷新卡太久（并发探测，不累加）。
const PROBE_TIMEOUT_MS: u64 = 1500;

/// 服务状态快照（给设置页状态行 / IPC 的同步读取）。
#[derive(Clone, Debug, PartialEq)]
pub enum ServiceState {
    /// 未在运行（模块停用，或从未启动过）。
    Stopped,
    /// 正在监听。
    Listening { port: u16 },
    /// 启动失败。原因要能直接展示给用户（端口被占等）。
    Failed(String),
}

/// 一条待确认的传入请求。`decision_tx` 一发就废（oneshot），所以只能 move 出去。
struct PendingIncoming {
    view: IncomingRequest,
    decision_tx: oneshot::Sender<PrepareUploadDecision>,
}

struct Globals {
    state: Mutex<ServiceState>,
    peers: Mutex<Vec<Peer>>,
    pending: Mutex<Vec<PendingIncoming>>,
    stop_tx: Mutex<Option<oneshot::Sender<()>>>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
    /// 服务线程的 tokio 句柄：发送任务（send.rs）借它 spawn。
    runtime: Mutex<Option<tokio::runtime::Handle>>,
    /// 自签身份：发送侧的 LsClient 要用同一套证书（fingerprint 一致）。
    identity: Mutex<Option<TlsIdentity>>,
    /// 发现侧句柄（组播 + 注册表）：`refresh_peers_now()` 用它主动广播、
    /// 探测在线状态并剔除已离线设备。服务停止时清空。
    discovery: Mutex<Option<Arc<DiscoveryCtx>>>,
    /// 会话管理器（vendored lanflare 补丁暴露）：接收侧取消要用。
    session_mgr: Mutex<Option<Arc<lanflare_core::server::SessionManager>>>,
    /// 进行中 / 刚结束的传输快照（传输页数据源）。事件驱动更新，最多留 4 个会话。
    active: Mutex<Vec<ActiveTransfer>>,
    /// 会话 → 对端别名（进度事件里没有对端信息，建会话时记一笔）。
    session_peer: Mutex<HashMap<String, String>>,
    /// 会话 → 发起方 IP（接收侧取消必须带对方 IP，lanflare 用它校验）。
    session_ip: Mutex<HashMap<String, std::net::IpAddr>>,
    /// 会话 → 发起方服务地址（ip, port, scheme, 证书指纹）：
    /// 接收方取消时要去**对方的** /cancel 端点喊一声，对方 UI 才会显示「接收方已取消」，
    /// 否则它只看到连接被掐断，界面报「已完成但发生错误」。
    session_target: Mutex<HashMap<String, (std::net::IpAddr, u16, String, String)>>,
    /// `"会话|file_id"` → 文件名：`FileFailed` 事件只带 file_id，不带名字，
    /// 不查这张表 UI 上会多出一行叫 `f1` 的幽灵条目。
    session_file_names: Mutex<HashMap<String, String>>,
    /// 会话 → 待发文件 (名字, 本机路径)：发送侧进度事件里只有文件名，
    /// 历史记录要「打开文件」就得知道源路径。
    session_files: Mutex<HashMap<String, Vec<(String, String)>>>,
}

/// 发现侧的两件东西，只在服务运行期间有效。
struct DiscoveryCtx {
    mhandle: Arc<MulticastHandle>,
    registry: Arc<PeerRegistry>,
}

static GLOBALS: OnceLock<Globals> = OnceLock::new();

/// 后端 → 前端的事件出口（main.rs 在 setup 里接上 `app.emit`）。
type EventSink = Arc<dyn Fn(TransferEvent) + Send + Sync>;
static EVENT_SINK: Mutex<Option<EventSink>> = Mutex::new(None);

fn globals() -> &'static Globals {
    GLOBALS.get_or_init(|| Globals {
        state: Mutex::new(ServiceState::Stopped),
        peers: Mutex::new(Vec::new()),
        pending: Mutex::new(Vec::new()),
        stop_tx: Mutex::new(None),
        thread: Mutex::new(None),
        runtime: Mutex::new(None),
        identity: Mutex::new(None),
        discovery: Mutex::new(None),
        session_mgr: Mutex::new(None),
        active: Mutex::new(Vec::new()),
        session_peer: Mutex::new(HashMap::new()),
        session_ip: Mutex::new(HashMap::new()),
        session_target: Mutex::new(HashMap::new()),
        session_file_names: Mutex::new(HashMap::new()),
        session_files: Mutex::new(HashMap::new()),
    })
}

/// Mutex 被 panic 毒化时仍取到数据：快照类状态不值得为此整个挂掉。
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

// ---------- 事件出口 ----------

/// 注册事件出口。main.rs 在 setup 里调一次（闭包里 `app.emit("transfer:event", …)`）。
pub fn set_event_sink(f: impl Fn(TransferEvent) + Send + Sync + 'static) {
    *lock(&EVENT_SINK) = Some(Arc::new(f));
}

/// 发事件。没人听就丢（与 pump 同策略，只记日志）。`send.rs` 也用它。
///
/// 同时维护 [`ActiveTransfer`] 快照：传输页中途打开时靠它还原进度，
/// 不必依赖「窗口开着时的那些事件」（风险 71 的同族问题）。
pub(crate) fn emit(evt: TransferEvent) {
    if let TransferEvent::Progress { view } = &evt {
        record_progress(view);
    }
    let sink = lock(&EVENT_SINK).clone();
    match sink {
        Some(f) => f(evt),
        None => crate::dlog::dlog("transfer: 事件被丢弃（前端未接入）"),
    }
}

// ---------- 传输快照（传输页数据源） ----------

/// 记「这个会话的对端是谁」：进度事件里只有 session_id，没有对端信息。
pub(crate) fn remember_session_peer(session_id: &str, alias: &str) {
    let mut m = lock(&globals().session_peer);
    m.insert(session_id.to_string(), alias.to_string());
    // 同步会话数上限，避免长跑程序把表撑大
    if m.len() > 32 {
        let stale: Vec<String> = m.keys().take(m.len() - 32).cloned().collect();
        for k in stale {
            m.remove(&k);
        }
    }
}

/// 记「这个会话要发哪些文件」：发送侧事件里只有文件名，历史要能打开源文件。
pub(crate) fn remember_session_files(session_id: &str, files: &[(String, String)]) {
    let mut m = lock(&globals().session_files);
    m.insert(session_id.to_string(), files.to_vec());
    if m.len() > 32 {
        let stale: Vec<String> = m.keys().take(m.len() - 32).cloned().collect();
        for k in stale {
            m.remove(&k);
        }
    }
}

fn record_progress(view: &ProgressView) {
    let peer = lock(&globals().session_peer)
        .get(&view.session_id)
        .cloned()
        .unwrap_or_default();

    let mut finished: Option<ActiveTransfer> = None;
    {
        let mut list = lock(&globals().active);
        let sid = view.session_id.clone();

        if !list.iter().any(|t| t.session_id == sid) {
            // 新会话：先把已结束的老会话清掉，页面只显示「本次」
            list.retain(|t| !matches!(t.status.as_str(), "done" | "failed" | "cancelled"));
            list.push(ActiveTransfer {
                session_id: sid.clone(),
                direction: view.direction.clone(),
                peer: peer.clone(),
                status: "active".into(),
                message: None,
                bytes: 0,
                total: 0,
                speed_bps: None,
                files: Vec::new(),
                recorded: false,
            });
            if list.len() > 4 {
                list.remove(0);
            }
        }
        let Some(t) = list.iter_mut().find(|t| t.session_id == sid) else {
            return;
        };
        if t.peer.is_empty() && !peer.is_empty() {
            t.peer = peer;
        }
        if t.direction.is_empty() {
            t.direction = view.direction.clone();
        }

        // 会话级事件：发送侧会用空 file_name 发 preparing / 终态，
        // 完成时也会发一条 `N 个文件` 的汇总。都不该变成「文件行」。
        let session_level = view.file_name.is_empty() || view.file_name.ends_with(" 个文件");
        if session_level {
            if !view.status.is_empty() {
                t.status = view.status.clone();
            }
            if view.message.is_some() {
                t.message = view.message.clone();
            }
            if view.speed_bps.is_some() {
                t.speed_bps = view.speed_bps;
            }
            if view.total > 0 {
                t.total = view.total;
                t.bytes = view.bytes;
            }
        } else {
            // 接收侧的「完成」事件把落盘路径放在 message 里，正好当 path 用
            let done_path = if view.status == "done" {
                view.message
                    .as_deref()
                    .filter(|m| m.contains('\\') || m.contains('/'))
                    .map(str::to_string)
            } else {
                None
            };
            let send_path = lock(&globals().session_files)
                .get(&sid)
                .and_then(|list| {
                    list.iter()
                        .find(|(n, _)| n == &view.file_name)
                        .map(|(_, p)| p.clone())
                });

            match t.files.iter_mut().find(|f| f.name == view.file_name) {
                Some(f) => {
                    f.bytes = view.bytes;
                    if view.total > 0 {
                        f.total = view.total;
                    }
                    if matches!(view.status.as_str(), "done" | "failed" | "cancelled") {
                        f.status = view.status.clone();
                    }
                    if let Some(p) = done_path.or_else(|| send_path.clone()) {
                        if f.path.is_empty() {
                            f.path = p;
                        }
                    }
                }
                None => t.files.push(ActiveFile {
                    name: view.file_name.clone(),
                    bytes: view.bytes,
                    total: view.total,
                    status: if view.status == "done" {
                        "active".into()
                    } else {
                        view.status.clone()
                    },
                    path: done_path.or(send_path).unwrap_or_default(),
                }),
            }

            if let Some(p) = &view.message {
                if t.message.is_none() {
                    t.message = Some(p.clone());
                }
            }
            if view.speed_bps.is_some() {
                t.speed_bps = view.speed_bps;
            }

            // 会话内所有文件都完成了 → 会话也算完成（接收侧没有会话级终态事件）
            if !t.files.is_empty() && t.files.iter().all(|f| f.status == "done") {
                t.status = "done".into();
            }
            t.total = t.files.iter().map(|f| f.total).sum();
            t.bytes = t.files.iter().map(|f| f.bytes).sum();
        }

        // 终态：落一条历史（每个会话只落一次）
        if matches!(t.status.as_str(), "done" | "failed" | "cancelled") && !t.recorded {
            t.recorded = true;
            finished = Some(t.clone());
        }
    }

    if let Some(t) = finished {
        super::history::append(super::history::HistoryEntry {
            id: format!("{}-{}", t.session_id, super::history::now_ms()),
            session_id: t.session_id.clone(),
            direction: t.direction.clone(),
            peer: t.peer.clone(),
            status: t.status.clone(),
            message: t.message.clone(),
            total_bytes: t.total,
            at_ms: super::history::now_ms(),
            files: t
                .files
                .iter()
                .map(|f| super::history::HistoryFile {
                    name: f.name.clone(),
                    size: f.total.max(f.bytes),
                    path: f.path.clone(),
                })
                .collect(),
        });
    }
}

/// 通知对端「我们这边取消了」（对方收到才会显示「接收方已取消」）。
async fn notify_peer_cancel(
    session_id: &str,
    peer_ip: std::net::IpAddr,
    port: u16,
    scheme: &str,
    fingerprint: &str,
) {
    use lanflare_core::client::{LsClient, LsClientConfig};

    let pinned = if fingerprint.trim().is_empty() {
        None
    } else {
        Some(fingerprint.to_string())
    };
    let cfg = LsClientConfig {
        timeout: std::time::Duration::from_secs(8),
        connect_timeout: std::time::Duration::from_secs(3),
        tls_identity: None,
        target_fingerprint: pinned.clone(),
    };
    let client = match LsClient::new(cfg) {
        Ok(c) => c,
        Err(e) => {
            crate::dlog::dlog(&format!("transfer: 通知对端取消失败（建客户端）：{e}"));
            return;
        }
    };
    let base = format!("{scheme}://{peer_ip}:{port}");
    match client.cancel_session(&base, session_id).await {
        Ok(()) => crate::dlog::dlog(&format!("transfer: 已通知 {base} 取消 {session_id}")),
        Err(e) => crate::dlog::dlog(&format!("transfer: 通知 {base} 取消失败：{e}")),
    }
}

/// 当前 / 刚结束的传输快照（传输页开场拉一次，之后靠事件增量更新）。
pub fn active_transfers() -> Vec<ActiveTransfer> {
    lock(&globals().active).clone()
}

/// 取消一次**接收中**的会话（发送侧取消走 `send::cancel_send`）。
///
/// lanflare 的 `cancel_session` 要求同时给出 session_id 与发起方 IP
/// （`ServerSession` 用它校验「是不是同一台设备」），所以建会话时要把 IP 记下来。
pub fn cancel_receive(session_id: &str) -> Result<(), String> {
    let mgr = lock(&globals().session_mgr)
        .clone()
        .ok_or("传输服务未就绪")?;
    let ip = lock(&globals().session_ip)
        .get(session_id)
        .copied()
        .ok_or("找不到该会话的发起方，无法取消")?;
    let rt = runtime_handle().ok_or("请先启用局域网互传")?;
    let sid = session_id.to_string();
    let target = lock(&globals().session_target).get(session_id).cloned();
    rt.spawn(async move {
        let hit = mgr.cancel_session(Some(&sid), ip).await;
        match hit {
            Some(reason) => {
                crate::dlog::dlog(&format!("transfer: 本机取消接收 {sid}，lanflare 返回 {reason:?}"));
                // 会话级终态自己发一条：传输页立刻从「传输中」翻成「已取消」，
                // 不用等 lanflare 的 FileFailed（那条是文件级的，措辞也不同）。
                emit(TransferEvent::Progress {
                    view: ProgressView {
                        session_id: sid.clone(),
                        direction: "recv".into(),
                        file_name: String::new(),
                        bytes: 0,
                        total: 0,
                        speed_bps: None,
                        status: "cancelled".into(),
                        message: Some("已在本机取消".to_string()),
                    },
                });
                // 关键一步：去**对方**的 /cancel 端点喊一声。LocalSend 的接收方
                // 取消就是这么做的；不喊的话对方只看到连接被掐断，界面报
                // 「已完成，但发生错误」，而不是「接收方已取消」。
                if let Some((peer_ip, port, scheme, fp)) = target {
                    notify_peer_cancel(&sid, peer_ip, port, &scheme, &fp).await;
                }
            }
            None => crate::dlog::dlog(&format!("transfer: 取消接收 {sid} 未命中（会话已结束？）")),
        }
    });
    Ok(())
}

// ---------- 对外接口（tools/transfer.rs、main.rs、send.rs 用） ----------

/// 当前服务状态。
pub fn state() -> ServiceState {
    lock(&globals().state).clone()
}

/// 当前发现的设备快照。
pub fn peers() -> Vec<Peer> {
    lock(&globals().peers).clone()
}

/// 主动刷新设备列表（互传页「刷新」按钮）。
///
/// 与旧行为（只重读本地快照、一个网络包都不发）不同，这里做两件事：
///
/// 1. 发一轮 announce burst —— 对端收到后会反向 register，**新上线的设备**才会出现；
/// 2. 对已发现的每台设备做一次 TCP 连接探测（1.5 秒超时），连不上的直接从注册表
///    移除。这样对方关掉 LocalSend / 离开 WiFi 时点一下刷新就消失，不必干等
///    `transfer_peer_timeout_secs`（默认 90 秒）自然过期。
///
/// 异步执行、立即返回：结果通过既有的 `peers` 事件推给前端。
pub fn refresh_peers_now() -> Result<(), String> {
    let rt = runtime_handle().ok_or("请先启用局域网互传")?;
    let ctx = lock(&globals().discovery)
        .clone()
        .ok_or("传输服务未就绪，请稍后重试")?;

    rt.spawn(async move {
        ctx.mhandle.announce_burst().await;

        let current = ctx.registry.get_all().await;
        let mut set = tokio::task::JoinSet::new();
        for p in current {
            let Some(ip) = p.primary_ip() else { continue };
            let addr = std::net::SocketAddr::new(ip, p.port);
            let fp = p.fingerprint.clone();
            set.spawn(async move {
                let alive = tokio::time::timeout(
                    Duration::from_millis(PROBE_TIMEOUT_MS),
                    tokio::net::TcpStream::connect(addr),
                )
                .await
                .map(|r| r.is_ok())
                .unwrap_or(false);
                (fp, alive)
            });
        }

        let mut gone = 0usize;
        while let Some(joined) = set.join_next().await {
            let Ok((fp, alive)) = joined else { continue };
            if !alive {
                ctx.registry.remove(&fp).await;
                gone += 1;
            }
        }
        if gone > 0 {
            crate::dlog::dlog(&format!("transfer: 刷新剔除 {gone} 台已离线设备"));
        }
        refresh_peers(&ctx.registry).await;
    });

    Ok(())
}

/// 当前待确认的传入请求（最旧在前）。
///
/// 先剔掉「对端已经撤销」的条目：对方点取消时 `prepare-upload` 那侧会丢掉
/// 等待应答的 `oneshot` 接收端，发送端 `is_closed()` 随即为 true。
/// 不剔的话确认框会一直挂着一条早就没人等的请求（会话 80 用户反馈）。
pub fn pending_list() -> Vec<IncomingRequest> {
    let mut pending = lock(&globals().pending);
    let before = pending.len();
    pending.retain(|p| !p.decision_tx.is_closed());
    if pending.len() != before {
        crate::dlog::dlog(&format!(
            "transfer: 清理 {} 条已撤销的待确认请求",
            before - pending.len()
        ));
    }
    pending.iter().map(|p| p.view.clone()).collect()
}

/// 接受 / 拒绝一条待确认请求（S4.7.2 确认框的两个按钮）。
///
/// 接受 = 全部文件放行（对方一次发的是一组）；拒绝 = 403，对方收到「用户拒绝」。
/// 找不到 session_id（已过期 / 已处理）返回 Err，前端提示即可。
pub fn respond(session_id: &str, accept: bool) -> Result<(), String> {
    let mut pending = lock(&globals().pending);
    let Some(idx) = pending.iter().position(|p| p.view.session_id == session_id) else {
        return Err("该传输请求已过期或已处理".to_string());
    };
    let item = pending.remove(idx);
    drop(pending);
    let decision = if accept {
        PrepareUploadDecision::Accept(item.view.files.iter().map(|f| f.id.clone()).collect())
    } else {
        PrepareUploadDecision::Decline
    };
    let sender = item.view.sender_alias.clone();
    if item.decision_tx.send(decision).is_err() {
        crate::dlog::dlog("transfer: 应答失败（对端已断开）");
        return Err("对方已取消传输".to_string());
    }
    let msg = if accept {
        format!("已接受 {sender} 的传输")
    } else {
        format!("已拒绝 {sender} 的传输")
    };
    crate::dlog::dlog(&format!("transfer: {msg}"));
    emit(TransferEvent::Notice { message: msg });
    Ok(())
}

/// 服务线程的 tokio 句柄（发送任务用）。未启动时 None。
pub(crate) fn runtime_handle() -> Option<tokio::runtime::Handle> {
    lock(&globals().runtime).clone()
}

/// 自签身份（发送侧 LsClient 用同一套证书）。
pub(crate) fn identity() -> Option<TlsIdentity> {
    lock(&globals().identity).clone()
}

/// 启动传输服务。已在运行时幂等返回 Ok。
///
/// **同步等到绑定结果**（最多 10 秒）：`set_enabled(true)` 必须能把
/// 「端口被占」这类失败原因报给用户（S2 契约），不能点完开关才悄悄挂掉。
pub fn start_service(cfg: &TransferConfig) -> Result<(), String> {
    let g = globals();
    {
        let th = lock(&g.thread);
        if th.is_some() {
            return Ok(());
        }
    }

    let cfg = cfg.clone();
    let (tx, rx) = std::sync::mpsc::channel::<Result<u16, String>>();
    let (stop_tx, stop_rx) = oneshot::channel::<()>();

    let handle = std::thread::Builder::new()
        .name("vexto-transfer".into())
        .spawn(move || service_thread(cfg, tx, stop_rx))
        .map_err(|_| "传输服务线程启动失败，请重试".to_string())?;

    let started = match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(res) => res,
        Err(_) => Err("启动超时（10 秒内没有响应），请重试".to_string()),
    };

    match started {
        Ok(port) => {
            *lock(&g.stop_tx) = Some(stop_tx);
            *lock(&g.thread) = Some(handle);
            *lock(&g.state) = ServiceState::Listening { port };
            Ok(())
        }
        Err(e) => {
            // 线程会在回报错误后自行退出；stop_tx 落地即断开，让它收干净。
            drop(stop_tx);
            let _ = handle.join();
            *lock(&g.state) = ServiceState::Failed(e.clone());
            Err(e)
        }
    }
}

/// 停止传输服务。幂等；最多等 5 秒收尾，超时记日志放弃等待。
pub fn stop_service() -> Result<(), String> {
    let g = globals();
    let handle = lock(&g.thread).take();
    // 未决请求先全部拒绝：服务都没了，不许把对方晾在 409/超时上。
    let pending: Vec<PendingIncoming> = lock(&g.pending).drain(..).collect();
    for p in pending {
        let _ = p.decision_tx.send(PrepareUploadDecision::Decline);
    }
    let Some(handle) = handle else {
        *lock(&g.state) = ServiceState::Stopped;
        return Ok(());
    };
    if let Some(tx) = lock(&g.stop_tx).take() {
        // 对端已退出时 send 会失败——线程都没了，没有用户可见的错误要展示。
        let _ = tx.send(());
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while !handle.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    if handle.is_finished() {
        let _ = handle.join();
    } else {
        crate::dlog::dlog("transfer: 停止超时（5 秒），放弃等待线程收尾");
    }
    *lock(&g.state) = ServiceState::Stopped;
    *lock(&g.runtime) = None;
    *lock(&g.identity) = None;
    lock(&g.peers).clear();
    Ok(())
}

/// ADR-010 的兑换点：换自研实现时只改 [`LanflareBackend`] 的实现体。
/// 调用方统一从 `transfer::backend()` 拿 trait 对象，不直接依赖本文件。
pub struct LanflareBackend;

/// 全局唯一实现。换实现时换成别的 unit struct 即可，调用方不动。
pub static BACKEND: LanflareBackend = LanflareBackend;

impl TransferBackend for LanflareBackend {
    fn start(&self, cfg: &TransferConfig) -> Result<(), String> {
        start_service(cfg)
    }

    fn stop(&self) -> Result<(), String> {
        stop_service()
    }

    fn peers(&self) -> Vec<Peer> {
        peers()
    }

    fn refresh(&self) -> Result<(), String> {
        refresh_peers_now()
    }

    fn send(
        &self,
        peer_id: &str,
        files: &[PathBuf],
        pin: Option<String>,
    ) -> Result<TransferHandle, String> {
        super::send::send_files(peer_id, files, pin)
    }

    fn pending_requests(&self) -> Vec<IncomingRequest> {
        pending_list()
    }

    fn respond(&self, session_id: &str, accept: bool) -> Result<(), String> {
        respond(session_id, accept)
    }

    fn active_transfers(&self) -> Vec<ActiveTransfer> {
        active_transfers()
    }

    fn cancel_receive(&self, session_id: &str) -> Result<(), String> {
        cancel_receive(session_id)
    }
}

// ---------- 配置解析 ----------

/// 接收目录：配置里留空 → 系统「下载」下的 `Vexto` 子目录（设置页文案同款，ADR-038）。
///
/// 不能直接拼 `%USERPROFILE%\Downloads`（可能被重定向），复用下载模块
/// `SHGetKnownFolderPath` 的实现（不改那边代码，只调它的 pub fn）。
pub fn resolve_receive_dir(cfg: &config::Config) -> PathBuf {
    let raw = cfg.receive_dir.trim();
    if raw.is_empty() {
        return PathBuf::from(crate::download::engine::default_download_dir()).join("Vexto");
    }
    // 与 download::resolve_dir 同口径：剔换行、正斜杠归一成反斜杠。
    PathBuf::from(raw.replace(['\r', '\n'], "").replace('/', "\\"))
}

/// 持久化 TLS 身份（自签证书 + 私钥 + 指纹）。
///
/// 存 `%APPDATA%\vexto\transfer-identity.json`（ADR-038），**不进 config.json**：
/// 证书体是运行数据不是用户设置。fingerprint = 证书 SHA-256，
/// 正是 LocalSend 协议「HTTPS 模式指纹」的定义（§S4.3.2）。
fn identity_path() -> PathBuf {
    config::log_dir().join("transfer-identity.json")
}

/// announce 里的设备类型串。未知 / 缺失按 `desktop`（协议 §7.1 兜底）。
fn device_type_str(dt: Option<DeviceType>) -> &'static str {
    match dt {
        Some(DeviceType::Mobile) => "mobile",
        Some(DeviceType::Desktop) => "desktop",
        Some(DeviceType::Web) => "web",
        Some(DeviceType::Headless) => "headless",
        Some(DeviceType::Server) => "server",
        None => "desktop",
    }
}

/// 配置里的设备类型串 → 协议枚举。未知值按 `desktop` 兜底。
pub(crate) fn parse_device_type(s: &str) -> DeviceType {
    match s.trim().to_ascii_lowercase().as_str() {
        "mobile" => DeviceType::Mobile,
        "web" => DeviceType::Web,
        "headless" => DeviceType::Headless,
        "server" => DeviceType::Server,
        _ => DeviceType::Desktop,
    }
}

/// `DiscoveredPeer` → 快照 `Peer`（IPv4 优先的主 IP）。
fn map_peer(p: &DiscoveredPeer) -> Peer {
    Peer {
        fingerprint: p.fingerprint.clone(),
        alias: p.alias.clone(),
        ip: p
            .primary_ip()
            .map(|i| i.to_string())
            .unwrap_or_default(),
        port: p.port,
        device_model: p.device_model.clone().unwrap_or_default(),
        device_type: device_type_str(p.device_type).to_string(),
        version: p.version.clone(),
        protocol: p.protocol.as_str().to_string(),
    }
}

// ---------- 服务线程 ----------

/// 传输服务线程体：自建 tokio 运行时，不占用 Tauri 的 async_runtime。
///
/// 生命周期：建运行时 → 绑 TCP（失败立刻回报）→ 绑组播（失败回报）→
/// 事件循环直到 stop 信号 → 组播/HTTP/发现逐级收干净 → 状态回 Stopped。
fn service_thread(
    cfg: TransferConfig,
    result_tx: std::sync::mpsc::Sender<Result<u16, String>>,
    stop_rx: oneshot::Receiver<()>,
) {
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            let msg = format!("传输 tokio 运行时创建失败：{e}");
            let _ = result_tx.send(Err(msg));
            return;
        }
    };
    // 发送任务（send.rs）借这个句柄 spawn，不另起运行时。
    *lock(&globals().runtime) = Some(rt.handle().clone());
    rt.block_on(service_main(cfg, result_tx, stop_rx));
}

/// 启动时清理接收目录里超过 24 小时的临时文件（任务书 §S4.10 坑 3）。
/// 崩溃 / 强杀会留 `*.part` / `*.lanflare-part`，不清理会越积越多。
fn cleanup_stale_parts(dir: &std::path::Path) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let day = Duration::from_secs(24 * 3600);
    let now = SystemTime::now();
    let mut removed = 0u32;
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let is_part = name.ends_with(".part") || name.ends_with(".lanflare-part");
        if !is_part {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age > day);
        if stale && std::fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    if removed > 0 {
        crate::dlog::dlog(&format!("transfer: 清理 {removed} 个超过 24 小时的临时文件"));
    }
}

async fn service_main(
    cfg: TransferConfig,
    result_tx: std::sync::mpsc::Sender<Result<u16, String>>,
    mut stop_rx: oneshot::Receiver<()>,
) {
    // 组播地址先校验：配错了宁可启动失败，也不要用默认值悄悄兜底。
    let group: Ipv4Addr = match cfg.multicast.parse() {
        Ok(g) => g,
        Err(_) => {
            let _ = result_tx.send(Err(format!("组播地址不合法：{}", cfg.multicast)));
            return;
        }
    };

    let registry = PeerRegistry::with_config(PeerRegistryConfig {
        peer_timeout: Duration::from_secs(cfg.peer_timeout_secs),
        ..Default::default()
    });

    // 证书始终生成/加载：即使服务走 HTTP，发送侧连接 HTTPS 对端仍需要它。
    let identity = match load_or_generate_identity(&identity_path()) {
        Ok(id) => id,
        Err(e) => {
            let _ = result_tx.send(Err(format!("生成传输证书失败：{e}")));
            return;
        }
    };
    *lock(&globals().identity) = Some(identity.clone());

    let app_cfg = AppConfig {
        alias: cfg.alias.clone(),
        port: cfg.port,
        download_dir: cfg.receive_dir.clone(),
        // 未开 PIN 时传 None；开了由 lanflare 的 PinTracker 在 prepare-upload 校验
        // （401 在事件之前返回，确认框只处理过了 PIN 的请求）。
        pin: if cfg.require_pin && !cfg.pin.is_empty() {
            Some(cfg.pin.clone())
        } else {
            None
        },
        auto_accept: cfg.auto_accept,
        multicast_address: cfg.multicast.clone(),
        ..Default::default()
    };

    // 落盘目录建好 + 清残留临时文件（坑 3）。
    let _ = std::fs::create_dir_all(&cfg.receive_dir);
    cleanup_stale_parts(&cfg.receive_dir);

    let (stx, mut srx) = mpsc::channel::<ServerTransferEvent>(256);
    let tls_identity = if cfg.use_https {
        Some(identity.clone())
    } else {
        None
    };
    let server = match start_server(app_cfg, tls_identity, registry.clone(), Some(stx)).await {
        Ok(s) => s,
        Err(e) => {
            // 用户要求：通知里不出现错误码，只说原因（会话 52）。
            // 端口被占是最常见的失败；其余异常也给一句能懂的话，细节在日志里。
            let looks_like_port = ["占用", "already in use", "os error 10048", "os error 10013"]
                .iter()
                .any(|k| e.to_string().to_lowercase().contains(k));
            let msg = if looks_like_port {
                format!(
                    "无法启动传输服务（端口 {} 可能被占用）。请释放端口或改用其他端口后，在「高级 → 维护」里重启传输服务",
                    cfg.port
                )
            } else {
                "无法启动传输服务（服务异常退出）。请在「高级 → 维护」里重启传输服务；反复失败可在日志中查看原因".to_string()
            };
            let _ = result_tx.send(Err(msg));
            return;
        }
    };
    let port = server.bound_port();
    // 拿一份会话管理器：接收侧取消要用（vendored 补丁暴露的 getter）。
    *lock(&globals().session_mgr) = Some(server.session_manager().clone());

    let whitelist: Vec<String> = cfg
        .interface_whitelist
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let interface_filter = if whitelist.is_empty() {
        InterfaceFilter::default()
    } else {
        InterfaceFilter::new().with_whitelist(whitelist)
    };

    let node_info = MulticastNodeInfo {
        alias: cfg.alias.clone(),
        // 任务书 §S4.3.2 的示例值。与 /info 应答（lanflare 内部写死 "2.0"）
        // 不一致是已知偏差（RISKS 91.），官方 LocalSend 不按 version 串做兼容判断。
        version: "2.2".to_string(),
        device_model: Some(cfg.device_model.clone()),
        device_type: Some(parse_device_type(&cfg.device_type)),
        fingerprint: if cfg.use_https {
            identity.fingerprint.clone()
        } else {
            // HTTP 模式没有证书指纹；给一个每进程稳定的串，只用于「别把自己当设备」。
            format!("lightbox-{}-{}", std::process::id(), port)
        },
        port,
        protocol: if cfg.use_https {
            ProtocolType::Https
        } else {
            ProtocolType::Http
        },
        download: false,
    };
    let mcfg = MulticastConfig {
        group_v4: group,
        port: cfg.port,
        interface_filter,
        node_info,
        // 无物理网卡时允许回环兜底：开发机 / 虚拟网环境下也能自测发现。
        allow_loopback_fallback: true,
        ..Default::default()
    };
    let (mtx, mut mrx) = mpsc::channel::<MulticastDiscoveryEvent>(64);
    // 持有 sender 不发：引擎停止统一走 MulticastHandle::stop() 的取消令牌。
    let (_mstop_keep, mstop_rx) = oneshot::channel::<()>();
    let mhandle = match start_multicast_engine(mcfg, mtx, mstop_rx).await {
        Ok(h) => std::sync::Arc::new(h),
        Err(e) => {
            server.stop();
            let _ = result_tx.send(Err(format!("组播监听失败（无可用网卡？）：{e}")));
            return;
        }
    };

    if result_tx.send(Ok(port)).is_err() {
        // 启动方已超时走人：整个服务没有存在意义，直接收干净。
        mhandle.stop().await;
        server.stop();
        return;
    }

    // 把发现侧句柄挂到全局：`refresh_peers_now()` 要用它主动广播 / 探测。
    *lock(&globals().discovery) = Some(Arc::new(DiscoveryCtx {
        mhandle: mhandle.clone(),
        registry: registry.clone(),
    }));

    // announce burst（100ms/500ms/2000ms，各带抖动）后台发，不阻塞事件循环。
    {
        let h = mhandle.clone();
        tokio::spawn(async move {
            h.announce_burst().await;
        });
    }

    let mut snapshot_rx = registry.subscribe();
    refresh_peers(&registry).await;
    let scheme = if cfg.use_https { "https" } else { "http" };
    crate::dlog::dlog(&format!(
        "transfer: 服务已启动 {scheme} 0.0.0.0:{port}，组播 {}:{}",
        cfg.multicast, cfg.port
    ));

    loop {
        tokio::select! {
            ev = mrx.recv() => match ev {
                Some(MulticastDiscoveryEvent::PeerDiscovered { remote_ip, scope_id, message }) => {
                    crate::dlog::dlog(&format!(
                        "transfer: 组播发现 {} ({remote_ip})",
                        message.alias
                    ));
                    // 入库后由 RegistryEvent 驱动快照刷新，这里不用重复刷。
                    registry.handle_multicast(remote_ip, scope_id, &message).await;
                }
                Some(MulticastDiscoveryEvent::AllSocketsFailed) => {
                    crate::dlog::dlog("transfer: 全部组播 socket 失效，设备发现已停（HTTP 服务仍在）");
                }
                None => break,
            },
            ev = srx.recv() => match ev {
                Some(ServerTransferEvent::DeviceRegistered { ip, info }) => {
                    crate::dlog::dlog(&format!(
                        "transfer: 设备注册 {} ({ip})",
                        info.alias
                    ));
                }
                Some(ServerTransferEvent::PrepareUpload {
                    session_id,
                    sender_ip,
                    sender_info,
                    files,
                    decision_tx,
                    ..
                }) => {
                    on_prepare_upload(session_id, sender_ip, sender_info, files, decision_tx);
                }
                Some(other) => on_transfer_progress(other),
                None => break,
            },
            ev = snapshot_rx.recv() => match ev {
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    // PeerAdded/Updated/Expired/Removed 都刷一次；滞后丢事件也刷（全量读）。
                    refresh_peers(&registry).await;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            },
            _ = &mut stop_rx => break,
        }
    }

    crate::dlog::dlog("transfer: 服务停止中");
    // 未决请求全部拒绝（stop_service 已经清过一遍，这里是兜底）。
    let leftover: Vec<PendingIncoming> = lock(&globals().pending).drain(..).collect();
    for p in leftover {
        let _ = p.decision_tx.send(PrepareUploadDecision::Decline);
    }
    mhandle.stop().await;
    server.stop();
    registry.shutdown();
    *lock(&globals().discovery) = None;
    *lock(&globals().session_mgr) = None;
    lock(&globals().active).clear();
    lock(&globals().session_peer).clear();
    lock(&globals().session_ip).clear();
    lock(&globals().session_target).clear();
    lock(&globals().session_file_names).clear();
    lock(&globals().session_files).clear();
    *lock(&globals().state) = ServiceState::Stopped;
    crate::dlog::dlog("transfer: 服务已停止");
}

/// prepare-upload 到达：登记进确认队列并弹确认（S4a 的统一拒绝在此退役）。
fn on_prepare_upload(
    session_id: String,
    sender_ip: std::net::IpAddr,
    sender_info: lanflare_proto::discovery::RegisterDtoV2,
    files: HashMap<String, lanflare_proto::transfer::FileDto>,
    decision_tx: oneshot::Sender<PrepareUploadDecision>,
) {
    let mut views: Vec<IncomingFile> = files
        .values()
        .map(|f| IncomingFile {
            id: f.id.clone(),
            name: super::sanitize::sanitize_filename(&f.file_name),
            size: f.size,
        })
        .collect();
    views.sort_by(|a, b| a.name.cmp(&b.name));
    let total_bytes = views.iter().map(|f| f.size).sum();
    let view = IncomingRequest {
        session_id: session_id.clone(),
        sender_alias: sender_info.alias.clone(),
        sender_ip: sender_ip.to_string(),
        sender_model: sender_info.device_model.clone().unwrap_or_default(),
        sender_type: device_type_str(sender_info.device_type).to_string(),
        total_bytes,
        files: views,
    };
    crate::dlog::dlog(&format!(
        "transfer: 收到 {session_id} 来自 {} ({sender_ip}) 的传输请求（{} 个文件）",
        sender_info.alias,
        view.files.len()
    ));
    lock(&globals().pending).push(PendingIncoming {
        view: view.clone(),
        decision_tx,
    });
    // 进度事件只有 session_id，对端别名与 IP 在建会话时记一笔：
    // 前者给传输页显示「谁发来的」，后者是接收侧取消的必填参数。
    remember_session_peer(&session_id, &view.sender_alias);
    if let Ok(ip) = view.sender_ip.parse::<std::net::IpAddr>() {
        lock(&globals().session_ip).insert(session_id.clone(), ip);
        lock(&globals().session_target).insert(
            session_id.clone(),
            (
                ip,
                sender_info.port,
                sender_info.protocol.as_str().to_string(),
                sender_info.fingerprint.clone(),
            ),
        );
    }
    emit(TransferEvent::Incoming { req: view });
}

/// 收方向的进度 / 终态 → 前端事件。
fn on_transfer_progress(ev: ServerTransferEvent) {
    match ev {
        ServerTransferEvent::FileStarted {
            session_id,
            file_id,
            file_name,
            total_bytes,
            ..
        } => {
            // 记下 file_id → 文件名，供后面的 FileFailed 反查（那条事件只有 id）
            lock(&globals().session_file_names)
                .insert(format!("{session_id}|{file_id}"), file_name.clone());
            emit(TransferEvent::Progress {
                view: ProgressView {
                    session_id,
                    direction: "recv".into(),
                    file_name,
                    bytes: 0,
                    total: total_bytes,
                    speed_bps: None,
                    status: "active".into(),
                    message: None,
                },
            });
        }
        ServerTransferEvent::FileProgress {
            session_id,
            file_name,
            bytes_received,
            total_bytes,
            ..
        } => {
            emit(TransferEvent::Progress {
                view: ProgressView {
                    session_id,
                    direction: "recv".into(),
                    file_name,
                    bytes: bytes_received,
                    total: total_bytes,
                    speed_bps: None,
                    status: "active".into(),
                    message: None,
                },
            });
        }
        ServerTransferEvent::FileCompleted {
            session_id,
            result,
            ..
        } => {
            crate::dlog::dlog(&format!(
                "transfer: 接收完成 {} ({} 字节) → {:?}",
                result.sha256, result.bytes_written, result.target_path
            ));
            emit(TransferEvent::Progress {
                view: ProgressView {
                    session_id,
                    direction: "recv".into(),
                    file_name: result
                        .target_path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default(),
                    bytes: result.bytes_written,
                    total: result.bytes_written,
                    speed_bps: None,
                    status: "done".into(),
                    message: Some(result.target_path.to_string_lossy().to_string()),
                },
            });
        }
        ServerTransferEvent::FileFailed {
            session_id,
            file_id,
            error,
        } => {
            crate::dlog::dlog(&format!("transfer: 接收失败 {file_id}: {error}"));
            // 事件只带 file_id：用 FileStarted 时记下的名字，避免 UI 多出一行 `f1`
            let name = lock(&globals().session_file_names)
                .get(&format!("{session_id}|{file_id}"))
                .cloned()
                .unwrap_or(file_id);
            let aborted = error.contains("Stream error") || error.contains("已在本机取消");
            emit(TransferEvent::Progress {
                view: ProgressView {
                    session_id: session_id.clone(),
                    direction: "recv".into(),
                    file_name: name,
                    bytes: 0,
                    total: 0,
                    speed_bps: None,
                    status: "failed".into(),
                    message: Some(error.clone()),
                },
            });
            // 流量中途断掉 = 对方把连接掐了（手机上点「取消」就是这个行为）。
            // 只发文件级 failed 的话，传输页会停在「传输中」、按钮还是「取消」，
            // 所以补一条会话级 terminated，让页面翻成「对方已取消」。
            if aborted {
                emit(TransferEvent::Progress {
                    view: ProgressView {
                        session_id,
                        direction: "recv".into(),
                        file_name: String::new(),
                        bytes: 0,
                        total: 0,
                        speed_bps: None,
                        status: "cancelled".into(),
                        message: Some("对方已取消".to_string()),
                    },
                });
            }
        }
        ServerTransferEvent::SessionEnded { session_id, reason } => {
            crate::dlog::dlog(&format!("transfer: 会话 {session_id} 结束：{reason:?}"));
            // 对方在确认框还没应答时取消：把这条待确认请求也撤掉，
            // 否则确认框会一直留着一条已经没人等的请求（会话 80）。
            if reason == SessionEndReason::CancelledBySender {
                let mut pending = lock(&globals().pending);
                pending.retain(|p| p.view.session_id != session_id);
            }
            // 会话级终态也要发一条：传输页靠它把「传输中」翻成「已完成 / 已取消」，
            // 否则取消之后页面会停在最后那个百分比上不动。
            let (status, message) = match reason {
                SessionEndReason::Finished => ("done", "传输完成".to_string()),
                SessionEndReason::CancelledBySender => ("cancelled", "对方已取消".to_string()),
                SessionEndReason::CancelledLocally => ("cancelled", "已在本机取消".to_string()),
                SessionEndReason::TimedOut => ("failed", "传输超时".to_string()),
            };
            emit(TransferEvent::Progress {
                view: ProgressView {
                    session_id,
                    direction: "recv".into(),
                    file_name: String::new(),
                    bytes: 0,
                    total: 0,
                    speed_bps: None,
                    status: status.to_string(),
                    message: Some(message),
                },
            });
        }
        // 其余事件（注册等）在 select 里已处理；Debug 兜底不刷屏。
        other => {
            crate::dlog::dlog(&format!("transfer: 其他事件：{other:?}"));
        }
    }
}

/// 全量刷新设备快照（registry → `Globals::peers`）。
async fn refresh_peers(registry: &PeerRegistry) {
    let mapped: Vec<Peer> = registry.get_all().await.iter().map(map_peer).collect();
    crate::dlog::dlog(&format!("transfer: 设备快照 {} 台", mapped.len()));
    emit(TransferEvent::Peers {
        peers: mapped.clone(),
    });
    *lock(&globals().peers) = mapped;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_type_falls_back_to_desktop() {
        assert_eq!(device_type_str(None), "desktop");
        assert_eq!(device_type_str(Some(DeviceType::Mobile)), "mobile");
        assert_eq!(device_type_str(Some(DeviceType::Desktop)), "desktop");
        assert_eq!(device_type_str(Some(DeviceType::Web)), "web");
        assert_eq!(device_type_str(Some(DeviceType::Headless)), "headless");
        assert_eq!(device_type_str(Some(DeviceType::Server)), "server");
    }

    #[test]
    fn receive_dir_empty_resolves_to_vexto_subdir() {
        let cfg = config::Config::default();
        let p = resolve_receive_dir(&cfg);
        assert!(p.ends_with("Vexto"), "留空应解析到 Vexto 子目录：{p:?}");
    }

    #[test]
    fn receive_dir_explicit_kept_and_slashes_normalized() {
        let mut cfg = config::Config::default();
        cfg.receive_dir = "D:/recv\n".to_string();
        assert_eq!(resolve_receive_dir(&cfg), PathBuf::from("D:\\recv"));
    }

    #[test]
    fn identity_path_lives_next_to_config() {
        assert_eq!(identity_path().file_name().unwrap(), "transfer-identity.json");
    }

    /// 对方在确认框上还没被应答就取消：接收端 `oneshot` 被丢掉，
    /// 待确认列表必须自动剔掉这条，确认框才会自己关（会话 80）。
    #[test]
    fn pending_list_drops_entries_whose_sender_went_away() {
        let (tx, rx) = oneshot::channel::<PrepareUploadDecision>();
        drop(rx);
        let view = IncomingRequest {
            session_id: "gone80".into(),
            sender_alias: "phone".into(),
            sender_ip: "192.168.1.2".into(),
            sender_model: String::new(),
            sender_type: "mobile".into(),
            total_bytes: 1,
            files: vec![IncomingFile {
                id: "a".into(),
                name: "a".into(),
                size: 1,
            }],
        };
        lock(&globals().pending).push(PendingIncoming {
            view,
            decision_tx: tx,
        });

        let list = pending_list();
        assert!(
            !list.iter().any(|r| r.session_id == "gone80"),
            "对端已撤销的请求必须从待确认列表里消失"
        );
    }

    /// respond 的决策语义：accept 全收、decline 全拒；一次性 oneshot 用后即废。
    #[test]
    fn respond_semantics() {
        let (tx, mut rx) = oneshot::channel::<PrepareUploadDecision>();
        let view = IncomingRequest {
            session_id: "s1".into(),
            sender_alias: "phone".into(),
            sender_ip: "192.168.1.2".into(),
            sender_model: String::new(),
            sender_type: "mobile".into(),
            total_bytes: 3,
            files: vec![
                IncomingFile { id: "a".into(), name: "a".into(), size: 1 },
                IncomingFile { id: "b".into(), name: "b".into(), size: 2 },
            ],
        };
        lock(&globals().pending).push(PendingIncoming { view: view.clone(), decision_tx: tx });

        assert!(respond("s1", true).is_ok());
        let d = rx.try_recv().unwrap();
        match d {
            PrepareUploadDecision::Accept(ids) => {
                assert_eq!(ids.len(), 2);
                assert!(ids.contains(&"a".to_string()) && ids.contains(&"b".to_string()));
            }
            _ => panic!("accept 应该全收"),
        }
        // 已处理 → 报错
        assert!(respond("s1", true).is_err());
    }

    #[test]
    fn respond_decline_and_unknown_session() {
        let (tx, mut rx) = oneshot::channel::<PrepareUploadDecision>();
        let view = IncomingRequest {
            session_id: "s2".into(),
            sender_alias: "phone".into(),
            sender_ip: "192.168.1.2".into(),
            sender_model: String::new(),
            sender_type: "desktop".into(),
            total_bytes: 0,
            files: vec![],
        };
        lock(&globals().pending).push(PendingIncoming { view, decision_tx: tx });
        assert!(respond("s2", false).is_ok());
        assert!(matches!(rx.try_recv().unwrap(), PrepareUploadDecision::Decline));
        assert!(respond("nope", true).is_err());
    }
}
