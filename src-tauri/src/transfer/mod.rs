//! 局域网互传（S4）：LocalSend v2.2 互操作。
//!
//! 分层（ADR-010「lanflare-core + trait 外包」）：
//! - [`backend`]：`TransferBackend` trait 与数据类型 —— 换传输实现时的契约面
//! - [`server`]：lanflare-core 实现 + 服务生命周期 + 设备快照 + 接收确认 —— **换实现只改这个文件**
//! - [`send`]：发送侧（`TransferOrchestrator`，进度/取消）
//! - [`events`]：`transfer:event` 的载荷形状
//! - [`sanitize`]：文件名清洗（§S4.5）与重名递增
//!
//! 与 `tools/transfer.rs` 的分工：那个文件只管设置 schema（Tool trait），
//! 传输实现在这里（NEXT_TASK 明确要求，不许塞回 tools/）。
//!
//! 会话 39（S4a）：发现链路（绑定 53317、组播 announce、设备快照）。
//! 会话 40（S4b）：收发链路——接收确认 → 流式落盘（lanflare 内部，
//! 128 KB + SHA-256 + 原子 rename）、发送、进度 UI 数据面。

pub mod backend;
pub mod events;
pub mod history;
pub mod sanitize;
pub mod send;
pub mod server;

// 只重导出当前被外部用到的类型；其余用 `transfer::backend::X` 全路径访问。
pub use backend::{ActiveTransfer, IncomingRequest, Peer, TransferConfig};

/// 全局传输实现（ADR-010 的兑换点：换实现只改 `server.rs`）。
///
/// 调用方（`main.rs` 的 IPC 命令）一律走这个 trait 对象，
/// 不直接摸 `server` 的自由函数——`state()` 除外（任务书 trait 没有状态查询，
/// 设置页状态行需要它）。
pub fn backend() -> &'static dyn backend::TransferBackend {
    &server::BACKEND
}

/// LocalSend 协议端口（UDP 组播 + TCP 同号）。任务书 §S4.3 定死，
/// 「多播组自定义 / 换端口」是明确不做项。
pub const TRANSFER_PORT: u16 = 53317;
