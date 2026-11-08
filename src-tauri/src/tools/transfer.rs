//! 局域网传文件（LocalSend 协议）。
//!
//! **S2 只做设置分区**，真正的传输在 S4。字段提前定好。

use super::{Field, Section, Tool};
use crate::config;

pub struct Transfer;

impl Tool for Transfer {
    fn id(&self) -> &'static str {
        "transfer"
    }

    fn name(&self) -> &'static str {
        "局域网互传"
    }

    fn desc(&self) -> &'static str {
        "同一局域网内跨设备传文件"
    }

    fn enabled(&self) -> bool {
        config::load().enable_transfer
    }

    fn set_enabled(&self, on: bool) -> Result<(), String> {
        let mut cfg = config::load();
        cfg.enable_transfer = on;
        config::save(&cfg)
    }

    fn status_line(&self) -> String {
        let cfg = config::load();
        if !cfg.enable_transfer {
            return "已停用".to_string();
        }
        if cfg.receive_dir.trim().is_empty() {
            "未设置接收目录".to_string()
        } else {
            format!("接收目录 {}", cfg.receive_dir)
        }
    }

    fn sections(&self) -> Vec<Section> {
        vec![Section::new(
            "transfer",
            "局域网互传",
            vec![
                Field::dir("receive_dir", "接收目录"),
                Field::note("留空则用系统「下载」文件夹下的 LightBox 子目录。"),
                Field::note("传输功能在 S4 实现，这里先把配置存好。"),
            ],
        )]
    }
}
