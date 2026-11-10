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
                Field::group("启动"),
                Field::switch("autostart", "开机自动启动"),
                Field::note(
                    "登录 Windows 后在托盘静默运行（不弹窗口）。\n\
                     只写当前用户的启动项（HKCU），不需要管理员权限；\n\
                     换了安装目录的话下次启动会自动把路径改正。",
                ),
                Field::group("性能"),
                Field::switch("lite_mode", "极轻模式"),
                Field::note(
                    "开启时关闭窗口立即释放 WebView 内存；关闭后窗口只是隐藏、再次打开更快，但会一直占内存。",
                ),
                Field::group("翻译与弹窗"),
                Field::switch("cache_enabled", "翻译缓存"),
                Field::note("开着缓存时，同一段文字再次翻译直接用上次结果，不消耗额度。"),
                Field::number_unit("popup_timeout_ms", "弹窗自动关闭", 3000, 120000, "毫秒"),
                Field::group("配置与日志"),
                Field::note("日志默认关闭，避免常驻进程无谓写盘。"),
                Field::button("open_config_file", "打开配置文件"),
                Field::note("用记事本之类的程序打开 config.json。改完保存后部分设置要重启才生效。"),
                Field::button("open_config_dir", "打开配置目录"),
            ],
        )]
    }
}
