//! 下载工具（HTTP / BT / magnet，aria2c sidecar）。
//!
//! **S2 只做设置分区，不做下载功能**——真正的下载引擎在 S3。这里定义的字段
//! 是 S3 要读的配置项，提前定好避免 S3 再改一次 `Config`。

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

    fn set_enabled(&self, on: bool) -> Result<(), String> {
        let mut cfg = config::load();
        cfg.enable_download = on;
        config::save(&cfg)
    }

    fn status_line(&self) -> String {
        let cfg = config::load();
        if !cfg.enable_download {
            return "已停用".to_string();
        }
        if cfg.download_dir.trim().is_empty() {
            "未设置下载目录".to_string()
        } else {
            format!("保存到 {}", cfg.download_dir)
        }
    }

    fn sections(&self) -> Vec<Section> {
        vec![Section::new(
            "download",
            "下载",
            vec![
                Field::dir("download_dir", "下载目录"),
                Field::note("留空则用系统「下载」文件夹。"),
                Field::hotkey("hotkey_download", "唤起下载热键"),
                Field::note("下载引擎在 S3 实现，这里先把配置存好。"),
            ],
        )]
    }
}
