//! 下载工具（HTTP / BT / magnet，aria2c sidecar）。
//!
//! 字段全部由 `Tool::sections()` 描述，设置页 HTML/JS 不写任何具体字段分支
//! （ADR-015）。会话 25 起分三个页签（通用 / 网络 / 高级），页签内部再用
//! `Field::group()` 分组——分组标题是第 10 种字段类型，只画标题和一条分隔线。

use super::{Field, Section, Tool};
use crate::config;

/// 默认 User-Agent 的四条预设。
///
/// **是固定快照，不是动态版本号**：站点要求更新的版本时，用户直接在文本框里改。
/// 放在 Rust 侧而不是前端，是为了让「字段长什么样」仍然全部由 schema 描述。
const UA_PRESETS: &[(&str, &str)] = &[
    (
        "Chrome",
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
    ),
    (
        "Edge",
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 Edg/131.0.0.0",
    ),
    (
        "Safari",
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Safari/605.1.15",
    ),
    (
        "Firefox",
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:133.0) Gecko/20100101 Firefox/133.0",
    ),
];

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

    /// 没有主窗口时，设置页这一行就是用户能看到运行状态的唯一地方。
    fn status_line(&self) -> String {
        let cfg = config::load();
        if !cfg.enable_download {
            return "未启用".to_string();
        }
        format!(
            "{}；{}；代理：{}",
            crate::download::engine::status_line(),
            crate::extension_bridge::status_line(),
            crate::download::proxy::describe(&cfg)
        )
    }

    fn sections(&self) -> Vec<Section> {
        vec![
            // ── 通用 ──────────────────────────────────────────────────
            Section::new(
                "general",
                "通用",
                vec![
                    Field::group("下载路径"),
                    Field::dir("download_dir", "下载目录"),
                    Field::group("速度与并发"),
                    Field::number_unit("max_concurrent", "同时下载数", 1, 16, "个"),
                    Field::number_unit("max_connection_per_server", "单文件连接数", 1, 16, "个"),
                    Field::number_unit("max_speed_kb", "下载限速", 0, 102_400, "KB/s"),
                    Field::note(
                        "限速填 0 表示不限；单文件连接数决定一个大文件被切成几条连接拉。",
                    ),
                    Field::group("通知"),
                    Field::switch("notify_download", "启用系统通知"),
                    Field::switch("notify_on_start", "下载开始时通知"),
                    Field::switch("notify_on_done", "下载完成时通知"),
                    Field::button("notify_test", "发送测试通知"),
                    Field::note(
                        "「启用系统通知」是总开关，下面两项只在它打开时生效。\n\
                         开始通知默认关——一次贴十条链接会连弹十条。",
                    ),
                    Field::group("快捷键"),
                    Field::hotkey("hotkey_download", "打开新建下载任务"),
                ],
            ),
            // ── 网络 ──────────────────────────────────────────────────
            Section::new(
                "network",
                "网络",
                vec![
                    Field::group("代理"),
                    Field::select(
                        "proxy_mode",
                        "代理模式",
                        &[
                            ("direct", "直连（不使用代理）"),
                            ("system", "跟随系统代理"),
                            ("manual", "手动指定"),
                        ],
                    ),
                    Field::text_hint("proxy_url", "代理地址", "http://127.0.0.1:7890")
                        .show_if("proxy_mode", "manual"),
                    Field::note(
                        "aria2 自己不读 Windows 的系统代理，所以「跟随系统」由本程序读注册表再翻译。\n\
                         改完立即切换，不用重启下载引擎；回环地址与局域网始终直连。",
                    ),
                    Field::group("请求"),
                    Field::text_presets(
                        "default_user_agent",
                        "默认 User-Agent",
                        "留空 = 用 aria2 自带 UA",
                        UA_PRESETS,
                    ),
                    Field::note(
                        "新建任务页的 UA 框默认填这个值；浏览器接管时会被网页真实的 UA 覆盖。\n\
                         预设是固定快照，站点要求更新的版本时直接改文本框。",
                    ),
                    Field::group("DNS"),
                    Field::text_hint(
                        "dns_servers",
                        "DNS 服务器",
                        "223.5.5.5,119.29.29.29（留空 = 用系统）",
                    ),
                    Field::note(
                        "只支持普通 DNS（明文），最多两个地址。\n\
                         这不是 DNS over HTTPS——aria2 本身不提供 DoH，要 DoH 得另起一个 DNS 代理。\n\
                         改完等下一次启动下载引擎才生效。",
                    ),
                    Field::group("TLS"),
                    Field::switch("check_certificate", "校验证书"),
                    Field::note("关掉可以下载自签名证书的站点，但会失去对中间人攻击的防护。"),
                ],
            ),
            // ── 高级 ──────────────────────────────────────────────────
            Section::new(
                "advanced",
                "高级",
                vec![
                    Field::group("浏览器插件接管"),
                    Field::switch("extension_bridge_enabled", "启用接管"),
                    Field::number_unit("extension_bridge_port", "插件端口", 1024, 65535, ""),
                    Field::password("extension_bridge_secret", "插件密钥"),
                    Field::note(
                        "会启动一个只监听 127.0.0.1 的接口，供 Rayburst-Connect 扩展把网页下载交过来。\n\
                         端口与密钥要和扩展 Options 页里填的完全一致；密钥留空 = 不校验（与 Rayburst 相同）。\n\
                         改端口或密钥后需要重新保存才会重启监听；LightBox 没运行时扩展无法唤醒它。",
                    ),
                    Field::group("下载引擎"),
                    Field::number_unit("rpc_port", "RPC 端口", 1024, 65535, ""),
                    Field::note(
                        "aria2 的控制接口，只绑 127.0.0.1。被别的程序占用时会自动换一个可用端口，\n\
                         状态行显示的是实际端口。改完等下一次启动下载引擎才生效。",
                    ),
                    Field::group("维护"),
                    Field::button("open_download_dir", "打开下载目录"),
                ],
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::FieldKind;

    /// 三个页签的顺序与 id 是用户定的信息架构（通用 / 网络 / 高级）。
    #[test]
    fn has_three_tabs_in_order() {
        let ids: Vec<&str> = Download.sections().iter().map(|s| s.id).collect();
        assert_eq!(ids, vec!["general", "network", "advanced"]);
    }

    /// 每个页签都必须有分组标题，否则又退回「一长条平铺」，白拆三个页签。
    #[test]
    fn every_tab_has_groups() {
        for s in Download.sections() {
            let n = s
                .fields
                .iter()
                .filter(|f| matches!(f.kind, FieldKind::Group { .. }))
                .count();
            assert!(n >= 2, "页签 {} 只有 {n} 个分组标题", s.id);
        }
    }

    /// UA 预设四个、顺序固定，且值必须是合法 UA（以 `Mozilla/5.0` 开头）。
    #[test]
    fn ua_presets_are_four_browsers() {
        let names: Vec<&str> = UA_PRESETS.iter().map(|(n, _)| *n).collect();
        assert_eq!(names, vec!["Chrome", "Edge", "Safari", "Firefox"]);
        for (name, ua) in UA_PRESETS {
            assert!(ua.starts_with("Mozilla/5.0"), "{name} 的 UA 不合法：{ua}");
        }
    }
}
