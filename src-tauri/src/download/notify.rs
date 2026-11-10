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

/// 最近发出去的通知：正文 → 落盘路径（没有路径就是空串）。
///
/// 点击回调能拿到的只有 `title` / `body`——插件的 builder **没有 payload
/// setter**（只有 title / body / icon / sound / action），所以只能按正文反查
/// 是哪条任务。撞名（两个同名文件同时下完）概率极低，查错了也只是打开另一个
/// 同目录的文件，且下面还有兜底。
static SENT: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
const SENT_MAX: usize = 20;

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
                send(app, "开始下载", &t.name, &t.file_path);
            }
            "complete" if cfg.notify_on_done => send(app, "下载完成", &t.name, &t.file_path),
            "error" if cfg.notify_on_done => {
                let reason = t
                    .error_message
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or("未知错误");
                send(
                    app,
                    "下载失败",
                    &format!("{}：{reason}", t.name),
                    &t.file_path,
                );
            }
            _ => {}
        }
    }
}

/// 注册「点击通知」的处理器。**只在 setup 里调一次**。
///
/// Windows 的限制（插件文档写明）：只有 toast 还停在屏幕上时才会上报点击，
/// 已经被收进操作中心的那条点了不会有反应。所以这里做的是「尽力而为」：
/// 能定位到文件就选中它，定位不到就打开下载列表。
pub fn register_click(app: &tauri::AppHandle) {
    let handle = app.clone();
    let r = app.notification().on_action(move |e| {
        // 只处理「点了通知本体」；dismiss 之类的忽略
        if e.action_id() != "tap" {
            return;
        }
        let body = e
            .notification()
            .and_then(|n| n.body())
            .unwrap_or_default()
            .to_string();
        crate::dlog::dlog(&format!("notify: 点击通知 body={body}"));
        let cfg = crate::config::load();
        if cfg.notify_click_action == "folder" {
            if let Some(path) = lookup(&body) {
                if !path.trim().is_empty() {
                    if let Err(err) = crate::fileops::reveal(&path) {
                        crate::dlog::dlog(&format!("notify: 打开文件失败：{err}"));
                    } else {
                        return;
                    }
                }
            }
        }
        // 兜底：打开下载列表。建窗必须在主线程。
        let a = handle.clone();
        let _ = handle.run_on_main_thread(move || crate::popup::open_main_window(&a));
    });
    if let Err(err) = r {
        crate::dlog::dlog(&format!("notify: 注册点击回调失败：{err}"));
    }
}

fn lookup(body: &str) -> Option<String> {
    SENT.lock()
        .ok()
        .and_then(|g| g.iter().find(|(b, _)| b == body).map(|(_, p)| p.clone()))
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

fn send(app: &tauri::AppHandle, title: &str, body: &str, path: &str) {
    if let Ok(mut g) = SENT.lock() {
        g.retain(|(b, _)| b != body);
        g.push((body.to_string(), path.to_string()));
        if g.len() > SENT_MAX {
            let drop = g.len() - SENT_MAX;
            g.drain(0..drop);
        }
    }
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
