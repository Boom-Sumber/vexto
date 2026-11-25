//! S3b 轮询泵：定时取一次全量快照推给 UI，队列空且空闲 60 秒后关掉 sidecar。
//!
//! **不依赖 tauri**（沿用 download 模块的约定）：「把快照发出去」和「窗口是否可见」
//! 都由 `main.rs` 以闭包注入。这样 `Tool::status_line()` 拿不到 AppHandle 也能读状态，
//! 而且本模块可以单独跑单测，不需要起 WebView。
//!
//! - 任务书 §S3.5.5：主窗口可见 500 ms、后台 2 s；任务状态变化立即唤醒一次。
//! - 任务书 §S3.6.1：队列空 && 空闲 60 秒 → `aria2.shutdown`。
//! - 一次 `system.multicall` 拿全部，不要一个任务发一次请求。

use crate::dlog;
use crate::download::{engine, history, rpc};
use rust_i18n::t;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

const VISIBLE_INTERVAL: Duration = Duration::from_millis(500);
const BACKGROUND_INTERVAL: Duration = Duration::from_secs(2);
const IDLE_SHUTDOWN: Duration = Duration::from_secs(60);
/// 没有 aria2c 时没必要 500 ms 空转，等唤醒即可。
const NO_ENGINE_INTERVAL: Duration = Duration::from_secs(2);
/// 队列空多久才认「全部完成」。给「暂停一下再继续」留出反应时间。
const AFTER_DONE_DELAY: Duration = Duration::from_secs(10);
/// 启动后多久内不执行「完成后动作」。
///
/// 没有这道闸，开机自启 + 上次全是已完成，启动那一刻就满足了「队列空」，
/// 直接给用户关机。三分钟足够人反应过来自己刚开机。
const AFTER_DONE_GRACE: Duration = Duration::from_secs(180);
/// 自动限速的调整间隔。太快会在上限附近来回抖。
const LIMIT_TICK: Duration = Duration::from_secs(15);

static RUNNING: AtomicBool = AtomicBool::new(false);
static STARTED: AtomicBool = AtomicBool::new(false);
/// 完成后动作只执行一次。进程活着就不再重复触发。
static AFTER_DONE_FIRED: AtomicBool = AtomicBool::new(false);
static WAKE: OnceLock<(Mutex<bool>, Condvar)> = OnceLock::new();

/// 最近一次取到的快照。`/stat`（浏览器扩展的就绪探针）直接用缓存，
/// **不能现取**：扩展给 `/stat` 的超时只有 500 ms，而 aria2 忙时一次
/// RPC 就可能超过这个数，探针会误判「桌面端不可用」。
static LAST: Mutex<Option<rpc::Snapshot>> = Mutex::new(None);

/// 复制一份最近快照；没取过时返回 `None`。
pub fn last_snapshot() -> Option<rpc::Snapshot> {
    LAST.lock().ok().and_then(|g| g.clone())
}

fn wake_pair() -> &'static (Mutex<bool>, Condvar) {
    WAKE.get_or_init(|| (Mutex::new(false), Condvar::new()))
}

/// 入队 / 暂停 / 继续 / 删除 / 清空后调一次：立即唤醒 pump，并重置空闲计时。
pub fn notify() {
    let (m, cv) = wake_pair();
    let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
    *g = true;
    cv.notify_one();
}

/// 睡 `d`，被 `notify()` 打断时立刻返回 true。
/// 用 bool 标志而不是 `WaitTimeoutResult`，是为了把 Condvar 的虚假唤醒排除掉。
fn wait_or_wake(d: Duration) -> bool {
    let (m, cv) = wake_pair();
    let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
    if *g {
        *g = false;
        return true;
    }
    let (mut guard, _) = cv.wait_timeout(g, d).unwrap_or_else(|e| e.into_inner());
    let woke = *guard;
    *guard = false;
    woke
}

/// 只有这三种状态值得留着 aria2c；complete / error / removed 都算「队列空」。
pub fn is_pending(status: &str) -> bool {
    matches!(status, "active" | "waiting" | "paused")
}

pub fn has_pending(rows: &[rpc::TaskRow]) -> bool {
    rows.iter().any(|r| is_pending(&r.status))
}

pub fn interval_for(visible: bool) -> Duration {
    if visible {
        VISIBLE_INTERVAL
    } else {
        BACKGROUND_INTERVAL
    }
}

pub fn idle_expired(since: Instant, now: Instant) -> bool {
    now.saturating_duration_since(since) >= IDLE_SHUTDOWN
}

/// 「全部完成后做什么」现在该不该触发。
///
/// 四个条件必须同时满足，缺一不可：
/// 1. 用户配了动作（不是 `none`）；
/// 2. **本轮真的跑过任务**——只看「队列空」会把「启动后什么都没干」也算进去；
/// 3. 还没触发过（一次运行只执行一次）；
/// 4. 队列连续空够 10 秒，且进程已经活了 3 分钟以上（躲开开机自启的场景）。
pub fn after_done_ready(
    action: &str,
    saw_active: bool,
    already_fired: bool,
    idle: Duration,
    uptime: Duration,
) -> bool {
    action != "none"
        && saw_active
        && !already_fired
        && idle >= AFTER_DONE_DELAY
        && uptime >= AFTER_DONE_GRACE
}

/// 自动限速：给定用户设的上限与最近实测峰值，算出这次该用的上限。
///
/// 规则刻意简单，且**只降不升**（同一轮里降下去就稳住，不来回抖）：
/// - 上限是 0（不限）→ 保持 0，没有可收敛的目标；
/// - 实测峰值已经到上限的 85% → 说明上限没卡住速度，不动；
/// - 否则把上限收到「峰值 × 1.3」，但不低于 128 KB/s。
///
/// 目的不是"更快"，而是"别用一个虚高的上限让 aria2 开满连接去空转"。
pub fn next_limit(ceiling_kb: u32, peak_kb: u32, applied_kb: u32) -> u32 {
    if ceiling_kb == 0 {
        return 0;
    }
    let applied = if applied_kb == 0 {
        ceiling_kb
    } else {
        applied_kb
    };
    if peak_kb.saturating_mul(100) >= ceiling_kb.saturating_mul(85) {
        return applied;
    }
    let target = (peak_kb.saturating_mul(13) / 10).max(128).min(ceiling_kb);
    applied.min(target)
}

/// 执行「全部完成后」的动作。
///
/// 关机用 `shutdown /s /t 60`：一分钟内 `shutdown /a` 就能撤销。
/// 不用 `InitiateSystemShutdownExW` —— 那要提权、还要多加一个 windows feature。
fn fire_after_done(action: &str, cmd: &str) {
    let result = match action {
        "shutdown" => {
            let notice = t!("status.line.pumpShutdownNotice").into_owned();
            Command::new("shutdown")
                .args(["/s", "/t", "60", "/c"])
                .arg(notice)
                .spawn()
                .map(|_| ())
        }
        // SetSuspendState 在 powrprof.dll 里，没有稳定的命令行入口，
        // rundll32 是官方文档给出的调用方式。
        "sleep" => Command::new("rundll32.exe")
            .args(["powrprof.dll,SetSuspendState", "0,1,0"])
            .spawn()
            .map(|_| ()),
        "run" => {
            let c = cmd.trim();
            if c.is_empty() {
                dlog::dlog("pump: 配了「完成后运行程序」但没填路径，跳过");
                return;
            }
            Command::new(c).spawn().map(|_| ())
        }
        _ => return,
    };
    match result {
        Ok(()) => {
            dlog::dlog(&format!("pump: 全部下载完成，已执行 {action}"));
            engine::set_status(match action {
                "shutdown" => engine::EngineStatus::PumpShutdown,
                "sleep" => engine::EngineStatus::PumpSleep,
                _ => engine::EngineStatus::PumpRun,
            });
        }
        Err(e) => {
            dlog::dlog(&format!("pump: 执行 {action} 失败：{e}"));
            // 状态行是用户可见的：不显示错误码（会话 52），细节在日志里
            engine::set_status(engine::EngineStatus::PumpFailed {
                action: action.to_string(),
            });
        }
    }
}

/// 启动一次。重复调用不会重复起线程（设置页开关会反复触发重注册）。
pub fn start<F, V, O>(emit: F, is_visible: V, on_rows: O)
where
    F: Fn(rpc::Snapshot) + Send + 'static,
    V: Fn() -> bool + Send + 'static,
    O: Fn(&[rpc::TaskRow]) + Send + 'static,
{
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    RUNNING.store(true, Ordering::SeqCst);
    std::thread::spawn(move || run(emit, is_visible, on_rows));
}

/// 程序退出时停泵。不 join：退出路径上等一个正在做 HTTP 的线程没有意义，
/// RUNNING 置位后它下一轮就会自己结束。
pub fn stop() {
    RUNNING.store(false, Ordering::SeqCst);
    STARTED.store(false, Ordering::SeqCst);
    notify();
}

fn run<F, V, O>(emit: F, is_visible: V, on_rows: O)
where
    F: Fn(rpc::Snapshot),
    V: Fn() -> bool,
    O: Fn(&[rpc::TaskRow]),
{
    let started = Instant::now();
    let mut idle_since: Option<Instant> = None;
    // 「全部完成」的计时。与 `idle_since` 分开：那个管 60 秒关 sidecar，
    // 这个管 10 秒后执行用户配的动作，两个阈值互不相干。
    let mut done_since: Option<Instant> = None;
    let mut saw_active = false;
    // 自动限速的三个状态：本轮峰值、当前已生效上限、上次调整时间。
    let mut limit_peak_kb: u32 = 0;
    let mut limit_applied_kb: u32 = 0;
    let mut limit_tick = Instant::now();

    while RUNNING.load(Ordering::SeqCst) {
        let Some(handle) = engine::rpc_handle() else {
            // 本来就没有进程可关，不累计空闲时间；缓存也清掉，别把上一轮的
            // 速度/任务数当成「现在」的（扩展的 /stat 会读这份缓存）。
            if let Ok(mut g) = LAST.lock() {
                *g = None;
            }
            idle_since = None;
            done_since = None;
            saw_active = false;
            limit_peak_kb = 0;
            limit_applied_kb = 0;
            wait_or_wake(NO_ENGINE_INTERVAL);
            continue;
        };
        match handle.snapshot() {
            Ok(snap) => {
                // 先让历史与通知看到**实时**任务（不含历史条目），
                // 再把历史补进快照推给前端。顺序反了会把历史当成新任务重复记录。
                on_rows(&snap.tasks);
                if let Ok(mut g) = LAST.lock() {
                    *g = Some(snap.clone());
                }
                let pending = has_pending(&snap.tasks);
                if pending {
                    saw_active = true;
                    idle_since = None;
                    done_since = None;
                }
                let speed_kb = (snap.global.download_speed / 1024) as u32;
                let mut ui = snap;
                history::merge(&mut ui);
                emit(ui);

                if pending {
                    // 自动限速只在一轮下载里收敛，任务清空后复位。
                    limit_peak_kb = limit_peak_kb.max(speed_kb);
                    if limit_tick.elapsed() >= LIMIT_TICK {
                        let cfg = crate::config::load();
                        if cfg.auto_limit {
                            let cur = if limit_applied_kb == 0 {
                                cfg.max_speed_kb
                            } else {
                                limit_applied_kb
                            };
                            let next = next_limit(cfg.max_speed_kb, limit_peak_kb, cur);
                            if next != cur {
                                let v = if next == 0 {
                                    "0".to_string()
                                } else {
                                    format!("{next}K")
                                };
                                let opt = serde_json::json!({ "max-overall-download-limit": v });
                                match handle.change_global_option(opt) {
                                    Ok(()) => {
                                        limit_applied_kb = next;
                                        dlog::dlog(&format!(
                                            "pump: 自动限速 {cur} → {next} KB/s（峰值 {limit_peak_kb}）"
                                        ));
                                    }
                                    Err(e) => dlog::dlog(&format!("pump: 自动限速下发失败：{e}")),
                                }
                            }
                        }
                        limit_peak_kb = 0;
                        limit_tick = Instant::now();
                    }
                } else {
                    limit_peak_kb = 0;
                    limit_applied_kb = 0;

                    let cfg = crate::config::load();
                    let since = *done_since.get_or_insert_with(Instant::now);
                    if after_done_ready(
                        &cfg.after_all_done,
                        saw_active,
                        AFTER_DONE_FIRED.load(Ordering::SeqCst),
                        Instant::now().saturating_duration_since(since),
                        started.elapsed(),
                    ) {
                        AFTER_DONE_FIRED.store(true, Ordering::SeqCst);
                        fire_after_done(&cfg.after_all_done, &cfg.after_all_done_cmd);
                    }

                    let idle = *idle_since.get_or_insert_with(Instant::now);
                    if idle_expired(idle, Instant::now()) {
                        dlog::dlog("pump: 队列空且空闲 60 秒，关闭 aria2c");
                        if let Err(e) = engine::shutdown_all() {
                            dlog::dlog(&format!("pump: 空闲关闭 aria2c 失败：{e}"));
                        }
                        idle_since = None;
                    }
                }
            }
            Err(e) => dlog::dlog(&format!("pump: 取快照失败：{e}")),
        }
        if wait_or_wake(interval_for(is_visible())) {
            // 有活动：立即重取一次，并把 60 秒计时归零。
            idle_since = None;
        }
    }
    dlog::dlog("pump: 已停止");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(status: &str) -> rpc::TaskRow {
        rpc::TaskRow {
            gid: "g".into(),
            status: status.into(),
            name: "a.zip".into(),
            total_length: 100,
            completed_length: 0,
            download_speed: 0,
            upload_speed: 0,
            eta_seconds: None,
            dir: String::new(),
            file_path: String::new(),
            file_missing: false,
            url: String::new(),
            historical: false,
            error_message: None,
        }
    }

    /// 验收第 5 条的判据：删掉所有任务后不能因为残留 complete 记录而一直留着进程。
    #[test]
    fn only_pending_states_keep_sidecar_alive() {
        assert!(has_pending(&[row("active")]));
        assert!(has_pending(&[row("waiting"), row("paused")]));
        assert!(!has_pending(&[
            row("complete"),
            row("error"),
            row("removed")
        ]));
        assert!(!has_pending(&[]));
    }

    #[test]
    fn intervals_follow_visibility() {
        assert_eq!(interval_for(true), Duration::from_millis(500));
        assert_eq!(interval_for(false), Duration::from_secs(2));
    }

    #[test]
    fn idle_expires_only_after_60_seconds() {
        let t = Instant::now();
        assert!(!idle_expired(t, t + Duration::from_secs(59)));
        assert!(idle_expired(t, t + Duration::from_secs(60)));
        // 时钟倒退也不能 panic / 误判
        assert!(!idle_expired(t, t));
    }

    /// 四个条件缺一不可，尤其「启动 3 分钟内」和「本轮跑过任务」这两道闸。
    #[test]
    fn after_done_needs_all_four_conditions() {
        let idle_ok = Duration::from_secs(10);
        let up_ok = Duration::from_secs(200);
        assert!(after_done_ready("shutdown", true, false, idle_ok, up_ok));
        // 没配动作
        assert!(!after_done_ready("none", true, false, idle_ok, up_ok));
        // 本轮没跑过任务（启动后一直空闲）
        assert!(!after_done_ready("shutdown", false, false, idle_ok, up_ok));
        // 已经触发过
        assert!(!after_done_ready("shutdown", true, true, idle_ok, up_ok));
        // 刚清空，不满 10 秒（用户可能马上又加任务）
        assert!(!after_done_ready(
            "shutdown",
            true,
            false,
            Duration::from_secs(9),
            up_ok
        ));
        // 刚启动，还没过 3 分钟
        assert!(!after_done_ready(
            "shutdown",
            true,
            false,
            idle_ok,
            Duration::from_secs(30)
        ));
    }

    /// 自动限速：没设上限就不动、达到上限附近就保持、明显够不上就收敛、只降不升。
    #[test]
    fn auto_limit_converges_downwards_only() {
        // 不限速：没有可收敛的目标
        assert_eq!(next_limit(0, 900, 0), 0);
        // 峰值顶到上限的 85%：上限没卡住速度，保持
        assert_eq!(next_limit(2000, 1900, 2000), 2000);
        assert_eq!(next_limit(2000, 1700, 2000), 2000);
        // 峰值远低于上限：收敛到 峰值×1.3
        assert_eq!(next_limit(2000, 1000, 2000), 1300);
        // 但有个地板 128 KB/s，别把限速收到几乎不动
        assert_eq!(next_limit(2000, 10, 2000), 128);
        // 只降不升：已经收到 800 了，峰值 1500 也不许涨回去
        assert_eq!(next_limit(2000, 1500, 800), 800);
        // 收敛结果不会超过用户设的上限
        assert!(next_limit(500, 400, 500) <= 500);
    }
}
