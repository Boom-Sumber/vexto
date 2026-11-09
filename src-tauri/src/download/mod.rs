//! 下载模块（S3a）：aria2c sidecar + JSON-RPC + 剪贴板入队。
//!
//! 本阶段只做「后端最小闭环」：能起进程、能加任务、能查进度、退出不留孤儿。
//! 主窗口任务列表、暂停/删除、轮询 pump、空闲自动退出都在 S3b（见 stages/S3-下载引擎.md §S3.1）。
//!
//! 这个模块**不依赖 tauri**：状态用静态量、路径从自身 exe 推导，这样
//! `Tool::status_line()` / `Tool::set_enabled()` 没有 AppHandle 也能读。

pub mod engine;
pub mod rpc;

use crate::{config, dlog};

/// 前缀比较（大小写不敏感）。用 `get` 而不是切片索引，非 ASCII 开头时返回 None 而不是 panic。
fn starts_ci(s: &str, prefix: &str) -> bool {
    s.get(..prefix.len())
        .is_some_and(|h| h.eq_ignore_ascii_case(prefix))
}

/// 剪贴板内容是不是下载链接。磁力链可能几 KB 长，**不截断判断**（任务书 §S3.8）。
pub fn looks_like_url(s: &str) -> bool {
    let t = s.trim();
    starts_ci(t, "http://")
        || starts_ci(t, "https://")
        || starts_ci(t, "magnet:")
        || starts_ci(t, "ftp://")
        || t.to_ascii_lowercase().ends_with(".torrent")
}

/// 热键 / 托盘入口：读一次剪贴板，把链接交给 aria2c。
///
/// 失败时把中文原因写进 `engine` 的状态摘要：S3a 没有主窗口，设置页「下载」
/// 分区那一行就是用户能看到反馈的地方；主窗口通知留到 S3b。
pub fn add_from_clipboard() -> Result<rpc::TaskStatus, String> {
    match add_from_clipboard_inner() {
        Ok(st) => Ok(st),
        Err(e) => {
            dlog::dlog(&format!("download: {e}"));
            engine::set_message(e.clone());
            Err(e)
        }
    }
}

fn add_from_clipboard_inner() -> Result<rpc::TaskStatus, String> {
    let cfg = config::load();
    if !cfg.enable_download {
        return Err("下载工具还没启用：右键托盘 → 设置… → 下载 → 打开开关".into());
    }
    let mut cb = arboard::Clipboard::new().map_err(|e| format!("打不开剪贴板：{e}"))?;
    let text = cb
        .get_text()
        .map_err(|_| "剪贴板里没有文本内容".to_string())?;
    let uri = text.trim().to_string();
    if !looks_like_url(&uri) {
        return Err(
            "剪贴板内容不是下载链接（支持 http / https / ftp / magnet / .torrent）".into(),
        );
    }
    engine::add_uri(&cfg, &uri)
}

#[cfg(test)]
mod tests {
    use super::looks_like_url;

    /// 06-TESTING §6.2.4 的三条，外加大小写与长磁力链。
    #[test]
    fn looks_like_url_accepts_magnet() {
        assert!(looks_like_url("magnet:?xt=urn:btih:abc123"));
        assert!(looks_like_url("https://example.com/f.zip"));
        assert!(!looks_like_url("just some text"));
    }

    #[test]
    fn looks_like_url_covers_all_supported_schemes() {
        assert!(looks_like_url("http://example.com/a.bin"));
        assert!(looks_like_url("HTTPS://EXAMPLE.COM/A.BIN"));
        assert!(looks_like_url("ftp://example.com/a.bin"));
        assert!(looks_like_url("D:\\Downloads\\Ubuntu.TORRENT"));
        assert!(looks_like_url("  https://example.com/a.bin  "));
    }

    /// 磁力链可能很长，不许因为长度截断而误判。
    #[test]
    fn long_magnet_is_not_truncated() {
        let long = format!("magnet:?xt=urn:btih:{}&dn={}", "a".repeat(3000), "file");
        assert!(looks_like_url(&long));
    }

    #[test]
    fn rejects_empty_and_plain_text() {
        assert!(!looks_like_url(""));
        assert!(!looks_like_url("   "));
        assert!(!looks_like_url("这是一段中文，不是链接"));
        assert!(!looks_like_url("file:///C:/a.zip"));
    }
}
