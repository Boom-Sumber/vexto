//! lanflare-core 传输实现 + 服务生命周期 + 设备快照。
//!
//! **换传输实现只改这个文件**（ADR-010）：`backend.rs` 的 trait 是契约面，
//! 本文件是 lanflare-core 0.5 的实现体。全部 lanflare 依赖都关在这里，
//! 其余模块只看 `TransferBackend` 与 [`ServiceState`]。
//!
//! 会话 39（S4a）职责：
//! - 启用 → 独立线程 + 2 worker tokio 运行时上起 HTTP 服务（TCP 53317，
//!   HTTPS + 持久化自签证书）与 UDP 组播发现（`224.0.0.167:53317`）
//! - announce burst 能被官方 LocalSend 收到；对方 register / 组播 announce
//!   都会进 `PeerRegistry`，经快照给 `peers()` / 状态行
//! - 停用 → 组播停、端口关、线程收干净（幂等）
//!
//! **S4a 不做收发链路**：外部 prepare-upload 一律回 403 拒绝（接收确认 UI
//! 与流式落盘在 S4b）。`send` / `pending_requests` / `respond` 占位报错。

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use lanflare_core::config::AppConfig;
use lanflare_core::crypto::cert::load_or_generate_identity;
use lanflare_core::discovery::multicast::{
    start_multicast_engine, MulticastConfig, MulticastDiscoveryEvent, MulticastNodeInfo,
};
use lanflare_core::discovery::{DiscoveredPeer, PeerRegistry};
use lanflare_core::server::{
    start_server, PrepareUploadDecision, ServerTransferEvent,
};
use lanflare_proto::discovery::{DeviceType, ProtocolType};
use tokio::sync::{mpsc, oneshot};

use super::backend::{Peer, TransferBackend, TransferConfig, TransferHandle, IncomingRequest};
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

struct Globals {
    state: Mutex<ServiceState>,
    peers: Mutex<Vec<Peer>>,
    stop_tx: Mutex<Option<oneshot::Sender<()>>>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

fn globals() -> &'static Globals {
    GLOBALS.get_or_init(|| Globals {
        state: Mutex::new(ServiceState::Stopped),
        peers: Mutex::new(Vec::new()),
        stop_tx: Mutex::new(None),
        thread: Mutex::new(None),
    })
}

static GLOBALS: OnceLock<Globals> = OnceLock::new();

/// Mutex 被 panic 毒化时仍取到数据：快照类状态不值得为此整个挂掉。
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

// ---------- 对外接口（tools/transfer.rs、main.rs 用） ----------

/// 当前服务状态。
pub fn state() -> ServiceState {
    lock(&globals().state).clone()
}

/// 当前发现的设备快照。
pub fn peers() -> Vec<Peer> {
    lock(&globals().peers).clone()
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
    let Some(handle) = handle else {
        // 没线程也要把状态收干净（启动失败后重试的场景会看到 Failed）
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

    fn send(&self, _peer_id: &str, _files: &[PathBuf]) -> Result<TransferHandle, String> {
        Err("发送功能在 S4b 实现".to_string())
    }

    fn pending_requests(&self) -> Vec<IncomingRequest> {
        // S4a 的事件循环对 prepare-upload 即时拒绝，不会留下待确认请求。
        Vec::new()
    }

    fn respond(&self, _session_id: &str, _accept: bool) -> Result<(), String> {
        Err("接收确认在 S4b 实现".to_string())
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
    rt.block_on(service_main(cfg, result_tx, stop_rx));
}

async fn service_main(
    cfg: TransferConfig,
    result_tx: std::sync::mpsc::Sender<Result<u16, String>>,
    mut stop_rx: oneshot::Receiver<()>,
) {
    let registry = PeerRegistry::new();
    registry.start_eviction_task();

    // HTTPS + 持久化自签证书（用户会话 39 选定；偏离任务书 §S4.6「第一版 HTTP」
    // 的理由见台账：lanflare 的 /info 在 HTTP 模式下指纹空、protocol 硬编码
    // https，自相矛盾；HTTPS 模式协议字段全部自洽）。
    let identity = match load_or_generate_identity(&identity_path()) {
        Ok(id) => id,
        Err(e) => {
            let _ = result_tx.send(Err(format!("生成传输证书失败：{e}")));
            return;
        }
    };

    let app_cfg = AppConfig {
        alias: cfg.alias.clone(),
        port: cfg.port,
        download_dir: cfg.receive_dir.clone(),
        // 未开 PIN 时传 None；开了也只在 prepare-upload 校验（那条路 S4a 拒绝）。
        pin: if cfg.require_pin && !cfg.pin.is_empty() {
            Some(cfg.pin.clone())
        } else {
            None
        },
        auto_accept: false,
        ..Default::default()
    };

    let (stx, mut srx) = mpsc::channel::<ServerTransferEvent>(64);
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
        // 不一致是已知偏差，官方 LocalSend 不按 version 串做兼容判断。
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
                    decision_tx,
                    ..
                }) => {
                    // S4a 没有接收确认 UI：统一拒绝（403），不许会话悬挂。
                    // S4b 改成进 pending_requests() 等用户确认。
                    crate::dlog::dlog(&format!(
                        "transfer: 收到 {session_id} 来自 {} ({sender_ip}) 的传输请求，S4a 统一拒绝",
                        sender_info.alias
                    ));
                    if decision_tx.send(PrepareUploadDecision::Decline).is_err() {
                        crate::dlog::dlog("transfer: 拒绝传输请求失败（对端已断开）");
                    }
                }
                // 传输进度 / 完成 / 失败等事件属于 S4b 的收发链路：
                // S4a 所有传输请求都被拒绝，正常走不到这里，来了只记日志。
                Some(other) => {
                    crate::dlog::dlog(&format!("transfer: 未处理事件（S4b）：{other:?}"));
                }
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
    mhandle.stop().await;
    server.stop();
    registry.shutdown();
    *lock(&globals().state) = ServiceState::Stopped;
    crate::dlog::dlog("transfer: 服务已停止");
}

/// 全量刷新设备快照（registry → `Globals::peers`）。
async fn refresh_peers(registry: &PeerRegistry) {
    let mapped: Vec<Peer> = registry.get_all().await.iter().map(map_peer).collect();
    crate::dlog::dlog(&format!("transfer: 设备快照 {} 台", mapped.len()));
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
}
