//! 常规：影响两个以上工具的东西才放这里（S2 任务书 §2.6.1 的判定标准）。
//!
//! 三个标签：常规（共享设置）/ 工具管理（统一启停）/ 快捷键（按工具分组集中配置）。

use rust_i18n::t;

use super::{Field, Section, Tool};
use crate::config;

pub struct General;

impl Tool for General {
    fn id(&self) -> &'static str {
        "general"
    }

    fn name(&self) -> &'static str {
        "常规"
    }

    fn desc(&self) -> &'static str {
        "所有工具共用的设置"
    }

    fn icon(&self) -> &'static str {
        "gear"
    }

    fn enabled(&self) -> bool {
        true
    }

    fn set_enabled(&self, _on: bool) -> Result<(), String> {
        // 常规项不允许整体停用：它只是配置容器，停用了没有意义
        Err("「常规」是配置分组，不能停用".to_string())
    }

    /// 设置页不显示「启用」开关：这里没有可停用的东西，
    /// 给一个永远切不动的开关只会让用户以为程序坏了。
    fn switchable(&self) -> bool {
        false
    }

    fn status_line(&self) -> String {
        let cfg = config::load();
        let cache = if cfg.cache_enabled {
            t!("status.line.cacheOn")
        } else {
            t!("status.line.cacheOff")
        };
        let log = if cfg.log_enabled {
            t!("status.line.logOn")
        } else {
            t!("status.line.logOff")
        };
        t!("status.line.general", cache = cache, log = log).into_owned()
    }

    fn sections(&self) -> Vec<Section> {
        sections_for(&config::load())
    }
}

/// 常规的三个标签：常规 / 工具管理 / 快捷键。
///
/// 工具管理与快捷键都随「工具启用状态」变化；做成纯函数便于单测，
/// 不依赖进程当前的 config.json。
fn sections_for(cfg: &config::Config) -> Vec<Section> {
    let general = Section::new(
        "general",
        "常规",
        vec![
            Field::group("appearance", "外观"),
            Field::select(
                "ui_language",
                "界面语言",
                &[
                    ("system", "跟随系统"),
                    ("zh-CN", "简体中文"),
                    ("en", "English"),
                    ("ja", "日本語"),
                    ("ko", "한국어"),
                    ("es", "Español"),
                ],
            )
            .auto_apply(),
            Field::select(
                "appearance_theme",
                "主题",
                &[("light", "浅色"), ("dark", "深色"), ("system", "系统")],
            )
            .auto_apply(),
            Field::color("accent_color", "主色调", ACCENT_PRESETS).auto_apply(),
            Field::group("startup", "启动"),
            Field::switch("autostart", "开机自动启动"),
            Field::select(
                "tray_left_click",
                "托盘图标左键",
                &[
                    ("settings", "打开设置页"),
                    ("tasks", "打开任务窗口"),
                    ("input", "打开输入翻译"),
                    ("screenshot", "区域截图"),
                ],
            ),
            Field::group("performance", "性能"),
            Field::switch("lite_mode", "轻量模式").help("托盘状态下释放界面内存"),
            Field::group("translatePopup", "翻译与弹窗"),
            Field::switch("cache_enabled", "翻译缓存"),
            Field::number_unit("popup_timeout_ms", "弹窗自动关闭", 3000, 120000, "毫秒"),
            Field::group("configLog", "配置与日志"),
            Field::switch("log_enabled", "日志启用"),
            Field::select(
                "log_level",
                "日志记录等级",
                &[
                    ("info", "INFO"),
                    ("debug", "DEBUG"),
                    ("warn", "WARN"),
                    ("error", "ERROR"),
                    ("trace", "TRACE"),
                ],
            )
            .show_if("log_enabled", "true"),
            Field::buttons(&[
                ("open_config_file", "打开配置文件"),
                ("open_config_dir", "打开配置目录"),
                ("reset_all_config", "恢复默认配置"),
            ]),
        ],
    );

    let tools = Section::new(
        "tools",
        "工具管理",
        vec![Field::tool_list("tool_list", "")],
    );

    let mut hotkeys = Vec::new();
    if cfg.enable_translate {
        hotkeys.push(Field::group("hotkeysTranslate", "翻译"));
        hotkeys.push(Field::hotkey("hotkey", "划词翻译"));
        hotkeys.push(Field::hotkey("hotkey_input", "输入翻译"));
        hotkeys.push(Field::buttons(&[("reset_hotkeys_translate", "恢复默认")]));
    }
    if cfg.enable_screenshot {
        hotkeys.push(Field::group("hotkeysScreenshot", "截图"));
        hotkeys.push(Field::hotkey("hotkey_screenshot", "区域截图"));
        hotkeys.push(Field::hotkey("hotkey_fullscreen", "全屏截图"));
        hotkeys.push(Field::buttons(&[("reset_hotkeys_screenshot", "恢复默认")]));
    }
    if cfg.enable_download {
        hotkeys.push(Field::group("hotkeysDownload", "下载"));
        hotkeys.push(Field::hotkey("hotkey_download", "打开新建下载任务"));
        hotkeys.push(Field::buttons(&[("reset_hotkeys_download", "恢复默认")]));
    }

    vec![general, tools, Section::new("hotkeys", "快捷键", hotkeys)]
}

/// 主色调预设色板（外观功能会话，用户已确认六色）。
/// 渐变端 / 淡底 / 描边全部由前端 oklch 从这里给的主色现算。
pub const ACCENT_PRESETS: &[(&str, &str)] = &[
    ("#11a780", "翡翠绿"),
    ("#7c5cff", "高级紫"),
    ("#3b82f6", "湛蓝"),
    ("#14b8a6", "青碧"),
    ("#f59e0b", "琥珀"),
    ("#ec4899", "玫红"),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn hotkey_keys(sections: &[Section]) -> Vec<&'static str> {
        sections
            .iter()
            .find(|s| s.id == "hotkeys")
            .map(|s| {
                s.fields
                    .iter()
                    .filter(|f| matches!(f.kind, crate::tools::FieldKind::Hotkey))
                    .map(|f| f.key)
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn sections_are_general_tools_and_hotkeys() {
        let s = sections_for(&config::Config::default());
        let ids: Vec<&str> = s.iter().map(|sec| sec.id).collect();
        assert_eq!(ids, vec!["general", "tools", "hotkeys"]);
        assert_eq!((s[1].id, s[1].name), ("tools", "工具管理"));
        assert_eq!((s[2].id, s[2].name), ("hotkeys", "快捷键"));
        assert!(s[1]
            .fields
            .iter()
            .any(|f| matches!(f.kind, crate::tools::FieldKind::ToolList)));
    }

    #[test]
    fn hotkeys_follow_enabled_tools() {
        let mut cfg = config::Config::default();
        let keys = hotkey_keys(&sections_for(&cfg));
        assert_eq!(
            keys,
            vec![
                "hotkey",
                "hotkey_input",
                "hotkey_screenshot",
                "hotkey_fullscreen",
                "hotkey_download"
            ],
            "四个功能工具出厂全部启用（ADR-092）"
        );

        cfg.enable_screenshot = false;
        cfg.enable_download = false;
        let keys = hotkey_keys(&sections_for(&cfg));
        assert_eq!(keys, vec!["hotkey", "hotkey_input"]);

        cfg.enable_translate = false;
        assert!(hotkey_keys(&sections_for(&cfg)).is_empty());
    }

    #[test]
    fn disabled_tool_groups_are_hidden() {
        let mut cfg = config::Config::default();
        cfg.enable_translate = false;
        let s = sections_for(&cfg);
        let hot = s.iter().find(|sec| sec.id == "hotkeys").unwrap();
        let groups: Vec<&str> = hot
            .fields
            .iter()
            .filter_map(|f| match &f.kind {
                crate::tools::FieldKind::Group { title } => Some(*title),
                _ => None,
            })
            .collect();
        assert!(!groups.contains(&"翻译"));
        assert_eq!(groups, vec!["截图", "下载"]);
    }

    /// 每组热键末尾要有「恢复默认」按钮（ADR-092）。
    #[test]
    fn hotkey_groups_end_with_reset_buttons() {
        let s = sections_for(&config::Config::default());
        let hot = s.iter().find(|sec| sec.id == "hotkeys").unwrap();
        let actions: Vec<&str> = hot
            .fields
            .iter()
            .filter_map(|f| match &f.kind {
                crate::tools::FieldKind::ButtonRow { actions } => Some(actions[0].0),
                _ => None,
            })
            .collect();
        assert_eq!(
            actions,
            vec![
                "reset_hotkeys_translate",
                "reset_hotkeys_screenshot",
                "reset_hotkeys_download"
            ]
        );
    }

    /// 「配置与日志」分类里要有全量恢复按钮（ADR-092）。
    #[test]
    fn config_section_has_reset_all_button() {
        let s = sections_for(&config::Config::default());
        let general = s.iter().find(|sec| sec.id == "general").unwrap();
        let actions: Vec<&str> = general
            .fields
            .iter()
            .flat_map(|f| match &f.kind {
                crate::tools::FieldKind::ButtonRow { actions } => actions.iter().map(|a| a.0).collect(),
                _ => Vec::new(),
            })
            .collect();
        assert_eq!(
            actions,
            vec!["open_config_file", "open_config_dir", "reset_all_config"]
        );
    }
}
