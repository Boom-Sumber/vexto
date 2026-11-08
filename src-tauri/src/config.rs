//! 配置读写：`%APPDATA%\lightbox\config.json`。
//!
//! **只增字段**（ADR-013）：新字段一律 `#[serde(default)]` 或 `#[serde(default = "...")]`，
//! 这样老配置文件永远能被新版本读出来。反过来（同版本写的配置被老版本读）不做保证。
//!
//! S2 一次性把 S3–S5 要用的字段全定好，避免每个阶段都动一次这个文件。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Serialize, Deserialize)]
pub struct Config {
    // ---- 翻译引擎 ----
    /// 引擎标识：`baidu` | `openai`。对应 `translate::engine_by_id`
    #[serde(default = "d_engine")]
    pub engine: String,
    /// 百度翻译 APPID
    #[serde(default)]
    pub appid: String,
    /// 百度翻译密钥
    #[serde(default)]
    pub secret: String,
    /// 源语言。`auto` = 自动检测
    #[serde(default = "d_from")]
    pub from: String,
    /// 目标语言
    #[serde(default = "d_to")]
    pub to: String,

    // ---- OpenAI 兼容（DeepSeek / 硅基流动 / Ollama）----
    /// 接口根地址，不含 `/v1/chat/completions`
    #[serde(default = "d_openai_base")]
    pub openai_base: String,
    #[serde(default = "d_openai_model")]
    pub openai_model: String,
    #[serde(default)]
    pub openai_key: String,
    /// 自定义 system prompt，空则用内置的翻译指令
    #[serde(default)]
    pub openai_prompt: String,

    // ---- 热键 ----
    /// 划词翻译热键，**永远存归一化形式**（`Ctrl+Alt+T`），
    /// 不存 `global-hotkey` 的 `control+alt+KeyT`。见 hotkey::validate_hotkey。
    #[serde(default = "d_hotkey")]
    pub hotkey: String,
    /// 截图翻译热键（S5 才真正注册，S2 只存值）
    #[serde(default = "d_hotkey_ocr")]
    pub hotkey_ocr: String,
    /// 下载热键（S3 才真正注册，S2 只存值）
    #[serde(default = "d_hotkey_download")]
    pub hotkey_download: String,

    // ---- 模块开关 ----
    #[serde(default = "d_true")]
    pub enable_translate: bool,
    /// 下载默认关：S3 做完再改默认值
    #[serde(default)]
    pub enable_download: bool,
    #[serde(default)]
    pub enable_transfer: bool,
    #[serde(default)]
    pub enable_ocr: bool,

    // ---- 行为 ----
    #[serde(default = "d_popup_timeout")]
    pub popup_timeout_ms: u64,
    /// 译完是否自动把译文塞进剪贴板
    #[serde(default)]
    pub auto_copy: bool,
    /// 翻译缓存开关。默认开——同一段文字反复划是最高频场景，省一次就是省一次配额
    #[serde(default = "d_true")]
    pub cache_enabled: bool,

    // ---- 路径 ----
    #[serde(default)]
    pub download_dir: String,
    #[serde(default)]
    pub receive_dir: String,

    // ---- 版本 ----
    /// 配置格式版本，从 1 开始。老配置没有这个字段时按 1 处理
    #[serde(default = "d_version")]
    pub version: u32,
}

fn d_engine() -> String {
    "baidu".to_string()
}
fn d_from() -> String {
    "auto".to_string()
}
fn d_to() -> String {
    "zh".to_string()
}
fn d_openai_base() -> String {
    "https://api.deepseek.com".to_string()
}
fn d_openai_model() -> String {
    "deepseek-chat".to_string()
}
fn d_hotkey() -> String {
    "Ctrl+Alt+T".to_string()
}
fn d_hotkey_ocr() -> String {
    "Ctrl+Alt+S".to_string()
}
fn d_hotkey_download() -> String {
    "Ctrl+Alt+D".to_string()
}
fn d_true() -> bool {
    true
}
fn d_popup_timeout() -> u64 {
    15000
}
fn d_version() -> u32 {
    1
}

impl Default for Config {
    fn default() -> Self {
        Self {
            engine: d_engine(),
            appid: String::new(),
            secret: String::new(),
            from: d_from(),
            to: d_to(),
            openai_base: d_openai_base(),
            openai_model: d_openai_model(),
            openai_key: String::new(),
            openai_prompt: String::new(),
            hotkey: d_hotkey(),
            hotkey_ocr: d_hotkey_ocr(),
            hotkey_download: d_hotkey_download(),
            enable_translate: true,
            enable_download: false,
            enable_transfer: false,
            enable_ocr: false,
            popup_timeout_ms: d_popup_timeout(),
            auto_copy: false,
            cache_enabled: true,
            download_dir: String::new(),
            receive_dir: String::new(),
            version: d_version(),
        }
    }
}

pub fn config_path() -> PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(base).join("lightbox").join("config.json")
}

/// 日志目录（设置页「打开日志目录」用）。
pub fn log_dir() -> PathBuf {
    match config_path().parent() {
        Some(p) => p.to_path_buf(),
        None => PathBuf::from("."),
    }
}

/// 读配置。文件不存在、读不动、JSON 坏掉，一律回落到默认值——
/// 配置坏了不该让程序起不来，用户重填一次即可。
pub fn load() -> Config {
    match std::fs::read_to_string(config_path()) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => Config::default(),
    }
}

pub fn save(cfg: &Config) -> Result<(), String> {
    let p = config_path();
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("无法创建配置目录：{e}"))?;
    }
    let s = serde_json::to_string_pretty(cfg).map_err(|e| format!("序列化配置失败：{e}"))?;
    std::fs::write(&p, s).map_err(|e| format!("写入配置失败：{e}"))
}

#[cfg(test)]
mod tests {
    use super::Config;

    /// 老版本只有 appid，新版本加了字段也必须能读出来（ADR-013）。
    #[test]
    fn config_deserializes_with_missing_fields() {
        let json = r#"{"appid":"x"}"#;
        let c: Config = serde_json::from_str(json).unwrap();
        assert_eq!(c.appid, "x");
        assert_eq!(c.from, "auto");
        assert_eq!(c.to, "zh");
        assert_eq!(c.hotkey, "Ctrl+Alt+T");
        assert_eq!(c.secret, "");
    }

    /// S1 时代的完整配置（只有 5 个字段）必须能被 S2 读出来，且新字段取默认值。
    #[test]
    fn config_missing_new_fields_still_loads() {
        let json = r#"{"appid":"a","secret":"s","from":"auto","to":"zh","hotkey":"Ctrl+Alt+T"}"#;
        let c: Config = serde_json::from_str(json).unwrap();
        assert_eq!(c.engine, "baidu");
        assert_eq!(c.popup_timeout_ms, 15000);
        assert_eq!(c.hotkey_ocr, "Ctrl+Alt+S");
        assert_eq!(c.hotkey_download, "Ctrl+Alt+D");
        assert!(c.enable_translate);
        assert!(!c.enable_download);
        assert!(c.cache_enabled);
        assert_eq!(c.version, 1);
        assert_eq!(c.openai_model, "deepseek-chat");
    }

    #[test]
    fn config_roundtrip() {
        let c = Config::default();
        let s = serde_json::to_string(&c).unwrap();
        let d: Config = serde_json::from_str(&s).unwrap();
        assert_eq!(d.hotkey, c.hotkey);
        assert_eq!(d.from, c.from);
        assert_eq!(d.to, c.to);
        assert_eq!(d.engine, c.engine);
        assert_eq!(d.version, c.version);
    }

    /// 空对象 + 空文件内容都必须回落默认值，不能 panic。
    #[test]
    fn config_empty_object_falls_back_to_defaults() {
        let c: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(c.hotkey, "Ctrl+Alt+T");
    }

    /// 乱码 JSON 走 `unwrap_or_default()` 这条路：不能 panic。
    #[test]
    fn config_corrupt_json_falls_back_to_default() {
        let bad = "{ this is not json";
        let c: Config = serde_json::from_str(bad).unwrap_or_default();
        assert_eq!(c.engine, "baidu");
    }

    /// 关掉缓存时 `cache_enabled=false` 要能读出来（布尔字段最容易被漏 default）。
    #[test]
    fn config_respects_explicit_false() {
        let json = r#"{"enable_translate":false,"cache_enabled":false,"enable_download":true}"#;
        let c: Config = serde_json::from_str(json).unwrap();
        assert!(!c.enable_translate);
        assert!(!c.cache_enabled);
        assert!(c.enable_download);
    }
}
