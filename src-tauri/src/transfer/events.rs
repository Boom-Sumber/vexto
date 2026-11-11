//! 传输事件（后端 → 前端）。单一 `transfer:event` 通道，按 `type` 分流。
//!
//! **事件只是通知，数据靠命令拉取**（风险 71 的教训）：窗口刚建、页面还没
//! 注册监听时带数据的 emit 会丢，所以前端收到事件后再调 `transfer_pending` /
//! `transfer_peers` 拉全量。事件里携带的数据只用于「当场显示」，丢了不影响正确性。

use serde::Serialize;

use super::backend::{IncomingRequest, Peer};

/// 一条收/发进度（含终态）。`direction` 是 `"recv"` / `"send"`。
#[derive(Clone, Debug, Serialize)]
pub struct ProgressView {
    /// 会话 id（发送侧是我们生成的 id）。
    pub session_id: String,
    pub direction: String,
    pub file_name: String,
    pub bytes: u64,
    pub total: u64,
    /// 字节/秒；`preparing` / 终态时为 None。
    pub speed_bps: Option<u64>,
    /// `preparing` / `active` / `done` / `failed` / `cancelled`。
    pub status: String,
    /// 终态时的原因（失败原因 / 完成落盘路径）。
    pub message: Option<String>,
}

/// `transfer:event` 的载荷。
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TransferEvent {
    /// 设备快照变化（发现 / 过期）。
    Peers { peers: Vec<Peer> },
    /// 新的待确认请求。
    Incoming { req: IncomingRequest },
    /// 收/发进度或终态。
    Progress { view: ProgressView },
    /// 一句话提示（如「已拒绝」「发送失败」）。
    Notice { message: String },
}
