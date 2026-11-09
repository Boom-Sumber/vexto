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
use crate::download::{engine, rpc};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

const VISIBLE_INTERVAL: Duration = Duration::from_millis(500);
const BACKGROUND_INTERVAL: Duration = Duration::from_secs(2);
const IDLE_SHUTDOWN: Duration = Duration::from_secs(60);
/// 没有 aria2c 时没必要 500 ms 空转，等唤醒即可。
const NO_ENGINE_INTERVAL: Duration = Duration::from_secs(2);

static RUNNING: AtomicBool = AtomicBool::new(false);
static STARTED: AtomicBool = AtomicBool::new(false);
static WAKE: OnceLock<(Mutex<bool>, Condvar)> = OnceLock::new();

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

/// 启动一次。重复调用不会重复起线程（设置页开关会反复触发重注册）。
pub fn start<F, V>(emit: F, is_visible: V)
where
    F: Fn(rpc::Snapshot) + Send + 'static,
    V: Fn() -> bool + Send + 'static,
{
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    RUNNING.store(true, Ordering::SeqCst);
    std::thread::spawn(move || run(emit, is_visible));
}

/// 程序退出时停泵。不 join：退出路径上等一个正在做 HTTP 的线程没有意义，
/// RUNNING 置位后它下一轮就会自己结束。
pub fn stop() {
    RUNNING.store(false, Ordering::SeqCst);
    STARTED.store(false, Ordering::SeqCst);
    notify();
}

fn run<F, V>(emit: F, is_visible: V)
where
    F: Fn(rpc::Snapshot),
    V: Fn() -> bool,
{
    let mut idle_since: Option<Instant> = None;
    while RUNNING.load(Ordering::SeqCst) {
        let Some(handle) = engine::rpc_handle() else {
            // 本来就没有进程可关，不累计空闲时间。
            idle_since = None;
            wait_or_wake(NO_ENGINE_INTERVAL);
            continue;
        };
        match handle.snapshot() {
            Ok(snap) => {
                let pending = has_pending(&snap.tasks);
                emit(snap);
                if pending {
                    idle_since = None;
                } else {
                    let since = *idle_since.get_or_insert_with(Instant::now);
                    if idle_expired(since, Instant::now()) {
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
}
