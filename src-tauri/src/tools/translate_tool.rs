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
use crate::translate::{engine_by_id, engine_id_of};

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
        if id.is_empty() {
            // 一个实例都没配：别拿 engine_by_id("") 的默认百度来冒充
            return "未配置引擎".to_string();
        }
        match engine_by_id(&id) {
            Some(e) => {
                // 就绪判断走统一入口（要先投影实例里的凭据，不能直接问 cfg）
                if crate::translate::engine_ready(&cfg) {
                    format!("{} · 已就绪", e.display_name(&cfg))
                } else {
                    format!("{} · 未配置", e.display_name(&cfg))
                }
            }
            None => format!("未知引擎「{id}」"),
        }
    }

    fn sections(&self) -> Vec<Section> {
        vec![
            Section::new(
                "engine",
                "引擎",
                vec![
                    // ── 翻译：选一个已配置的实例 ──
                    Field::group("翻译"),
                    Field::select_dynamic("active_engine", "翻译模式", "engines"),
                    Field::note("从下面两个列表里选一个已配置的实例。"),
                    // ── 厂商：卡片列表 + 弹窗新建 ──
                    Field::group("厂商管理"),
                    Field::list("vendor_profiles", "已配置的厂商", vendor_item_fields())
                        .summary(&["name", "vendor", "use_llm"])
                        .add_dialog()
                        .test_action("test_engine_profile"),
                    // ── 大模型（OpenAI 兼容）──
                    Field::group("大模型管理"),
                    Field::list("openai_profiles", "已配置的模型", openai_item_fields())
                        .summary(&["name", "model"])
                        .add_dialog()
                        .test_action("test_engine_profile"),
                ],
            ),
            Section::new(
                "selection",
                "翻译配置",
                vec![
                    Field::group("翻译方向"),
                    Field::select("lang_detect", "语种检测方式", DETECT_OPTIONS),
                    Field::note("「百度在线检测」不需要 APPID / 密钥，失败时自动回落到本地检测。"),
                    Field::select("from", "源语言", FROM_STATIC),
                    Field::note(
                        "选「自动检测」就把 auto 发给厂商，让它再判一次（二次确认）；\n\
                         选具体语言时如果和检测结果冲突，以检测结果为准。",
                    ),
                    Field::select("to", "目标语言", TO_STATIC),
                    Field::select("to2", "第二目标语言", TO_STATIC),
                    Field::note(
                        "规则：检测出的语言**等于「目标语言」**时译到「第二目标语言」，否则译到「目标语言」。\n\
                         常用：目标=中文、第二目标=英语，中英双向免切。",
                    ),
                    Field::group("交互"),
                    Field::hotkey("hotkey", "划词热键"),
                    Field::note("选中文字后按这个组合键，光标旁弹出译文。"),
                    Field::hotkey("hotkey_input", "输入翻译热键"),
                    Field::note(
                        "按这个组合键唤出一个可输入的翻译框，打完字回车即翻译。\n\
                         它和划词翻译共用一个弹窗，只是不会自动取词。",
                    ),
                    Field::switch("auto_copy", "译完自动复制译文"),
                ],
            ),
            Section::new(
                "ocr",
                "截图翻译",
                vec![
                    Field::group("截图翻译"),
                    Field::switch("enable_ocr", "启用截图翻译"),
                    Field::hotkey("hotkey_ocr", "截图热键"),
                    Field::note("截图翻译的框选与 OCR 在 S5 实现，这里先把热键配好。"),
                ],
            ),
        ]
    }
}

/// 「厂商管理」卡片 / 弹窗里的字段。
///
/// 两家的凭据都放进来、按 `vendor` 联动显示：用户在弹窗里切厂商时，
/// 已经填过的另一家字段不会丢（切回来还在）。
fn vendor_item_fields() -> Vec<Field> {
    vec![
        Field::text_hint("name", "名称", "例如 百度（主账号）"),
        Field::select("vendor", "厂商", crate::translate::VENDOR_OPTIONS),
        Field::switch("use_llm", "使用大模型翻译"),
        Field::text_hint("appid", "APPID", "百度翻译开放平台")
            .show_if("vendor", crate::config::VENDOR_BAIDU),
        Field::password("secret", "密钥").show_if("vendor", crate::config::VENDOR_BAIDU),
        Field::text_hint("youdao_app_key", "应用 ID", "有道智云")
            .show_if("vendor", crate::config::VENDOR_YOUDAO),
        Field::password("youdao_app_secret", "应用密钥")
            .show_if("vendor", crate::config::VENDOR_YOUDAO),
    ]
}

/// 「大模型管理」卡片 / 弹窗里的字段。
fn openai_item_fields() -> Vec<Field> {
    vec![
        Field::text_hint("name", "名称", "例如 DeepSeek"),
        Field::text_hint("base", "BaseURL", "https://api.deepseek.com"),
        Field::text_hint("model", "模型", "deepseek-chat"),
        Field::password("key", "API Key"),
        Field::text_hint("prompt", "自定义指令", "留空用内置翻译指令"),
    ]
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

    // ── 会话 31：引擎页签按类别重组 ────────────────────────

    /// 引擎页签的顺序：翻译（选实例）→ 厂商管理 → 大模型管理。
    /// 三段的 `group` 标题必须按这个顺序出现，否则用户得自己在页面上找。
    #[test]
    fn engine_section_orders_groups() {
        let s = Translate.sections();
        let groups: Vec<&str> = s[0]
            .fields
            .iter()
            .filter_map(|f| match &f.kind {
                crate::tools::FieldKind::Group { title } => Some(*title),
                _ => None,
            })
            .collect();
        assert_eq!(groups, vec!["翻译", "厂商管理", "大模型管理"]);
    }

    #[test]
    fn engine_section_exposes_engine_select_and_both_lists() {
        let s = Translate.sections();
        let keys: Vec<&str> = s[0].fields.iter().map(|f| f.key).collect();
        for k in ["active_engine", "vendor_profiles", "openai_profiles"] {
            assert!(keys.contains(&k), "引擎页缺字段 {k}，实际：{keys:?}");
        }
        // 老的两代字段都不该再出现在界面上
        for old in ["engine", "engine_mode", "vendor", "appid", "openai_key"] {
            assert!(!keys.contains(&old), "老字段 {old} 不该还在引擎页");
        }
    }

    /// 「翻译模式」是动态下拉：选项来自配置，不是静态表。
    #[test]
    fn engine_select_is_dynamic() {
        let s = Translate.sections();
        let f = s[0]
            .fields
            .iter()
            .find(|f| f.key == "active_engine")
            .expect("找不到翻译模式字段");
        match &f.kind {
            crate::tools::FieldKind::Select { dynamic, options } => {
                assert_eq!(*dynamic, Some("engines"));
                assert!(options.is_empty(), "动态下拉不该带静态选项");
            }
            other => panic!("翻译模式应该是 select，实际：{other:?}"),
        }
    }

    /// 会话 32：底部那个「测试一下」和两张列表下面的长说明都删了，
    /// 测试改挂到每张卡片 / 新建弹窗上。
    #[test]
    fn engine_section_has_no_standalone_test_or_long_notes() {
        let s = Translate.sections();
        let keys: Vec<&str> = s[0].fields.iter().map(|f| f.key).collect();
        assert!(!keys.contains(&"test_translate"), "不该再有独立的测试按钮");
        let notes = s[0]
            .fields
            .iter()
            .filter(|f| matches!(f.kind, crate::tools::FieldKind::Note { .. }))
            .count();
        assert_eq!(notes, 1, "引擎页只留「翻译模式」下的一行说明");
    }

    /// 厂商卡片：折叠摘要取「名称 · 厂商 · 机翻/大模型」，新建走弹窗。
    #[test]
    fn vendor_list_is_collapsed_with_dialog() {
        let s = Translate.sections();
        let f = s[0]
            .fields
            .iter()
            .find(|f| f.key == "vendor_profiles")
            .expect("找不到厂商列表字段");
        match &f.kind {
            crate::tools::FieldKind::List {
                item_fields,
                summary,
                add_dialog,
                test_action,
            } => {
                let keys: Vec<&str> = item_fields.iter().map(|x| x.key).collect();
                for k in [
                    "name",
                    "vendor",
                    "use_llm",
                    "appid",
                    "secret",
                    "youdao_app_key",
                    "youdao_app_secret",
                ] {
                    assert!(keys.contains(&k), "厂商卡片里缺字段 {k}，实际：{keys:?}");
                }
                assert_eq!(*summary, vec!["name", "vendor", "use_llm"]);
                assert!(*add_dialog, "新建要走弹窗");
                assert_eq!(*test_action, Some("test_engine_profile"));
            }
            other => panic!("应该是 list 字段，实际：{other:?}"),
        }
    }

    /// 两家的凭据按「厂商」联动：只显示当前选的那家，另一家藏起来。
    /// 用户切厂商时，已经填过的字段还在（值不丢），只是看不见。
    #[test]
    fn vendor_credentials_follow_vendor_field() {
        let s = Translate.sections();
        let f = s[0]
            .fields
            .iter()
            .find(|f| f.key == "vendor_profiles")
            .expect("找不到厂商列表字段");
        let crate::tools::FieldKind::List { item_fields, .. } = &f.kind else {
            panic!("应该是 list");
        };
        for (key, want) in [
            ("appid", crate::config::VENDOR_BAIDU),
            ("secret", crate::config::VENDOR_BAIDU),
            ("youdao_app_key", crate::config::VENDOR_YOUDAO),
            ("youdao_app_secret", crate::config::VENDOR_YOUDAO),
        ] {
            let fld = item_fields
                .iter()
                .find(|x| x.key == key)
                .unwrap_or_else(|| panic!("找不到 {key}"));
            let cond = fld.show_if.as_ref().expect("凭据字段要有联动条件");
            assert_eq!(cond.key, "vendor", "{key} 要按厂商联动");
            assert_eq!(cond.equals, vec![want], "{key} 只属于 {want}");
        }
    }

    /// 大模型卡片：名称 / BaseURL / 模型 / Key / 指令，摘要取「名称 · 模型」。
    #[test]
    fn openai_list_is_collapsed_with_dialog() {
        let s = Translate.sections();
        let f = s[0]
            .fields
            .iter()
            .find(|f| f.key == "openai_profiles")
            .expect("找不到模型列表字段");
        match &f.kind {
            crate::tools::FieldKind::List {
                item_fields,
                summary,
                add_dialog,
                test_action,
            } => {
                let keys: Vec<&str> = item_fields.iter().map(|x| x.key).collect();
                for k in ["name", "base", "model", "key", "prompt"] {
                    assert!(keys.contains(&k), "卡片里缺输入框 {k}，实际：{keys:?}");
                }
                assert_eq!(*summary, vec!["name", "model"]);
                assert!(*add_dialog);
                assert_eq!(*test_action, Some("test_engine_profile"));
            }
            other => panic!("应该是 list 字段，实际：{other:?}"),
        }
    }

    /// 另外两个页签也要按类别分段（用户要求「统一成按类别分类」）。
    #[test]
    fn other_sections_are_grouped_too() {
        let s = Translate.sections();
        let groups_of = |i: usize| -> Vec<&str> {
            s[i]
                .fields
                .iter()
                .filter_map(|f| match &f.kind {
                    crate::tools::FieldKind::Group { title } => Some(*title),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(groups_of(1), vec!["翻译方向", "交互"]);
        assert_eq!(groups_of(2), vec!["截图翻译"]);
    }
}
