//! GitHub Releases 查询。
//!
//! 纯解析 / 版本比较写成不联网的函数，单测直接覆盖；只有 `fetch_latest` 碰网络。
//! 元数据优先直连 GitHub；直连不通时依次尝试两个 gh-proxy 前缀（国内网络下
//! 直连发布资产经常失败，但 API 本身通常可用）。

use std::time::Duration;

use serde::Deserialize;

use crate::dlog;

pub const REPO: &str = "Ruxsmiles/vexto";
const API_URL: &str = "https://api.github.com/repos/Ruxsmiles/vexto/releases/latest";
/// 下载线路候选（`auto` 模式按此顺序）。前缀拼接完整 GitHub URL。
pub const PROXIES: [&str; 2] = ["https://v4.gh-proxy.org/", "https://gh-proxy.org/"];

/// 一次检查拿到的、后续下载/展示所需的全部信息。
#[derive(Clone, Debug)]
pub struct ReleaseInfo {
    /// 去掉 `v` 前缀的版本号
    pub version: String,
    pub notes: String,
    pub url: String,
    pub asset_name: String,
    pub asset_url: String,
    pub asset_size: u64,
    /// GitHub 给的 `sha256:...`，老版本发布可能没有
    pub digest: Option<String>,
    /// Release 说明里的最低版本标记；没有则不做强制更新
    pub min_version: Option<String>,
}

#[derive(Deserialize)]
struct ReleaseJson {
    #[serde(default)]
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    assets: Vec<AssetJson>,
}

#[derive(Deserialize)]
struct AssetJson {
    #[serde(default)]
    name: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    digest: Option<String>,
    #[serde(default)]
    browser_download_url: String,
}

pub(crate) fn user_agent() -> String {
    format!(
        "Vexto/{} (+https://github.com/{REPO})",
        env!("CARGO_PKG_VERSION")
    )
}

/// 元数据请求的候选地址：直连优先，失败再走代理。
fn metadata_urls() -> Vec<String> {
    let mut urls = vec![API_URL.to_string()];
    for p in PROXIES {
        urls.push(format!("{p}{API_URL}"));
    }
    urls
}

/// 请求最新 Release。只有这里碰网络。
pub fn fetch_latest() -> Result<ReleaseInfo, String> {
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(20))
        .user_agent(user_agent())
        .build()
        .map_err(|e| {
            dlog::warn(&format!("update: 创建 HTTP 客户端失败：{e}"));
            "无法创建更新请求".to_string()
        })?;

    let mut errors: Vec<String> = Vec::new();
    for url in metadata_urls() {
        match client
            .get(&url)
            .header("Accept", "application/vnd.github+json")
            .send()
        {
            Ok(resp) => {
                let status = resp.status();
                let text = resp.text().unwrap_or_default();
                if status.is_success() {
                    return parse_release(&text);
                }
                errors.push(status_error(status.as_u16()));
            }
            Err(e) => {
                dlog::dlog(&format!("update: 请求 {url} 失败：{e}"));
                errors.push("无法连接更新服务器".to_string());
            }
        }
    }
    Err(pick_error(&errors))
}

/// 状态码 → 面向用户的中文。不把原始错误文本透给界面。
fn status_error(code: u16) -> String {
    match code {
        403 | 429 => "GitHub 接口访问过于频繁，请稍后再试".to_string(),
        404 => "GitHub 上还没有已发布的版本".to_string(),
        _ => format!("更新服务器返回 HTTP {code}"),
    }
}

/// 多处失败时优先返回信息量更大的那条。
fn pick_error(errors: &[String]) -> String {
    for key in ["频繁", "还没有已发布"] {
        if let Some(e) = errors.iter().find(|e| e.contains(key)) {
            return e.clone();
        }
    }
    "无法连接更新服务器，请检查网络或代理".to_string()
}

pub(crate) fn parse_release(body: &str) -> Result<ReleaseInfo, String> {
    let r: ReleaseJson = serde_json::from_str(body).map_err(|e| {
        dlog::warn(&format!("update: 解析 Release JSON 失败：{e}"));
        "更新信息格式异常".to_string()
    })?;
    let version = normalize_version(&r.tag_name).ok_or_else(|| "发布版本号格式异常".to_string())?;
    let asset = pick_asset(&r.assets).ok_or_else(|| "发布里没有找到 Windows 安装包".to_string())?;
    if !asset.browser_download_url.starts_with("https://") {
        return Err("安装包下载地址不是 HTTPS".to_string());
    }
    let asset_name =
        safe_file_name(&asset.name).ok_or_else(|| "安装包文件名异常".to_string())?;
    let body = r.body.unwrap_or_default();
    Ok(ReleaseInfo {
        version,
        notes: truncate_notes(&strip_min_marker(&body), 8 * 1024),
        url: r.html_url,
        asset_name,
        asset_url: asset.browser_download_url.clone(),
        asset_size: asset.size,
        digest: asset.digest.clone().filter(|d| !d.trim().is_empty()),
        min_version: parse_min_version(&body),
    })
}

/// 最低版本标记是给程序看的，不能出现在更新说明里。
fn strip_min_marker(body: &str) -> String {
    const MARK: &str = "vexto-min-version:";
    body.lines()
        .filter(|line| !line.to_ascii_lowercase().contains(MARK))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 从 Release 说明里读 `<!-- vexto-min-version: x.y.z -->`。
///
/// 用隐藏标记而不是单独文件：发布时只多写一行，读取仍走同一个 releases/latest。
fn parse_min_version(body: &str) -> Option<String> {
    const MARK: &str = "vexto-min-version:";
    for line in body.lines() {
        let lower = line.to_ascii_lowercase();
        let Some(pos) = lower.find(MARK) else {
            continue;
        };
        let rest = &line[pos + MARK.len()..];
        let rest = rest.split("-->").next().unwrap_or(rest).trim();
        if let Some(v) = normalize_version(rest) {
            return Some(v);
        }
    }
    None
}

fn pick_asset(assets: &[AssetJson]) -> Option<&AssetJson> {
    assets.iter().find(|a| {
        safe_file_name(&a.name)
            .map(|n| n.to_ascii_lowercase().ends_with("_x64-setup.exe"))
            .unwrap_or(false)
    })
}

/// 只保留文件名本身，挡住 `../`、盘符、反斜杠路径这些写法。
fn safe_file_name(name: &str) -> Option<String> {
    let base = name.rsplit(['/', '\\']).next()?.trim();
    if base.is_empty() || base == "." || base == ".." || base.contains(':') {
        return None;
    }
    Some(base.to_string())
}

fn truncate_notes(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

// ── 版本比较 ────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
struct Version {
    nums: [u64; 3],
    pre: Vec<PreId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PreId {
    Num(u64),
    Text(String),
}

/// 归一化 tag：去 `v`、校验格式，返回可展示的版本号。
fn normalize_version(raw: &str) -> Option<String> {
    let s = raw.trim();
    let s = s.strip_prefix('v').or_else(|| s.strip_prefix('V')).unwrap_or(s);
    let v = parse_version(s)?;
    Some(format_version(&v))
}

fn format_version(v: &Version) -> String {
    let mut out = format!("{}.{}.{}", v.nums[0], v.nums[1], v.nums[2]);
    if !v.pre.is_empty() {
        let pre: Vec<String> = v
            .pre
            .iter()
            .map(|p| match p {
                PreId::Num(n) => n.to_string(),
                PreId::Text(s) => s.clone(),
            })
            .collect();
        out.push('-');
        out.push_str(&pre.join("."));
    }
    out
}

fn parse_version(raw: &str) -> Option<Version> {
    let s = raw.trim();
    let s = s.strip_prefix('v').or_else(|| s.strip_prefix('V')).unwrap_or(s);
    // 构建元数据（+...）不参与比较
    let s = s.split('+').next().unwrap_or("");
    if s.is_empty() {
        return None;
    }
    let (core, pre) = match s.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (s, None),
    };
    let parts: Vec<&str> = core.split('.').collect();
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    let mut nums = [0u64; 3];
    for (i, p) in parts.iter().enumerate() {
        if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        nums[i] = p.parse().ok()?;
    }
    let mut pre_ids = Vec::new();
    if let Some(p) = pre {
        if p.is_empty() {
            return None;
        }
        for id in p.split('.') {
            if id.is_empty() {
                return None;
            }
            if id.bytes().all(|b| b.is_ascii_digit()) {
                pre_ids.push(PreId::Num(id.parse().ok()?));
            } else {
                if !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
                    return None;
                }
                pre_ids.push(PreId::Text(id.to_ascii_lowercase()));
            }
        }
    }
    Some(Version {
        nums,
        pre: pre_ids,
    })
}

fn cmp_version(a: &Version, b: &Version) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match a.nums.cmp(&b.nums) {
        Ordering::Equal => {}
        other => return other,
    }
    match (a.pre.is_empty(), b.pre.is_empty()) {
        (true, true) => Ordering::Equal,
        // 正式版高于同号预发布版
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) => {
            for (x, y) in a.pre.iter().zip(b.pre.iter()) {
                let o = match (x, y) {
                    (PreId::Num(n1), PreId::Num(n2)) => n1.cmp(n2),
                    // 数字标识低于字母标识（semver 惯例）
                    (PreId::Num(_), PreId::Text(_)) => Ordering::Less,
                    (PreId::Text(_), PreId::Num(_)) => Ordering::Greater,
                    (PreId::Text(s1), PreId::Text(s2)) => s1.cmp(s2),
                };
                if o != Ordering::Equal {
                    return o;
                }
            }
            a.pre.len().cmp(&b.pre.len())
        }
    }
}

/// `latest` 是否比 `current` 新。解析失败返回 Err，调用方当作「无法比较」，
/// 不能因此误报有新版本。
pub fn is_newer(latest: &str, current: &str) -> Result<bool, String> {
    let l = parse_version(latest).ok_or_else(|| format!("无法识别版本号 {latest}"))?;
    let c = parse_version(current).ok_or_else(|| format!("无法识别版本号 {current}"))?;
    Ok(cmp_version(&l, &c) == std::cmp::Ordering::Greater)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_compare_handles_numeric_order() {
        assert!(is_newer("0.10.0", "0.9.9").unwrap());
        assert!(is_newer("v0.3.0", "0.2.2").unwrap());
        assert!(!is_newer("0.2.2", "0.2.2").unwrap());
        assert!(is_newer("0.2", "0.1.9").unwrap());
        assert!(!is_newer("0.2", "0.2.0").unwrap());
    }

    #[test]
    fn version_compare_handles_prerelease() {
        assert!(is_newer("1.0.0", "1.0.0-beta.2").unwrap());
        assert!(!is_newer("1.0.0-beta.2", "1.0.0").unwrap());
        assert!(is_newer("1.0.0-beta.10", "1.0.0-beta.2").unwrap());
        assert!(!is_newer("0.2.2-beta.1", "0.2.2").unwrap());
        assert!(is_newer("0.2.2-rc.1", "0.2.2-beta.9").unwrap());
    }

    #[test]
    fn version_compare_rejects_garbage() {
        assert!(is_newer("banana", "0.2.2").is_err());
        assert!(is_newer("0.2.2", "0.2.2.1").is_err());
        assert!(is_newer("0.2.2", "").is_err());
    }

    #[test]
    fn normalize_version_strips_v_and_build_metadata() {
        assert_eq!(normalize_version("v0.3.0").as_deref(), Some("0.3.0"));
        assert_eq!(normalize_version("0.2+abc").as_deref(), Some("0.2.0"));
        assert_eq!(
            normalize_version("V1.2.3-beta.1").as_deref(),
            Some("1.2.3-beta.1")
        );
        assert!(normalize_version("release-1").is_none());
    }

    #[test]
    fn parse_release_picks_the_nsis_asset_and_keeps_digest() {
        let body = r#"{
            "tag_name": "v0.3.0",
            "body": "更新说明",
            "html_url": "https://github.com/Ruxsmiles/vexto/releases/tag/v0.3.0",
            "assets": [
                {"name": "source.tar.gz", "size": 10, "browser_download_url": "https://github.com/x/source.tar.gz"},
                {"name": "Vexto_0.3.0_x64-setup.exe", "size": 22154793,
                 "digest": "sha256:acd7081b729453de5d2524e6564f4166b906621fd588a29d8708e7294d06f749",
                 "browser_download_url": "https://github.com/Ruxsmiles/vexto/releases/download/v0.3.0/Vexto_0.3.0_x64-setup.exe"}
            ]
        }"#;
        let info = parse_release(body).unwrap();
        assert_eq!(info.version, "0.3.0");
        assert_eq!(info.asset_name, "Vexto_0.3.0_x64-setup.exe");
        assert_eq!(info.asset_size, 22154793);
        assert!(info.digest.unwrap().starts_with("sha256:"));
    }

    #[test]
    fn parse_release_reports_missing_asset() {
        let body = r#"{"tag_name":"v0.3.0","assets":[]}"#;
        assert!(parse_release(body).unwrap_err().contains("安装包"));
    }

    #[test]
    fn parse_release_rejects_non_https_asset() {
        let body = r#"{"tag_name":"v0.3.0","assets":[
            {"name":"Vexto_0.3.0_x64-setup.exe","size":1,"browser_download_url":"http://evil/x.exe"}
        ]}"#;
        assert!(parse_release(body).unwrap_err().contains("HTTPS"));
    }

    #[test]
    fn parse_release_accepts_missing_digest() {
        let body = r#"{"tag_name":"v0.3.0","assets":[
            {"name":"Vexto_0.3.0_x64-setup.exe","size":1,"browser_download_url":"https://github.com/x/y.exe"}
        ]}"#;
        assert!(parse_release(body).unwrap().digest.is_none());
        assert!(parse_release(body).unwrap().min_version.is_none());
    }

    #[test]
    fn min_version_marker_is_parsed() {
        assert_eq!(
            parse_min_version("说明\n<!-- vexto-min-version: 0.4.0 -->\n更多说明").as_deref(),
            Some("0.4.0")
        );
        assert_eq!(
            parse_min_version("<!-- VEXTO-MIN-VERSION: v0.4 -->").as_deref(),
            Some("0.4.0")
        );
        assert_eq!(
            parse_min_version("最低版本 <!-- vexto-min-version: 0.4.0 --> 请升级").as_deref(),
            Some("0.4.0")
        );
        assert!(parse_min_version("没有标记").is_none());
        assert!(parse_min_version("<!-- vexto-min-version: banana -->").is_none());
    }

    #[test]
    fn min_version_marker_is_hidden_from_notes() {
        let body = "<!-- vexto-min-version: 0.4.0 -->\n\n## 变更\n- 一些说明";
        assert_eq!(strip_min_marker(body), "\n## 变更\n- 一些说明");
    }

    #[test]
    fn safe_file_name_strips_paths() {
        assert_eq!(
            safe_file_name("..\\..\\evil\\Vexto_0.3.0_x64-setup.exe").as_deref(),
            Some("Vexto_0.3.0_x64-setup.exe")
        );
        // 带盘符的写法取最后一段，不是拒绝；真正的危险是「文件名里带冒号」
        assert_eq!(safe_file_name("C:\\x.exe").as_deref(), Some("x.exe"));
        assert!(safe_file_name("a:b.exe").is_none());
        assert!(safe_file_name("   ").is_none());
    }

    #[test]
    fn notes_are_truncated_at_char_boundary() {
        let s = "更".repeat(5000);
        let out = truncate_notes(&s, 8 * 1024);
        assert!(out.ends_with('…'));
        assert!(out.len() <= 8 * 1024 + 3);
    }
}
