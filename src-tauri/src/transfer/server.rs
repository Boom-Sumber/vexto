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
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use lanflare_core::config::AppConfig;
use lanflare_core::crypto::cert::{load_or_generate_identity, TlsIdentity};
use lanflare_core::discovery::multicast::{
    start_multicast_engine, MulticastConfig, MulticastDiscoveryEvent, MulticastNodeInfo,
};
use lanflare_core::discovery::{DiscoveredPeer, PeerRegistry};
use lanflare_core::server::{
    start_server, PrepareUploadDecision, ServerTransferEvent,
};
use lanflare_proto::discovery::{DeviceType, ProtocolType};
use tokio::sync::{mpsc, oneshot};

use super::backend::{IncomingFile, IncomingRequest, Peer, TransferBackend, TransferConfig, TransferHandle};
use super::events::{ProgressView, TransferEvent};
use crate::config;

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
pub(crate) fn emit(evt: TransferEvent) {
    let sink = lock(&EVENT_SINK).clone();
    match sink {
        Some(f) => f(evt),
        None => crate::dlog::dlog("transfer: 事件被丢弃（前端未接入）"),
    }
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

/// 当前待确认的传入请求（最旧在前）。
pub fn pending_list() -> Vec<IncomingRequest> {
    lock(&globals().pending)
        .iter()
        .map(|p| p.view.clone())
        .collect()
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
        .name("lightbox-transfer".into())
        .spawn(move || service_thread(cfg, tx, stop_rx))
        .map_err(|e| format!("启动传输线程失败：{e}"))?;

    let started = match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(res) => res,
        Err(_) => Err("启动超时（10 秒内没拿到绑定结果）".to_string()),
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
}

// ---------- 配置解析 ----------

/// 接收目录：配置里留空 → 系统「下载」下的 `LightBox` 子目录（设置页文案同款）。
///
/// 不能直接拼 `%USERPROFILE%\Downloads`（可能被重定向），复用下载模块
/// `SHGetKnownFolderPath` 的实现（不改那边代码，只调它的 pub fn）。
pub fn resolve_receive_dir(cfg: &config::Config) -> PathBuf {
    let raw = cfg.receive_dir.trim();
    if raw.is_empty() {
        return PathBuf::from(crate::download::engine::default_download_dir()).join("LightBox");
    }
    // 与 download::resolve_dir 同口径：剔换行、正斜杠归一成反斜杠。
    PathBuf::from(raw.replace(['\r', '\n'], "").replace('/', "\\"))
}

/// 持久化 TLS 身份（自签证书 + 私钥 + 指纹）。
///
/// 存 `%APPDATA%\lightbox\transfer-identity.json`，**不进 config.json**：
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
    let registry = PeerRegistry::new();
    registry.start_eviction_task();

    // HTTPS + 持久化自签证书（ADR-035）。fingerprint = 证书 SHA-256。
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
        auto_accept: false,
        ..Default::default()
    };

    // 落盘目录建好 + 清残留临时文件（坑 3）。
    let _ = std::fs::create_dir_all(&cfg.receive_dir);
    cleanup_stale_parts(&cfg.receive_dir);

    let (stx, mut srx) = mpsc::channel::<ServerTransferEvent>(256);
    let server = match start_server(app_cfg, Some(identity.clone()), registry.clone(), Some(stx))
        .await
    {
        Ok(s) => s,
        Err(e) => {
            // 端口被占是这里最常见的失败，原因要能原样展示给用户。
            let _ = result_tx.send(Err(format!(
                "无法启动传输服务（端口 {} 可能被占用）：{e}",
                cfg.port
            )));
            return;
        }
    };
    let port = server.bound_port();

    let node_info = MulticastNodeInfo {
        alias: cfg.alias.clone(),
        // 任务书 §S4.3.2 的示例值。与 /info 应答（lanflare 内部写死 "2.0"）
        // 不一致是已知偏差（RISKS 91.），官方 LocalSend 不按 version 串做兼容判断。
        version: "2.2".to_string(),
        device_model: Some("Windows".to_string()),
        device_type: Some(DeviceType::Desktop),
        fingerprint: identity.fingerprint.clone(),
        port,
        protocol: ProtocolType::Https,
        download: false,
    };
    let mcfg = MulticastConfig {
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

    // announce burst（100ms/500ms/2000ms，各带抖动）后台发，不阻塞事件循环。
    {
        let h = mhandle.clone();
        tokio::spawn(async move {
            h.announce_burst().await;
        });
    }

    let mut snapshot_rx = registry.subscribe();
    refresh_peers(&registry).await;
    crate::dlog::dlog(&format!(
        "transfer: 服务已启动 https 0.0.0.0:{port}，组播 224.0.0.167:{}",
        cfg.port
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
    emit(TransferEvent::Incoming { req: view });
}

/// 收方向的进度 / 终态 → 前端事件。
fn on_transfer_progress(ev: ServerTransferEvent) {
    match ev {
        ServerTransferEvent::FileStarted {
            session_id,
            file_name,
            total_bytes,
            ..
        } => {
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
            emit(TransferEvent::Progress {
                view: ProgressView {
                    session_id,
                    direction: "recv".into(),
                    file_name: file_id,
                    bytes: 0,
                    total: 0,
                    speed_bps: None,
                    status: "failed".into(),
                    message: Some(error),
                },
            });
        }
        ServerTransferEvent::SessionEnded { session_id, reason } => {
            crate::dlog::dlog(&format!("transfer: 会话 {session_id} 结束：{reason:?}"));
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
    fn receive_dir_empty_resolves_to_lightbox_subdir() {
        let cfg = config::Config::default();
        let p = resolve_receive_dir(&cfg);
        assert!(p.ends_with("LightBox"), "留空应解析到 LightBox 子目录：{p:?}");
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
