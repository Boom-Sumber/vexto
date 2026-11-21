//! 下载模块（S3a 后端闭环 + S3b 轮询泵 + S3c 新建任务与重名递增
//! + S3.5 历史、通知、代理、种子与高级选项）。
//!
//! 除 `notify` 外**不依赖 tauri**：状态用静态量、路径从自身 exe 推导，这样
//! `Tool::status_line()` / `Tool::set_enabled()` 没有 AppHandle 也能读。
//! `notify` 必须拿 `AppHandle` 才能发系统通知，是唯一的例外。

pub mod engine;
pub mod github;
pub mod history;
pub mod naming;
pub mod notify;
pub mod proxy;
pub mod pump;
pub mod rpc;
pub mod validate;

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
    /// 浏览器扩展交过来的 Cookie。写进 aria2 的 `header` 选项，
    /// **不进界面、不写日志**：它等于用户的登录态。
    #[serde(default)]
    pub cookie: String,
    /// `Content-Disposition` 之外的校验和，格式 `类型=摘要`（如 `sha-256=abcd…`）。
    /// 由 `validate::checksum_option` 校验后写进 aria2 的 `checksum` 选项。
    #[serde(default)]
    pub checksum: String,
    /// HTTP Basic / Digest 鉴权用户名，写进 aria2 的 `http-user`。
    #[serde(default)]
    pub http_user: String,
    /// HTTP Basic / Digest 鉴权密码，写进 aria2 的 `http-passwd`。
    /// **不写日志、不回显**：和 Cookie 一样按敏感值处理。
    #[serde(default)]
    pub http_passwd: String,
    /// 原始 `Authorization` 头（如 `Bearer xxx`），写进 aria2 的 `header` 数组。
    ///
    /// 与用户名 / 密码是**同一个「认证信息」区块**的两种填法：填了这个，
    /// 后端就不发 Basic 头（`engine::request_headers` 里二者互斥，见 ADR-024）。
    /// 同样不写日志、不回显。
    #[serde(default)]
    pub authorization: String,
    /// 同一文件的其他下载地址（GitHub 镜像 / 直连兜底）。aria2 会把它们
    /// 当成 mirrors 依次尝试；不展示在新建页，也不写日志。
    #[serde(default)]
    pub mirrors: Vec<String>,
}

/// 「种子任务」页提交的一条。
///
/// `.torrent` 文件由前端 `<input type="file">` 读成 base64 传进来：Rust 侧不碰
/// 文件对话框、也不需要 `dialog` 插件与文件权限（无边框窗口的 WebView2 里，
/// 原生文件选择器由前端触发最省事，也不破坏自绘 UI 规范）。
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct NewTorrent {
    /// `.torrent` 文件内容的 base64。
    pub torrent_base64: String,
    /// 只用于界面显示与错误回显；aria2 用种子里的名字，不接受 `out` 覆盖。
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub dir: String,
    #[serde(default)]
    pub max_speed_kb: u32,
}

/// 种子任务入队。与 `add_task` 分开：校验和、HTTP 鉴权、UA / Referer 对 BT
/// 没有意义，不往里塞；`checksum` 只作用于 HTTP/FTP 下载。
pub fn add_torrent(cfg: &config::Config, t: &NewTorrent) -> Result<rpc::TaskStatus, String> {
    if !cfg.enable_download {
        return Err("下载工具还没启用：右键托盘 → 设置… → 下载 → 打开开关".into());
    }
    let st = engine::add_torrent(cfg, t)?;
    pump::notify();
    Ok(st)
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
        // 磁力单独给一句话：说「不是支持的链接」用户会以为自己复制错了。
        if starts_ci(uri, "magnet:") {
            return Err("暂不支持磁力链接：BT / 磁力功能将在后续版本开放".to_string());
        }
        return Err(format!("不是支持的下载链接（http / https / ftp）：{uri}"));
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

/// 磁力链接是否开放。**暂不开**（会话 25 用户要求）。
///
/// S3.5 第三轮已经把 `aria2.addTorrent` 与种子页签做完了，但用户要求先把 BT
/// 相关入口关掉，等真正集成时再开。留这一个开关而不是删代码：剪贴板识别、
/// 扩展接收、新建页提交三处都看它，改成 `true` 就一起恢复。
pub const ALLOW_MAGNET: bool = false;

/// 剪贴板内容是不是下载链接。磁力链可能几 KB 长，**不截断判断**（任务书 §S3.8）。
pub fn looks_like_url(s: &str) -> bool {
    let t = s.trim();
    starts_ci(t, "http://")
        || starts_ci(t, "https://")
        || (ALLOW_MAGNET && starts_ci(t, "magnet:"))
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

    /// 06-TESTING §6.2.4 的三条，外加大小写。
    ///
    /// 磁力链接在会话 25 被**主动关掉**（`ALLOW_MAGNET = false`）：BT 入口还没开放，
    /// 认进来只会让用户一路填到提交才看到「暂不支持」。
    #[test]
    fn looks_like_url_rejects_magnet_while_bt_is_closed() {
        assert!(!looks_like_url("magnet:?xt=urn:btih:abc123"));
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

    /// 超长链接不许因为长度截断而误判。
    ///
    /// 原本这条测的是磁力链；会话 25 磁力关闭后，磁力那半个断言改成跟着
    /// `ALLOW_MAGNET` 走——开关一打开这条测试就自动恢复原意。
    #[test]
    fn long_url_is_not_truncated() {
        let long = format!("https://example.com/{}", "a".repeat(3000));
        assert!(looks_like_url(&long));
        let long_magnet = format!("magnet:?xt=urn:btih:{}&dn={}", "a".repeat(3000), "file");
        assert_eq!(looks_like_url(&long_magnet), super::ALLOW_MAGNET);
    }

    #[test]
    fn rejects_empty_and_plain_text() {
        assert!(!looks_like_url(""));
        assert!(!looks_like_url("   "));
        assert!(!looks_like_url("这是一段中文，不是链接"));
        assert!(!looks_like_url("file:///C:/a.zip"));
    }
}
