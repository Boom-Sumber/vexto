//! 局域网互传（S4）：LocalSend v2.2 互操作。
//!
//! 分层（ADR-010「lanflare-core + trait 外包」）：
//! - [`backend`]：`TransferBackend` trait 与数据类型 —— 换传输实现时的契约面
//! - [`server`]：lanflare-core 实现 + 服务生命周期 + 设备快照 —— **换实现只改这个文件**
//!
//! 与 `tools/transfer.rs` 的分工：那个文件只管设置 schema（Tool trait），
//! 传输实现在这里（NEXT_TASK 明确要求，不许塞回 tools/）。
//!
//! 会话 39（S4a）只做发现：启用 → 绑 53317（TCP+UDP）→ 组播 announce →
//! 被官方 LocalSend 发现 → 设备列表有数据可查。收发文件链路
//! （prepare-upload → upload → SHA-256 → 128 KB 流式落盘）在 S4b；
//! 本轮外部传入的文件请求统一拒绝（403），不做接收确认 UI。

pub mod backend;
pub mod server;

// 只重导出当前被外部用到的类型；`backend` 里的 S4b 占位类型
// 用 `transfer::backend::X` 全路径访问。
pub use backend::{Peer, TransferConfig};

/// 全局传输实现（ADR-010 的兑换点：换实现只改 `server.rs`）。
///
/// 调用方（`tools/transfer.rs`、`main.rs`）一律走这个 trait 对象，
/// 不直接摸 `server` 的自由函数——`state()` 除外（任务书 trait 没有状态查询，
/// 设置页状态行需要它）。
pub fn backend() -> &'static dyn backend::TransferBackend {
    &server::BACKEND
}

/// LocalSend 协议端口（UDP 组播 + TCP 同号）。任务书 §S4.3 定死，
/// 「多播组自定义 / 换端口」是明确不做项。
pub const TRANSFER_PORT: u16 = 53317;
