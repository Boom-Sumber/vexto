//! 关于：版本、功能概览、链接、许可证与第三方组件。
//!
//! 内容（文案 / 功能卡 / 链接 / 组件表）全部由本文件的 `info()` 返回，前端只按
//! 通用 `about` 字段渲染；加文案不用改 JS。命令名也从这里下发。

use serde::Serialize;

use super::{Field, Section, Tool};

/// 更新下载线路。`auto` 对国内网络最友好（先走 gh-proxy，最后才直连）。
const UPDATE_SOURCES: &[(&str, &str)] = &[
    ("auto", "自动（代理优先）"),
    ("direct", "直连 GitHub"),
    ("custom", "自定义代理"),
];

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AboutInfo {
    pub name: &'static str,
    pub version: &'static str,
    pub subtitle: &'static str,
    pub intro: &'static str,
    pub features: Vec<AboutFeature>,
    pub links: Vec<AboutLink>,
    pub components: Vec<AboutComponent>,
    pub license: &'static str,
    pub attribution: &'static str,
    pub footer: &'static str,
    pub update: AboutUpdate,
    pub open_link_command: &'static str,
    pub diagnostics_command: &'static str,
    pub open_dir_command: &'static str,
}

/// 更新命令名由后端下发，关于页的动作区按当前 phase 调用。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AboutUpdate {
    pub check: &'static str,
    pub download: &'static str,
    pub cancel: &'static str,
    pub install: &'static str,
    pub open: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AboutFeature {
    pub icon: &'static str,
    pub title: &'static str,
    pub desc: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AboutLink {
    pub id: &'static str,
    pub label: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AboutComponent {
    pub name: &'static str,
    pub version: &'static str,
    pub license: &'static str,
}

pub fn info() -> AboutInfo {
    AboutInfo {
        name: "Vexto",
        version: env!("CARGO_PKG_VERSION"),
        subtitle: "划词翻译 · 截图 · 下载 · 局域网互传",
        intro: "一个 Windows 常驻托盘的轻量工具箱，把划词翻译、截图 OCR、下载和局域网互传\
                合进一个进程。翻译只把你选中的文本发给你配置的引擎；文件传输只在局域网内；\
                不收集使用数据。",
        features: vec![
            AboutFeature {
                icon: "translate",
                title: "划词翻译",
                desc: "UIA 取词 · 微软 / 百度 / 有道 / LLM",
            },
            AboutFeature {
                icon: "camera",
                title: "截图 OCR",
                desc: "区域 / 全屏 / 窗口 · 长图 · 钉屏",
            },
            AboutFeature {
                icon: "download",
                title: "下载",
                desc: "HTTP / BT / magnet · aria2c",
            },
            AboutFeature {
                icon: "transfer",
                title: "局域网互传",
                desc: "与 LocalSend 互通 · 加密传输",
            },
        ],
        links: vec![
            AboutLink {
                id: "repo",
                label: "源码仓库",
            },
            AboutLink {
                id: "issues",
                label: "问题反馈",
            },
            AboutLink {
                id: "releases",
                label: "更新日志",
            },
            AboutLink {
                id: "license",
                label: "开源许可",
            },
        ],
        components: vec![
            AboutComponent {
                name: "Tauri",
                version: "2.x",
                license: "MIT / Apache-2.0",
            },
            AboutComponent {
                name: "aria2c",
                version: "1.37",
                license: "GPL-2.0-or-later",
            },
            AboutComponent {
                name: "lanflare-core",
                version: "0.5",
                license: "Apache-2.0",
            },
            AboutComponent {
                name: "PaddleOCR 模型",
                version: "—",
                license: "Apache-2.0",
            },
            AboutComponent {
                name: "whatlang",
                version: "0.18",
                license: "MIT",
            },
            AboutComponent {
                name: "reqwest",
                version: "0.13",
                license: "MIT / Apache-2.0",
            },
        ],
        license: "本程序以 GNU General Public License v3.0 发布。你可以自由使用、研究、\
                  修改和再分发本软件；分发修改后的版本时，须同样以 GPL-3.0 开放源代码，\
                  并附带许可证副本。本程序不含任何担保。",
        attribution: "本项目在功能与设计上借鉴了 Pot、ShareX、Rayburst、LocalSend 等\
                      开源项目，未拷贝其源代码；相关版权与商标归各自作者所有。",
        footer: "本程序按「现状」提供，不对使用后果作任何担保。\n\
                 © 2026 Boom-Sumber · GNU GPL-3.0",
        update: AboutUpdate {
            check: "update_check",
            download: "update_download",
            cancel: "update_cancel",
            install: "update_install",
            open: "update_open_release",
        },
        open_link_command: "about_open_link",
        diagnostics_command: "about_diagnostics",
        open_dir_command: "open_config_dir",
    }
}

/// 只允许打开固定入口，不接受任意 URL。
pub fn open_link(kind: &str) -> Result<(), String> {
    let url = match kind {
        "repo" => "https://github.com/Boom-Sumber/vexto",
        "issues" => "https://github.com/Boom-Sumber/vexto/issues",
        "releases" => "https://github.com/Boom-Sumber/vexto/releases",
        "license" => "https://github.com/Boom-Sumber/vexto/blob/main/LICENSE",
        _ => return Err("未知的链接".to_string()),
    };
    crate::update::open_url(url)
}

/// 复制给用户排障用；不包含任何配置值 / 凭据。
pub fn diagnostics() -> String {
    format!(
        "Vexto {}\nOS: {} / {}\n配置: {}\n日志: {}\n更新: {}\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        crate::config::config_path().display(),
        crate::config::log_dir().display(),
        crate::update::diagnostics_line()
    )
}

pub struct About;

impl Tool for About {
    fn id(&self) -> &'static str {
        "about"
    }

    fn name(&self) -> &'static str {
        "关于"
    }

    fn desc(&self) -> &'static str {
        "版本与许可"
    }

    fn icon(&self) -> &'static str {
        "info"
    }

    fn enabled(&self) -> bool {
        true
    }

    fn set_enabled(&self, _on: bool) -> Result<(), String> {
        Err("「关于」不能停用".to_string())
    }

    /// 只读信息页，没有「启用」这件事。
    fn switchable(&self) -> bool {
        false
    }

    /// 用新品牌区代替设置页的通用页头。
    fn hide_header(&self) -> bool {
        true
    }

    fn status_line(&self) -> String {
        format!(
            "Vexto {}{}",
            env!("CARGO_PKG_VERSION"),
            crate::update::status_suffix()
        )
    }

    fn sections(&self) -> Vec<Section> {
        vec![Section::new(
            "about",
            "关于",
            vec![
                Field::about_top("about_top", "关于"),
                Field::group("更新"),
                Field::select("update_download_source", "下载线路", UPDATE_SOURCES),
                Field::text_hint(
                    "update_custom_proxy",
                    "自定义代理",
                    "例如 https://gh-proxy.org/",
                )
                .show_if("update_download_source", "custom"),
                Field::about_legal("about_legal", "许可"),
            ],
        )]
    }
}
