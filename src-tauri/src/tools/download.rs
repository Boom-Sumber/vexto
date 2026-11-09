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
        crate::download::engine::status_line()
    }

    fn sections(&self) -> Vec<Section> {
        vec![Section::new(
            "download",
            "下载",
            vec![
                Field::dir("download_dir", "下载目录"),
                Field::number_unit("max_speed_kb", "全局限速", 0, 102_400, "KB/s"),
                Field::number_unit("max_concurrent", "同时下载数", 1, 16, "个"),
                Field::hotkey("hotkey_download", "把剪贴板链接加入下载"),
                // 启用开关由设置页顶部统一的「启用此工具」渲染（renderEnable），
                // 这里再放一个 switch 会变成两个控件写同一个字段，故只写说明。
                Field::note("留空则用系统「下载」文件夹；工具的启用开关在本页顶部。"),
                Field::note("限速与并发数是 aria2 的启动参数，改完要等下一次启动才生效。"),
                Field::note("复制 http / https / ftp / magnet 链接或 .torrent 路径后按热键入队。"),
                Field::note("BT / magnet 任务会开启对外监听端口，首次运行 BT 时 Windows 可能询问网络权限；RPC 只监听 127.0.0.1。本机实测结论见 licenses/aria2-VERSION.txt。"),
            ],
        )]
    }
}
