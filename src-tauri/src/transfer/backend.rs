//! `TransferBackend` trait：传输实现的契约面（ADR-010）。
//!
//! 任务书 §S4.2 定的形状，按实现微调过两处（会话 39 记录在案）：
//! `stop` 等返回 `Result<(), String>`；`send` 带 `pin` 参数（对方开了 PIN 时用）。
//!
//! 当前唯一实现是 `server::LanflareBackend`（lanflare-core 0.5）；将来若按
//! ADR-010 的触发条件换自研实现，只需要在 `server.rs` 换掉实现体，
//! 调用方（main.rs 的 IPC 命令）不动。

use std::path::PathBuf;

use crate::config;

/// 传输服务的配置投影。trait 不直接依赖 `config::Config`，
/// 将来换实现时配置读取留在调用方。
#[derive(Clone, Debug)]
pub struct TransferConfig {
    /// 本机对其他设备显示的名字（已解析：空值 → 计算机名）。
    pub alias: String,
    /// 服务端口（UDP 组播 + TCP 同号）。
    pub port: u16,
    /// 接收目录（已解析：空值 → 系统「下载」下 LightBox 子目录）。
    pub receive_dir: PathBuf,
    /// 接收是否要求对方输入 PIN（校验链路在 S4b）。
    pub require_pin: bool,
    /// PIN（4–6 位数字）。
    pub pin: String,
    /// 收到请求时是否跳过确认框、直接接受。
    pub auto_accept: bool,
    /// 是否使用 HTTPS（ADR-035 默认开）。
    pub use_https: bool,
    /// 组播地址（已解析：空值 → 224.0.0.167）。
    pub multicast: String,
    /// 设备类型：mobile / desktop / web / headless / server。
    pub device_type: String,
    /// 设备型号（组播公告用）。
    pub device_model: String,
    /// 设备列表保留时长（秒）。
    pub peer_timeout_secs: u64,
    /// 网络接口白名单（逗号分隔的 IP 前缀；空 = 所有网卡）。
    pub interface_whitelist: String,
}

impl TransferConfig {
    /// 从全局配置映射。空 `device_alias` 用计算机名兜底。
    pub fn from_app(cfg: &config::Config) -> Self {
        let alias = if cfg.device_alias.trim().is_empty() {
            std::env::var("COMPUTERNAME").unwrap_or_else(|_| "LightBox".to_string())
        } else {
            cfg.device_alias.trim().to_string()
        };
        Self {
            alias,
            port: if cfg.transfer_port == 0 {
                crate::transfer::TRANSFER_PORT
            } else {
                cfg.transfer_port.clamp(1024, 65535)
            },
            receive_dir: crate::transfer::server::resolve_receive_dir(cfg),
            require_pin: cfg.require_pin,
            pin: cfg.pin.trim().to_string(),
            auto_accept: cfg.transfer_auto_accept,
            use_https: cfg.transfer_use_https,
            multicast: if cfg.transfer_multicast.trim().is_empty() {
                "224.0.0.167".to_string()
            } else {
                cfg.transfer_multicast.trim().to_string()
            },
            device_type: if cfg.transfer_device_type.trim().is_empty() {
                "desktop".to_string()
            } else {
                cfg.transfer_device_type.trim().to_lowercase()
            },
            device_model: if cfg.transfer_device_model.trim().is_empty() {
                "Windows".to_string()
            } else {
                cfg.transfer_device_model.trim().to_string()
            },
            peer_timeout_secs: cfg.transfer_peer_timeout_secs.clamp(5, 3600),
            interface_whitelist: cfg.transfer_interface_whitelist.trim().to_string(),
        }
    }
}

/// 一台已发现的设备（快照，给 IPC / 状态行用）。
#[derive(Clone, Debug, serde::Serialize)]
pub struct Peer {
    /// 设备指纹（HTTPS 模式为证书 SHA-256）。只用于识别，不是安全机制。
    pub fingerprint: String,
    pub alias: String,
    /// 主 IP（IPv4 优先）。
    pub ip: String,
    pub port: u16,
    /// 设备型号字符串（对方自报，可能为空）。
    pub device_model: String,
    /// `mobile` / `desktop` / `web` / `headless` / `server`，未知按 `desktop`。
    pub device_type: String,
    /// 对方协议版本串（如 "2.2"）。
    pub version: String,
    /// `http` / `https`。
    pub protocol: String,
}

/// 一条待确认请求里的文件（名字已过 `sanitize`）。
#[derive(Clone, Debug, serde::Serialize)]
pub struct IncomingFile {
    pub id: String,
    pub name: String,
    pub size: u64,
}

/// 一条待确认的传入请求（prepare-upload）。确认框（§S4.7.2）与
/// `transfer:event` 的 `Incoming` 事件共用这个形状。
#[derive(Clone, Debug, serde::Serialize)]
pub struct IncomingRequest {
    pub session_id: String,
    pub sender_alias: String,
    pub sender_ip: String,
    /// 对方自报的设备型号（可能为空）。
    pub sender_model: String,
    /// 对方设备类型：`mobile` / `desktop` / `web` / `headless` / `server`。
    pub sender_type: String,
    pub total_bytes: u64,
    pub files: Vec<IncomingFile>,
}

/// 一次发送的句柄（取消用）。
#[derive(Debug)]
pub struct TransferHandle {
    pub session_id: String,
}

/// 传输实现的契约。实现体在 `server.rs`（接收/发现）与 `send.rs`（发送）。
pub trait TransferBackend: Send + Sync {
    /// 启动传输服务（绑端口 + 起发现）。失败必须把原因说清楚
    /// （端口被占 / 无网络 / 组播不可用），调用方会展示给用户。
    fn start(&self, cfg: &TransferConfig) -> Result<(), String>;
    /// 停止传输服务：停组播、关端口、释放发现状态。幂等。
    fn stop(&self) -> Result<(), String>;
    /// 当前发现的设备快照。
    fn peers(&self) -> Vec<Peer>;
    /// 发送文件给一台设备（`peer_id` = 指纹或 `ip:port`）。
    /// `pin`：对方开了 PIN 时传；异步跑，进度走 `transfer:event`。
    fn send(
        &self,
        peer_id: &str,
        files: &[PathBuf],
        pin: Option<String>,
    ) -> Result<TransferHandle, String>;
    /// 待确认的传入请求（最旧在前）。
    fn pending_requests(&self) -> Vec<IncomingRequest>;
    /// 接受 / 拒绝一条传入请求。
    fn respond(&self, session_id: &str, accept: bool) -> Result<(), String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_app_maps_fields_and_trims() {
        let mut cfg = config::Config::default();
        cfg.device_alias = "  我的电脑  ".to_string();
        cfg.require_pin = true;
        cfg.pin = " 123456 ".to_string();
        cfg.transfer_auto_accept = true;
        cfg.transfer_port = 53319;
        cfg.transfer_use_https = false;
        cfg.transfer_multicast = " 239.1.2.3 ".to_string();
        cfg.transfer_device_type = "Mobile".to_string();
        cfg.transfer_device_model = "  Mate 80  ".to_string();
        cfg.transfer_peer_timeout_secs = 30;
        cfg.transfer_interface_whitelist = " 192.168.1.* ".to_string();
        let tc = TransferConfig::from_app(&cfg);
        assert_eq!(tc.alias, "我的电脑");
        assert!(tc.require_pin);
        assert_eq!(tc.pin, "123456");
        assert!(tc.auto_accept);
        assert_eq!(tc.port, 53319);
        assert!(!tc.use_https);
        assert_eq!(tc.multicast, "239.1.2.3");
        assert_eq!(tc.device_type, "mobile");
        assert_eq!(tc.device_model, "Mate 80");
        assert_eq!(tc.peer_timeout_secs, 30);
        assert_eq!(tc.interface_whitelist, "192.168.1.*");
    }

    #[test]
    fn from_app_alias_falls_back_to_computername() {
        let cfg = config::Config::default();
        let tc = TransferConfig::from_app(&cfg);
        assert!(!tc.alias.trim().is_empty(), "空别名必须兜底成计算机名");
    }
}
