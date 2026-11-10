//! 下载完成 / 失败的系统通知。
//!
//! 只在「终态发生变化」时发：pump 每 500 ms 会给一次快照，如果每次都给已完成
//! 的任务发通知，用户会被刷屏。用 gid → 上次状态 的映射做边沿检测。
//!
//! 首次看到某任务且它已经是终态（极快的小文件）也会通知——这正是用户想要的。

use std::collections::HashMap;
use std::sync::Mutex;

use tauri_plugin_notification::NotificationExt;

use super::rpc::TaskRow;

static SEEN: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

fn is_final(status: &str) -> bool {
    matches!(status, "complete" | "error" | "removed")
}

/// 观察一轮快照；`app` 用来发通知。
pub fn observe(app: &tauri::AppHandle, tasks: &[TaskRow]) {
    let cfg = crate::config::load();
    let mut guard = match SEEN.lock() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    };
    let seen = guard.get_or_insert_with(HashMap::new);

    for t in tasks {
        let prev = seen.insert(t.gid.clone(), t.status.clone());
        if prev.as_deref() == Some(t.status.as_str()) {
            continue;
        }
        // 已经是终态的任务再被刷新（例如清空失败后重试），不重复通知。
        if prev.as_deref().is_some_and(is_final) {
            continue;
        }
        // 状态照常记录，只是关掉开关时不发：否则「关掉再打开」会把当前
        // 所有已完成任务当成刚完成，一次性刷一堆通知。
        if !cfg.notify_download {
            continue;
        }
        match t.status.as_str() {
            // 「开始」只认第一次见到就已是 active 的任务：paused → active
            // 是用户点继续，不该再弹一条。
            "active" if prev.is_none() && cfg.notify_on_start => {
                send(app, "开始下载", &t.name);
            }
            "complete" if cfg.notify_on_done => send(app, "下载完成", &t.name),
            "error" if cfg.notify_on_done => {
                let reason = t
                    .error_message
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or("未知错误");
                send(app, "下载失败", &format!("{}：{reason}", t.name));
            }
            _ => {}
        }
    }
}

/// 「发送测试通知」按钮。
///
/// **不看 `notify_download`**：用户点测试就是想当场确认通道通不通，
/// 总开关关着也该发一条——否则点了没反应，看起来像坏了。
pub fn test(app: &tauri::AppHandle) -> Result<(), String> {
    app.notification()
        .builder()
        .title("LightBox")
        .body("看到这条说明系统通知是通的。")
        .show()
        .map_err(|e| format!("发送测试通知失败：{e}"))
}

fn send(app: &tauri::AppHandle, title: &str, body: &str) {
    if let Err(e) = app
        .notification()
        .builder()
        .title(title)
        .body(body)
        .show()
    {
        // 通知失败不影响下载：记日志即可（企业策略可能禁掉通知）。
        crate::dlog::dlog(&format!("notify: 发送失败：{e}"));
    }
}

#[cfg(test)]
mod tests {
    use super::is_final;

    #[test]
    fn final_states_are_recognized() {
        assert!(is_final("complete"));
        assert!(is_final("error"));
        assert!(is_final("removed"));
        assert!(!is_final("active"));
        assert!(!is_final("paused"));
        assert!(!is_final("waiting"));
    }
}
