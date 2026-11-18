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
        "互传"
    }

    fn desc(&self) -> &'static str {
        "同一局域网内跨设备传文件"
    }

    fn icon(&self) -> &'static str {
        "transfer"
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
                let preferred = transfer::TransferConfig::from_app(&config::load()).port;
                let switched = if port != preferred {
                    format!("（{preferred} 不可用，已自动切换）")
                } else {
                    String::new()
                };
                format!(
                    "已启用 · 监听 {port}{switched} · 发现 {} 台设备",
                    transfer::backend().peers().len()
                )
            }
            // 只说人话不带错误码；完整细节在日志里（会话 52 用户要求）
            transfer::server::ServiceState::Failed(_) => {
                "启动失败，无法启动服务（端口可能被占用）".to_string()
            }
            transfer::server::ServiceState::Stopped => "已启用 · 服务未运行".to_string(),
        }
    }

    /// 服务没起来必须红；「已启用但没运行」黄（会话 52）。
    fn status_kind(&self) -> &'static str {
        if !config::load().enable_transfer {
            return "ok";
        }
        match transfer::server::state() {
            transfer::server::ServiceState::Failed(_) => "error",
            transfer::server::ServiceState::Stopped => "warn",
            transfer::server::ServiceState::Listening { .. } => "ok",
        }
    }

    fn sections(&self) -> Vec<Section> {
        vec![
            // ── 通用 ──────────────────────────────────────────────────
            Section::new(
                "general",
                "通用",
                vec![
                    Field::group("接收"),
                    Field::switch("transfer_auto_accept", "自动保存"),
                    Field::switch("transfer_auto_complete", "自动完成"),
                    Field::switch("require_pin", "需要一个 PIN"),
                    Field::text_hint("pin", "PIN（4–6 位数字）", "4–6 位数字")
                        .show_if("require_pin", "true"),
                    Field::dir("receive_dir", "保存目录").wide(),
                    Field::group("设备"),
                    Field::text_hint("device_alias", "别名", "不填用计算机名"),
                    Field::select(
                        "transfer_device_type",
                        "设备类型",
                        &[
                            ("mobile", "手机"),
                            ("desktop", "桌面"),
                            ("web", "网页"),
                            ("headless", "无头"),
                            ("server", "服务器"),
                        ],
                    ),
                    Field::text_hint("transfer_device_model", "设备型号", "Windows"),
                    Field::number_unit("transfer_peer_timeout_secs", "设备保留时长", 5, 3600, "秒"),
                ],
            ),
            // ── 高级 ──────────────────────────────────────────────────
            Section::new(
                "advanced",
                "高级",
                vec![
                    Field::group("网络"),
                    Field::number_unit("transfer_port", "端口", 1024, 65535, ""),
                    Field::switch("transfer_use_https", "加密（HTTPS）"),
                    Field::text_hint("transfer_multicast", "多播地址", "224.0.0.167")
                        .help("组播地址要和对方一致；默认 224.0.0.167。"),
                    Field::text_hint(
                        "transfer_interface_whitelist",
                        "网络接口白名单",
                        "留空表示所有网卡",
                    )
                    .help("逗号分割的 IP 前缀")
                    .wide(),
                    Field::buttons(&[
                        ("transfer_restart", "重启传输服务"),
                        ("transfer_open_dir", "打开接收目录"),
                    ]),
                ],
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::FieldKind;

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
        for k in [
            "device_alias",
            "receive_dir",
            "require_pin",
            "pin",
            "transfer_auto_accept",
            "transfer_auto_complete",
            "transfer_port",
            "transfer_use_https",
            "transfer_multicast",
            "transfer_device_type",
            "transfer_device_model",
            "transfer_peer_timeout_secs",
            "transfer_interface_whitelist",
        ] {
            assert!(keys.contains(&k), "transfer schema 缺少 key：{k}");
        }
    }

    /// 两个页签的顺序与 id 是用户定的信息架构（通用 / 高级）。
    #[test]
    fn has_two_tabs_in_order() {
        let ids: Vec<&str> = Transfer.sections().iter().map(|s| s.id).collect();
        assert_eq!(ids, vec!["general", "advanced"]);
    }

    /// 两组字段都按类别分段：通用是「接收 / 设备」，高级是「网络」。
    #[test]
    fn every_tab_has_groups() {
        let s = Transfer.sections();
        let count = |i: usize| {
            s[i]
                .fields
                .iter()
                .filter(|f| matches!(f.kind, FieldKind::Group { .. }))
                .count()
        };
        assert_eq!(count(0), 2);
        assert_eq!(count(1), 1);
    }

    /// 设备身份三项（类型 / 型号 / 保留时长）归入「设备」分组。
    #[test]
    fn device_fields_are_in_general_device_group() {
        let s = Transfer.sections();
        let keys: Vec<&str> = s[0].fields.iter().map(|f| f.key).collect();
        for k in [
            "transfer_device_type",
            "transfer_device_model",
            "transfer_peer_timeout_secs",
        ] {
            assert!(keys.contains(&k), "设备分组缺 {k}");
        }
        let general: Vec<&str> = s[1].fields.iter().map(|f| f.key).collect();
        assert!(!general.contains(&"transfer_device_type"));
    }
}
