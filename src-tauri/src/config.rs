//! 配置读写：`%APPDATA%\lightbox\config.json`。
//!
//! **只增字段**（ADR-013）：新字段一律 `#[serde(default)]` 或 `#[serde(default = "...")]`，
//! 这样老配置文件永远能被新版本读出来。反过来（同版本写的配置被老版本读）不做保证。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Serialize, Deserialize)]
pub struct Config {
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
    /// 划词翻译的全局热键，**永远存归一化形式**（`Ctrl+Alt+T`），
    /// 不存 `global-hotkey` 的 `control+alt+KeyT`。见 hotkey::validate_hotkey。
    #[serde(default = "d_hotkey")]
    pub hotkey: String,
}

fn d_from() -> String {
    "auto".to_string()
}
fn d_to() -> String {
    "zh".to_string()
}
fn d_hotkey() -> String {
    "Ctrl+Alt+T".to_string()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            appid: String::new(),
            secret: String::new(),
            from: d_from(),
            to: d_to(),
            hotkey: d_hotkey(),
        }
    }
}

pub fn config_path() -> PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(base).join("lightbox").join("config.json")
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

    #[test]
    fn config_roundtrip() {
        let c = Config::default();
        let s = serde_json::to_string(&c).unwrap();
        let d: Config = serde_json::from_str(&s).unwrap();
        assert_eq!(d.hotkey, c.hotkey);
        assert_eq!(d.from, c.from);
        assert_eq!(d.to, c.to);
    }

    /// 空对象 + 空文件内容都必须回落默认值，不能 panic。
    #[test]
    fn config_empty_object_falls_back_to_defaults() {
        let c: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(c.hotkey, "Ctrl+Alt+T");
    }
}
