//! 通用：影响两个以上工具的东西才放这里（S2 任务书 §2.6.1 的判定标准）。
//!
//! 目前只有：翻译缓存、弹窗时长、日志、配置目录。热键刻意**不**放这里——划词热键
//! 属翻译、截图热键属截图、下载热键属下载，各归各的分区，这样用户改划词热键时不会
//! 担心动到别的东西。

use super::{Field, Section, Tool};
use crate::config;

pub struct General;

impl Tool for General {
    fn id(&self) -> &'static str {
        "general"
    }

    fn name(&self) -> &'static str {
        "通用"
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
        // 通用项不允许整体停用：它只是配置容器，停用了没有意义
        Err("「通用」是配置分组，不能停用".to_string())
    }

    /// 设置页不显示「启用」开关：这里没有可停用的东西，
    /// 给一个永远切不动的开关只会让用户以为程序坏了。
    fn switchable(&self) -> bool {
        false
    }

    fn status_line(&self) -> String {
        let cfg = config::load();
        let cache = if cfg.cache_enabled {
            "缓存开启"
        } else {
            "缓存关闭"
        };
        let log = if crate::dlog::is_enabled() {
            "日志开启"
        } else {
            "日志关闭（LIGHTBOX_DEBUG=1 可开）"
        };
        format!("{cache} · {log}")
    }

    fn sections(&self) -> Vec<Section> {
        vec![Section::new(
            "general",
            "通用",
            vec![
                Field::group("外观"),
                Field::select(
                    "appearance_theme",
                    "主题",
                    &[
                        ("dark", "暗色"),
                        ("light", "明亮"),
                        ("system", "跟随系统"),
                    ],
                )
                .help("「跟随系统」跟着 Windows 的应用模式走（设置 → 个性化 → 颜色）。"),
                Field::color("accent_color", "主色调", ACCENT_PRESETS)
                    .help("按钮、光晕、LOGO 渐变都会跟着变。点已选中的色块恢复默认；「自定义」用系统取色器随便挑。"),
                Field::group("启动"),
                Field::switch("autostart", "开机自动启动"),
                Field::group("性能"),
                Field::switch("lite_mode", "极轻模式")
                    .help("开启时关闭窗口立即释放 WebView 内存；关闭后窗口只是隐藏，再次打开更快，但会一直占内存。"),
                Field::group("翻译与弹窗"),
                Field::switch("cache_enabled", "翻译缓存"),
                Field::number_unit("popup_timeout_ms", "弹窗自动关闭", 3000, 120000, "毫秒"),
                Field::group("配置与日志"),
                Field::button("open_config_file", "打开配置文件"),
                Field::button("open_config_dir", "打开配置目录"),
            ],
        )]
    }
}

/// 主色调预设色板（外观功能会话，用户已确认六色）。
/// 渐变端 / 淡底 / 描边全部由前端 oklch 从这里给的主色现算。
pub const ACCENT_PRESETS: &[(&str, &str)] = &[
    ("#7c5cff", "高级紫"),
    ("#3b82f6", "湛蓝"),
    ("#14b8a6", "青碧"),
    ("#22c55e", "翠绿"),
    ("#f59e0b", "琥珀"),
    ("#ec4899", "玫红"),
];
