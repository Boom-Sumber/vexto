//! 下载工具（HTTP / BT / magnet，aria2c sidecar）。
//!
//! 字段全部由 `Tool::sections()` 描述，设置页 HTML/JS 不写任何具体字段分支
//! （ADR-015）。会话 25 起分三个页签（通用 / 网络 / 高级），页签内部再用
//! `Field::group()` 分组——分组标题是第 10 种字段类型，只画标题和一条分隔线。

use super::{Field, Section, Tool};
use crate::config;
use rust_i18n::t;

/// GitHub 下载 / clone 线路。`auto` 先走 gh-proxy，最后才直连。
///
/// 与关于页原来的「更新下载线路」共用 `update_download_source` /
/// `update_custom_proxy` 两个配置键：应用自身更新仍读同一份值，不迁移配置。
const GITHUB_SOURCES: &[(&str, &str)] = &[
    ("auto", "自动（镜像优先）"),
    ("direct", "直连 GitHub"),
    ("custom", "自定义代理"),
];

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

    fn icon(&self) -> &'static str {
        "download"
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
            crate::download::engine::set_status(
                crate::download::engine::EngineStatus::Idle,
            );
        } else {
            crate::download::engine::shutdown_all()?;
            crate::download::engine::set_status(
                crate::download::engine::EngineStatus::Stopped,
            );
        }
        Ok(())
    }

    /// 没有主窗口时，设置页这一行就是用户能看到运行状态的唯一地方。
    /// 只保留「引擎状态 · 插件接管」两段，代理等细节不进状态栏。
    fn status_line(&self) -> String {
        let cfg = config::load();
        if !cfg.enable_download {
            return t!("status.line.disabled").into_owned();
        }
        let bridge = if cfg.extension_bridge_enabled {
            t!("status.line.bridgeOn")
        } else {
            t!("status.line.bridgeOff")
        };
        t!(
            "status.line.download",
            engine = crate::download::engine::status_line(),
            bridge = bridge
        )
        .into_owned()
    }

    /// 引擎/插件接管有失败信息时红，aria2c 还没起来时黄（会话 52）。
    fn status_kind(&self) -> &'static str {
        let cfg = config::load();
        if !cfg.enable_download {
            return "ok";
        }
        if crate::download::engine::status_is_error() || crate::extension_bridge::has_error() {
            return "error";
        }
        if crate::download::engine::status_is_idle() {
            return "warn";
        }
        "ok"
    }

    fn sections(&self) -> Vec<Section> {
        vec![
            // ── 通用 ──────────────────────────────────────────────────
            Section::new(
                "general",
                "通用",
                vec![
                    Field::group("paths", "下载路径"),
                    Field::dir("download_dir", "下载目录").wide(),
                    Field::switch("remember_last_dir", "记住上次保存位置"),
                    Field::group("speed", "速度与并发"),
                    Field::number_unit("max_concurrent", "同时下载数", 1, 16, "个"),
                    Field::number_unit("max_connection_per_server", "单文件连接数", 1, 16, "个"),
                    Field::number_unit("max_speed_kb", "下载限速", 0, 102_400, "KB/s")
                        .help("填 0 表示不限；单位 KB/s。"),
                    Field::switch("auto_limit", "自动限速")
                        .help("按实测速度把上限收到「峰值 × 1.3」，只降不升，避免上限虚高时 aria2 开满连接空转。"),
                    Field::group("notifications", "通知"),
                    Field::switch("notify_download", "下载系统通知")
                        .help("只控制下载开始 / 完成 / 失败的系统通知；强制更新下载完成仍会提醒。"),
                    Field::switch("notify_on_start", "下载开始时通知"),
                    Field::switch("notify_on_done", "下载完成时通知"),
                    Field::select(
                        "notify_click_action",
                        "点击通知时",
                        &[("folder", "在文件夹中显示"), ("list", "打开下载列表")],
                    ),
                    Field::button("notify_test", "发送测试通知"),
                    Field::select(
                        "after_all_done",
                        "全部下载完成后",
                        &[
                            ("none", "什么都不做"),
                            ("shutdown", "关机（60 秒倒计时，可取消）"),
                            ("sleep", "睡眠"),
                            ("run", "运行程序"),
                        ],
                    ),
                    Field::text_hint("after_all_done_cmd", "要运行的程序", r"D:\tools\done.exe")
                        .show_if("after_all_done", "run")
                        .wide(),
                    Field::group("cleanup", "删除与清理"),
                    Field::select(
                        "delete_mode",
                        "文件删除方式",
                        &[("recycle", "移入回收站"), ("permanent", "彻底删除")],
                    ),
                    Field::switch("delete_file_with_task", "删除任务时同时删文件"),
                    Field::switch("confirm_remove", "删除前确认"),
                    Field::switch("remove_missing_records", "文件丢失时移除记录"),
                    Field::switch("clear_finished_on_exit", "退出时清空已完成记录"),
                    // 短标签：这一项在设置页渲染成分段滑块，三个选项平分 214px，
                    // 「最近 100 条」放不下（见 components/SegmentedControl.vue）。
                    Field::select(
                        "history_keep",
                        "已完成记录保留",
                        &[("100", "100 条"), ("300", "300 条"), ("forever", "永久")],
                    ),
                ],
            ),
            // ── 网络 ──────────────────────────────────────────────────
            Section::new(
                "network",
                "网络",
                vec![
                    Field::group("proxy", "代理"),
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
                        .show_if("proxy_mode", "manual")
                        .wide(),
                    Field::group("github", "GitHub"),
                    Field::select("update_download_source", "GitHub 下载线路", GITHUB_SOURCES)
                        .help("GitHub 加速下载与 clone 共用；没有单独配置下载代理时按这里的线路走，配置了下载代理时下载优先走代理。")
                        .help_tip(),
                    Field::text_hint(
                        "update_custom_proxy",
                        "自定义代理",
                        "例如 https://gh-proxy.org/",
                    )
                    .show_if("update_download_source", "custom")
                    .wide(),
                    Field::password("github_token", "访问令牌")
                        .help("可选。填入后接口限额从 60 次/小时升到 5000 次/小时；只用于 api.github.com，不写日志，也不会带进下载任务。")
                        .help_tip(),
                    Field::group("request", "请求"),
                    Field::text_presets(
                        "default_user_agent",
                        "User-Agent",
                        "留空 = 用 aria2 自带 UA",
                        UA_PRESETS,
                    )
                    .wide(),
                    Field::group("dns", "DNS"),
                    Field::select(
                        "dns_servers",
                        "DNS 服务器",
                        &[
                            ("", "系统默认"),
                            ("223.5.5.5,223.6.6.6", "阿里 DNS"),
                            ("119.29.29.29,182.254.116.116", "腾讯 DNS"),
                            ("101.226.4.6,218.30.118.6", "360 DNS"),
                        ],
                    )
                    .help("下载引擎不支持 DoH，这里使用对应服务商的明文 DNS 地址。"),
                    Field::group("tls", "TLS"),
                    Field::switch("check_certificate", "校验证书")
                        .help("关掉可以下载自签名证书的站点，但会失去对中间人攻击的防护。"),
                ],
            ),
            // ── 高级 ──────────────────────────────────────────────────
            Section::new(
                "advanced",
                "高级",
                vec![
                    Field::group("extensionBridge", "浏览器插件接管"),
                    Field::switch("extension_bridge_enabled", "启用接管")
                        .help("启动一个只监听 127.0.0.1 的接口，供 Vexto Connect 扩展把网页下载交过来。"),
                    Field::number_unit("extension_bridge_port", "插件端口", 1024, 65535, ""),
                    Field::password("extension_bridge_secret", "插件密钥"),
                    Field::buttons(&[
                        ("open_extension_edge", "Edge 扩展"),
                        ("open_extension_firefox", "Firefox 扩展"),
                        ("download_extension_chrome", "Chrome 离线包"),
                    ]),
                    Field::note(
                        "extensionInstallNote",
                        "Chrome 暂无商店版本：先点「Chrome 离线包」下载并解压，在 chrome://extensions 开启开发者模式，再点「加载已解压的扩展程序」。",
                    ),
                    Field::group("engine", "下载引擎"),
                    Field::number_unit("rpc_port", "RPC 端口", 1024, 65535, "")
                        .help("aria2 的控制接口，只绑 127.0.0.1；被占用时会自动换一个可用端口。"),
                    Field::buttons(&[
                        ("download_engine_restart", "重启下载引擎"),
                        ("open_download_dir", "打开下载目录"),
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

    /// GitHub 令牌属于「网络」页；它是密码框，且必须在配置里有对应字段。
    #[test]
    fn network_tab_has_github_token_password() {
        let network = Download
            .sections()
            .into_iter()
            .find(|s| s.id == "network")
            .expect("网络页签");
        let f = network
            .fields
            .iter()
            .find(|f| f.key == "github_token")
            .expect("github_token 字段");
        assert!(matches!(f.kind, FieldKind::Password));
    }

    /// 浏览器插件接管分组要有三个安装入口，并说明 Chrome 的离线装法（ADR-094）。
    #[test]
    fn extension_group_has_install_entries() {
        let sections = Download.sections();
        let fields: Vec<&crate::tools::Field> =
            sections.iter().flat_map(|s| s.fields.iter()).collect();
        let actions: Vec<&str> = fields
            .iter()
            .filter_map(|f| match &f.kind {
                FieldKind::ButtonRow { actions } => {
                    Some(actions.iter().map(|a| a.0).collect::<Vec<_>>())
                }
                _ => None,
            })
            .flatten()
            .collect();
        for want in [
            "open_extension_edge",
            "open_extension_firefox",
            "download_extension_chrome",
        ] {
            assert!(actions.contains(&want), "缺少扩展安装入口 {want}");
        }
        let note = fields
            .iter()
            .find(|f| f.key == "extensionInstallNote")
            .expect("Chrome 离线安装说明不能少");
        assert!(matches!(note.kind, FieldKind::Note { .. }));
    }
}
