//! 取词：UIA 优先，剪贴板兜底。
//!
//! 剪贴板路线用 `GetClipboardSequenceNumber()` 判断"复制是否真的发生"，
//! 而不是比较文本内容 —— 后者在"复制成功但内容与原来相同"时会误判为失败
//! （用户选中的是同一段文字、连按两次热键就会踩到）。

use std::time::Duration;
use windows::Win32::System::DataExchange::GetClipboardSequenceNumber;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
    VIRTUAL_KEY, VK_C, VK_CONTROL, VK_ESCAPE, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT, VK_SPACE, VK_TAB,
};

fn clipboard_seq() -> u32 {
    unsafe { GetClipboardSequenceNumber() }
}

fn key_input(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// 先松开可能卡住的修饰键，避免 Ctrl/Alt/Shift 处于按下状态干扰 Ctrl+C。
///
/// 热键回调触发时，用户的手指很可能还按在 Alt/Ctrl 上（Windows 的
/// `RegisterHotKey` 不会替你松开物理键），这时候直接发 Ctrl+C 会变成
/// `Alt+Ctrl+C`，目标程序收到的是别的东西。
///
/// 注意不要碰 `VK_CAPITAL`：它是切换键，发 keyup 有可能把大写锁状态搞乱。
fn release_modifiers() {
    let keys = [
        VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN, VK_SPACE, VK_TAB, VK_ESCAPE,
    ];
    let inputs: Vec<INPUT> = keys.iter().map(|k| key_input(*k, true)).collect();
    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

fn send_ctrl_c() {
    // 顺序必须是 Ctrl↓ → C↓ → C↑ → Ctrl↑。
    // 如果先松开 Ctrl 再松开 C，目标程序收到的最后一个按键是"单独的 c"，
    // 会被当成输入字符而不是复制快捷键 —— 现象是选中文字被替换成一个 "c"。
    let inputs = [
        key_input(VK_CONTROL, false),
        key_input(VK_C, false),
        key_input(VK_C, true),
        key_input(VK_CONTROL, true),
    ];
    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

fn read_clipboard_text() -> Option<String> {
    let mut cb = arboard::Clipboard::new().ok()?;
    cb.get_text().ok()
}

/// 写纯文本到剪贴板。除了还原旧内容，弹窗的「复制译文」也用它兜底
/// （不可聚焦的窗口里浏览器 clipboard API 可能被拒）。
pub fn write_clipboard_text(s: &str) {
    if let Ok(mut cb) = arboard::Clipboard::new() {
        let _ = cb.set_text(s.to_string());
    }
}

/// UI Automation 路线：适合 Edge / Chrome / UWP / 多数 Qt 与 WinUI 程序。
///
/// 对 Electron、旧版 Win32 程序里的非标准控件无效，这是 UIA 的固有限制，
/// 不是 bug —— 这些程序会走剪贴板兜底。
fn capture_uia() -> Option<String> {
    use uiautomation::patterns::UITextPattern;
    use uiautomation::UIAutomation;

    let automation = match UIAutomation::new() {
        Ok(a) => a,
        Err(e) => {
            crate::dlog::dlog(&format!("capture_uia: UIAutomation::new failed: {e}"));
            return None;
        }
    };
    let focused = match automation.get_focused_element() {
        Ok(f) => f,
        Err(e) => {
            crate::dlog::dlog(&format!("capture_uia: get_focused_element failed: {e}"));
            return None;
        }
    };
    let name = focused.get_name().unwrap_or_default();
    let class = focused.get_classname().unwrap_or_default();
    let pattern = match focused.get_pattern::<UITextPattern>() {
        Ok(p) => p,
        Err(e) => {
            crate::dlog::dlog(&format!(
                "capture_uia: no TextPattern (name={name:?} class={class:?}): {e}"
            ));
            return None;
        }
    };
    let ranges = match pattern.get_selection() {
        Ok(r) => r,
        Err(e) => {
            crate::dlog::dlog(&format!("capture_uia: get_selection failed: {e}"));
            return None;
        }
    };
    let mut text = String::new();
    for r in ranges {
        if let Ok(t) = r.get_text(-1) {
            if !t.is_empty() {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&t);
            }
        }
    }
    let t = text.trim().to_string();
    if t.is_empty() {
        crate::dlog::dlog(&format!(
            "capture_uia: empty selection (name={name:?} class={class:?})"
        ));
        None
    } else {
        // 记前 60 字：UIA 有时会把整个文档当选中区返回，
        // 只看字符数判断不了对不对，得看得回来的内容。
        let preview: String = t.chars().take(60).collect();
        crate::dlog::dlog(&format!(
            "capture_uia: ok {} chars (name={name:?} class={class:?}) preview={preview:?}",
            t.chars().count()
        ));
        Some(t)
    }
}

/// 剪贴板路线：几乎对所有程序有效，代价是短暂占用剪贴板。
fn capture_clipboard() -> Result<String, String> {
    let old = read_clipboard_text();
    let before = clipboard_seq();

    release_modifiers();
    std::thread::sleep(Duration::from_millis(20));
    send_ctrl_c();

    // 等剪贴板序号变化，最多约 800ms。慢程序（大文档、远程桌面）需要这么久。
    let mut changed = false;
    let mut after = before;
    for _ in 0..80 {
        std::thread::sleep(Duration::from_millis(10));
        after = clipboard_seq();
        if after != before {
            changed = true;
            break;
        }
    }

    if !changed {
        // 序号没变 = 目标程序根本没响应 Ctrl+C。这条日志是排查现场的关键，
        // 看到 `seq 123->123` 就说明复制没发生，别再怀疑翻译环节。
        crate::dlog::dlog(&format!("capture_clipboard: seq {before}->{after}（复制没发生）"));
        return Err("没有取到选中的文字。\n\
             可以先选中文本再按热键；如果某些程序不支持自动复制，\n\
             先手动 Ctrl+C 再按热键也能用。"
            .to_string());
    }

    // 序号变了，给剪贴板一点时间落定（有些程序先设空再填内容）
    std::thread::sleep(Duration::from_millis(40));
    let new = read_clipboard_text();
    crate::dlog::dlog(&format!(
        "capture_clipboard: seq {before}->{after}, old={:?}, new={:?}",
        old.as_deref().map(|s| s.chars().take(40).collect::<String>()),
        new.as_deref().map(|s| s.chars().take(40).collect::<String>())
    ));

    // 恢复原剪贴板（仅纯文本；富文本/图片无法还原，这是当前实现的限制）
    if let Some(o) = old {
        if Some(o.as_str()) != new.as_deref() {
            write_clipboard_text(&o);
        }
    }

    match new {
        Some(t) if !t.trim().is_empty() => Ok(t.trim().to_string()),
        _ => Err("复制动作发生了，但剪贴板里没有文本（可能是图片或空选）".to_string()),
    }
}

/// 取当前选中的文字。先 UIA，失败再走剪贴板。
///
/// **会短暂改写系统剪贴板**（走兜底路线时），所以调用方必须假设剪贴板内容被动过。
pub fn capture_selection() -> Result<String, String> {
    if let Some(t) = capture_uia() {
        return Ok(t);
    }
    capture_clipboard()
}
