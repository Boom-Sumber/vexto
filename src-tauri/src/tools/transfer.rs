//! 局域网互传（LocalSend 协议）的设置分区。
//!
//! **本文件只管设置 schema（ADR-015 数据驱动）**，传输实现在
//! `src/transfer/`（会话 39 起）。字段 key 与 `config::Config` 一一对应，
//! 前端零改动（`tools/mod.rs` 的 Field/Section 机制）。

use super::{Field, Section, Tool};
use crate::config;
use crate::transfer;

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
        let cfg = config::load();
        if on {
            // 先起服务再落配置：起不来（端口被占）就别把 enable_transfer
            // 写成 true，否则下次启动还会撞同一个错。
            let tc = transfer::TransferConfig::from_app(&cfg);
            transfer::backend().start(&tc)?;
            let mut cfg = cfg;
            cfg.enable_transfer = true;
            config::save(&cfg)
        } else {
            // 先停服务再落配置：立即生效优先；保存失败下次启动也干净。
            transfer::backend().stop()?;
            let mut cfg = cfg;
            cfg.enable_transfer = false;
            config::save(&cfg)
        }
    }

    fn status_line(&self) -> String {
        if !config::load().enable_transfer {
            return "已停用".to_string();
        }
        match transfer::server::state() {
            transfer::server::ServiceState::Listening { port } => {
                format!(
                    "已启用 · 监听 {port} · 发现 {} 台设备",
                    transfer::backend().peers().len()
                )
            }
            transfer::server::ServiceState::Failed(e) => format!("启动失败：{e}"),
            transfer::server::ServiceState::Stopped => "已启用 · 服务未运行".to_string(),
        }
    }

    fn sections(&self) -> Vec<Section> {
        vec![Section::new(
            "transfer",
            "局域网互传",
            vec![
                Field::text_hint(
                    "device_alias",
                    "本机对其他设备显示的名字",
                    "不填用计算机名",
                ),
                Field::dir("receive_dir", "接收目录"),
                Field::note("留空则用系统「下载」文件夹下的 LightBox 子目录。"),
                Field::switch("require_pin", "接收需要 PIN"),
                Field::text_hint("pin", "PIN（4–6 位数字）", "4–6 位数字")
                    .show_if("require_pin", "true"),
                Field::note(
                    "端口 53317（UDP 组播 + TCP）。首次启用时 Windows 可能弹防火墙提示，需要允许专用网络访问。",
                ),
                Field::note(
                    "改设备名 / 接收目录 / PIN 后立即生效（会重启传输服务，进行中的传输会中断）。",
                ),
            ],
        )]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// schema 锁定：设置页字段 key 必须与 Config 字段一致，
    /// 加字段时两处同改（apply_values 按 key 落盘）。
    #[test]
    fn schema_carries_transfer_config_keys() {
        let t = Transfer;
        let keys: Vec<&str> = t
            .sections()
            .iter()
            .flat_map(|s| s.fields.iter().map(|f| f.key))
            .collect();
        for k in ["device_alias", "receive_dir", "require_pin", "pin"] {
            assert!(keys.contains(&k), "transfer schema 缺少 key：{k}");
        }
    }
}
