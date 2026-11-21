//! GitHub Releases 查询（下载页「GitHub 加速」）。
//!
//! 只读公开仓库：`/releases/latest` 或 `/releases?per_page=30&page=N`。
//! 请求复用「下载代理」配置（含 socks5），令牌只用于 `api.github.com`，
//! 资产下载仍交给 aria2 的全局 `all-proxy`。
//!
//! 解析、平台分类、错误文案都拆成纯函数；只有 [`fetch`] 碰网络。

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::config;
use crate::dlog;

const API: &str = "https://api.github.com";
const PER_PAGE: u32 = 30;
const MAX_PAGE: u32 = 20;
const CACHE_TTL: Duration = Duration::from_secs(600);
const CACHE_MAX: usize = 16;
/// 缓存总字节上限。常驻托盘程序不能因为「解析过很多仓库」把几十 MB
/// JSON 一直攥在内存里；超出时按最久未用淘汰。
const CACHE_BYTES: usize = 8 * 1024 * 1024;
/// 单次响应上限。30 个版本的正常 JSON 远小于这个值；设上限是为了不让
/// 异常仓库把整块响应读进内存。
const MAX_BODY: u64 = 8 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoRef {
    pub owner: String,
    pub repo: String,
}

impl RepoRef {
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Windows,
    Other,
    Unknown,
    Aux,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Asset {
    pub name: String,
    pub size: u64,
    pub downloads: u64,
    pub url: String,
    pub platform: Platform,
    /// 「仅显示本设备」开启时是否保留。未知平台也保留：文件名没写平台
    /// 不代表不能用，误杀比多留一条更糟。
    pub fits: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    pub tag: String,
    pub name: String,
    pub published_at: String,
    pub prerelease: bool,
    pub html_url: String,
    pub assets: Vec<Asset>,
    pub hidden_assets: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloneInfo {
    pub raw: String,
    pub url: String,
    pub command: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleasesOut {
    pub repo: String,
    pub releases: Vec<Release>,
    pub page: u32,
    pub has_more: bool,
    pub rate_limit: Option<u32>,
    pub rate_remaining: Option<u32>,
    pub rate_reset: Option<i64>,
    pub cached: bool,
    /// 兜底提示（例如「没有正式版，以下为预发布」）。
    pub message: Option<String>,
    /// 仓库 clone 信息；线路按当前配置现算，不进 API 缓存。
    pub clone: CloneInfo,
}

// ── 输入解析 ────────────────────────────────────────────────────────────

/// 接受完整 URL、`owner/repo` 简写、SSH 地址，以及带 `/releases/...` 的地址。
pub fn parse_repo(input: &str) -> Result<RepoRef, String> {
    let raw = input.trim();
    if raw.is_empty() {
        return Err("请输入 GitHub 仓库地址".to_string());
    }
    let lower = raw.to_ascii_lowercase();
    let mut s = raw;

    if lower.starts_with("git@github.com:") {
        s = &raw["git@github.com:".len()..];
    } else if lower.starts_with("ssh://git@github.com/") {
        s = &raw["ssh://git@github.com/".len()..];
    } else if let Some((scheme, rest)) = raw.split_once("://") {
        if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
            return Err("只支持 github.com 的公开仓库".to_string());
        }
        let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
        let host = host.split('@').next().unwrap_or(host);
        let host = host.split(':').next().unwrap_or(host);
        if !host.eq_ignore_ascii_case("github.com") && !host.eq_ignore_ascii_case("www.github.com")
        {
            return Err("只支持 github.com 的公开仓库".to_string());
        }
        s = path;
    } else if lower.starts_with("github.com/") {
        s = &raw["github.com/".len()..];
    } else if lower.starts_with("www.github.com/") {
        s = &raw["www.github.com/".len()..];
    }

    let s = s.split(['?', '#']).next().unwrap_or(s);
    let parts: Vec<&str> = s.trim_matches('/').split('/').collect();
    if parts.len() < 2 || parts[0].is_empty() || parts[1].is_empty() {
        return Err("地址里缺少仓库名，应为 https://github.com/owner/repo".to_string());
    }
    let owner = parts[0];
    let mut repo = parts[1];
    if let Some(stripped) = repo.strip_suffix(".git") {
        repo = stripped;
    }
    if !valid_segment(owner, 39, false) || !valid_segment(repo, 100, true) {
        return Err("仓库名格式不正确，应为 https://github.com/owner/repo".to_string());
    }
    Ok(RepoRef {
        owner: owner.to_string(),
        repo: repo.to_string(),
    })
}

fn valid_segment(s: &str, max: usize, allow_dot: bool) -> bool {
    !s.is_empty()
        && s.len() <= max
        && s != "."
        && s != ".."
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || (allow_dot && c == '.'))
}

/// 资产下载线路。与更新模块的线路保持一致：v4 优先，gh-proxy 次之。
pub const MIRRORS: [&str; 2] = ["https://v4.gh-proxy.org/", "https://gh-proxy.org/"];

/// 把用户填的自定义前缀规范成 `https://host/` 形式。
fn normalize_prefix(raw: &str) -> Option<String> {
    let s = raw.trim();
    let lower = s.to_ascii_lowercase();
    let rest = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))?;
    if rest.is_empty() {
        return None;
    }
    let mut out = s.to_string();
    if !out.ends_with('/') {
        out.push('/');
    }
    Some(out)
}

/// 按「下载线路」配置生成候选。
///
/// - `auto`：v4 → gh-proxy → 直连；
/// - `direct`：原始地址；
/// - `custom`：自定义前缀 + 原始地址（与更新模块同语义）。
pub fn line_candidates(url: &str, source: &str, custom: &str) -> Result<Vec<String>, String> {
    match source.trim().to_ascii_lowercase().as_str() {
        "direct" => Ok(vec![url.to_string()]),
        "custom" => {
            let prefix = normalize_prefix(custom)
                .ok_or_else(|| "自定义代理地址需要以 http:// 或 https:// 开头".to_string())?;
            Ok(vec![format!("{prefix}{url}")])
        }
        _ => {
            let mut out: Vec<String> = MIRRORS.iter().map(|p| format!("{p}{url}")).collect();
            out.push(url.to_string());
            Ok(out)
        }
    }
}

/// 下载线路：配置了下载代理时让位给代理，否则按 GitHub 线路配置。
pub fn download_candidates(url: &str, cfg: &config::Config) -> Result<Vec<String>, String> {
    if crate::download::proxy::resolve(cfg).is_some() {
        return Ok(vec![url.to_string()]);
    }
    line_candidates(url, &cfg.update_download_source, &cfg.update_custom_proxy)
}

/// 仓库 clone 的展示信息与复制命令。线路按当前配置现算。
pub fn clone_info(repo: &RepoRef, cfg: &config::Config) -> CloneInfo {
    let raw = format!("https://github.com/{}.git", repo.full_name());
    let url = line_candidates(&raw, &cfg.update_download_source, &cfg.update_custom_proxy)
        .ok()
        .and_then(|v| v.into_iter().next())
        .unwrap_or_else(|| raw.clone());
    CloneInfo {
        raw,
        command: format!("git clone {url}"),
        url,
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloneResult {
    pub path: String,
    pub url: String,
}

/// 后台 clone 一个公开仓库到 `<parent>/<repo>`。
///
/// 线路依次尝试（auto 时 v4 → gh-proxy → 直连）；每一轮失败都清理本轮
/// 创建的半成品目录，再换下一个地址。git 输出不落盘、不进日志，失败时
/// 只把最后一段 stderr 返回给界面。
pub fn clone_repo(
    repo: &RepoRef,
    parent: &Path,
    cfg: &config::Config,
) -> Result<CloneResult, String> {
    if !parent.is_dir() {
        return Err("所选位置不是文件夹".to_string());
    }
    let dest = parent.join(&repo.repo);
    if dest.exists() {
        return Err(format!("目标目录已存在：{}", dest.display()));
    }
    let raw = format!("https://github.com/{}.git", repo.full_name());
    let candidates = line_candidates(&raw, &cfg.update_download_source, &cfg.update_custom_proxy)?;
    ensure_git()?;

    let mut last = String::new();
    for url in candidates {
        match run_git_clone(&url, &dest) {
            Ok(()) => {
                return Ok(CloneResult {
                    path: dest.display().to_string(),
                    url,
                })
            }
            Err(e) => {
                last = e;
                if dest.exists() {
                    let _ = std::fs::remove_dir_all(&dest);
                }
            }
        }
    }
    Err(format!("clone 失败：{last}"))
}

fn git_command() -> std::process::Command {
    let mut cmd = std::process::Command::new("git");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

fn ensure_git() -> Result<(), String> {
    let out = git_command()
        .arg("--version")
        .output()
        .map_err(|_| "未找到 git，请先安装 Git for Windows".to_string())?;
    if !out.status.success() {
        return Err("git 无法正常运行，请检查 Git for Windows 安装".to_string());
    }
    Ok(())
}

fn run_git_clone(url: &str, dest: &Path) -> Result<(), String> {
    let out = git_command()
        .arg("clone")
        .arg("--progress")
        .arg(url)
        .arg(dest)
        // 公开仓库遇到需要认证时直接失败，不要卡在凭据输入上。
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .output()
        .map_err(|e| format!("启动 git 失败：{e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let text = stderr.trim();
    if text.is_empty() {
        return Err(format!("git 退出码 {}", out.status));
    }
    let tail: String = text
        .chars()
        .rev()
        .take(1200)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    Err(tail)
}

/// 资产下载地址必须是 GitHub 的 release 下载路径；新建任务命令会再校验一次。
pub fn validate_asset_url(url: &str) -> Result<(), String> {
    let u = url.trim();
    let rest = u
        .strip_prefix("https://github.com/")
        .ok_or_else(|| "不是 GitHub 的下载地址".to_string())?;
    let mut parts = rest.split('/');
    let owner = parts.next().unwrap_or("");
    let repo = parts.next().unwrap_or("");
    let releases = parts.next().unwrap_or("");
    let download = parts.next().unwrap_or("");
    let tag = parts.next().unwrap_or("");
    let file = parts.next().unwrap_or("");
    if owner.is_empty()
        || repo.is_empty()
        || releases != "releases"
        || download != "download"
        || tag.is_empty()
        || file.is_empty()
    {
        return Err("不是 GitHub Release 的下载地址".to_string());
    }
    Ok(())
}

// ── 平台分类 ────────────────────────────────────────────────────────────

const OTHER_OS: &[&str] = &[
    "linux", "darwin", "macos", "mac", "osx", "apple", "android", "ios", "freebsd", "openbsd",
    "netbsd",
];
const OTHER_ARCH: &[&str] = &[
    "arm", "arm64", "aarch64", "armv6", "armv7", "armhf", "i386", "i686", "x86", "32bit",
    "riscv64", "ppc64", "ppc64le", "s390x",
];
const WINDOWS: &[&str] = &[
    "win", "win32", "win64", "windows", "x64", "amd64", "win32bit",
];
const NON_WINDOWS_EXT: &[&str] = &[
    ".dmg",
    ".pkg",
    ".deb",
    ".rpm",
    ".appimage",
    ".snap",
    ".flatpak",
    ".apk",
    ".ipa",
];
const AUX_EXT: &[&str] = &[
    ".sig",
    ".asc",
    ".sha1",
    ".sha256",
    ".sha512",
    ".md5",
    ".pem",
    ".blockmap",
    ".zsync",
];

/// 按文件名判断资源适合哪类设备。保守：认不出来就归为 `Unknown`，默认保留。
pub fn classify_asset(name: &str) -> Platform {
    let lower = name.to_ascii_lowercase();
    if AUX_EXT.iter().any(|e| lower.ends_with(e))
        || has_token(
            &lower,
            &[
                "checksum",
                "checksums",
                "sha256sum",
                "sha256sums",
                "shasums",
            ],
        )
    {
        return Platform::Aux;
    }
    if NON_WINDOWS_EXT.iter().any(|e| lower.ends_with(e)) {
        return Platform::Other;
    }
    if has_token(&lower, OTHER_OS) {
        return Platform::Other;
    }
    // x86_64 会被分隔符切成 x86 + 64，必须在 32 位判断之前单独认出来。
    if lower.contains("x86_64") || lower.contains("x86-64") {
        return Platform::Windows;
    }
    if has_token(&lower, OTHER_ARCH) {
        return Platform::Other;
    }
    if has_token(&lower, WINDOWS) {
        return Platform::Windows;
    }
    Platform::Unknown
}

fn has_token(lower: &str, words: &[&str]) -> bool {
    lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|t| words.contains(&t))
}

fn rank(p: Platform) -> u8 {
    match p {
        Platform::Windows => 0,
        Platform::Unknown => 1,
        Platform::Other => 2,
        Platform::Aux => 3,
    }
}

// ── JSON 解析 ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ReleaseJson {
    #[serde(default)]
    tag_name: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    assets: Vec<AssetJson>,
}

#[derive(Debug, Deserialize)]
struct AssetJson {
    #[serde(default)]
    name: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    download_count: u64,
    #[serde(default)]
    browser_download_url: String,
}

fn parse_latest(body: &str) -> Result<Release, String> {
    let r: ReleaseJson = serde_json::from_str(body).map_err(|e| {
        dlog::warn(&format!("github: 解析 Release JSON 失败：{e}"));
        "GitHub 返回的数据格式异常".to_string()
    })?;
    build_release(r).ok_or_else(|| "该仓库没有可用的 Release".to_string())
}

fn parse_list(body: &str) -> Result<Vec<Release>, String> {
    let list: Vec<ReleaseJson> = serde_json::from_str(body).map_err(|e| {
        dlog::warn(&format!("github: 解析 Releases JSON 失败：{e}"));
        "GitHub 返回的数据格式异常".to_string()
    })?;
    Ok(list.into_iter().filter_map(build_release).collect())
}

fn parse_body(body: &str, all: bool) -> Result<Vec<Release>, String> {
    if all {
        parse_list(body)
    } else {
        parse_latest(body).map(|r| vec![r])
    }
}

fn build_release(r: ReleaseJson) -> Option<Release> {
    if r.draft {
        return None;
    }
    let tag = r.tag_name.trim().to_string();
    if tag.is_empty() {
        return None;
    }
    let mut assets: Vec<Asset> = r.assets.into_iter().filter_map(build_asset).collect();
    assets.sort_by_key(|a| rank(a.platform));
    let hidden_assets = assets.iter().filter(|a| !a.fits).count();
    Some(Release {
        name: r.name.unwrap_or_else(|| tag.clone()),
        tag,
        published_at: r
            .published_at
            .map(|s| s.chars().take(10).collect())
            .unwrap_or_default(),
        prerelease: r.prerelease,
        html_url: if r.html_url.starts_with("https://github.com/") {
            r.html_url
        } else {
            String::new()
        },
        assets,
        hidden_assets,
    })
}

fn build_asset(a: AssetJson) -> Option<Asset> {
    let name = safe_asset_name(&a.name)?;
    validate_asset_url(&a.browser_download_url).ok()?;
    let platform = classify_asset(&name);
    Some(Asset {
        name,
        size: a.size,
        downloads: a.download_count,
        url: a.browser_download_url,
        platform,
        fits: matches!(platform, Platform::Windows | Platform::Unknown),
    })
}

fn safe_asset_name(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() || s == "." || s == ".." || s.chars().count() > 240 {
        return None;
    }
    if s.chars()
        .any(|c| matches!(c, '/' | '\\' | ':') || (c as u32) < 0x20 || c as u32 == 0x7f)
    {
        return None;
    }
    Some(s.to_string())
}

// ── 网络与缓存 ──────────────────────────────────────────────────────────

#[derive(Clone)]
struct CacheEntry {
    body: String,
    has_more: bool,
    rate_limit: Option<u32>,
    rate_remaining: Option<u32>,
    rate_reset: Option<i64>,
    at: Instant,
}

static CACHE: OnceLock<Mutex<HashMap<String, CacheEntry>>> = OnceLock::new();

fn cache() -> &'static Mutex<HashMap<String, CacheEntry>> {
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cache_get(key: &str) -> Option<CacheEntry> {
    let mut c = cache().lock().unwrap_or_else(|e| e.into_inner());
    match c.get(key) {
        Some(e) if e.at.elapsed() < CACHE_TTL => Some(e.clone()),
        Some(_) => {
            c.remove(key);
            None
        }
        None => None,
    }
}

fn cache_put(key: String, f: &Fetched) {
    let bytes = f.body.len();
    if bytes > CACHE_BYTES {
        return;
    }
    let mut c = cache().lock().unwrap_or_else(|e| e.into_inner());
    // 覆盖同一个 key 时先扣掉旧条目，否则反复刷新会让总量虚高。
    let had_old = c.remove(&key).is_some();
    if c.len() >= CACHE_MAX && !had_old {
        if let Some(old) = c.iter().min_by_key(|(_, v)| v.at).map(|(k, _)| k.clone()) {
            c.remove(&old);
        }
    }
    while c.values().map(|e| e.body.len()).sum::<usize>() + bytes > CACHE_BYTES {
        let Some(old) = c.iter().min_by_key(|(_, v)| v.at).map(|(k, _)| k.clone()) else {
            break;
        };
        c.remove(&old);
    }
    c.insert(
        key,
        CacheEntry {
            body: f.body.clone(),
            has_more: f.has_more,
            rate_limit: f.rate_limit,
            rate_remaining: f.rate_remaining,
            rate_reset: f.rate_reset,
            at: Instant::now(),
        },
    );
}

struct Fetched {
    body: String,
    has_more: bool,
    rate_limit: Option<u32>,
    rate_remaining: Option<u32>,
    rate_reset: Option<i64>,
}

struct ApiError {
    message: String,
    not_found: bool,
}

fn user_agent() -> String {
    format!(
        "Vexto/{} (+https://github.com/Boom-Sumber/vexto)",
        env!("CARGO_PKG_VERSION")
    )
}

fn client(cfg: &config::Config) -> Result<reqwest::blocking::Client, String> {
    let mut b = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(25))
        .user_agent(user_agent());
    if let Some(p) = crate::download::proxy::resolve(cfg) {
        let proxy = reqwest::Proxy::all(&p.url)
            .map_err(|e| format!("代理地址无法用于 GitHub 接口：{e}"))?;
        let proxy = match (p.user.as_deref(), p.passwd.as_deref()) {
            (Some(u), Some(pw)) => proxy.basic_auth(u, pw),
            (Some(u), None) => proxy.basic_auth(u, ""),
            _ => proxy,
        };
        b = b.proxy(proxy);
    }
    b.build().map_err(|e| format!("无法创建 GitHub 请求：{e}"))
}

fn request(
    cfg: &config::Config,
    repo: &RepoRef,
    all: bool,
    page: u32,
) -> Result<Fetched, ApiError> {
    let url = if all {
        format!(
            "{API}/repos/{}/releases?per_page={PER_PAGE}&page={page}",
            repo.full_name()
        )
    } else {
        format!("{API}/repos/{}/releases/latest", repo.full_name())
    };
    let client = client(cfg).map_err(|message| ApiError {
        message,
        not_found: false,
    })?;
    let mut req = client
        .get(&url)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28");
    let token = cfg.github_token.trim();
    if !token.is_empty() {
        if token.contains(['\r', '\n']) {
            return Err(ApiError {
                message: "GitHub 访问令牌格式不正确".to_string(),
                not_found: false,
            });
        }
        req = req.header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"));
    }

    let resp = req.send().map_err(|e| {
        dlog::dlog(&format!("github: 请求 {url} 失败：{e}"));
        let message = if e.is_timeout() {
            "连接 GitHub 超时，请检查网络或下载代理设置"
        } else {
            "无法连接 GitHub API，请检查网络或下载代理设置"
        };
        ApiError {
            message: message.to_string(),
            not_found: false,
        }
    })?;

    let status = resp.status().as_u16();
    let rate_limit = header_u32(&resp, "x-ratelimit-limit");
    let rate_remaining = header_u32(&resp, "x-ratelimit-remaining");
    let rate_reset = header_i64(&resp, "x-ratelimit-reset");
    let has_more = has_next_link(&resp);
    if !(200..300).contains(&status) {
        return Err(ApiError {
            message: status_error(status, rate_remaining, rate_reset),
            not_found: status == 404,
        });
    }

    let mut body = String::new();
    resp.take(MAX_BODY + 1)
        .read_to_string(&mut body)
        .map_err(|e| ApiError {
            message: format!("读取 GitHub 响应失败：{e}"),
            not_found: false,
        })?;
    if body.len() as u64 > MAX_BODY {
        return Err(ApiError {
            message: "GitHub 返回的数据过大，已中止".to_string(),
            not_found: false,
        });
    }
    Ok(Fetched {
        body,
        has_more,
        rate_limit,
        rate_remaining,
        rate_reset,
    })
}

fn header_u32(resp: &reqwest::blocking::Response, name: &str) -> Option<u32> {
    resp.headers().get(name)?.to_str().ok()?.trim().parse().ok()
}

fn header_i64(resp: &reqwest::blocking::Response, name: &str) -> Option<i64> {
    resp.headers().get(name)?.to_str().ok()?.trim().parse().ok()
}

fn has_next_link(resp: &reqwest::blocking::Response) -> bool {
    resp.headers()
        .get("link")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(',').any(|p| p.contains("rel=\"next\"")))
        .unwrap_or(false)
}

fn status_error(code: u16, remaining: Option<u32>, reset: Option<i64>) -> String {
    match code {
        401 => "GitHub 访问令牌无效，请在 设置 → 下载 → 网络 中检查或清空".to_string(),
        403 | 429 if remaining == Some(0) => rate_limit_message(reset),
        403 => "GitHub 拒绝了请求（403），请稍后再试".to_string(),
        404 => "仓库不存在，或它是私有仓库（当前只支持公开仓库）".to_string(),
        422 => "仓库名格式不正确".to_string(),
        500..=599 => format!("GitHub 服务暂时不可用（HTTP {code}）"),
        _ => format!("GitHub 返回 HTTP {code}"),
    }
}

fn rate_limit_message(reset: Option<i64>) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let wait = reset.map(|r| r - now).unwrap_or(0);
    if wait > 0 {
        let mins = (wait + 59) / 60;
        format!(
            "GitHub 接口限额已用完，约 {mins} 分钟后恢复；可在 设置 → 下载 → 网络 填入访问令牌提高额度"
        )
    } else {
        "GitHub 接口限额已用完，请稍后再试；可在 设置 → 下载 → 网络 填入访问令牌提高额度"
            .to_string()
    }
}

// ── 对命令暴露的入口 ────────────────────────────────────────────────────

/// 查询一个仓库的 Release。`all = false` 只取最新正式版；否则按页取全部。
pub fn fetch(
    cfg: &config::Config,
    input: &str,
    all: bool,
    page: u32,
    refresh: bool,
) -> Result<ReleasesOut, String> {
    let repo = parse_repo(input)?;
    if page > MAX_PAGE {
        return Ok(ReleasesOut {
            repo: repo.full_name(),
            releases: Vec::new(),
            page: MAX_PAGE,
            has_more: false,
            rate_limit: None,
            rate_remaining: None,
            rate_reset: None,
            cached: false,
            message: Some("已到加载上限（最多 600 个版本）".to_string()),
            clone: clone_info(&repo, cfg),
        });
    }
    let page = page.max(1);
    let token_mode = if cfg.github_token.trim().is_empty() {
        "anon"
    } else {
        "token"
    };
    let key = cache_key(&repo, all, page, token_mode);

    if !refresh {
        if let Some(hit) = cache_get(&key) {
            return Ok(ReleasesOut {
                repo: repo.full_name(),
                releases: parse_body(&hit.body, all)?,
                page,
                has_more: hit.has_more && page < MAX_PAGE,
                rate_limit: hit.rate_limit,
                rate_remaining: hit.rate_remaining,
                rate_reset: hit.rate_reset,
                cached: true,
                message: None,
                clone: clone_info(&repo, cfg),
            });
        }
    }

    let fetched = match request(cfg, &repo, all, page) {
        Ok(f) => f,
        Err(e) if e.not_found && !all => {
            // `/releases/latest` 在「只有预发布」和「仓库不存在」两种情况下都
            // 返回 404。再取一次列表页区分，同时给用户一个预发布兜底。
            let fallback = request(cfg, &repo, true, 1).map_err(|e2| e2.message)?;
            let releases = parse_list(&fallback.body)?;
            if releases.is_empty() {
                return Err("该仓库还没有发布 Release".to_string());
            }
            let message = if releases.iter().any(|r| !r.prerelease) {
                "GitHub 未返回正式版信息，以下是最近的版本"
            } else {
                "该仓库没有正式版，以下是预发布版本"
            };
            cache_put(cache_key(&repo, true, 1, token_mode), &fallback);
            return Ok(ReleasesOut {
                repo: repo.full_name(),
                releases,
                page: 1,
                has_more: fallback.has_more,
                rate_limit: fallback.rate_limit,
                rate_remaining: fallback.rate_remaining,
                rate_reset: fallback.rate_reset,
                cached: false,
                message: Some(message.to_string()),
                clone: clone_info(&repo, cfg),
            });
        }
        Err(e) => return Err(e.message),
    };

    let releases = parse_body(&fetched.body, all)?;
    cache_put(key, &fetched);
    Ok(ReleasesOut {
        repo: repo.full_name(),
        releases,
        page,
        has_more: fetched.has_more && page < MAX_PAGE,
        rate_limit: fetched.rate_limit,
        rate_remaining: fetched.rate_remaining,
        rate_reset: fetched.rate_reset,
        cached: false,
        message: None,
        clone: clone_info(&repo, cfg),
    })
}

fn cache_key(repo: &RepoRef, all: bool, page: u32, token_mode: &str) -> String {
    format!(
        "{}|{}|{}|{}",
        repo.full_name(),
        if all { "list" } else { "latest" },
        page,
        token_mode
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full(s: &str) -> String {
        match parse_repo(s) {
            Ok(r) => r.full_name(),
            Err(e) => panic!("{s} 应能解析：{e}"),
        }
    }

    #[test]
    fn parse_repo_accepts_common_forms() {
        assert_eq!(
            full("https://github.com/Boom-Sumber/vexto"),
            "Boom-Sumber/vexto"
        );
        assert_eq!(
            full("https://github.com/Boom-Sumber/vexto/releases/tag/v0.3.9"),
            "Boom-Sumber/vexto"
        );
        assert_eq!(
            full("https://github.com/Boom-Sumber/vexto/"),
            "Boom-Sumber/vexto"
        );
        assert_eq!(
            full("https://github.com/Boom-Sumber/vexto.git"),
            "Boom-Sumber/vexto"
        );
        assert_eq!(full("Boom-Sumber/vexto"), "Boom-Sumber/vexto");
        assert_eq!(
            full("git@github.com:Boom-Sumber/vexto.git"),
            "Boom-Sumber/vexto"
        );
        assert_eq!(
            full("ssh://git@github.com/Boom-Sumber/vexto.git"),
            "Boom-Sumber/vexto"
        );
        assert_eq!(full("https://www.github.com/o/r?tab=readme#top"), "o/r");
    }

    #[test]
    fn parse_repo_rejects_other_hosts_and_bad_names() {
        assert!(parse_repo("https://gitlab.com/o/r").is_err());
        assert!(parse_repo("https://gist.github.com/o/r").is_err());
        assert!(parse_repo("https://github.com/o").is_err());
        assert!(parse_repo("https://github.com/o/r/x/y/z").is_ok());
        assert!(parse_repo("").is_err());
        assert!(parse_repo("example.com/r").is_err());
        assert!(parse_repo("https://github.com/o/../x").is_err());
    }

    #[test]
    fn asset_url_must_be_release_download() {
        assert!(
            validate_asset_url("https://github.com/o/r/releases/download/v1/app-win-x64.zip")
                .is_ok()
        );
        assert!(validate_asset_url("https://github.com/o/r/archive/v1.zip").is_err());
        assert!(validate_asset_url("https://example.com/o/r/releases/download/v1/a.zip").is_err());
        assert!(validate_asset_url("https://github.com/o/r/releases/download/v1").is_err());
    }

    #[test]
    fn line_candidates_follow_source() {
        let raw = "https://github.com/o/r/releases/download/v1/a.zip";
        let auto = line_candidates(raw, "auto", "").unwrap();
        assert_eq!(auto.len(), 3);
        assert!(auto[0].starts_with("https://v4.gh-proxy.org/"));
        assert!(auto[1].starts_with("https://gh-proxy.org/"));
        assert_eq!(auto[2], raw);
        assert_eq!(
            line_candidates(raw, "direct", "").unwrap(),
            vec![raw.to_string()]
        );
        let custom = line_candidates(raw, "custom", "https://gh-proxy.org").unwrap();
        assert_eq!(custom, vec![format!("https://gh-proxy.org/{raw}")]);
        assert!(line_candidates(raw, "custom", "127.0.0.1:7890").is_err());
    }

    #[test]
    fn clone_info_uses_the_same_line_config() {
        let repo = RepoRef {
            owner: "o".to_string(),
            repo: "r".to_string(),
        };
        let cfg = crate::config::Config::default();
        let info = clone_info(&repo, &cfg);
        assert_eq!(info.raw, "https://github.com/o/r.git");
        assert!(info.url.starts_with("https://v4.gh-proxy.org/"));
        assert!(info
            .command
            .starts_with("git clone https://v4.gh-proxy.org/"));
    }

    #[test]
    fn platform_classification_is_conservative() {
        for (name, want) in [
            ("app-win-x64.zip", Platform::Windows),
            ("app-windows-x86_64.exe", Platform::Windows),
            ("app-win64.zip", Platform::Windows),
            ("app-amd64.msi", Platform::Windows),
            ("app_1.0_x86_64.tar.gz", Platform::Windows),
            ("app-linux-x64.tar.gz", Platform::Other),
            ("app-macos-arm64.dmg", Platform::Other),
            ("app-windows-arm64.zip", Platform::Other),
            ("app-x86.zip", Platform::Other),
            ("app_amd64.deb", Platform::Other),
            ("app_x86_64.AppImage", Platform::Other),
            ("checksums.txt", Platform::Aux),
            ("app.zip.sig", Platform::Aux),
            ("SHA256SUMS", Platform::Aux),
            ("app-1.0.zip", Platform::Unknown),
            ("Setup.exe", Platform::Unknown),
        ] {
            assert_eq!(classify_asset(name), want, "{name}");
        }
    }

    #[test]
    fn parse_latest_filters_bad_assets_and_sorts_windows_first() {
        let body = r#"{
            "tag_name": "v1.2.3",
            "name": "Release 1.2.3",
            "draft": false,
            "prerelease": false,
            "published_at": "2026-11-18T10:00:00Z",
            "html_url": "https://github.com/o/r/releases/tag/v1.2.3",
            "assets": [
                {"name":"app-linux.tar.gz","size":10,"download_count":3,
                 "browser_download_url":"https://github.com/o/r/releases/download/v1.2.3/app-linux.tar.gz"},
                {"name":"app-win-x64.zip","size":20,"download_count":5,
                 "browser_download_url":"https://github.com/o/r/releases/download/v1.2.3/app-win-x64.zip"},
                {"name":"checksums.txt","size":1,"download_count":0,
                 "browser_download_url":"https://github.com/o/r/releases/download/v1.2.3/checksums.txt"},
                {"name":"../evil.zip","size":1,"download_count":0,
                 "browser_download_url":"https://github.com/o/r/releases/download/v1.2.3/evil.zip"}
            ]
        }"#;
        let r = parse_latest(body).unwrap();
        assert_eq!(r.tag, "v1.2.3");
        assert_eq!(r.published_at, "2026-11-18");
        assert_eq!(r.assets.len(), 3);
        assert_eq!(r.assets[0].name, "app-win-x64.zip");
        assert!(r.assets[0].fits);
        assert_eq!(r.assets[1].name, "app-linux.tar.gz");
        assert!(!r.assets[1].fits);
        assert_eq!(r.hidden_assets, 2);
    }

    #[test]
    fn parse_list_skips_drafts_and_keeps_prerelease_flag() {
        let body = r#"[
            {"tag_name":"v2.0.0-rc1","draft":false,"prerelease":true,
             "html_url":"https://github.com/o/r/releases/tag/v2.0.0-rc1","assets":[]},
            {"tag_name":"v2.0.0","draft":true,"prerelease":false,
             "html_url":"https://github.com/o/r/releases/tag/v2.0.0","assets":[]}
        ]"#;
        let list = parse_list(body).unwrap();
        assert_eq!(list.len(), 1);
        assert!(list[0].prerelease);
    }

    #[test]
    fn rate_limit_error_says_when_to_retry() {
        let msg = status_error(403, Some(0), Some(1_700_000_120));
        assert!(msg.contains("限额已用完"));
        assert!(msg.contains("访问令牌"));
    }
}
