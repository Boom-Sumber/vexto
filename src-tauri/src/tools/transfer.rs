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
        vec![
            // ── 通用 ──────────────────────────────────────────────────
            Section::new(
                "general",
                "通用",
                vec![
                    Field::group("接收"),
                    Field::switch("transfer_auto_accept", "自动保存"),
                    Field::note("打开后收到的文件不再弹确认框，直接写入保存目录。"),
                    Field::switch("require_pin", "需要一个 PIN"),
                    Field::text_hint("pin", "PIN（4–6 位数字）", "4–6 位数字")
                        .show_if("require_pin", "true"),
                    Field::dir("receive_dir", "保存目录"),
                    Field::note("留空则用系统「下载」文件夹下的 LightBox 子目录。"),
                    Field::group("设备"),
                    Field::text_hint("device_alias", "别名", "不填用计算机名"),
                    Field::note("对方在设备列表 / 确认框里看到的就是这个名字。"),
                ],
            ),
            // ── 高级 ──────────────────────────────────────────────────
            Section::new(
                "advanced",
                "高级",
                vec![
                    Field::group("网络"),
                    Field::note("端口 53317（UDP 组播 + TCP，LocalSend 协议固定值）。"),
                    Field::note("加密：始终使用 HTTPS（自签证书，ADR-035），不提供关闭选项。"),
                    Field::note("多播地址 224.0.0.167；设备类型 / 型号固定为 desktop / Windows。"),
                    Field::note("搜索设备超时由 lanflare 内部处理，当前不开放调整。"),
                    Field::note("首次启用时 Windows 可能弹防火墙提示，需要允许专用网络访问。"),
                    Field::group("维护"),
                    Field::button("transfer_restart", "重启传输服务"),
                    Field::button("transfer_open_dir", "打开接收目录"),
                    Field::note(
                        "修改别名 / 保存目录 / PIN / 自动保存后保存即生效（会重启传输服务，\n\
                         进行中的传输会中断）。「重启传输服务」用于端口被占或服务异常后手动恢复。",
                    ),
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

    /// 每个页签都必须有分组标题，否则又退回「一长条平铺」，白拆两个页签。
    #[test]
    fn every_tab_has_groups() {
        for s in Transfer.sections() {
            let n = s
                .fields
                .iter()
                .filter(|f| matches!(f.kind, FieldKind::Group { .. }))
                .count();
            assert!(n >= 2, "页签 {} 只有 {n} 个分组标题", s.id);
        }
    }
}
