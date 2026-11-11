//! `TransferBackend` trait：传输实现的契约面（ADR-010）。
//!
//! 任务书 §S4.2 定的形状。当前唯一实现是 `server::LanflareBackend`
//! （lanflare-core 0.5）；将来若按 ADR-010 的触发条件换自研实现，
//! 只需要在 `server.rs` 里换掉实现体，调用方（tools/transfer.rs、
//! main.rs 的 IPC）不动。
//!
//! 会话 39（S4a）只落地 `start` / `stop` / `peers` 三个方法；
//! `send` / `pending_requests` / `respond` 是 S4b 的收发链路，先占位报错，
//! 不许在 S4a 顺手实现（铁律 1）。

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
            port: crate::transfer::TRANSFER_PORT,
            receive_dir: crate::transfer::server::resolve_receive_dir(cfg),
            require_pin: cfg.require_pin,
            pin: cfg.pin.trim().to_string(),
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

/// S4b：一个待确认的传入文件。
/// （S4a 只做发现，收发链路在 S4b 才构造这些类型。）
#[allow(dead_code)]
#[derive(Debug)]
pub struct IncomingFile {
    pub id: String,
    pub name: String,
    pub size: u64,
}

/// S4b：一条待确认的传输请求（prepare-upload）。
#[allow(dead_code)]
#[derive(Debug)]
pub struct IncomingRequest {
    pub session_id: String,
    pub sender_alias: String,
    pub sender_ip: String,
    pub files: Vec<IncomingFile>,
}

/// S4b：一次发送的句柄（进度查询 / 取消）。
#[allow(dead_code)]
#[derive(Debug)]
pub struct TransferHandle {
    pub session_id: String,
}

/// 传输实现的契约。实现体在 `server.rs`。
///
/// `send` / `pending_requests` / `respond` 是 S4b 的收发链路，
/// S4a 没有调用方（只做发现）——保留是契约面，不是死代码。
#[allow(dead_code)]
pub trait TransferBackend: Send + Sync {
    /// 启动传输服务（绑端口 + 起发现）。失败必须把原因说清楚
    /// （端口被占 / 无网络 / 组播不可用），调用方会展示给用户。
    fn start(&self, cfg: &TransferConfig) -> Result<(), String>;
    /// 停止传输服务：停组播、关端口、释放发现状态。幂等。
    fn stop(&self) -> Result<(), String>;
    /// 当前发现的设备快照。
    fn peers(&self) -> Vec<Peer>;
    /// 发送文件。**S4b 实现**，S4a 返回 Err。
    fn send(&self, peer_id: &str, files: &[PathBuf]) -> Result<TransferHandle, String>;
    /// 待确认的传入请求。**S4b 实现**，S4a 恒空（请求到达即拒绝）。
    fn pending_requests(&self) -> Vec<IncomingRequest>;
    /// 接受 / 拒绝一条传入请求。**S4b 实现**，S4a 返回 Err。
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
        let tc = TransferConfig::from_app(&cfg);
        assert_eq!(tc.alias, "我的电脑");
        assert!(tc.require_pin);
        assert_eq!(tc.pin, "123456");
        assert_eq!(tc.port, crate::transfer::TRANSFER_PORT);
    }

    #[test]
    fn from_app_alias_falls_back_to_computername() {
        let cfg = config::Config::default();
        let tc = TransferConfig::from_app(&cfg);
        assert!(!tc.alias.trim().is_empty(), "空别名必须兜底成计算机名");
    }
}
