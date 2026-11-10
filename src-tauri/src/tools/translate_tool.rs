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
use crate::config::{
    MODE_OPENAI, MODE_VENDOR_LLM, MODE_VENDOR_MT, VENDOR_BAIDU, VENDOR_YOUDAO,
};
use crate::translate::{engine_by_id, engine_id_of, MODE_OPTIONS, VENDOR_OPTIONS};

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
        let id = engine_id_of(&cfg);
        match engine_by_id(&id) {
            Some(e) => {
                if e.is_ready(&cfg) {
                    format!("{} · 已就绪", e.display_name(&cfg))
                } else {
                    format!("{} · 未配置", e.display_name(&cfg))
                }
            }
            None => format!("未知引擎「{id}」"),
        }
    }

    fn sections(&self) -> Vec<Section> {
        // 厂商模式下的「同时满足」条件：凭据字段既要看厂商，也要看模式不是 OpenAI。
        // 单条件表达不了，所以用 show_if + and_if_any（见 tools::ShowIf 的注释）。
        let vendor_modes: &[&'static str] = &[MODE_VENDOR_MT, MODE_VENDOR_LLM];

        vec![
            Section::new(
                "engine",
                "引擎",
                vec![
                    Field::select("engine_mode", "翻译模式", MODE_OPTIONS),
                    Field::note(
                        "厂商机器翻译快、便宜；厂商大模型翻译质量更好但慢一些；\n\
                         「OpenAI 兼容」可以接 DeepSeek / 硅基流动 / Ollama 等任何兼容接口。",
                    ),
                    Field::select("vendor", "厂商", VENDOR_OPTIONS)
                        .show_if_any("engine_mode", vendor_modes),
                    Field::note("百度与有道都要先在各自控制台创建应用，拿到「应用 ID / 密钥」。")
                        .show_if_any("engine_mode", vendor_modes),
                    // ── 百度（两种厂商模式共用同一套凭据）──
                    Field::text_hint("appid", "APPID", "百度翻译开放平台的 APPID")
                        .show_if("vendor", VENDOR_BAIDU)
                        .and_if_any("engine_mode", vendor_modes),
                    Field::password("secret", "密钥")
                        .show_if("vendor", VENDOR_BAIDU)
                        .and_if_any("engine_mode", vendor_modes),
                    Field::note(
                        "百度 APPID / 密钥在 fanyi-api.baidu.com 控制台获取，标准版有免费额度。",
                    )
                    .show_if("vendor", VENDOR_BAIDU)
                    .and_if_any("engine_mode", vendor_modes),
                    // ── 有道 ──
                    Field::text_hint("youdao_app_key", "应用 ID", "有道智云的应用 ID")
                        .show_if("vendor", VENDOR_YOUDAO)
                        .and_if_any("engine_mode", vendor_modes),
                    Field::password("youdao_app_secret", "应用密钥")
                        .show_if("vendor", VENDOR_YOUDAO)
                        .and_if_any("engine_mode", vendor_modes),
                    Field::note(
                        "有道在 ai.youdao.com 的「应用管理」里取应用 ID / 应用密钥；\n\
                         文本翻译与大模型翻译共用这一套，不必建两个应用。\n\
                         注意：有道只有这两项，没有 API Key 这种鉴权方式。",
                    )
                    .show_if("vendor", VENDOR_YOUDAO)
                    .and_if_any("engine_mode", vendor_modes),
                    // ── OpenAI 兼容（可以存多套，各起名字）──
                    Field::list(
                        "openai_profiles",
                        "模型列表",
                        vec![
                            Field::text_hint("name", "名称", "例如 DeepSeek"),
                            Field::text_hint("base", "BaseURL", "https://api.deepseek.com"),
                            Field::text_hint("model", "模型", "deepseek-chat"),
                            Field::password("key", "API Key"),
                            Field::text_hint("prompt", "自定义指令", "留空用内置翻译指令"),
                        ],
                    )
                    .active_key("openai_active")
                    .show_if("engine_mode", MODE_OPENAI),
                    Field::note(
                        "每套模型都可以改个名字，点左边的圆点选「当前用这套」。\n\
                         BaseURL 填 https://api.deepseek.com 或 https://xxx/v1 都行，会自动补全路径。",
                    )
                    .show_if("engine_mode", MODE_OPENAI),
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

    // ── 会话 29：三模式 + 厂商 ──────────────────────────────

    #[test]
    fn engine_section_exposes_modes_and_both_vendors() {
        let s = Translate.sections();
        let keys: Vec<&str> = s[0].fields.iter().map(|f| f.key).collect();
        for k in [
            "engine_mode",
            "vendor",
            "appid",
            "secret",
            "youdao_app_key",
            "youdao_app_secret",
            "openai_profiles",
        ] {
            assert!(keys.contains(&k), "引擎页缺字段 {k}，实际：{keys:?}");
        }
        assert!(
            !keys.contains(&"engine"),
            "老的单一下拉不该再出现（值只用于迁移）"
        );
    }

    /// 凭据字段必须同时看「厂商」和「模式」：只看厂商的话，切到 OpenAI 兼容时
    /// 百度/有道的输入框还会挂在页面上。
    #[test]
    fn vendor_credentials_require_both_conditions() {
        let s = Translate.sections();
        for key in ["youdao_app_secret", "secret"] {
            let f = s[0]
                .fields
                .iter()
                .find(|f| f.key == key)
                .unwrap_or_else(|| panic!("找不到字段 {key}"));
            let cond = f.show_if.as_ref().expect("凭据字段要有联动条件");
            assert!(
                matches!(cond.key, "vendor"),
                "{key} 的第一条件是厂商，实际 {}",
                cond.key
            );
            let also = cond.also.as_ref().expect("还要看模式，否则 OpenAI 模式下会露出");
            assert_eq!(also.key, "engine_mode");
            assert!(also.equals.contains(&MODE_VENDOR_MT));
            assert!(also.equals.contains(&MODE_VENDOR_LLM));
        }
    }

    /// OpenAI 兼容的模型列表：卡片里要能填名称 / BaseURL / 模型 / Key / 指令，
    /// 并且有「使用中」单选（写到 `openai_active`）。
    #[test]
    fn openai_profile_list_has_all_item_fields_and_active_key() {
        let s = Translate.sections();
        let f = s[0]
            .fields
            .iter()
            .find(|f| f.key == "openai_profiles")
            .expect("找不到模型列表字段");
        match &f.kind {
            crate::tools::FieldKind::List {
                item_fields,
                active_key,
            } => {
                let keys: Vec<&str> = item_fields.iter().map(|x| x.key).collect();
                for k in ["name", "base", "model", "key", "prompt"] {
                    assert!(keys.contains(&k), "卡片里缺输入框 {k}，实际：{keys:?}");
                }
                assert_eq!(*active_key, Some("openai_active"));
            }
            other => panic!("应该是个 list 字段，实际：{other:?}"),
        }
        // 只在 OpenAI 模式下显示
        let cond = f.show_if.as_ref().expect("模型列表要有联动条件");
        assert_eq!(cond.key, "engine_mode");
        assert_eq!(cond.equals, vec![MODE_OPENAI]);
    }

    /// 厂商下拉只在两种厂商模式下出现，OpenAI 兼容模式下藏起来。
    #[test]
    fn vendor_select_is_hidden_for_openai_mode() {
        let s = Translate.sections();
        let f = s[0]
            .fields
            .iter()
            .find(|f| f.key == "vendor")
            .expect("找不到厂商字段");
        let cond = f.show_if.as_ref().expect("厂商字段要有联动条件");
        assert_eq!(cond.key, "engine_mode");
        assert!(!cond.equals.contains(&MODE_OPENAI), "OpenAI 模式不选厂商");
    }
}
