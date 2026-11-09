//! 下载工具（HTTP / BT / magnet，aria2c sidecar）。
//!
//! S2 只放了设置分区骨架，S3a 接上了真正的引擎。字段仍然全部由 `Tool::sections()`
//! 描述，设置页 HTML/JS 一行没改（ADR-015 / 任务书 §S3.8b 验收 9.3.1）。

use super::{Field, Section, Tool};
use crate::config;

pub struct Download;

impl Tool for Download {
    fn id(&self) -> &'static str {
        "download"
    }

    fn name(&self) -> &'static str {
        "下载"
    }

    fn desc(&self) -> &'static str {
        "HTTP / BT / magnet 下载"
    }

    fn enabled(&self) -> bool {
        config::load().enable_download
    }

    /// 关掉工具必须真的把 sidecar 停掉：不然「已停用」还挂着 5 MB 常驻进程。
    /// 反注册热键由 `main.rs::set_tool_enabled` 统一重注册（台账坑 ⑪）。
    fn set_enabled(&self, on: bool) -> Result<(), String> {
        let mut cfg = config::load();
        cfg.enable_download = on;
        config::save(&cfg)?;
        if on {
            crate::download::engine::set_message("已启用 · 空闲（aria2c 未启动）");
        } else {
            crate::download::engine::shutdown_all()?;
            crate::download::engine::set_message("已停用（aria2c 已退出）");
        }
        Ok(())
    }

    /// S3a 没有主窗口，设置页这一行就是用户能看到运行状态的唯一地方。
    fn status_line(&self) -> String {
        let cfg = config::load();
        if !cfg.enable_download {
            return "未启用".to_string();
        }
        format!(
            "{}；{}",
            crate::download::engine::status_line(),
            crate::extension_bridge::status_line()
        )
    }

    fn sections(&self) -> Vec<Section> {
        vec![Section::new(
            "download",
            "下载",
            vec![
                // 输入框直接显示默认路径（后端在留空时回填），不再用占位符提示。
                Field::dir("download_dir", "下载目录"),
                Field::number_unit("max_speed_kb", "全局限速", 0, 102_400, "KB/s"),
                Field::number_unit("max_concurrent", "同时下载数", 1, 16, "个"),
                Field::hotkey("hotkey_download", "打开新建下载任务"),
                // 浏览器插件接管：端口 / 密钥必须与扩展 Options 页一致。
                Field::switch("extension_bridge_enabled", "浏览器插件接管"),
                Field::number_unit("extension_bridge_port", "插件端口", 1024, 65535, ""),
                Field::password("extension_bridge_secret", "插件密钥"),
                Field::note(
                    "「浏览器插件接管」会启动一个只监听 127.0.0.1 的接口，供 Rayburst-Connect 扩展把网页下载交过来。\n\
                     端口与密钥要和扩展 Options 页里填的完全一致；密钥留空 = 不校验（与 Rayburst 相同）。\n\
                     改端口或密钥后需要重新保存才会重启监听；LightBox 没运行时扩展无法唤醒它，请保持托盘常驻。",
                ),
            ],
        )]
    }
}
