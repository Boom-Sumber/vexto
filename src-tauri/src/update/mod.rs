//! 应用内检查更新。
//!
//! 链路：GitHub Releases 元数据 → 下载 NSIS 安装包（可走 gh-proxy）→ sha256 校验
//! → 用户点「立即安装」→ 以 `/UPDATE /P /R` 原地升级并重启。见 ADR-061。
//!
//! 线程边界：命令与托盘回调只负责改状态 + `spawn`；网络与写盘都在工作线程，
//! 完成后用 `update:state` 事件把整份快照推给设置页。

pub mod download;
pub mod github;
pub mod install;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

use crate::dlog;

const EVENT: &str = "update:state";
/// 下载进度事件的节流间隔。
const PROGRESS_MS: u128 = 300;

static STATE: OnceLock<Mutex<Inner>> = OnceLock::new();
static CANCEL: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Idle,
    Checking,
    Available,
    UpToDate,
    Downloading,
    Ready,
    Installing,
    Error,
}

/// 给前端的完整快照；命令返回值和事件 payload 用同一个形状。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateView {
    pub phase: Phase,
    pub current: String,
    pub latest: Option<String>,
    pub notes: Option<String>,
    pub url: Option<String>,
    pub asset_name: Option<String>,
    pub asset_size: u64,
    pub downloaded: u64,
    pub via: Option<String>,
    pub error: Option<String>,
    pub last_check: Option<i64>,
    /// Release 说明里声明的最低版本；None = 不做强制更新
    pub min_version: Option<String>,
    /// 当前版本是否低于最低版本
    pub required: bool,
}

struct Inner {
    view: UpdateView,
    /// 不直接暴露给前端内部字段
    asset_url: Option<String>,
    digest: Option<String>,
    ready_path: Option<PathBuf>,
}

impl Inner {
    fn new() -> Self {
        Self {
            view: UpdateView {
                phase: Phase::Idle,
                current: env!("CARGO_PKG_VERSION").to_string(),
                latest: None,
                notes: None,
                url: None,
                asset_name: None,
                asset_size: 0,
                downloaded: 0,
                via: None,
                error: None,
                last_check: None,
                min_version: None,
                required: false,
            },
            asset_url: None,
            digest: None,
            ready_path: None,
        }
    }

    fn reset_release(&mut self) {
        self.asset_url = None;
        self.digest = None;
        self.ready_path = None;
        self.view.asset_name = None;
        self.view.asset_size = 0;
        self.view.downloaded = 0;
        self.view.via = None;
    }
}

fn state() -> MutexGuard<'static, Inner> {
    STATE
        .get_or_init(|| Mutex::new(Inner::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

fn snapshot() -> UpdateView {
    state().view.clone()
}

/// 改状态后广播。锁必须在 emit 前释放：事件处理器可能反过来调命令。
fn publish(app: Option<&AppHandle>) {
    let view = snapshot();
    if let Some(app) = app {
        if let Err(e) = app.emit(EVENT, &view) {
            dlog::warn(&format!("update: 推送状态失败：{e}"));
        }
    }
}

fn set(app: Option<&AppHandle>, f: impl FnOnce(&mut Inner)) {
    {
        let mut g = state();
        f(&mut g);
    }
    publish(app);
}

// ── 命令 ────────────────────────────────────────────────────────────────

#[tauri::command]
pub fn update_state() -> UpdateView {
    snapshot()
}

#[tauri::command]
pub fn update_check(app: AppHandle) -> Result<UpdateView, String> {
    if matches!(
        state().view.phase,
        Phase::Checking | Phase::Downloading | Phase::Installing
    ) {
        return Ok(snapshot());
    }
    set(Some(&app), |s| {
        s.view.phase = Phase::Checking;
        s.view.error = None;
    });
    let worker = app.clone();
    std::thread::spawn(move || run_check(worker, true));
    Ok(snapshot())
}

/// 托盘 / 启动日检共用入口：不返回快照，失败只走日志。
pub fn spawn_check(app: AppHandle) {
    if let Err(e) = update_check(app) {
        dlog::warn(&format!("update: 启动检查失败：{e}"));
    }
}

#[tauri::command]
pub fn update_download(app: AppHandle) -> Result<UpdateView, String> {
    let (asset_url, name, size, digest) = {
        let g = state();
        if g.view.phase != Phase::Available {
            return Err("当前没有可下载的新版本".to_string());
        }
        let url = g
            .asset_url
            .clone()
            .ok_or_else(|| "缺少安装包地址，请重新检查更新".to_string())?;
        let name = g
            .view
            .asset_name
            .clone()
            .ok_or_else(|| "缺少安装包文件名，请重新检查更新".to_string())?;
        (url, name, g.view.asset_size, g.digest.clone())
    };
    let cfg = crate::config::load();
    let lines = download::candidates(
        &cfg.update_download_source,
        &cfg.update_custom_proxy,
        &asset_url,
    )?;

    CANCEL.store(false, Ordering::SeqCst);
    set(Some(&app), |s| {
        s.view.phase = Phase::Downloading;
        s.view.downloaded = 0;
        s.view.via = None;
        s.view.error = None;
    });
    let worker = app.clone();
    std::thread::spawn(move || run_download(worker, name, size, digest, lines));
    Ok(snapshot())
}

#[tauri::command]
pub fn update_cancel() -> UpdateView {
    CANCEL.store(true, Ordering::SeqCst);
    snapshot()
}

#[tauri::command]
pub fn update_install(app: AppHandle) -> Result<UpdateView, String> {
    let path = {
        let g = state();
        if g.view.phase != Phase::Ready {
            return Err("安装包还没准备好".to_string());
        }
        g.ready_path
            .clone()
            .ok_or_else(|| "安装包路径丢失，请重新下载".to_string())?
    };
    if !path.is_file() {
        set(Some(&app), |s| {
            s.view.phase = Phase::Error;
            s.view.error = Some("安装包不存在，请重新下载".to_string());
            s.ready_path = None;
        });
        return Err("安装包不存在，请重新下载".to_string());
    }
    set(Some(&app), |s| s.view.phase = Phase::Installing);
    if let Err(e) = install::launch(&app, &path) {
        set(Some(&app), |s| {
            s.view.phase = Phase::Ready;
            s.view.error = Some(e.clone());
        });
        return Err(e);
    }
    Ok(snapshot())
}

#[tauri::command]
pub fn update_open_release() -> Result<(), String> {
    let url = state()
        .view
        .url
        .clone()
        .ok_or_else(|| "还没有可打开的发布页".to_string())?;
    if !url.starts_with("https://") {
        return Err("发布页地址无效".to_string());
    }
    open_url(&url)
}

pub(crate) fn open_url(url: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        // 和安装器一样走系统 shell：`cmd /C start` 在部分环境（作业对象 /
        // 重定向目录）会被错误解析成 `\\`，弹「Windows 找不到」。
        use windows::core::{HSTRING, PCWSTR};
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

        if !url.starts_with("https://") {
            return Err("只允许打开 HTTPS 链接".to_string());
        }
        let op = HSTRING::from("open");
        let file = HSTRING::from(url);
        let h = unsafe {
            ShellExecuteW(
                None,
                &op,
                &file,
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };
        if h.0 as isize <= 32 {
            dlog::warn(&format!(
                "update: ShellExecuteW 打开链接失败，code={}",
                h.0 as isize
            ));
            return Err("无法打开浏览器".to_string());
        }
        dlog::dlog(&format!("update: 已通过系统 shell 打开链接 {url}"));
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = url;
        Err("仅支持 Windows".to_string())
    }
}

// ── 工作线程 ────────────────────────────────────────────────────────────

/// 当前版本是否低于 Release 声明的最低版本。解析失败时不强制。
fn is_required(min: Option<&str>, current: &str) -> bool {
    min.and_then(|m| github::is_newer(m, current).ok())
        .unwrap_or(false)
}

fn run_check(app: AppHandle, manual: bool) {
    let result = github::fetch_latest();
    let now = now_secs();
    match result {
        Ok(info) => {
            let current = env!("CARGO_PKG_VERSION");
            let newer = github::is_newer(&info.version, current).unwrap_or(false);
            // 最低版本不可能高于线上最新版；写错时忽略标记，避免永远强制。
            let min_opt = info.min_version.clone().filter(|m| {
                !github::is_newer(m, &info.version).unwrap_or(false)
            });
            let required = is_required(min_opt.as_deref(), current);
            {
                let mut g = state();
                g.view.phase = if newer { Phase::Available } else { Phase::UpToDate };
                g.view.latest = Some(info.version.clone());
                g.view.notes = Some(info.notes);
                g.view.url = if info.url.starts_with("https://") {
                    Some(info.url)
                } else {
                    None
                };
                g.reset_release();
                g.view.asset_name = Some(info.asset_name);
                g.view.asset_size = info.asset_size;
                g.view.min_version = min_opt.clone();
                g.view.required = required;
                g.asset_url = Some(info.asset_url);
                g.digest = info.digest;
                g.view.error = None;
                g.view.last_check = Some(now);
            }
            publish(Some(&app));
            crate::tray::set_update_hint(
                &app,
                if newer {
                    Some(info.version.as_str())
                } else {
                    None
                },
                required,
            );
            persist(
                now,
                Some(if newer { info.version.as_str() } else { "" }),
                Some(min_opt.as_deref().unwrap_or("")),
            );
            dlog::dlog(&format!(
                "update: 检查完成，当前 {current}，最新 {}，{}，最低版本 {}",
                info.version,
                if newer { "有新版本" } else { "已是最新" },
                min_opt.as_deref().unwrap_or("未设置")
            ));
            // 低于最低版本：不等用户点「下载并安装」，直接后台开始下载。
            // 安装仍然要用户点「立即安装」（ADR-062）。
            if required && newer {
                dlog::dlog("update: 低于最低版本，自动开始下载");
                if let Err(e) = update_download(app.clone()) {
                    dlog::warn(&format!("update: 自动下载失败：{e}"));
                    set(Some(&app), |s| s.view.error = Some(e));
                }
            }
        }
        Err(e) => {
            dlog::warn(&format!("update: 检查更新失败：{e}"));
            // 失败也要记时间，避免启动崩溃循环反复打 GitHub。
            persist(now, None, None);
            if manual {
                let mut g = state();
                if !matches!(g.view.phase, Phase::Available | Phase::Ready | Phase::Downloading) {
                    g.view.phase = Phase::Error;
                }
                g.view.error = Some(e);
                g.view.last_check = Some(now);
                drop(g);
                publish(Some(&app));
            }
        }
    }
}

fn run_download(
    app: AppHandle,
    name: String,
    size: u64,
    digest: Option<String>,
    lines: Vec<download::Candidate>,
) {
    let dir = download::updates_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        dlog::warn(&format!("update: 创建更新目录失败：{e}"));
        fail(&app, "无法创建更新目录");
        return;
    }
    let final_path = dir.join(&name);
    let part = dir.join(format!("{name}.part"));
    let expected = digest.as_deref().and_then(download::parse_digest);

    if final_path.is_file() && download::verify_file(&final_path, expected.as_ref(), size).is_ok() {
        dlog::dlog("update: 已有校验通过的安装包，跳过下载");
        finish_ready(&app, final_path, Some("已下载".to_string()), size);
        return;
    }
    let _ = std::fs::remove_file(&final_path);

    let mut errors: Vec<String> = Vec::new();
    for cand in lines {
        if CANCEL.load(Ordering::SeqCst) {
            let _ = std::fs::remove_file(&part);
            cancel_done(&app);
            return;
        }
        let mut last_emit = Instant::now();
        let label = cand.label.clone();
        let res = download::download_one(
            &cand.url,
            &part,
            size,
            expected.as_ref(),
            &mut |got, total| {
                if last_emit.elapsed().as_millis() >= PROGRESS_MS {
                    last_emit = Instant::now();
                    set(Some(&app), |s| {
                        s.view.downloaded = got;
                        if total > 0 {
                            s.view.asset_size = total;
                        }
                    });
                }
            },
            &CANCEL,
        );
        match res {
            Ok(()) => {
                let _ = std::fs::remove_file(&final_path);
                if let Err(e) = promote(&part, &final_path) {
                    let _ = std::fs::remove_file(&part);
                    errors.push(e);
                    continue;
                }
                dlog::dlog(&format!("update: 已通过「{label}」下载安装包"));
                finish_ready(&app, final_path, Some(label), size);
                return;
            }
            Err(e) if e == download::CANCELLED => {
                let _ = std::fs::remove_file(&part);
                cancel_done(&app);
                return;
            }
            Err(e) => {
                dlog::warn(&format!("update: 线路「{label}」下载失败：{e}"));
                let _ = std::fs::remove_file(&part);
                errors.push(e);
            }
        }
    }
    let msg = match errors.last() {
        Some(last) => format!("下载失败：{last}"),
        None => "下载失败".to_string(),
    };
    fail(&app, &msg);
}

/// `.part` → 正式文件名。
///
/// 正常环境用 rename；但在 MSIX 虚拟化 / 重定向的 `%LOCALAPPDATA%` 下，
/// `MoveFileExW` 会把同一目录误判成不同卷并返回 `os error 17`。退回复制 + 删除，
/// 代价只在异常环境里多一次 21 MB 写入。
fn promote(part: &Path, final_path: &Path) -> Result<(), String> {
    match std::fs::rename(part, final_path) {
        Ok(()) => Ok(()),
        Err(e) => {
            dlog::warn(&format!("update: 改名失败（{e}），改为复制"));
            if let Err(e2) = std::fs::copy(part, final_path) {
                dlog::warn(&format!("update: 复制安装包失败：{e2}"));
                return Err("无法保存安装包".to_string());
            }
            let _ = std::fs::remove_file(part);
            Ok(())
        }
    }
}

/// 安装包已就绪时，关闭设置窗口 / 退出 / 重启都改为启动安装器。
/// 返回 true 表示已接管，调用方不要再执行原动作。
pub fn install_if_ready(app: &AppHandle) -> bool {
    let path = {
        let g = state();
        if g.view.phase != Phase::Ready {
            return false;
        }
        g.ready_path.clone()
    };
    let Some(path) = path else {
        return false;
    };
    if !path.is_file() {
        dlog::warn("update: 就绪的安装包不存在，取消事件触发的安装");
        set(Some(app), |s| {
            s.view.phase = Phase::Error;
            s.view.error = Some("安装包不存在，请重新下载".to_string());
            s.ready_path = None;
        });
        return false;
    }
    set(Some(app), |s| s.view.phase = Phase::Installing);
    match install::launch(app, &path) {
        Ok(()) => true,
        Err(e) => {
            dlog::warn(&format!("update: 事件触发安装失败：{e}"));
            set(Some(app), |s| {
                s.view.phase = Phase::Ready;
                s.view.error = Some(e);
            });
            false
        }
    }
}

fn finish_ready(app: &AppHandle, path: PathBuf, via: Option<String>, size: u64) {    set(Some(app), |s| {
        s.ready_path = Some(path);
        s.view.phase = Phase::Ready;
        s.view.downloaded = size;
        s.view.via = via;
        s.view.error = None;
    });
    // 强制更新的安装包下载完只发一次通知；普通更新不打扰。
    if state().view.required {
        notify_ready(app);
    }
}

fn notify_ready(app: &AppHandle) {
    use rust_i18n::t;
    use tauri_plugin_notification::NotificationExt;
    if let Err(e) = app
        .notification()
        .builder()
        .title(t!("notify.updateTitle"))
        .body(t!("notify.updateBody"))
        .show()
    {
        dlog::warn(&format!("update: 发送更新通知失败：{e}"));
    }
}

/// 下载通知的点击回调先问这里：更新通知要打开设置页，而不是下载列表。
pub fn handle_notification_click(app: &AppHandle, title: &str, _body: &str) -> bool {
    use rust_i18n::t;
    if title == t!("notify.updateTitle") || title == "Vexto 需要更新" {
        crate::open_settings_at(app, "about");
        return true;
    }
    false
}

fn cancel_done(app: &AppHandle) {
    set(Some(app), |s| {
        s.view.phase = Phase::Idle;
        s.view.downloaded = 0;
        s.view.via = None;
        s.view.error = None;
    });
}

fn fail(app: &AppHandle, msg: &str) {
    set(Some(app), |s| {
        s.view.phase = Phase::Error;
        s.view.downloaded = 0;
        s.view.error = Some(msg.to_string());
    });
}

// ── 启动检查 + 状态文件 ─────────────────────────────────────────────────

#[derive(Default, Serialize, Deserialize)]
struct Persist {
    #[serde(default)]
    last_check_at: i64,
    #[serde(default)]
    latest_seen: String,
    /// 上次检查拿到的「最低版本」标记；启动时低于它就直接检查 + 自动下载。
    #[serde(default)]
    min_version: String,
}

fn persist_path() -> PathBuf {
    crate::config::data_dir().join("update-state.json")
}

fn read_persist() -> Persist {
    std::fs::read_to_string(persist_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_persist(p: &Persist) {
    let path = persist_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match serde_json::to_string_pretty(p) {
        Ok(text) => {
            if let Err(e) = std::fs::write(&path, text) {
                dlog::warn(&format!("update: 写状态文件失败：{e}"));
            }
        }
        Err(e) => dlog::warn(&format!("update: 序列化状态失败：{e}")),
    }
}

/// `seen = Some("")` = 清掉「有新版本」提示；`None` = 保持原值。
/// `min = Some("")` = 清掉最低版本；`None` = 保持原值。
fn persist(now: i64, seen: Option<&str>, min: Option<&str>) {
    let mut p = read_persist();
    p.last_check_at = now;
    if let Some(v) = seen {
        p.latest_seen = v.to_string();
    }
    if let Some(v) = min {
        p.min_version = v.to_string();
    }
    write_persist(&p);
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 启动后延迟检查（静默），并顺带清理过期安装包。
pub fn spawn_startup(app: AppHandle) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(15));
        let p = read_persist();
        let current = env!("CARGO_PKG_VERSION");
        if p.last_check_at > 0 {
            set(Some(&app), |s| s.view.last_check = Some(p.last_check_at));
        }
        // 上次已知的最低版本已经高于当前版本：这一次启动就检查并自动下载
        // （用户仍要自己点「立即安装」）。
        let forced = is_required(
            Some(p.min_version.as_str()).filter(|m| !m.is_empty()),
            current,
        );
        if forced {
            set(Some(&app), |s| {
                s.view.required = true;
                s.view.min_version = Some(p.min_version.clone());
            });
            dlog::dlog("update: 低于最低版本，启动检查并自动下载");
        }
        if !p.latest_seen.is_empty()
            && github::is_newer(&p.latest_seen, current).unwrap_or(false)
        {
            crate::tray::set_update_hint(&app, Some(p.latest_seen.as_str()), forced);
        }
        // 每次启动都检查，不再按 24 小时节流。
        run_check(app, forced);
    });

    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(30));
        cleanup_stale();
    });
}

/// 删掉 7 天前的半成品 / 旧安装包；当前 Ready 的文件不动。
fn cleanup_stale() {
    let dir = download::updates_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let now = SystemTime::now();
    let ready = state().ready_path.clone();
    for entry in entries.flatten() {
        let path = entry.path();
        if Some(&path) == ready.as_ref() {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let old = meta
            .modified()
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .map(|d| d.as_secs() > 7 * 24 * 60 * 60)
            .unwrap_or(false);
        if old {
            if let Err(e) = std::fs::remove_file(&path) {
                dlog::warn(&format!("update: 清理旧安装包失败：{e}"));
            }
        }
    }
}

/// 关于页状态行后缀。
pub fn status_suffix() -> String {
    let g = state();
    if g.view.required {
        if let Some(v) = &g.view.latest {
            return format!(" · 立即更新到 {v}");
        }
    }
    if g.view.phase == Phase::Available {
        if let Some(v) = &g.view.latest {
            return format!(" · 可更新到 {v}");
        }
    }
    String::new()
}

/// 「复制诊断信息」里的更新状态，不包含安装包 URL / 路径。
pub fn diagnostics_line() -> String {
    let g = state();
    format!(
        "phase={:?} current={} latest={} required={}",
        g.view.phase,
        g.view.current,
        g.view.latest.as_deref().unwrap_or("-"),
        g.view.required
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_state_is_idle_with_package_version() {
        let v = UpdateView {
            phase: Phase::Idle,
            current: env!("CARGO_PKG_VERSION").to_string(),
            latest: None,
            notes: None,
            url: None,
            asset_name: None,
            asset_size: 0,
            downloaded: 0,
            via: None,
            error: None,
            last_check: None,
            min_version: None,
            required: false,
        };
        assert_eq!(v.current, env!("CARGO_PKG_VERSION"));
        assert_eq!(
            serde_json::to_value(v).unwrap()["phase"],
            serde_json::json!("idle")
        );
    }

    #[test]
    fn required_only_when_below_min_version() {
        assert!(is_required(Some("0.3.2"), "0.3.1"));
        assert!(!is_required(Some("0.3.1"), "0.3.1"));
        assert!(!is_required(Some("0.3.0"), "0.3.1"));
        assert!(!is_required(None, "0.3.1"));
        assert!(!is_required(Some("banana"), "0.3.1"));
    }

    /// 版本号两处维护，漏改一处会让安装包文件名和检查逻辑对不上。
    #[test]
    fn package_version_matches_tauri_conf() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
        assert_eq!(
            conf["version"].as_str(),
            Some(env!("CARGO_PKG_VERSION")),
            "tauri.conf.json 与 Cargo.toml 的版本号不一致"
        );
    }

    /// `.part` 提升为正式文件：正常环境走 rename。
    #[test]
    fn promote_moves_part_into_place() {
        let dir = std::env::temp_dir().join(format!("vexto-promote-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let part = dir.join("a.part");
        let final_path = dir.join("a.exe");
        std::fs::write(&part, b"abc").unwrap();
        promote(&part, &final_path).unwrap();
        assert_eq!(std::fs::read(&final_path).unwrap(), b"abc");
        assert!(!part.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
