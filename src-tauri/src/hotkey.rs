//! 全局热键：结构体、校验、归一化、注册。
//!
//! 规格（ADR-090，取代 ADR-018 的字符串格式）：
//! - 修饰键只允许 `Ctrl` / `Alt` / `Shift`，非空，顺序无关、自动去重；
//! - 至少包含一个 `Ctrl` 或 `Alt`，`Shift` 只能辅助；
//! - 恰好一个主键：字母、数字、F1–F24、方向键、编辑键、Space/Tab/Enter/Esc、
//!   常用标点与 PrintScreen；F12 是 Windows 调试器保留键，拒绝；
//! - 配置存 `{ modifiers, key, match }`，`match` 目前只支持 `exact`；
//! - 空绑定表示「未设置」，不注册。
//!
//! **为什么必须自己校验**：`global-hotkey` 对修饰键数量、组合是否被系统保留
//! 都不做业务层检查；规格是我们定的，只能靠这里的校验与测试守住。

use rust_i18n::t;
use serde::{Deserialize, Deserializer, Serialize};
use std::sync::Mutex;
use tauri::AppHandle;
use tauri_plugin_global_shortcut::{Code, Modifiers, Shortcut};

/// 最近一次热键注册失败的原因。`HotKeyErr` 在 Windows 上是静默失败，
/// GUI 程序看不到 stderr，用户只会觉得"按了没反应"——所以把原因存下来给设置页显示。
pub static HOTKEY_ERR: Mutex<Option<String>> = Mutex::new(None);

const MATCH_EXACT: &str = "exact";

/// 一个快捷键绑定。空对象（`key` 为空）表示未设置。
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HotkeyBinding {
    /// 规范顺序：Ctrl → Alt → Shift；顺序无关，读取时自动归一化。
    pub modifiers: Vec<String>,
    pub key: String,
    #[serde(rename = "match")]
    pub match_mode: String,
}

fn default_match() -> String {
    MATCH_EXACT.to_string()
}

impl Default for HotkeyBinding {
    fn default() -> Self {
        Self {
            modifiers: Vec::new(),
            key: String::new(),
            match_mode: default_match(),
        }
    }
}

impl<'de> Deserialize<'de> for HotkeyBinding {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct ObjRepr {
            #[serde(default)]
            modifiers: Vec<String>,
            #[serde(default)]
            key: String,
            #[serde(default = "default_match", rename = "match")]
            match_mode: String,
        }

        // 手改配置可能把热键写成任意 JSON；这里永远不返回 Err，
        // 避免一个坏字段让整份 config.json 回落到默认值（ADR-013）。
        let value = serde_json::Value::deserialize(deserializer)?;
        Ok(match value {
            serde_json::Value::String(text) => Self::from_legacy(&text),
            serde_json::Value::Object(obj) => {
                serde_json::from_value::<ObjRepr>(serde_json::Value::Object(obj))
                    .map(|obj| Self {
                        modifiers: obj.modifiers,
                        key: obj.key,
                        match_mode: obj.match_mode,
                    })
                    .unwrap_or_default()
                    .lenient()
            }
            _ => Self::default(),
        })
    }
}

impl std::fmt::Display for HotkeyBinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.display())
    }
}

impl HotkeyBinding {
    pub fn is_empty(&self) -> bool {
        self.key.trim().is_empty()
    }

    /// 界面显示：`Ctrl + Alt + T`；未设置返回空串。
    pub fn display(&self) -> String {
        if self.is_empty() {
            return String::new();
        }
        let mut parts = self.modifiers.clone();
        parts.push(key_label(&self.key).to_string());
        parts.join(" + ")
    }

    /// 紧凑形式，用于查重与错误信息：`Ctrl+Alt+T`。
    pub fn compact(&self) -> String {
        if self.is_empty() {
            return String::new();
        }
        let mut parts = self.modifiers.clone();
        parts.push(self.key.clone());
        parts.join("+")
    }

    /// 从旧配置字符串（`Ctrl+Alt+T`）解析，尽量保留用户原值。
    pub fn from_legacy(raw: &str) -> Self {
        let parts: Vec<&str> = raw
            .split('+')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .collect();
        let Some((key, modifiers)) = parts.split_last() else {
            return Self::default();
        };
        Self {
            modifiers: modifiers.iter().map(|m| (*m).to_string()).collect(),
            key: (*key).to_string(),
            match_mode: default_match(),
        }
        .lenient()
    }

    /// 宽松归一化：用于读取配置。未知主键保留原文，交给后续校验报错；
    /// 未知修饰键丢弃，`match` 统一回 exact。
    fn lenient(mut self) -> Self {
        let mut modifiers: Vec<String> = Vec::new();
        for raw in &self.modifiers {
            if let Some(name) = canonical_modifier(raw) {
                if !modifiers.iter().any(|m| m == name) {
                    modifiers.push(name.to_string());
                }
            }
        }
        modifiers.sort_by_key(|name| modifier_order(name));
        self.modifiers = modifiers;
        self.match_mode = default_match();

        let key = self.key.trim();
        self.key = if key.is_empty() {
            String::new()
        } else {
            canonical_key(key).unwrap_or_else(|| key.to_string())
        };
        self
    }

    /// 严格校验并归一化。Err 是给用户看的原因。
    pub fn validate(&self) -> Result<Self, String> {
        if !self.match_mode.eq_ignore_ascii_case(MATCH_EXACT) {
            return Err(t!("error.hotkey.invalidMatch", value = self.match_mode).into_owned());
        }
        if self.key.trim().is_empty() {
            if self.modifiers.is_empty() {
                return Ok(Self::default());
            }
            return Err(t!("error.hotkey.needKey").into_owned());
        }
        if self.key.contains('+') {
            return Err(t!("error.hotkey.multipleKeys").into_owned());
        }

        let mut modifiers: Vec<String> = Vec::new();
        for raw in &self.modifiers {
            let name = canonical_modifier(raw).ok_or_else(|| {
                if raw.eq_ignore_ascii_case("WIN")
                    || raw.eq_ignore_ascii_case("SUPER")
                    || raw.eq_ignore_ascii_case("CMD")
                    || raw.eq_ignore_ascii_case("META")
                {
                    t!("error.hotkey.winKey").into_owned()
                } else {
                    t!("error.hotkey.unknownModifier", modifier = raw).into_owned()
                }
            })?;
            if modifiers.iter().any(|m| m == name) {
                return Err(t!("error.hotkey.duplicateModifier", name = name).into_owned());
            }
            modifiers.push(name.to_string());
        }
        if modifiers.is_empty() {
            return Err(t!("error.hotkey.needModifier").into_owned());
        }
        if !modifiers.iter().any(|m| m == "Ctrl" || m == "Alt") {
            return Err(t!("error.hotkey.needPrimaryModifier").into_owned());
        }
        modifiers.sort_by_key(|name| modifier_order(name));

        let key = canonical_key(self.key.trim())
            .ok_or_else(|| t!("error.hotkey.unsupportedMain", main = &self.key).into_owned())?;
        let binding = Self {
            modifiers,
            key: key.to_string(),
            match_mode: default_match(),
        };
        if binding.key == "F12" || reserved_combo(&binding.compact()) {
            return Err(t!("error.hotkey.reserved", combo = binding.display()).into_owned());
        }
        if binding.to_shortcut().is_none() {
            return Err(
                t!("error.hotkey.invalidCombo", normalized = binding.compact()).into_owned(),
            );
        }
        Ok(binding)
    }

    /// 转成注册库的 `Shortcut`；空绑定或未知识别键返回 None。
    pub fn to_shortcut(&self) -> Option<Shortcut> {
        if self.is_empty() {
            return None;
        }
        let mut mods = Modifiers::empty();
        for name in &self.modifiers {
            match name.as_str() {
                "Ctrl" => mods |= Modifiers::CONTROL,
                "Alt" => mods |= Modifiers::ALT,
                "Shift" => mods |= Modifiers::SHIFT,
                _ => return None,
            }
        }
        Some(Shortcut::new(Some(mods), key_code(&self.key)?))
    }
}

fn modifier_order(name: &str) -> u8 {
    match name {
        "Ctrl" => 0,
        "Alt" => 1,
        "Shift" => 2,
        _ => 9,
    }
}

fn canonical_modifier(raw: &str) -> Option<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "ctrl" | "control" => Some("Ctrl"),
        "alt" => Some("Alt"),
        "shift" => Some("Shift"),
        _ => None,
    }
}

/// 把用户输入（`a` / `KeyA` / `ArrowUp` / `PgUp` …）规整为配置里的规范名。
fn canonical_key(raw: &str) -> Option<String> {
    let text = raw.trim();
    if text.is_empty() {
        return None;
    }
    let upper = text.to_ascii_uppercase();
    if upper.len() == 1 && upper.as_bytes()[0].is_ascii_alphanumeric() {
        return Some(upper);
    }
    let key = match upper.as_str() {
        "UP" | "ARROWUP" => "Up",
        "DOWN" | "ARROWDOWN" => "Down",
        "LEFT" | "ARROWLEFT" => "Left",
        "RIGHT" | "ARROWRIGHT" => "Right",
        "INSERT" | "INS" => "Insert",
        "DELETE" | "DEL" => "Delete",
        "HOME" => "Home",
        "END" => "End",
        "PAGEUP" | "PGUP" => "PageUp",
        "PAGEDOWN" | "PGDN" => "PageDown",
        "BACKSPACE" => "Backspace",
        "SPACE" => "Space",
        "TAB" => "Tab",
        "ENTER" | "RETURN" => "Enter",
        "ESC" | "ESCAPE" => "Esc",
        "PRINTSCREEN" | "PRTSC" | "PRTSCR" => "PrintScreen",
        "BACKQUOTE" | "GRAVE" | "`" => "`",
        "MINUS" | "-" => "-",
        "EQUAL" | "=" => "=",
        "BRACKETLEFT" | "[" => "[",
        "BRACKETRIGHT" | "]" => "]",
        "BACKSLASH" | "\\" => "\\",
        "SEMICOLON" | ";" => ";",
        "QUOTE" | "APOSTROPHE" | "'" => "'",
        "COMMA" | "," => ",",
        "PERIOD" | "DOT" | "." => ".",
        "SLASH" | "/" => "/",
        _ => {
            if let Some(n) = upper.strip_prefix('F') {
                if let Ok(n) = n.parse::<u8>() {
                    if (1..=24).contains(&n) {
                        return Some(format!("F{n}"));
                    }
                }
            }
            return None;
        }
    };
    Some(key.to_string())
}

fn key_label(key: &str) -> &str {
    match key {
        "Up" => "↑",
        "Down" => "↓",
        "Left" => "←",
        "Right" => "→",
        "Insert" => "Ins",
        "Delete" => "Del",
        "PageUp" => "PgUp",
        "PageDown" => "PgDn",
        "Esc" => "Esc",
        "Backspace" => "Backspace",
        "PrintScreen" => "PrintScreen",
        other => other,
    }
}

fn key_code(key: &str) -> Option<Code> {
    Some(match key {
        "A" => Code::KeyA,
        "B" => Code::KeyB,
        "C" => Code::KeyC,
        "D" => Code::KeyD,
        "E" => Code::KeyE,
        "F" => Code::KeyF,
        "G" => Code::KeyG,
        "H" => Code::KeyH,
        "I" => Code::KeyI,
        "J" => Code::KeyJ,
        "K" => Code::KeyK,
        "L" => Code::KeyL,
        "M" => Code::KeyM,
        "N" => Code::KeyN,
        "O" => Code::KeyO,
        "P" => Code::KeyP,
        "Q" => Code::KeyQ,
        "R" => Code::KeyR,
        "S" => Code::KeyS,
        "T" => Code::KeyT,
        "U" => Code::KeyU,
        "V" => Code::KeyV,
        "W" => Code::KeyW,
        "X" => Code::KeyX,
        "Y" => Code::KeyY,
        "Z" => Code::KeyZ,
        "0" => Code::Digit0,
        "1" => Code::Digit1,
        "2" => Code::Digit2,
        "3" => Code::Digit3,
        "4" => Code::Digit4,
        "5" => Code::Digit5,
        "6" => Code::Digit6,
        "7" => Code::Digit7,
        "8" => Code::Digit8,
        "9" => Code::Digit9,
        "F1" => Code::F1,
        "F2" => Code::F2,
        "F3" => Code::F3,
        "F4" => Code::F4,
        "F5" => Code::F5,
        "F6" => Code::F6,
        "F7" => Code::F7,
        "F8" => Code::F8,
        "F9" => Code::F9,
        "F10" => Code::F10,
        "F11" => Code::F11,
        "F12" => Code::F12,
        "F13" => Code::F13,
        "F14" => Code::F14,
        "F15" => Code::F15,
        "F16" => Code::F16,
        "F17" => Code::F17,
        "F18" => Code::F18,
        "F19" => Code::F19,
        "F20" => Code::F20,
        "F21" => Code::F21,
        "F22" => Code::F22,
        "F23" => Code::F23,
        "F24" => Code::F24,
        "Up" => Code::ArrowUp,
        "Down" => Code::ArrowDown,
        "Left" => Code::ArrowLeft,
        "Right" => Code::ArrowRight,
        "Insert" => Code::Insert,
        "Delete" => Code::Delete,
        "Home" => Code::Home,
        "End" => Code::End,
        "PageUp" => Code::PageUp,
        "PageDown" => Code::PageDown,
        "Backspace" => Code::Backspace,
        "Space" => Code::Space,
        "Tab" => Code::Tab,
        "Enter" => Code::Enter,
        "Esc" => Code::Escape,
        "PrintScreen" => Code::PrintScreen,
        "`" => Code::Backquote,
        "-" => Code::Minus,
        "=" => Code::Equal,
        "[" => Code::BracketLeft,
        "]" => Code::BracketRight,
        "\\" => Code::Backslash,
        ";" => Code::Semicolon,
        "'" => Code::Quote,
        "," => Code::Comma,
        "." => Code::Period,
        "/" => Code::Slash,
        _ => return None,
    })
}

/// Windows 保留 / 无法稳定截获的组合。注册失败仍会兜底提示。
fn reserved_combo(combo: &str) -> bool {
    matches!(
        combo,
        "Ctrl+Alt+Delete"
            | "Ctrl+Shift+Esc"
            | "Ctrl+Esc"
            | "Ctrl+Alt+Tab"
            | "Alt+Tab"
            | "Alt+Shift+Tab"
            | "Alt+Esc"
            | "Alt+F4"
            | "Alt+Space"
    )
}

/// 一个待注册的热键：给用户看的名字、绑定、按下时的回调。
pub type Binding = (
    &'static str,
    HotkeyBinding,
    Box<dyn Fn(&AppHandle) + Send + Sync>,
);

/// 在一组绑定里找第一个重复组合。空绑定跳过。
pub fn find_duplicate(
    items: &[(&'static str, &HotkeyBinding)],
) -> Option<(&'static str, &'static str, String)> {
    let mut seen: Vec<(&'static str, String)> = Vec::new();
    for (key, binding) in items {
        let combo = (*binding).clone().lenient().compact();
        if combo.is_empty() {
            continue;
        }
        if let Some((prev, _)) = seen
            .iter()
            .find(|(_, value)| value.eq_ignore_ascii_case(&combo))
        {
            return Some((*prev, *key, combo));
        }
        seen.push((*key, combo));
    }
    None
}

/// 一次性注册多个全局热键。返回给用户看的错误列表（空 = 全部成功）。
///
/// 注册前先 `unregister_all()`：同一组合在系统里只能被一个进程占用，
/// 想"先注册新的、成功后再注销旧的"做不到。只提供批量接口，
/// 就不会有人写出"连着调两次、把前一个注销掉"的代码。
pub fn register_all(app: &AppHandle, list: Vec<Binding>) -> Vec<String> {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

    let gs = app.global_shortcut();
    let _ = gs.unregister_all();

    let mut errors: Vec<String> = Vec::new();
    let mut done: Vec<String> = Vec::new();

    for (what, binding, on_pressed) in list {
        if binding.is_empty() {
            continue;
        }
        let binding = match binding.validate() {
            Ok(value) => value,
            Err(e) => {
                errors.push(
                    t!("error.hotkey.invalidFormat", what = what, detail = e).into_owned(),
                );
                continue;
            }
        };
        let combo = binding.display();

        if done.iter().any(|value| value.eq_ignore_ascii_case(&binding.compact())) {
            errors.push(
                t!(
                    "error.hotkey.duplicateCombo",
                    what = what,
                    combo = combo
                )
                .into_owned(),
            );
            continue;
        }

        let Some(shortcut) = binding.to_shortcut() else {
            errors.push(
                t!(
                    "error.hotkey.invalidCombo",
                    normalized = binding.compact()
                )
                .into_owned(),
            );
            continue;
        };

        match gs.on_shortcut(shortcut, move |app, _sc, ev| {
            if ev.state() == ShortcutState::Pressed {
                on_pressed(app);
            }
        }) {
            Ok(()) => done.push(binding.compact()),
            Err(_) => errors.push(
                t!(
                    "error.hotkey.registerFailed",
                    what = what,
                    combo = combo
                )
                .into_owned(),
            ),
        }
    }

    *HOTKEY_ERR.lock().unwrap_or_else(|e| e.into_inner()) = if errors.is_empty() {
        None
    } else {
        Some(errors.join("\n"))
    };
    errors
}

/// 读最近一次注册失败的原因（空串表示正常）。
pub fn last_error() -> String {
    HOTKEY_ERR
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default()
        .unwrap_or_default()
}

#[cfg(test)]
mod hotkey_tests {
    use super::{find_duplicate, HotkeyBinding};

    fn bind(mods: &[&str], key: &str) -> HotkeyBinding {
        HotkeyBinding {
            modifiers: mods.iter().map(|m| (*m).to_string()).collect(),
            key: key.to_string(),
            match_mode: "exact".to_string(),
        }
    }

    #[test]
    fn legal_bindings_are_normalized() {
        let cases = [
            (bind(&["Ctrl", "Alt"], "T"), "Ctrl+Alt+T"),
            (bind(&["shift", "ctrl"], "f1"), "Ctrl+Shift+F1"),
            (bind(&["Alt"], "0"), "Alt+0"),
            (bind(&["Ctrl", "Alt", "Shift"], "A"), "Ctrl+Alt+Shift+A"),
            (bind(&["Ctrl"], "Up"), "Ctrl+Up"),
            (bind(&["Alt"], "PageUp"), "Alt+PageUp"),
            (bind(&["Ctrl"], "Space"), "Ctrl+Space"),
            (bind(&["Ctrl", "Shift"], ";"), "Ctrl+Shift+;"),
            (bind(&["alt", "shift"], "prtsc"), "Alt+Shift+PrintScreen"),
        ];
        for (input, expected) in cases {
            let value = input.validate().expect("应该合法");
            assert_eq!(value.compact(), expected);
        }
    }

    #[test]
    fn modifier_order_is_canonical() {
        for raw in ["ctrl+alt+t", "alt+ctrl+t", "CTRL+ALT+T", " Alt + Ctrl + T "] {
            let value = HotkeyBinding::from_legacy(raw).validate().unwrap();
            assert_eq!(value.modifiers, vec!["Ctrl", "Alt"]);
            assert_eq!(value.key, "T");
        }
    }

    #[test]
    fn shift_only_and_bare_keys_are_rejected() {
        assert!(bind(&["Shift"], "A").validate().is_err(), "Shift 只能辅助");
        assert!(bind(&[], "A").validate().is_err(), "必须有修饰键");
        assert!(bind(&[], "").validate().is_ok(), "空绑定表示未设置");
        assert!(bind(&["Ctrl"], "").validate().is_err(), "纯修饰键组合非法");
    }

    #[test]
    fn unsupported_and_reserved_keys_are_rejected() {
        assert!(bind(&["Ctrl"], "F12").validate().is_err(), "F12 是调试器保留键");
        assert!(bind(&["Ctrl"], "Numpad5").validate().is_err());
        assert!(bind(&["Ctrl"], "A+B").validate().is_err());
        for combo in [
            ("Ctrl", "Alt", "Delete"),
            ("Ctrl", "Shift", "Esc"),
            ("Ctrl", "", "Esc"),
            ("Alt", "", "Tab"),
            ("Alt", "", "F4"),
        ] {
            let mods: Vec<&str> = [combo.0, combo.1].into_iter().filter(|m| !m.is_empty()).collect();
            assert!(
                bind(&mods, combo.2).validate().is_err(),
                "{} 应该被拒绝",
                combo.2
            );
        }
    }

    #[test]
    fn legacy_strings_and_objects_roundtrip() {
        let legacy = HotkeyBinding::from_legacy("Ctrl+Alt+T");
        assert_eq!(legacy.modifiers, vec!["Ctrl", "Alt"]);
        assert_eq!(legacy.key, "T");
        assert_eq!(legacy.match_mode, "exact");

        let text: HotkeyBinding = serde_json::from_str("\"Ctrl+Shift+F2\"").unwrap();
        assert_eq!(text.compact(), "Ctrl+Shift+F2");

        let object: HotkeyBinding = serde_json::from_str(
            r#"{"modifiers":["alt","ctrl"],"key":"z","match":"exact"}"#,
        )
        .unwrap();
        assert_eq!(object.compact(), "Ctrl+Alt+Z");

        let missing_match: HotkeyBinding =
            serde_json::from_str(r#"{"modifiers":["Ctrl"],"key":"K"}"#).unwrap();
        assert_eq!(missing_match.match_mode, "exact");
    }

    #[test]
    fn duplicate_detection_names_both_sides() {
        let a = bind(&["Ctrl", "Alt"], "T");
        let b = bind(&["alt", "ctrl"], "t");
        let c = bind(&["Ctrl"], "T");
        let items = [("first", &a), ("second", &b)];
        let found = find_duplicate(&items).expect("应该发现重复");
        assert_eq!(found.0, "first");
        assert_eq!(found.1, "second");
        assert_eq!(found.2, "Ctrl+Alt+T");

        let no_dup = [("a", &a), ("c", &c)];
        assert!(find_duplicate(&no_dup).is_none());

        let empty = HotkeyBinding::default();
        let with_empty = [("a", &empty), ("b", &empty)];
        assert!(find_duplicate(&with_empty).is_none(), "空绑定不参与查重");
    }

    #[test]
    fn display_uses_spaced_labels() {
        let value = bind(&["Ctrl", "Alt"], "Up").validate().unwrap();
        assert_eq!(value.display(), "Ctrl + Alt + ↑");
        assert_eq!(HotkeyBinding::default().display(), "");
    }
}
