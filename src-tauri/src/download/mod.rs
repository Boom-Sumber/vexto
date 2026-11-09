//! 下载模块（S3a 后端闭环 + S3b 轮询泵 + S3c 新建任务与重名递增）。
//!
//! 这个模块**不依赖 tauri**：状态用静态量、路径从自身 exe 推导，这样
//! `Tool::status_line()` / `Tool::set_enabled()` 没有 AppHandle 也能读。

pub mod engine;
pub mod naming;
pub mod pump;
pub mod rpc;

use crate::config;

/// 新建任务窗口提交的一条。`uri` 必填；其余留空表示跟随设置 / 系统默认。
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct NewTask {
    pub uri: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub dir: String,
    #[serde(default)]
    pub max_speed_kb: u32,
    #[serde(default)]
    pub connections: u32,
    #[serde(default)]
    pub user_agent: String,
    #[serde(default)]
    pub referer: String,
}

/// 新建任务窗口提交：目录、文件名、单任务限速与连接数都可以覆盖全局设置。
///
/// 与热键时代的 `add_from_clipboard` 的区别：这里不再读剪贴板，参数由用户
/// 在新建页确认过；重名递增也在这里统一做（见 `naming::unique_file_name`）。
pub fn add_task(cfg: &config::Config, t: &NewTask) -> Result<rpc::TaskStatus, String> {
    if !cfg.enable_download {
        return Err("下载工具还没启用：右键托盘 → 设置… → 下载 → 打开开关".into());
    }
    let uri = t.uri.trim();
    if !looks_like_url(uri) {
        return Err(format!("不是支持的下载链接（http / https / ftp / magnet / .torrent）：{uri}"));
    }
    let st = engine::add_task(cfg, t)?;
    // 状态变了，立刻唤醒 pump 重取一次快照，不等下一个轮询周期。
    pump::notify();
    Ok(st)
}

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

/// 热键用：读一次剪贴板，是支持的链接就返回，否则 `None`。
///
/// **不报错**：热键的语义是「打开新建任务窗口」，剪贴板里是什么都不该拦住
/// 窗口弹出。工具没启用、剪贴板没权限之类的检查放到用户点「新建」时
/// （`add_task`）再做，那时才有明确的落点显示错误。
pub fn clipboard_url() -> Option<String> {
    let mut cb = arboard::Clipboard::new().ok()?;
    let text = cb.get_text().ok()?;
    let t = text.trim().to_string();
    if looks_like_url(&t) {
        Some(t)
    } else {
        None
    }
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
