//! 界面语言的运行时入口：系统语言解析、全局 locale、动态 key 查询。
//!
//! 翻译本体由 `rust-i18n` 在编译期从 `src-tauri/locales/*.json` 载入；
//! 这里只负责把配置里的 `ui_language`（可能是 `system`）解析成受支持的语言，
//! 并给 schema 这类运行时拼出来的 key 提供带中文兜底的查询。

use std::sync::RwLock;

use serde_json::Value;
use tauri::Manager;

static CURRENT: RwLock<&'static str> = RwLock::new("en");

fn canonical(code: &str) -> Option<&'static str> {
    match code {
        "zh-CN" => Some("zh-CN"),
        "en" => Some("en"),
        "ja" => Some("ja"),
        "ko" => Some("ko"),
        "es" => Some("es"),
        _ => None,
    }
}

/// 把语言标签映射到受支持的语言；未知一律回落英文。
fn from_tag(tag: &str) -> &'static str {
    let lower = tag.trim().to_ascii_lowercase();
    if lower.starts_with("zh") {
        "zh-CN"
    } else if lower.starts_with("ja") {
        "ja"
    } else if lower.starts_with("ko") {
        "ko"
    } else if lower.starts_with("es") {
        "es"
    } else {
        "en"
    }
}

/// 配置值 → 实际语言。`system` 读 Windows 显示语言；受支持值原样规范化。
pub fn resolve(pref: &str) -> &'static str {
    let value = pref.trim();
    if value.eq_ignore_ascii_case("system") || value.is_empty() {
        return from_tag(&system_locale());
    }
    canonical(value).unwrap_or_else(|| from_tag(value))
}

/// Windows 显示语言（BCP-47，如 `zh-CN` / `en-US`）。读不到按英文。
pub fn system_locale() -> String {
    #[cfg(windows)]
    {
        use windows::Win32::Globalization::GetUserDefaultLocaleName;
        let mut buf = [0u16; 85];
        // SAFETY: 传入的是有效缓冲区与长度；失败返回 0，不读取未初始化内容。
        let n = unsafe { GetUserDefaultLocaleName(&mut buf) };
        if n > 0 {
            let len = (n as usize).saturating_sub(1);
            return String::from_utf16_lossy(&buf[..len]);
        }
    }
    "en".to_string()
}

/// 设置全局语言；`rust-i18n` 与本地缓存同时更新。
pub fn set_locale(pref: &str) {
    let code = resolve(pref);
    rust_i18n::set_locale(code);
    *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = code;
}

pub fn current() -> &'static str {
    *CURRENT.read().unwrap_or_else(|e| e.into_inner())
}

/// 启动时调用一次；配置里的 `system` 在这里被解析。
pub fn init(cfg: &crate::config::Config) {
    set_locale(&cfg.ui_language);
}

/// 动态 key 查询：查不到时返回调用方给的中文兜底，避免把 key 显示到界面上。
///
/// 直接用生成的 `_rust_i18n_try_translate`，而不是 `t!`：后者在缺失时会返回
/// `"locale.key"`，无法区分「翻译值恰好等于 key」和「完全没有这条翻译」。
pub fn t_or(key: &str, fallback: &str) -> String {
    match crate::_rust_i18n_try_translate(current(), key) {
        Some(value) => value.into_owned(),
        None => fallback.to_string(),
    }
}

fn replace_text(slot: &mut Value, key: &str) {
    let Some(original) = slot.as_str().map(str::to_string) else {
        return;
    };
    let translated = t_or(key, &original);
    if translated != original {
        *slot = Value::String(translated);
    }
}

fn replace_optional(field: &mut Value, name: &str, key: &str) {
    if let Some(slot) = field.get_mut(name) {
        replace_text(slot, key);
    }
}

fn localize_field(field: &mut Value) {
    let Some(key) = field.get("key").and_then(Value::as_str).map(str::to_string) else {
        return;
    };
    if key.is_empty() {
        return;
    }
    if let Some(label) = field.get_mut("label") {
        replace_text(label, &format!("schema.field.{key}.label"));
    }
    replace_optional(field, "help", &format!("schema.field.{key}.help"));

    let kind = field
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    match kind.as_str() {
        "text" | "template" => {
            replace_optional(
                field,
                "placeholder",
                &format!("schema.field.{key}.placeholder"),
            );
        }
        "number" => {
            replace_optional(field, "unit", &format!("schema.field.{key}.unit"));
        }
        "select" => {
            if let Some(options) = field.get_mut("options").and_then(Value::as_array_mut) {
                for option in options {
                    let Some(pair) = option.as_array_mut() else {
                        continue;
                    };
                    if pair.len() < 2 {
                        continue;
                    }
                    let Some(value) = pair[0].as_str().map(str::to_string) else {
                        continue;
                    };
                    replace_text(&mut pair[1], &format!("schema.field.{key}.option.{value}"));
                }
            }
        }
        "button_row" => {
            if let Some(actions) = field.get_mut("actions").and_then(Value::as_array_mut) {
                for action in actions {
                    let Some(pair) = action.as_array_mut() else {
                        continue;
                    };
                    if pair.len() < 2 {
                        continue;
                    }
                    let Some(command) = pair[0].as_str().map(str::to_string) else {
                        continue;
                    };
                    replace_text(&mut pair[1], &format!("schema.action.{command}"));
                }
            }
        }
        "group" => {
            if let Some(title) = field.get_mut("title") {
                replace_text(title, &format!("schema.group.{key}.title"));
            }
        }
        "note" => {
            if let Some(text) = field.get_mut("text") {
                replace_text(text, &format!("schema.note.{key}.text"));
            }
        }
        "list" => {
            if let Some(items) = field.get_mut("item_fields").and_then(Value::as_array_mut) {
                for item in items {
                    localize_field(item);
                }
            }
        }
        _ => {}
    }
}

/// 设置页 schema 在返回前统一本地化；缺失的 key 保留中文原文。
pub fn localize_tool(tool: &mut Value) {
    let id = tool
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if let Some(name) = tool.get_mut("name") {
        replace_text(name, &format!("schema.tool.{id}.name"));
    }
    if let Some(desc) = tool.get_mut("desc") {
        replace_text(desc, &format!("schema.tool.{id}.desc"));
    }
    let Some(sections) = tool.get_mut("sections").and_then(Value::as_array_mut) else {
        return;
    };
    for section in sections {
        let sid = section
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if let Some(name) = section.get_mut("name") {
            replace_text(name, &format!("schema.section.{sid}.name"));
        }
        if let Some(fields) = section.get_mut("fields").and_then(Value::as_array_mut) {
            for field in fields {
                localize_field(field);
            }
        }
    }
}

/// 窗口 label → 标题 key；popup 是品牌名，不参与本地化。
pub fn window_title_key(label: &str) -> Option<&'static str> {
    match label {
        "settings" => Some("window.settings"),
        "main" => Some("window.downloads"),
        "new-task" => Some("window.newTask"),
        "preview" => Some("window.preview"),
        "shot_feedback" => Some("window.feedback"),
        "scroll_control" => Some("window.scroll"),
        "ocr_overlay" => Some("window.ocrTranslate"),
        "shot_overlay" => Some("window.feedback"),
        "scroll_overlay" => Some("window.scroll"),
        l if l == "pin" || l.starts_with("pin-") => Some("window.pin"),
        _ => None,
    }
}

/// 语言切换后更新所有已打开窗口的标题；frontend 的 document.title 也会更新。
pub fn apply_window_titles(app: &tauri::AppHandle) {
    for (label, window) in app.webview_windows() {
        let Some(key) = window_title_key(&label) else {
            continue;
        };
        let title = t_or(key, "");
        if !title.is_empty() {
            if let Err(e) = window.set_title(&title) {
                crate::dlog::warn(&format!("i18n: 更新窗口 {label} 标题失败：{e}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_map_to_supported_locales() {
        assert_eq!(resolve("system"), resolve("system")); // 只验证不 panic
        assert_eq!(from_tag("zh-Hans-CN"), "zh-CN");
        assert_eq!(from_tag("zh-TW"), "zh-CN");
        assert_eq!(from_tag("en-US"), "en");
        assert_eq!(from_tag("ja-JP"), "ja");
        assert_eq!(from_tag("ko-KR"), "ko");
        assert_eq!(from_tag("es-ES"), "es");
        assert_eq!(from_tag("fr-FR"), "en");
    }

    #[test]
    fn explicit_values_are_canonicalized() {
        assert_eq!(resolve("zh-CN"), "zh-CN");
        assert_eq!(resolve("en"), "en");
        assert_eq!(resolve("ja"), "ja");
        assert_eq!(resolve("ko"), "ko");
        assert_eq!(resolve("es"), "es");
        assert_eq!(resolve("fr"), "en");
    }

    #[test]
    fn dynamic_lookup_falls_back_to_chinese_text() {
        assert_eq!(t_or("schema.__missing__", "兜底"), "兜底");
    }
}
