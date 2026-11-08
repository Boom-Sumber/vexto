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
use crate::translate::md5_hex;
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

/// 是不是中文。繁体也算——「中文→第二目标语言」这条规则不该因为繁体而失效。
///
/// 大小写不敏感：配置是手写 JSON，用户可能填成 `ZH`。
pub fn is_chinese(baidu_code: &str) -> bool {
    let c = baidu_code.trim();
    c.eq_ignore_ascii_case("zh") || c.eq_ignore_ascii_case("cht")
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

/// 百度语种检测接口。
///
/// 文档：<https://fanyi-api.baidu.com/doc/25>，返回的是**百度自己的代码**
/// （`zh` / `en` / `jp` / `kor` / `th` / `vie` / `ru`），正好和翻译共用一套。
///
/// 实测局限（2026-11-08）：只支持 7 种语言，**法语/德语/西班牙语会被判成 `en`**。
/// 对本项目的流程影响不大——它们都是「非中文」，同样走目标语言那一条路。
pub fn detect_baidu(cfg: &Config, text: &str) -> Result<String, String> {
    if cfg.appid.trim().is_empty() || cfg.secret.trim().is_empty() {
        return Err("语种检测选了百度，但还没填百度翻译的 APPID / 密钥。\n右键托盘图标 → 设置… → 翻译 → 引擎。".to_string());
    }
    let q = text.trim();
    if q.is_empty() {
        return Err("没有可检测的文本".to_string());
    }
    // 接口限制单次 2000 bytes；这里按字符粗切（中文 3 字节/字）足够保守
    let q: String = q.chars().take(600).collect();

    let salt = salt();
    let sign = md5_hex(&format!("{}{}{}{}", cfg.appid, q, salt, cfg.secret));
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|e| format!("网络请求失败：{e}"))?;

    let resp = client
        .post("https://fanyi-api.baidu.com/api/trans/vip/language")
        .form(&[
            ("q", q.as_str()),
            ("appid", cfg.appid.as_str()),
            ("salt", salt.as_str()),
            ("sign", sign.as_str()),
        ])
        .send()
        .map_err(|e| format!("语种检测请求失败：{e}"))?;

    let v: serde_json::Value = resp
        .json()
        .map_err(|e| format!("语种检测返回了无法解析的内容：{e}"))?;

    // error_code 有时是数字 0、有时是字符串 "54001"，两种都要认
    let code_ok = match v.get("error_code") {
        Some(serde_json::Value::Number(n)) => n.as_i64() == Some(0),
        Some(serde_json::Value::String(s)) => s == "0",
        None => true,
        _ => false,
    };
    if !code_ok {
        let code = v
            .get("error_code")
            .and_then(|c| {
                c.as_str()
                    .map(|s| s.to_string())
                    .or_else(|| c.as_i64().map(|n| n.to_string()))
            })
            .unwrap_or_else(|| "未知".to_string());
        let msg = v
            .get("error_msg")
            .and_then(|m| m.as_str())
            .unwrap_or("没有错误信息");
        return Err(format!(
            "百度语种检测返回 {code}：{msg}\n{}",
            detect_error_hint(&code)
        ));
    }

    v.pointer("/data/src")
        .and_then(|s| s.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "语种检测返回里没有 data.src".to_string())
}

/// 检测接口的错误码。和翻译接口不完全一样（多了 `54009` 语种检测失败）。
fn detect_error_hint(code: &str) -> &'static str {
    match code {
        "52001" => "请求超时，重试一次。",
        "52002" => "百度服务端出错，重试一次。",
        "52003" => "未授权用户。检查 APPID 填错了，或服务没开通。",
        "54000" => "必填参数为空。文本太短或为空时会出现。",
        "54001" => "签名错误。检查密钥是否正确。",
        "54003" => "访问频率受限（标准版 QPS=1），等一秒再试。",
        "54004" => "账户余额不足，去百度翻译控制台充值。",
        "54009" => "语种检测失败：这段文字不在百度支持的 7 种语言里。",
        "58000" => "客户端 IP 非法。百度控制台里如果填了 IP 白名单，检查一下。",
        "58002" => "服务当前已关闭，去百度翻译控制台开启。",
        _ => "百度没有给出这个错误码的解释，去控制台的错误码表查一下。",
    }
}

fn salt() -> String {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    (ms % 100_000_000_000).to_string()
}

/// 按配置选检测方式。`lang_detect` 只认 `baidu`，其余一律本地——
/// 配置里就算写了不认识的值也不该让翻译挂掉。
///
/// 本地检测用 `detect_local_lenient`：短句（"Hello LightBox"）在严格版里
/// 判不出来，会让语种规则失效；宽松版把拉丁字母兜成英语，对
/// 「非中文 → 目标语言」这条规则来说已经足够准。
pub fn detect(cfg: &Config, text: &str) -> Result<String, String> {
    if cfg.lang_detect.trim().eq_ignore_ascii_case("baidu") {
        detect_baidu(cfg, text)
    } else {
        detect_local_lenient(text)
            .ok_or_else(|| "本地语种检测没认出这段文字是什么语言".to_string())
    }
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
    fn is_chinese_accepts_both_scripts() {
        assert!(is_chinese("zh"));
        assert!(is_chinese("cht"));
        assert!(is_chinese("ZH"));
        assert!(!is_chinese("en"));
        assert!(!is_chinese("jp"));
        assert!(!is_chinese(""));
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

    #[test]
    fn baidu_detect_without_credentials_says_where_to_fill() {
        let cfg = Config {
            lang_detect: "baidu".into(),
            appid: String::new(),
            secret: String::new(),
            ..Config::default()
        };
        let err = detect_baidu(&cfg, "hello").unwrap_err();
        assert!(err.contains("APPID"), "要指出缺什么：{err}");
        assert!(err.contains("设置"), "要指出去哪填：{err}");
    }

    #[test]
    fn detect_error_hints_cover_documented_codes() {
        for code in [
            "52001", "52002", "52003", "54000", "54001", "54003", "54004", "54009", "58000",
            "58002",
        ] {
            let h = detect_error_hint(code);
            assert!(
                !h.contains("没有给出这个错误码的解释"),
                "{code} 还没有中文解释：{h}"
            );
        }
    }
}
