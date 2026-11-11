lanflare-core（vendored）
=======================

上游：https://github.com/fokx/lanflare
来源：crates.io `lanflare-core` 0.5.0（2026-10-22 发布）
许可：Apache-2.0，原文见同目录 `LICENSE-APACHE`

为什么 vendor
-------------

LightBox 的「接收中取消」需要 `SessionManager::cancel_session(session_id, sender_ip)`。
上游 0.5.0 只在 `server::ServerState` 内部持有它，`ServerHandle` 没有暴露任何入口，
宿主拿不到，也就无法在接收过程中主动中止会话。

按 `docs/08-DECISIONS.md` ADR-010，传输实现外面包了一层 `TransferBackend` trait，
但 trait 只能隔离「换实现」，不能凭空拿到私有的会话管理器；上游也没有更新版本可升
（crates.io 最新就是 0.5.0）。所以选择本地 vendor + 最小补丁。

本地改动（全部集中在 `src/server/`，协议行为、落盘逻辑、TLS、分块与校验一律未改）
----------------------------------------------------------------------------------

1. `server/mod.rs`：`ServerHandle` 增加字段 `session_manager: Arc<SessionManager>`
   与只读访问器 `session_manager()`；`start_server_ext()` 里把
   `Arc::new(SessionManager::new())` 提前绑定到局部变量，`ServerState` 与
   `ServerHandle` 共用同一个 `Arc`（行为与上游一致，只是多持有一份引用）。
2. `server/session.rs`：`SessionManager` 增加 `upload_cancels: Mutex<HashMap<String, CancellationToken>>`
   与三个方法 `register_upload_cancel()` / `cancel_upload()` / `forget_upload_cancel()`；
   `cancel_session()` 命中 **Active** 会话时，除清槽位外还调用 `cancel_upload()`。
   （上游只对 Pending 有效，Active 会话取消后上传循环仍会继续写盘。）
3. `server/handlers.rs`：`handle_upload()` 在上传循环里用 `tokio::select!` 同时等
   body 与取消令牌；被取消时 `writer.abort()`（删临时文件）、标记文件失败、
   发 `FileFailed { error: "已在本机取消" }`，并返回 500。

其它差异（非代码）
------------------

- 未携带 `tests/` 与 `benches/`（体积与构建时间成本），因此删掉了 `Cargo.toml` 里
  对应的 `[[test]]` / `[[bench]]` target 声明。
- `README.md` 原样保留。

升级流程
--------

1. 从 crates.io 取新版源码；
2. 重新应用上述三处改动（若上游已暴露 `session_manager()`，则删掉本目录、移除
   根 `Cargo.toml` 的 `[patch.crates-io]`）；
3. 更新本文件顶部的版本号与日期；
4. 跑 `cargo build --release` + `cargo test`，并复验 06 §6.4.3 的 #2 / #5 / #6。
