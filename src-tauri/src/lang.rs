//! 语种检测 + 语言代码表。
//!
//! ## 为什么需要一张表
//!
//! 同一个语言有**三套代码**，而且互相不通用，这件事已经害过一次：
//!
//! | 语言 | 百度翻译 | whatlang（ISO 639-3） | 常见写法（错） |
//! |---|---|---|---|
//! | 日语 | `jp` | `jpn` | `ja` |
//! | 韩语 | `kor` | `kor` | `ko` |
//! | 法语 | `fra` | `fra` | `fr` |
//! | 西班牙语 | `spa` | `spa` | `es` |
//! | 德语 | `de` | `deu` | `de` |
//!
//! S1 的 `TO_OPTIONS` 用的就是「常见写法」那一列，导致**用户选日语/韩语/法语/
//! 西班牙语当目标语言时，百度一律返回 `58001 INVALID_TO_PARAM`**，这个 bug 一直
//! 活到 S2 才被逐码实测发现。所以现在所有语言代码只有这一个来源。
//!
//! ## 检测策略
//!
//! 本地检测（默认，`whatlang`，MIT 许可）先用**脚本启发式**兜底再问 whatlang：
//! whatlang 对短文本不可靠（"hi"、"ok" 这类几十毫秒就判错），但「含假名就是日语、
//! 含谚文就是韩语」这种判断接近 100% 准确，先判掉能省掉大部分误判。汉字放在假名
//! 之后判，因为日语里也有汉字。

use crate::config::Config;
use std::time::Duration;

/// 一个语言的三种写法。`LANGS` 是**唯一**的语言表，选项、检测映射都从它来。
pub struct Lang {
    /// 百度翻译的代码。实测有效的那些，见文件头注释
    pub baidu: &'static str,
    /// whatlang 的 ISO 639-3 代码
    pub whatlang: &'static str,
    /// 设置页下拉框和提示语里的名字
    pub name: &'static str,
}

/// 支持的语种。这一列的 `baidu` 字段**全部实测过**百度正式接口（2026-11-08），
/// 不要凭「常见写法」改。
pub const LANGS: &[Lang] = &[
    Lang {
        baidu: "zh",
        whatlang: "cmn",
        name: "中文",
    },
    Lang {
        baidu: "cht",
        whatlang: "cmn",
        name: "繁体中文",
    },
    Lang {
        baidu: "en",
        whatlang: "eng",
        name: "英语",
    },
    Lang {
        baidu: "jp",
        whatlang: "jpn",
        name: "日语",
    },
    Lang {
        baidu: "kor",
        whatlang: "kor",
        name: "韩语",
    },
    Lang {
        baidu: "fra",
        whatlang: "fra",
        name: "法语",
    },
    Lang {
        baidu: "de",
        whatlang: "deu",
        name: "德语",
    },
    Lang {
        baidu: "spa",
        whatlang: "spa",
        name: "西班牙语",
    },
    Lang {
        baidu: "ru",
        whatlang: "rus",
        name: "俄语",
    },
    Lang {
        baidu: "pt",
        whatlang: "por",
        name: "葡萄牙语",
    },
    Lang {
        baidu: "it",
        whatlang: "ita",
        name: "意大利语",
    },
    Lang {
        baidu: "th",
        whatlang: "tha",
        name: "泰语",
    },
    Lang {
        baidu: "vie",
        whatlang: "vie",
        name: "越南语",
    },
    Lang {
        baidu: "ara",
        whatlang: "ara",
        name: "阿拉伯语",
    },
    Lang {
        baidu: "nl",
        whatlang: "nld",
        name: "荷兰语",
    },
    Lang {
        baidu: "pl",
        whatlang: "pol",
        name: "波兰语",
    },
];

/// 中转站名。未知代码原样返回，免得给出空字符串。
///
/// `auto` 是"没检测出来"的兜底值（引擎自己判），它不在语言表里，
/// 但直接显示 `auto` 对用户没意义，所以单独翻成「自动」。
pub fn name_of(baidu_code: &str) -> &str {
    let c = baidu_code.trim();
    if c.eq_ignore_ascii_case("auto") {
        return "自动";
    }
    LANGS
        .iter()
        .find(|l| l.baidu.eq_ignore_ascii_case(c))
        .map(|l| l.name)
        .unwrap_or(c)
}

fn whatlang_to_baidu(code: &str) -> Option<&'static str> {
    LANGS
        .iter()
        .find(|l| l.whatlang.eq_ignore_ascii_case(code))
        .map(|l| l.baidu)
}

/// 本地语种检测，返回百度代码（好让整个项目只有一套代码在流动）。
///
/// 先脚本启发式，再 whatlang。返回 `None` 表示判不出来，调用方应当**退化成
/// 交给引擎自己检测**（就是把 `from` 传 `auto`），而不是编一个语言出来。
pub fn detect_local(text: &str) -> Option<String> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }

    // ── 脚本启发式：比 whatlang 更可靠，且对短文本免疫 ──
    // 顺序有讲究：日语含汉字，所以假名必须先于汉字判。
    // 平假名 0x3040-0x309F 与片假名 0x30A0-0x30FF 相邻，合成一个区间。
    let has_kana = t
        .chars()
        .any(|c| matches!(c as u32, 0x3040..=0x30FF));
    if has_kana {
        return Some("jp".to_string());
    }
    let has_hangul = t.chars().any(|c| {
        matches!(c as u32,
            0xAC00..=0xD7A3   // 谚文音节
            | 0x1100..=0x11FF // 谚文字母
            | 0x3130..=0x318F // 谚文兼容字母
        )
    });
    if has_hangul {
        return Some("kor".to_string());
    }
    let has_thai = t
        .chars()
        .any(|c| matches!(c as u32, 0x0E00..=0x0E7F));
    if has_thai {
        return Some("th".to_string());
    }
    let has_arabic = t
        .chars()
        .any(|c| matches!(c as u32, 0x0600..=0x06FF | 0x0750..=0x077F));
    if has_arabic {
        return Some("ara".to_string());
    }
    // 汉字（含扩展 A 与兼容区）。到这里假名已经排除，所以有汉字就是中文。
    let has_han = t.chars().any(|c| {
        matches!(c as u32,
            0x4E00..=0x9FFF   // 基本区
            | 0x3400..=0x4DBF // 扩展 A
            | 0xF900..=0xFAFF // 兼容汉字
        )
    });
    if has_han {
        return Some("zh".to_string());
    }

    // ── 拉丁/西里尔等交给 whatlang ──
    let info = whatlang::detect(t)?;
    let baidu = whatlang_to_baidu(info.lang().code())?;
    Some(baidu.to_string())
}

/// 专给**短文本**用的本地检测。
///
/// `detect_local` 对 "Hello LightBox" 这种两个词的输入会判不出来：whatlang
/// 用三元组统计，词太少时结果不可信——实测 "Hello world" 被判成荷兰语 `nl`。
/// 而短句恰恰是最常见的输入（输入翻译里用户就打一句问候语）。
///
/// 所以这里放宽一格：**短拉丁文本（字母数 < 24）除了中文/日文等脚本特征明确的，
/// 一律当英语**。这个近似对目标语言规则完全够用——规则只关心"是不是中文"，
/// 拉丁字母一律走"非中文 → 目标语言"那条路。界面上显示「英语」也比显示
/// 「荷兰语」更接近用户预期。长文本仍交给 whatlang 精细判定。
pub fn detect_local_lenient(text: &str) -> Option<String> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    // 脚本特征明确的（中日韩泰阿）先判，这些和文本长短无关
    if let Some(l) = detect_local(t) {
        let latin_letters = t.chars().filter(|c| c.is_ascii_alphabetic()).count();
        let is_latin_lang = !matches!(l.as_str(), "zh" | "cht" | "jp" | "kor" | "th" | "ara");
        if !(is_latin_lang && latin_letters < 24) {
            return Some(l);
        }
        // 短拉丁文本落到下面统一当英语
    }
    let latin = t.chars().filter(|c| c.is_ascii_alphabetic()).count();
    if latin >= 2 {
        return Some("en".to_string());
    }
    None
}

/// 网页接口会看 UA：非浏览器 UA 可能被直接拒。固定一条即可，版本号不重要。
const WEB_UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

/// 百度翻译**网页版**自己用的语种检测接口，免配置。
///
/// `POST https://fanyi.baidu.com/langdetect`，表单只有一个 `query`，返回
/// `{"error":0,"lan":"en"}`：不需要 appid / 密钥 / MD5 签名 / salt。
/// （需要签名的是开放平台那条 `/api/trans/vip/language`，本会话之前用的就是它。）
///
/// **这是未公开的网页接口**，随时可能改协议或限流，所以 `detect()` 在它失败时
/// 会自动回落本地检测——不能因为一个探测接口挂了就让整个翻译不可用。
pub fn detect_baidu(_cfg: &Config, text: &str) -> Result<String, String> {
    let q = text.trim();
    if q.is_empty() {
        return Err("没有可检测的文本".to_string());
    }
    // 网页接口没有明确长度上限，但整段贴上去只会拖慢；前 600 字足够判语种
    let q: String = q.chars().take(600).collect();

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|e| format!("网络请求失败：{e}"))?;

    let resp = client
        .post("https://fanyi.baidu.com/langdetect")
        .header("User-Agent", WEB_UA)
        .form(&[("query", q.as_str())])
        .send()
        .map_err(|e| format!("语种检测请求失败：{e}"))?;
    if !resp.status().is_success() {
        return Err(format!("语种检测返回 HTTP {}", resp.status()));
    }

    let v: serde_json::Value = resp
        .json()
        .map_err(|e| format!("语种检测返回了无法解析的内容：{e}"))?;

    // 成功: {"error":0,"msg":"success","lan":"en"}
    // 失败: {"errno":1000,"errmsg":"未知错误",...}
    let code = v.get("error").and_then(|e| e.as_i64()).unwrap_or(-1);
    if code != 0 {
        let msg = v
            .get("errmsg")
            .and_then(|m| m.as_str())
            .unwrap_or("没有错误信息");
        return Err(format!("百度在线检测返回错误 {code}：{msg}"));
    }
    let lan = v.get("lan").and_then(|l| l.as_str()).unwrap_or("");
    if lan.is_empty() {
        return Err("百度在线检测没返回语种".to_string());
    }
    baidu_web_to_code(lan)
        .map(str::to_string)
        .ok_or_else(|| format!("百度在线检测认出「{lan}」，但我们的语言表里没有这一项"))
}

/// 百度网页检测返回的 `lan` → 我们语言表里的代码。
///
/// 只收 `LANGS` 里有的那 16 种；其余（`tr`、`nor`、`yue`…）返回 `None`，
/// 由调用方回落本地检测。**绝不静默当英语**：那会让「等于第一目标语言就换
/// 第二目标」的规则误判。
fn baidu_web_to_code(lan: &str) -> Option<&'static str> {
    match lan {
        "zh" => Some("zh"),
        "cht" => Some("cht"),
        "en" => Some("en"),
        "jp" => Some("jp"),
        "kor" => Some("kor"),
        "fra" => Some("fra"),
        "spa" => Some("spa"),
        "ru" => Some("ru"),
        "de" => Some("de"),
        "it" => Some("it"),
        "pt" => Some("pt"),
        "vie" => Some("vie"),
        "th" => Some("th"),
        "ara" => Some("ara"),
        "nl" => Some("nl"),
        "pl" => Some("pl"),
        // 文言文按中文处理：判成 zh 才能走「中文 → 第二目标」那条路
        "wyw" => Some("zh"),
        _ => None,
    }
}

/// 按配置选检测方式。`lang_detect` 只认 `baidu`，其余一律本地——
/// 配置里写了不认识的值也不该让翻译挂掉。
///
/// 在线检测失败时**回落本地**，不是直接报错：探测接口不可用不该等于翻译不可用。
///
/// 本地检测用 `detect_local_lenient`：短句（"Hello LightBox"）在严格版里
/// 判不出来，会让语种规则失效；宽松版把拉丁字母兜成英语，对
/// 「源语言等于第一目标就换第二目标」这条规则来说已经足够准。
pub fn detect(cfg: &Config, text: &str) -> Result<String, String> {
    if cfg.lang_detect.trim().eq_ignore_ascii_case("baidu") {
        match detect_baidu(cfg, text) {
            Ok(v) => return Ok(v),
            Err(e) => crate::dlog::dlog(&format!("lang: 在线检测失败，回落本地（{e}）")),
        }
    }
    detect_local_lenient(text).ok_or_else(|| "本地语种检测没认出这段文字是什么语言".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_lang_row_has_unique_baidu_code() {
        let mut seen = std::collections::HashSet::new();
        for l in LANGS {
            assert!(
                seen.insert(l.baidu),
                "百度代码 {:?} 重复了，下拉框会出现两个一样的目标",
                l.baidu
            );
        }
    }

    /// 这个测试是给「选日语结果翻译失败」那个 bug 立的碑：语言表里的代码必须能
    /// 和 whatlang 代码互相映射回去。
    ///
    /// **不要求双射**：`zh` 与 `cht` 都挂 `cmn`（不管用哪个库，语种检测都分不出
    /// 简繁——whatlang 只认「这是普通话汉字」）。所以这里校验的是「映射回来落在
    /// 同一组」而不是「映射回同一行」。
    #[test]
    fn lang_table_roundtrips_whatlang_codes() {
        for l in LANGS {
            let back = whatlang_to_baidu(l.whatlang)
                .unwrap_or_else(|| panic!("{} 的 whatlang 代码 {} 映射不回去", l.name, l.whatlang));
            let row = LANGS
                .iter()
                .find(|x| x.baidu == back)
                .unwrap_or_else(|| panic!("映射结果 {back} 不在语言表里"));
            assert_eq!(
                row.whatlang, l.whatlang,
                "{} 映射到了 {}，whatlang 代码不一致",
                l.name, row.baidu
            );
        }
        // 明确这条已知限制，免得后人以为是 bug
        assert_eq!(whatlang_to_baidu("cmn"), Some("zh"), "简繁不分，cmn 一律归 zh");
    }

    #[test]
    fn common_wrong_codes_are_not_in_the_table() {
        // ja/ko/fr/es 是「常见写法」，百度不认，曾经害过 S1
        for wrong in ["ja", "ko", "fr", "es"] {
            assert!(
                !LANGS.iter().any(|l| l.baidu == wrong),
                "{wrong} 不是百度的有效代码，别加回表里"
            );
        }
    }

    #[test]
    fn name_of_translates_known_and_passes_through_unknown() {
        assert_eq!(name_of("jp"), "日语");
        assert_eq!(name_of("zh"), "中文");
        assert_eq!(name_of("xx"), "xx");
        // auto 不在语言表里，但直接显示 "auto" 用户看不懂
        assert_eq!(name_of("auto"), "自动");
        assert_eq!(name_of(" AUTO "), "自动");
    }

    /// 短句是输入翻译最常见的输入，宽松版必须认出来。
    /// 严格版对 "Hello LightBox" 会返回 None（whatlang 三元组统计在词太少时不可信），
    /// 对 "Hello world" 甚至判成荷兰语。
    #[test]
    fn lenient_detection_catches_short_latin_sentences() {
        assert_eq!(detect_local_lenient("Hello LightBox").as_deref(), Some("en"));
        assert_eq!(detect_local_lenient("Hi there").as_deref(), Some("en"));
        assert_eq!(detect_local_lenient("Hello world").as_deref(), Some("en"));
        // 中文/日文不受影响，走的还是严格版的脚本启发式
        assert_eq!(detect_local_lenient("你好").as_deref(), Some("zh"));
        assert_eq!(detect_local_lenient("こんにちは").as_deref(), Some("jp"));
    }

    /// 长拉丁文本仍交给 whatlang 精细判定，不被一律兜成英语。
    #[test]
    fn lenient_detection_keeps_long_text_precise() {
        let fr = "Bonjour le monde, ceci est une phrase assez longue pour que \
                  la detection statistique fonctionne correctement et donne du francais";
        assert_eq!(
            detect_local_lenient(fr).as_deref(),
            Some("fra"),
            "长法语文本应该被 whatlang 判成 fra"
        );
    }

    /// 宽松不等于乱猜：没有拉丁字母的字符串不能被兜成英语。
    #[test]
    fn lenient_detection_does_not_guess_without_latin_letters() {
        assert!(detect_local_lenient("12345").is_none());
        assert!(detect_local_lenient("---").is_none());
        assert!(detect_local_lenient("").is_none());
        // 单个字母太弱（"I" 这种），不判
        assert!(detect_local_lenient("I").is_none());
    }

    #[test]
    fn detect_uses_lenient_mode_for_short_text() {
        let cfg = Config::default();
        assert_eq!(detect(&cfg, "Hello world").unwrap(), "en");
    }

    // ── 本地检测的脚本启发式 ──────────────────────────────
    #[test]
    fn detect_local_recognizes_scripts() {
        assert_eq!(detect_local("你好世界").as_deref(), Some("zh"));
        assert_eq!(detect_local("こんにちは世界").as_deref(), Some("jp"));
        assert_eq!(detect_local("안녕하세요").as_deref(), Some("kor"));
        assert_eq!(detect_local("สวัสดี").as_deref(), Some("th"));
        assert_eq!(detect_local("مرحبا").as_deref(), Some("ara"));
    }

    /// 日语里既有假名也有汉字，必须先判假名，否则会被当成中文。
    #[test]
    fn detect_local_prefers_kana_over_han() {
        assert_eq!(
            detect_local("日本語のテキストです").as_deref(),
            Some("jp"),
            "带假名的日文不能判成中文"
        );
        // 纯汉字（没有假名）按中文处理——日文汉字和中文无法从字形区分
        assert_eq!(detect_local("日本語").as_deref(), Some("zh"));
    }

    #[test]
    fn detect_local_handles_latin_via_whatlang() {
        let got = detect_local("The quick brown fox jumps over the lazy dog and keeps running");
        assert_eq!(got.as_deref(), Some("en"), "实际检测到 {got:?}");
    }

    #[test]
    fn detect_local_empty_and_symbols_return_none() {
        assert!(detect_local("").is_none());
        assert!(detect_local("   ").is_none());
        // 纯符号/数字没有可用特征，不该硬猜一个语言出来
        assert!(detect_local("12345 --- !!!").is_none());
    }

    #[test]
    fn detect_prefers_local_unless_configured_baidu() {
        let cfg = Config::default();
        assert_eq!(cfg.lang_detect, "local");
        assert_eq!(detect(&cfg, "你好").unwrap(), "zh");
    }

    /// 空文本要就地拒绝，不能真去打网络。
    #[test]
    fn baidu_detect_rejects_empty_text_without_network() {
        let cfg = Config {
            lang_detect: "baidu".into(),
            ..Config::default()
        };
        let err = detect_baidu(&cfg, "   ").unwrap_err();
        assert!(err.contains("没有可检测的文本"), "实际：{err}");
    }

    /// 网页接口返回的码必须能落进我们自己的语言表——落不进去就没法参与
    /// 「等于第一目标就换第二目标」的判断。
    #[test]
    fn baidu_web_codes_map_into_our_table() {
        for l in LANGS {
            assert_eq!(
                baidu_web_to_code(l.baidu),
                Some(l.baidu),
                "语言表里的 {} 没有对应的网页码映射",
                l.baidu
            );
        }
    }

    /// 不认识的码必须返回 None 走回落，**不能兜底成英语**——
    /// 那会让中文文本被判成 en，规则整个反过来。
    #[test]
    fn unknown_baidu_codes_are_not_guessed_as_english() {
        for lan in ["tr", "nor", "yue", "xyz", "", " " ] {
            assert!(
                baidu_web_to_code(lan).is_none(),
                "{lan:?} 不该被猜成某种语言"
            );
        }
        // 文言文按中文处理（否则会走「非中文 → 第一目标」那条路）
        assert_eq!(baidu_web_to_code("wyw"), Some("zh"));
    }
}
