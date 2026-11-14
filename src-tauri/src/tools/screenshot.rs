//! 截图工具（S7）。字段全部由 `Tool::sections()` 描述，设置页 HTML/JS 零改动
//! （ADR-015）。未启用时热键不注册、不建浮层、不起线程（ADR-040）。

use super::{Field, Section, Tool};
use crate::config;

pub struct Screenshot;

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
        vec![Section::new(
            "general",
            "截图",
            vec![
                Field::group("截图"),
                Field::switch("enable_screenshot", "启用截图")
                    .help("按热键框选屏幕区域，松手后按下面的「截图后」动作交付；Esc 取消。"),
                Field::hotkey("hotkey_screenshot", "区域截图热键"),
                Field::dir("screenshot_dir", "保存目录")
                    .help("留空 = 系统「图片」文件夹下的 Vexto 子目录。"),
                Field::number_unit("screenshot_border_width", "选框边框宽度", 3, 5, "px")
                    .help("框选时选框边框的粗细，3–5 px。"),
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
                Field::group("全屏 / 活动窗口"),
                Field::button("shot_full", "全屏截图")
                    .help("截整个虚拟屏幕；多显示器时是一整张。"),
                Field::button("shot_window", "活动窗口截图")
                    .help("截当前前台窗口。从设置页点按钮时会截到设置页本身，建议用托盘菜单。"),
                Field::group("滚动截图"),
                Field::select(
                    "scroll_mode",
                    "默认模式",
                    &[("auto", "全自动（推荐）"), ("manual", "半自动：自己滚，点停止")],
                )
                .help("自动模式默认发滚动消息，窗口没反应时自动回退滚轮；仍无效会明确报错。"),
                Field::select(
                    "scroll_action",
                    "滚动方式",
                    &[
                        ("message", "滚动消息（默认）"),
                        ("wheel", "鼠标滚轮"),
                        ("key", "方向键"),
                        ("pagedown", "PageDown"),
                        ("manual", "手动滚动"),
                    ],
                ),
                Field::group("钉屏"),
                Field::switch("pinscreen_single_window", "单窗口多图")
                    .help("关闭后每张图一个窗口，可以摆到屏幕不同位置，但内存占用更高。"),
            ],
        )]
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
                    FieldKind::Note { .. } | FieldKind::Button | FieldKind::Group { .. }
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
}
