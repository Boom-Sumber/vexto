//! 翻译工具：一个工具、三个标签页。
//!
//! 为什么是三个标签而不是三个左侧项：引擎配置是**划词与截图共用**的。如果拆成
//! 「划词翻译」「截图翻译」两个左侧项，引擎配置无处可放——要么两边各渲染一遍
//! （隐性耦合：改一处忘一处），要么写「去划词翻译里改」（跨分区跳转）。
//! 用户 m01468 的建议正是这个方向：左侧按工具分，工具内部的子功能用标签页。
//!
//! S2.5 起「划词翻译」标签改名为「翻译配置」：它已经不只服务划词了——
//! 语种检测、第二目标语言对划词 / 输入 / 截图三条链路都生效
//! （输入的文本和 OCR 出来的文本走同一套检测流程，见 translate::resolve_direction）。

use super::{Field, Section, Tool};
use crate::config;
use crate::translate::{engine_by_id, ENGINE_OPTIONS};

// `LANGS` 只在 `target_options_match_lang_table` 里用到（生产代码走的是下面的
// `TO_STATIC` 静态表），所以 import 挂在测试模块上，否则会报 unused import。
#[cfg(test)]
use crate::lang::LANGS;

pub struct Translate;

/// 目标语言选项。
///
/// **必须和 `lang::LANGS` 严格一致**，不能手写代码——S1 手写的那一版用了
/// `ja`/`ko`/`fr`/`es`（常见写法），百度一个都不认，用户选这四种语言时翻译必然
/// 返回 `58001 INVALID_TO_PARAM`，这个 bug 活到 S2.5 才被逐码实测发现。
/// `target_options_match_lang_table` 单测盯着这两张表不许漂移。
const TO_STATIC: &[(&str, &str)] = &[
    ("zh", "中文"),
    ("cht", "繁体中文"),
    ("en", "英语"),
    ("jp", "日语"),
    ("kor", "韩语"),
    ("fra", "法语"),
    ("de", "德语"),
    ("spa", "西班牙语"),
    ("ru", "俄语"),
    ("pt", "葡萄牙语"),
    ("it", "意大利语"),
    ("th", "泰语"),
    ("vie", "越南语"),
    ("ara", "阿拉伯语"),
    ("nl", "荷兰语"),
    ("pl", "波兰语"),
];

/// 源语言选项 = 目标语言 + 首项「自动检测」。
/// 用户选了具体语言就不走检测（`translate::resolve_direction` 的第一条分支）。
const FROM_STATIC: &[(&str, &str)] = &[
    ("auto", "自动检测"),
    ("zh", "中文"),
    ("cht", "繁体中文"),
    ("en", "英语"),
    ("jp", "日语"),
    ("kor", "韩语"),
    ("fra", "法语"),
    ("de", "德语"),
    ("spa", "西班牙语"),
    ("ru", "俄语"),
    ("pt", "葡萄牙语"),
    ("it", "意大利语"),
    ("th", "泰语"),
    ("vie", "越南语"),
    ("ara", "阿拉伯语"),
    ("nl", "荷兰语"),
    ("pl", "波兰语"),
];

const DETECT_OPTIONS: &[(&str, &str)] = &[("local", "本地检测（离线，推荐）"), ("baidu", "百度语种检测")];

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
                "翻译配置",
                vec![
                    Field::hotkey("hotkey", "划词热键"),
                    Field::note("选中文字后按这个组合键，光标旁弹出译文。"),
                    Field::hotkey("hotkey_input", "输入翻译热键"),
                    Field::note(
                        "按这个组合键唤出一个可输入的翻译框，打完字回车即翻译。\
                         它和划词翻译共用一个弹窗，只是不会自动取词。",
                    ),
                    Field::select("lang_detect", "语种检测方式", DETECT_OPTIONS),
                    Field::note(
                        "「百度在线检测」不需要 APPID / 密钥，失败时自动回落到本地检测。",
                    ),
                    Field::select("from", "源语言", FROM_STATIC),
                    Field::note(
                        "选「自动检测」就把 auto 发给厂商，让它再判一次（二次确认）；\
                         选具体语言时如果和检测结果冲突，以检测结果为准。",
                    ),
                    Field::select("to", "目标语言", TO_STATIC),
                    Field::select("to2", "第二目标语言", TO_STATIC),
                    Field::note(
                        "规则：检测出的语言**等于「目标语言」**时译到「第二目标语言」，否则译到「目标语言」。\n\
                         常用：目标=中文、第二目标=英语，中英双向免切。",
                    ),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 选项表必须和 `lang::LANGS` 严格一致。
    ///
    /// 这条测试是给「选日语翻译失败」那个 bug 立的碑：两套语言代码各自维护，
    /// 迟早会漂移。谁加了新语言却忘了改这里，`cargo test` 立刻红。
    #[test]
    fn target_options_match_lang_table() {
        assert_eq!(
            TO_STATIC.len(),
            LANGS.len(),
            "目标语言选项 {} 项，语言表 {} 项，两边必须一一对应",
            TO_STATIC.len(),
            LANGS.len()
        );
        for (i, l) in LANGS.iter().enumerate() {
            let (code, name) = TO_STATIC[i];
            assert_eq!(code, l.baidu, "第 {i} 项代码不一致");
            assert_eq!(name, l.name, "第 {i} 项名字不一致");
        }
    }

    #[test]
    fn from_options_are_target_options_plus_auto() {
        assert_eq!(FROM_STATIC[0], ("auto", "自动检测"));
        assert_eq!(
            &FROM_STATIC[1..],
            TO_STATIC,
            "源语言除了首项 auto，其余必须和目标语言完全一样"
        );
    }

    /// 这个测试挡住 `ja`/`ko`/`fr`/`es` 那类「常见但百度不认」的代码再次混进来。
    #[test]
    fn options_avoid_baidu_invalid_codes() {
        for bad in ["ja", "ko", "fr", "es", "vi", "pt-BR", "zh-CN"] {
            assert!(
                !TO_STATIC.iter().any(|(c, _)| *c == bad),
                "{bad} 百度不认（返回 58001），不能进目标语言选项"
            );
        }
    }

    #[test]
    fn sections_are_named_for_the_ui() {
        let t = Translate;
        let s = t.sections();
        assert_eq!(s.len(), 3);
        assert_eq!(s[0].id, "engine");
        assert_eq!(s[1].id, "selection");
        assert_eq!(s[1].name, "翻译配置", "S2.5 改名，不再叫「划词翻译」");
        assert_eq!(s[2].id, "ocr");
    }

    /// 新加的三个字段必须在「翻译配置」标签页里，否则用户在界面上找不到。
    #[test]
    fn translation_config_exposes_direction_fields() {
        let t = Translate;
        let s = t.sections();
        let keys: Vec<&str> = s[1].fields.iter().map(|f| f.key).collect();
        for k in ["to", "to2", "lang_detect", "from", "hotkey", "auto_copy"] {
            assert!(keys.contains(&k), "「翻译配置」里缺字段 {k}，实际：{keys:?}");
        }
    }
}
