//! 关于：版本、功能概览、链接、许可证与第三方组件。
//!
//! 内容（文案 / 功能卡 / 链接 / 组件表）全部由本文件的 `info()` 返回，前端只按
//! 通用 `about` 字段渲染；加文案不用改 JS。命令名也从这里下发。

use rust_i18n::t;
use serde::Serialize;

use super::{Field, Section, Tool};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AboutInfo {
    pub name: &'static str,
    pub version: &'static str,
    pub subtitle: String,
    pub intro: String,
    pub features: Vec<AboutFeature>,
    pub links: Vec<AboutLink>,
    pub components: Vec<AboutComponent>,
    pub license: String,
    pub attribution: String,
    pub footer: String,
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
    pub title: String,
    pub desc: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AboutLink {
    pub id: &'static str,
    pub label: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AboutComponent {
    pub name: String,
    pub version: &'static str,
    pub license: &'static str,
}

pub fn info() -> AboutInfo {
    AboutInfo {
        name: "Vexto",
        version: env!("CARGO_PKG_VERSION"),
        subtitle: t!("about.subtitle").into_owned(),
        intro: t!("about.intro").into_owned(),
        features: vec![
            AboutFeature {
                icon: "translate",
                title: t!("about.feature.translate.title").into_owned(),
                desc: t!("about.feature.translate.desc").into_owned(),
            },
            AboutFeature {
                icon: "camera",
                title: t!("about.feature.screenshot.title").into_owned(),
                desc: t!("about.feature.screenshot.desc").into_owned(),
            },
            AboutFeature {
                icon: "download",
                title: t!("about.feature.download.title").into_owned(),
                desc: t!("about.feature.download.desc").into_owned(),
            },
            AboutFeature {
                icon: "transfer",
                title: t!("about.feature.transfer.title").into_owned(),
                desc: t!("about.feature.transfer.desc").into_owned(),
            },
        ],
        links: vec![
            AboutLink {
                id: "repo",
                label: t!("about.link.repo").into_owned(),
            },
            AboutLink {
                id: "issues",
                label: t!("about.link.issues").into_owned(),
            },
            AboutLink {
                id: "releases",
                label: t!("about.link.releases").into_owned(),
            },
            AboutLink {
                id: "license",
                label: t!("about.link.license").into_owned(),
            },
        ],
        components: vec![
            AboutComponent {
                name: "Tauri".to_string(),
                version: "2.x",
                license: "MIT / Apache-2.0",
            },
            AboutComponent {
                name: "aria2c".to_string(),
                version: "1.37",
                license: "GPL-2.0-or-later",
            },
            AboutComponent {
                name: "lanflare-core".to_string(),
                version: "0.5",
                license: "Apache-2.0",
            },
            AboutComponent {
                name: t!("about.component.paddleocr").into_owned(),
                version: "—",
                license: "Apache-2.0",
            },
            AboutComponent {
                name: "whatlang".to_string(),
                version: "0.18",
                license: "MIT",
            },
            AboutComponent {
                name: "reqwest".to_string(),
                version: "0.13",
                license: "MIT / Apache-2.0",
            },
        ],
        license: t!("about.license").into_owned(),
        attribution: t!("about.attribution").into_owned(),
        footer: t!("about.footer").into_owned(),
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
        "repo" => "https://github.com/Ruxsmiles/vexto",
        "issues" => "https://github.com/Ruxsmiles/vexto/issues",
        "releases" => "https://github.com/Ruxsmiles/vexto/releases",
        "license" => "https://github.com/Ruxsmiles/vexto/blob/main/LICENSE",
        _ => return Err(t!("error.about.unknownLink").into_owned()),
    };
    crate::update::open_url(url)
}

/// 复制给用户排障用；不包含任何配置值 / 凭据。
pub fn diagnostics() -> String {
    format!(
        "Vexto {}\nOS: {} / {}\n{}: {}\n{}: {}\n{}: {}\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        t!("about.diag.config"),
        crate::config::config_path().display(),
        t!("about.diag.logs"),
        crate::config::log_dir().display(),
        t!("about.diag.update"),
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
        Err(t!("error.about.cannotDisable").into_owned())
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
                Field::about_legal("about_legal", "许可"),
            ],
        )]
    }
}
