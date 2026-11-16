//! 截图工具（S7）。字段全部由 `Tool::sections()` 描述，设置页 HTML/JS 零改动
//! （ADR-015）。未启用时热键不注册、不建浮层、不起线程（ADR-040）。

use super::{Field, Section, Tool};
use crate::config;

pub struct Screenshot;

/// 命名规则里的 `$...$` 可填的格式。点一下只替换占位符内容，
/// 用户输入的前缀 / 后缀（如 `Vexto_`、`.png`）保持不变。
pub const NAME_PRESETS: &[(&str, &str)] = &[
    ("日期时间", "yyyyMMdd_HHmmss"),
    ("UUID", "uuid"),
    ("Unix 时间戳(秒)", "unix"),
];

impl Tool for Screenshot {
    fn id(&self) -> &'static str {
        "screenshot"
    }

    fn name(&self) -> &'static str {
        "截图"
    }

    fn desc(&self) -> &'static str {
        "区域 / 全屏 / 活动窗口截图，滚动长图与钉屏"
    }

    fn icon(&self) -> &'static str {
        "camera"
    }

    fn enabled(&self) -> bool {
        config::load().enable_screenshot
    }

    fn set_enabled(&self, on: bool) -> Result<(), String> {
        let mut cfg = config::load();
        cfg.enable_screenshot = on;
        config::save(&cfg)
    }

    fn status_line(&self) -> String {
        let cfg = config::load();
        if !cfg.enable_screenshot {
            return "未启用".to_string();
        }
        format!("已启用 · 区域截图 {}", cfg.hotkey_screenshot)
    }

    fn sections(&self) -> Vec<Section> {
        vec![
            Section::new(
                "general",
                "截图",
                vec![
                    Field::switch("enable_screenshot", "启用截图"),
                    Field::dir("screenshot_dir", "保存目录").wide(),
                    Field::template(
                        "screenshot_name_rule",
                        "截图命名规则",
                        "Vexto_$yyyyMMdd_HHmmss$.png",
                        NAME_PRESETS,
                        "preview_screenshot_name",
                    )
                    .wide(),
                    Field::number_unit("screenshot_border_width", "选框边框宽度", 3, 5, "px"),
                    Field::number_unit("screenshot_magnifier_zoom", "放大镜倍数", 2, 10, "×"),
                    Field::number_unit("screenshot_feedback_seconds", "缩略图显示时长", 3, 5, "秒"),
                    Field::number_unit("screenshot_feedback_fade_ms", "缩略图淡出时长", 200, 1500, "ms"),
                    Field::switch("screenshot_include_cursor", "截图包含鼠标指针"),
                    Field::select(
                        "screenshot_after_action",
                        "截图后",
                        &[
                            ("both", "复制图片并保存"),
                            ("clipboard", "只复制图片"),
                            ("path", "保存并复制路径"),
                            ("save", "只保存"),
                        ],
                    ),
                    Field::select(
                        "ocr_engine",
                        "识别引擎",
                        &[
                            ("paddle", "PaddleOCR"),
                            ("windows", "Windows 自带"),
                        ],
                    ),
                    Field::group("钉屏"),
                    Field::switch("pinscreen_single_window", "单窗口多图"),
                ],
            ),
            Section::new(
                "hotkeys",
                "快捷键配置",
                vec![
                    Field::hotkey("hotkey_screenshot", "区域截图"),
                    Field::hotkey("hotkey_fullscreen", "全屏截图"),
                ],
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::FieldKind;

    /// 每个要存值的 key 都必须真的存在于 `Config`，否则 `apply_values` 会
    /// 静默忽略它——用户以为保存了，重启后又变回去。
    #[test]
    fn every_value_key_exists_in_config() {
        let cfg = serde_json::to_value(crate::config::Config::default()).unwrap();
        let obj = cfg.as_object().unwrap();
        for s in Screenshot.sections() {
            for f in &s.fields {
                if matches!(
                    f.kind,
                    FieldKind::Note { .. }
                        | FieldKind::Button
                        | FieldKind::ButtonRow { .. }
                        | FieldKind::Group { .. }
                ) {
                    continue;
                }
                assert!(
                    obj.contains_key(f.key),
                    "截图工具字段 `{}` 在 Config 里不存在",
                    f.key
                );
            }
        }
    }

    /// 默认关闭（ADR-040）。
    #[test]
    fn disabled_by_default() {
        let c = crate::config::Config::default();
        assert!(!c.enable_screenshot);
        assert_eq!(c.hotkey_screenshot, "Ctrl+Alt+A");
    }

    /// 截图页与快捷键页分开；截图页不再画任何说明文本，
    /// 滚动 / 全屏 / 活动窗口的旧配置项全部撤掉（去开发化，ADR-054）。
    #[test]
    fn sections_are_general_and_hotkeys_without_help() {
        let s = Screenshot.sections();
        assert_eq!(s.len(), 2);
        assert_eq!((s[0].id, s[0].name), ("general", "截图"));
        assert_eq!((s[1].id, s[1].name), ("hotkeys", "快捷键配置"));
        for f in &s[0].fields {
            assert!(f.help.is_none(), "截图页不再显示说明文本：{}", f.key);
        }
        let keys: Vec<&str> = s
            .iter()
            .flat_map(|sec| sec.fields.iter().map(|f| f.key))
            .collect();
        for gone in ["scroll_mode", "scroll_action", "shot_full", "shot_window"] {
            assert!(!keys.contains(&gone), "旧配置项 {gone} 应删除");
        }
        let hot = &s[1].fields;
        assert!(hot.iter().any(|f| f.key == "hotkey_screenshot"));
        assert!(hot.iter().any(|f| f.key == "hotkey_fullscreen"));
    }
}
