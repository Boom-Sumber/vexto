//! 全局热键：格式校验、归一化、注册。
//!
//! 规格（ADR-018，锁死，不要自己放宽）：
//! - 修饰键只允许 `Ctrl` / `Shift` / `Alt`，**不含 Win**
//! - 修饰键 **1~2 个**；主键只能是 `0-9` / `A-Z` / `F1-F12`
//! - 归一化顺序固定 `Ctrl` → `Shift` → `Alt` → 主键，主键大写
//!
//! **为什么必须自己校验**：`global-hotkey` 的 `HotKey::new(mods, key)` 对修饰键数量
//! 不做任何限制（0 个、3 个都收），也不限制"只准 Ctrl/Shift/Alt"。规格是我们定的，
//! 只能靠 `validate_hotkey()` 与下面那组测试守住。

use std::str::FromStr;
use std::sync::Mutex;
use tauri::AppHandle;

/// 最近一次热键注册失败的原因。`HotKeyErr` 在 Windows 上是静默失败，
/// GUI 程序看不到 stderr，用户只会觉得"按了没反应"——所以把原因存下来给设置页显示。
pub static HOTKEY_ERR: Mutex<Option<String>> = Mutex::new(None);

/// 校验并归一化热键字符串。Err 里是给用户看的中文原因。
///
/// **只保证"格式合法且库认得"**，是否被别的程序占用要等真正注册才知道。
pub fn validate_hotkey(s: &str) -> Result<String, String> {
    let parts: Vec<&str> = s
        .split('+')
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect();
    if parts.is_empty() {
        return Err("热键不能为空".into());
    }
    // 最后一段是主键，前面都是修饰键
    let Some((main, mods)) = parts.split_last() else {
        return Err("热键不能为空".into());
    };

    let mut got: Vec<&str> = Vec::new();
    for m in mods {
        let name = match m.to_ascii_uppercase().as_str() {
            "CTRL" | "CONTROL" => "Ctrl",
            "SHIFT" => "Shift",
            "ALT" => "Alt",
            "WIN" | "SUPER" | "CMD" | "META" => {
                return Err("不支持 Win 键，请用 Ctrl / Shift / Alt".into())
            }
            _ => return Err(format!("不认识的修饰键「{m}」，只支持 Ctrl / Shift / Alt")),
        };
        if got.contains(&name) {
            return Err(format!("修饰键「{name}」重复了"));
        }
        got.push(name);
    }
    if got.is_empty() {
        return Err("至少需要一个 Ctrl / Shift / Alt".into());
    }
    if got.len() > 2 {
        return Err("最多两个修饰键".into());
    }

    let mk = main.to_ascii_uppercase();
    let is_alpha_num = mk.len() == 1 && mk.chars().next().is_some_and(|c| c.is_ascii_alphanumeric());
    let is_fkey = (2..=3).contains(&mk.len())
        && mk.starts_with('F')
        && mk[1..].parse::<u8>().is_ok_and(|n| (1..=12).contains(&n));
    if !is_alpha_num && !is_fkey {
        return Err(format!("主键「{main}」不支持，只能是 0-9、A-Z、F1-F12"));
    }

    // 固定顺序归一化，保证同一个组合只有一种写法落进配置
    let ordered: Vec<&str> = ["Ctrl", "Shift", "Alt"]
        .iter()
        .filter(|m| got.contains(m))
        .copied()
        .collect();
    let normalized = format!("{}+{}", ordered.join("+"), mk);

    // 兜底：真让库再解析一次，它认不出的组合不要存进配置
    tauri_plugin_global_shortcut::Shortcut::from_str(&normalized)
        .map_err(|_| format!("「{normalized}」不是有效的组合键"))?;

    Ok(normalized)
}

/// 校验 → 归一化 → 注册。替换旧热键（先 `unregister_all`）。
///
/// `on_pressed` 只在**按下**时调用（不响应松开），且运行在事件循环线程上——
/// 里面**不能**做 UIA / 网络 / 文件 IO，必须 `std::thread::spawn` 再干活。
pub fn register_hotkey<F>(app: &AppHandle, hotkey: &str, on_pressed: F) -> Result<(), String>
where
    F: Fn(&AppHandle) + Send + Sync + 'static,
{
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

    // 先校验：不合法就不要动已注册的旧热键，避免"改错了反而把好的也弄没了"
    let normalized = validate_hotkey(hotkey)?;
    let sc = tauri_plugin_global_shortcut::Shortcut::from_str(&normalized)
        .map_err(|e| format!("热键格式不对（{normalized}）：{e}"))?;

    let gs = app.global_shortcut();
    // 改热键时先注销旧的；注销失败不致命（旧的可能本来就没注册上）
    let _ = gs.unregister_all();

    match gs.on_shortcut(sc, move |app, _sc, ev| {
        if ev.state() == ShortcutState::Pressed {
            on_pressed(app);
        }
    }) {
        Ok(()) => {
            *HOTKEY_ERR.lock().unwrap_or_else(|e| e.into_inner()) = None;
            Ok(())
        }
        Err(e) => {
            // Windows 上 RegisterHotKey 被别的程序占用时就是走到这里
            let msg = format!(
                "热键 {normalized} 注册失败：{e}。多半是已被别的程序占用，换一个组合试试。"
            );
            *HOTKEY_ERR.lock().unwrap_or_else(|e| e.into_inner()) = Some(msg.clone());
            Err(msg)
        }
    }
}

/// 读最近一次注册失败的原因（空串表示正常）。
pub fn last_error() -> String {
    HOTKEY_ERR
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default()
        .unwrap_or_default()
}

/// 测试里要把 HOTKEY_ERR 复位，避免用例之间互相影响。
#[cfg(test)]
fn clear_error() {
    *HOTKEY_ERR.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

#[cfg(test)]
#[allow(clippy::needless_borrow)]
mod hotkey_tests {
    use super::validate_hotkey;

    // ── 合法：必须通过，且归一化结果要正确 ──────────────────────────
    #[test]
    fn legal_basic() {
        assert_eq!(validate_hotkey("Ctrl+Alt+T").unwrap(), "Ctrl+Alt+T");
        assert_eq!(validate_hotkey("Ctrl+Shift+F1").unwrap(), "Ctrl+Shift+F1");
        assert_eq!(validate_hotkey("Alt+0").unwrap(), "Alt+0");
    }

    #[test]
    fn legal_single_modifier() {
        // 最少 1 个修饰键是允许的
        assert_eq!(validate_hotkey("Ctrl+T").unwrap(), "Ctrl+T");
        assert_eq!(validate_hotkey("Shift+A").unwrap(), "Shift+A");
    }

    #[test]
    fn legal_all_f_keys() {
        for n in 1..=12 {
            let s = format!("Ctrl+F{n}");
            assert_eq!(validate_hotkey(&s).unwrap(), s, "F{n} 应该被接受");
        }
    }

    #[test]
    fn normalize_order_and_case() {
        // 输入顺序乱、大小写乱，输出必须唯一
        assert_eq!(validate_hotkey("shift+ctrl+t").unwrap(), "Ctrl+Shift+T");
        assert_eq!(validate_hotkey("CONTROL+ALT+t").unwrap(), "Ctrl+Alt+T");
        assert_eq!(validate_hotkey("alt+shift+q").unwrap(), "Shift+Alt+Q");
        // 同一个组合的四种写法归一化后必须完全相同
        let a = validate_hotkey("Ctrl+Shift+T").unwrap();
        let b = validate_hotkey("shift+ctrl+t").unwrap();
        let c = validate_hotkey("SHIFT+CTRL+T").unwrap();
        let d = validate_hotkey(" ctrl + shift + t ").unwrap();
        assert_eq!(a, b);
        assert_eq!(a, c);
        assert_eq!(a, d);
    }

    // ── 非法：必须被拒，且错误信息是中文、能指导用户 ────────────────
    #[test]
    fn reject_three_modifiers() {
        let e = validate_hotkey("Ctrl+Alt+Shift+T").unwrap_err();
        assert!(e.contains("最多两个修饰键"), "实际错误：{e}");
        // 顺序无关：三个修饰键一律拒绝，不论主键是什么
        let e = validate_hotkey("Alt+Ctrl+Shift+q").unwrap_err();
        assert!(e.contains("最多两个修饰键"), "实际错误：{e}");
    }

    #[test]
    fn reject_win_key() {
        for s in ["Ctrl+Win+T", "Super+T", "Cmd+T", "Meta+T"] {
            let e = validate_hotkey(s).unwrap_err();
            assert!(e.contains("Win 键"), "{s} 的实际错误：{e}");
        }
    }

    #[test]
    fn reject_no_modifier() {
        let e = validate_hotkey("T").unwrap_err();
        assert!(e.contains("至少"), "实际错误：{e}");
        let e = validate_hotkey("F5").unwrap_err();
        assert!(e.contains("至少"), "实际错误：{e}");
    }

    #[test]
    fn reject_out_of_whitelist_main_key() {
        // 方向键、符号键、F13+ 都不在白名单
        for s in ["Ctrl+Left", "Ctrl+-", "Ctrl+;", "Ctrl+F13", "Ctrl+F0"] {
            assert!(validate_hotkey(s).is_err(), "{s} 不该通过");
        }
    }

    #[test]
    fn reject_empty_and_duplicate() {
        assert!(validate_hotkey("").is_err());
        assert!(validate_hotkey("   ").is_err());
        assert!(validate_hotkey("Ctrl+Ctrl+T").is_err());
    }

    #[test]
    fn reject_two_main_keys() {
        // global-hotkey 只允许一个主键，两个必须在我们的校验层就被拒
        assert!(validate_hotkey("Ctrl+T+G").is_err());
    }

    #[test]
    fn clear_error_is_idempotent() {
        super::clear_error();
        assert_eq!(super::last_error(), "");
        super::clear_error();
        assert_eq!(super::last_error(), "");
    }
}
