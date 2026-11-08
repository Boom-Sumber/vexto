//! 翻译工具：一个工具、三个标签页。
//!
//! 为什么是三个标签而不是三个左侧项：引擎配置是**划词与截图共用**的。如果拆成
//! 「划词翻译」「截图翻译」两个左侧项，引擎配置无处可放——要么两边各渲染一遍
//! （隐性耦合：改一处忘一处），要么写「去划词翻译里改」（跨分区跳转）。
//! 用户 m01468 的建议正是这个方向：左侧按工具分，工具内部的子功能用标签页。

use super::{Field, Section, Tool};
use crate::config;
use crate::translate::{engine_by_id, ENGINE_OPTIONS};

pub struct Translate;

/// 目标语言选项。只列常用语种——百度支持 200+，全列出来下拉框没法用。
const TO_OPTIONS: &[(&str, &str)] = &[
    ("zh", "中文"),
    ("en", "英语"),
    ("ja", "日语"),
    ("ko", "韩语"),
    ("fr", "法语"),
    ("de", "德语"),
    ("ru", "俄语"),
    ("es", "西班牙语"),
];

const FROM_OPTIONS: &[(&str, &str)] = &[
    ("auto", "自动检测"),
    ("zh", "中文"),
    ("en", "英语"),
    ("ja", "日语"),
    ("ko", "韩语"),
    ("fr", "法语"),
    ("de", "德语"),
    ("ru", "俄语"),
    ("es", "西班牙语"),
];

impl Tool for Translate {
    fn id(&self) -> &'static str {
        "translate"
    }

    fn name(&self) -> &'static str {
        "翻译"
    }

    fn desc(&self) -> &'static str {
        "划词翻译与截图翻译"
    }

    fn enabled(&self) -> bool {
        config::load().enable_translate
    }

    fn set_enabled(&self, on: bool) -> Result<(), String> {
        let mut cfg = config::load();
        cfg.enable_translate = on;
        config::save(&cfg)
    }

    fn status_line(&self) -> String {
        let cfg = config::load();
        if !cfg.enable_translate {
            return "已停用".to_string();
        }
        match engine_by_id(&cfg.engine) {
            Some(e) => {
                if e.is_ready(&cfg) {
                    format!("{} · 已就绪", e.name())
                } else {
                    format!("{} · 未配置", e.name())
                }
            }
            None => format!("未知引擎「{}」", cfg.engine),
        }
    }

    fn sections(&self) -> Vec<Section> {
        vec![
            Section::new(
                "engine",
                "引擎",
                vec![
                    Field::select("engine", "翻译引擎", ENGINE_OPTIONS),
                    Field::note("划词翻译和截图翻译共用这里的引擎配置。"),
                    // ── 百度 ──
                    Field::text_hint("appid", "APPID", "百度翻译开放平台的 APPID")
                        .show_if("engine", "baidu"),
                    Field::password("secret", "密钥").show_if("engine", "baidu"),
                    Field::note(
                        "百度 APPID / 密钥在 fanyi-api.baidu.com 控制台获取，标准版有免费额度。",
                    )
                    .show_if("engine", "baidu"),
                    // ── OpenAI 兼容 ──
                    Field::text_hint("openai_base", "BaseURL", "https://api.deepseek.com")
                        .show_if("engine", "openai"),
                    Field::text_hint("openai_model", "模型", "deepseek-chat")
                        .show_if("engine", "openai"),
                    Field::password("openai_key", "API Key").show_if("engine", "openai"),
                    Field::text_hint("openai_prompt", "自定义指令", "留空用内置翻译指令")
                        .show_if("engine", "openai"),
                    Field::button("test_translate", "测试一下"),
                ],
            ),
            Section::new(
                "selection",
                "划词翻译",
                vec![
                    Field::hotkey("hotkey", "划词热键"),
                    Field::note("选中文字后按这个组合键，光标旁弹出译文。"),
                    Field::select("from", "源语言", FROM_OPTIONS),
                    Field::select("to", "目标语言", TO_OPTIONS),
                    Field::switch("auto_copy", "译完自动复制译文"),
                ],
            ),
            Section::new(
                "ocr",
                "截图翻译",
                vec![
                    Field::switch("enable_ocr", "启用截图翻译"),
                    Field::hotkey("hotkey_ocr", "截图热键"),
                    Field::note("截图翻译的框选与 OCR 在 S5 实现，这里先把热键配好。"),
                ],
            ),
        ]
    }
}
