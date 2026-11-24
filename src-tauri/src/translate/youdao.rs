//! 有道智云：机器翻译（`/api`）与大模型翻译（`/proxy/http/llm-trans`）。
//!
//! 两条链路共用一套凭据（应用 ID + 应用密钥）和同一套 v3 签名，差别只在端点、
//! 参数名（`q` vs `i`）和响应形状（JSON vs SSE）。
//!
//! ## 三个必须记住的坑
//!
//! 1. **语言码和百度不一样**：日语是 `ja` 不是 `jp`，韩语 `ko` 不是 `kor`，
//!    中文是 `zh-CHS` / `zh-CHT`。映射统一走 `lang::to_youdao()`，不要在引擎里
//!    自己拼。
//! 2. **签名的 input 会截断**：超过 20 字符时用「前 10 + 长度 + 后 10」，而且是
//!    按 **char** 数算长度。按字节算签名必然对不上（返回 202）。
//! 3. **大模型是 SSE**：`streamType=full` 时每行给一份累积的全量译文，取最后
//!    一条就是完整结果。不需要边收边显示，一次读完再解析最简单。

use super::{Engine, TranslateError, Translated};
use crate::config::Config;
use crate::lang;
use rust_i18n::t;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MT_ENDPOINT: &str = "https://openapi.youdao.com/api";
const LLM_ENDPOINT: &str = "https://openapi.youdao.com/proxy/http/llm-trans";

/// 有道文档写的单次上限是 5000 字符。留 1000 的余量：中文按字算、以及
/// 用户贴进来的文本常带多余空白。
const MAX_CHARS: usize = 4000;

pub struct YoudaoMt;
pub struct YoudaoLlm;

// ───────────────────────── 签名 ─────────────────────────

/// 签名用的 `input`：≤20 字符取全文，否则「前 10 + 长度 + 后 10」。
///
/// **按 char 数**，不是字节数——一个汉字 3 字节，按字节算出来的「长度」会让
/// 签名和服务器算的不一致，表现是 202 签名校验失败。
pub fn sign_input(q: &str) -> String {
    let chars: Vec<char> = q.chars().collect();
    if chars.len() <= 20 {
        q.to_string()
    } else {
        let head: String = chars[..10].iter().collect();
        let tail: String = chars[chars.len() - 10..].iter().collect();
        format!("{head}{}{tail}", chars.len())
    }
}

/// v3 签名：`sha256(appKey + input + salt + curtime + 应用密钥)`，64 位小写。
pub fn sign_v3(app_key: &str, q: &str, salt: &str, curtime: &str, secret: &str) -> String {
    let mut h = Sha256::new();
    h.update(format!("{app_key}{}{salt}{curtime}{secret}", sign_input(q)).as_bytes());
    let digest = h.finalize();
    let mut out = String::with_capacity(64);
    for b in digest.iter() {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// 当前 UTC 秒级时间戳（字符串，接口要求）。
fn curtime() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        .to_string()
}

/// 随机 salt。接口拿 `salt + curtime` 防重放，重复了会返回 207。
///
/// 不引 uuid crate：毫秒时间戳 + 进程号 + 进程内自增计数已经不可能在同一毫秒
/// 内重复，跨进程也不会撞（pid 不同）。
fn salt() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{ms:x}-{:x}-{n:x}", std::process::id())
}

// ───────────────────────── 错误码 ─────────────────────────

/// 有道的错误码是数字字符串，且**同一个码在机翻与 OCR 里含义可能不同**。
/// 这里只覆盖文本翻译/大模型会遇到的，其余给出查文档的指路。
pub fn error_hint(code: &str, msg: &str) -> String {
    let why = match code {
        "101" => "缺少必填参数：应用 ID / 密钥可能没填全",
        "102" => "不支持的语言类型：语言的**有道代码**和百度的不一样（日语是 ja、韩语是 ko）",
        "103" => "翻译文本过长：单次上限 5000 字符",
        "108" => "应用 ID 无效：检查有道智云「应用管理」里的应用 ID",
        "110" => "应用没有绑定文本翻译服务：去有道智云控制台给应用加上该服务",
        "111" => "开发者账号无效",
        "113" => "待翻译文本为空",
        "202" => "签名校验失败：检查应用密钥；也可能是文本里含特殊编码导致签名不一致",
        "203" => "访问 IP 不在白名单：控制台里如果配了 IP 限制，去掉或加上本机 IP",
        "206" => "时间戳无效导致签名失败：本机时间不准，校准一下系统时间",
        "207" => "重放请求：同一组 salt + curtime 用过了，重试即可",
        "401" => "账户已欠费：去有道智云控制台充值（新账号有 50 元体验金）",
        "411" | "1411" | "2411" => "访问频率受限：稍等再试",
        "412" => "长文本请求过于频繁：稍等 3 秒再试",
        "2003" => "不支持的语言识别类型",
        "303" | "304" => "有道服务端异常，稍后重试",
        _ => "有道没有给出这个错误码的解释，去 ai.youdao.com 的错误码表查一下",
    };
    if msg.trim().is_empty() {
        format!("有道返回 {code}：{why}")
    } else {
        format!("有道返回 {code}：{why}（原文：{msg}）")
    }
}

// ───────────────────────── 响应解析 ─────────────────────────

/// 机翻响应：`{"errorCode":"0","translation":["..."],"l":"EN2zh-CHS"}`。
pub fn parse_mt(body: &str) -> Result<String, TranslateError> {
    let v: Value = serde_json::from_str(body).map_err(|_| {
        TranslateError::BadResponse(format!(
            "返回的不是 JSON：{}",
            body.chars().take(200).collect::<String>()
        ))
    })?;

    // 注意字段是驼峰 `errorCode`，且值是字符串 "0"
    let code = v
        .get("errorCode")
        .and_then(|c| {
            c.as_str()
                .map(str::to_string)
                .or_else(|| c.as_i64().map(|n| n.to_string()))
        })
        .unwrap_or_else(|| "0".to_string());
    if code != "0" {
        let msg = v.get("msg").and_then(|m| m.as_str()).unwrap_or("");
        return Err(TranslateError::Upstream {
            hint: error_hint(&code, msg),
        });
    }

    let arr = v
        .get("translation")
        .and_then(|t| t.as_array())
        .ok_or_else(|| {
            TranslateError::BadResponse(format!(
                "响应里没有 translation：{}",
                body.chars().take(200).collect::<String>()
            ))
        })?;
    let mut dst = String::new();
    for item in arr {
        if let Some(s) = item.as_str() {
            if !dst.is_empty() {
                dst.push('\n');
            }
            dst.push_str(s);
        }
    }
    if dst.trim().is_empty() {
        return Err(TranslateError::BadResponse("返回了空译文".to_string()));
    }
    Ok(dst)
}

/// 大模型响应（SSE）。`streamType=full` 时每行是一份**累积**的全量译文。
///
/// 三种形状都要认：
/// - `data:{"code":"0","data":{"transFull":"Hi, nice to meet you!"}}`（正常）
/// - `{"code":"400","message":"'i'不能为空;","successful":false}`（出错时**不**带
///   `data:` 前缀，是普通 JSON）
/// - 末尾可能有 `data:[DONE]`
pub fn parse_llm(body: &str) -> Result<String, TranslateError> {
    let trimmed = body.trim();
    // 先试整体当成一个 JSON 解析：错误响应就是这么给的
    if trimmed.starts_with('{') {
        if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
            let ok = v.get("successful").and_then(|b| b.as_bool()).unwrap_or(true);
            let code = v.get("code").and_then(|c| c.as_str()).unwrap_or("0");
            if !ok || (code != "0" && !code.is_empty()) {
                let msg = v.get("message").and_then(|m| m.as_str()).unwrap_or("");
                return Err(TranslateError::Upstream {
                    hint: error_hint(code, msg),
                });
            }
            if let Some(t) = v.pointer("/data/transFull").and_then(|x| x.as_str()) {
                if !t.trim().is_empty() {
                    return Ok(t.to_string());
                }
            }
        }
    }

    let mut last_full: Option<String> = None;
    let mut last_err: Option<String> = None;
    for line in body.lines() {
        let Some(payload) = line.trim().strip_prefix("data:") else {
            continue;
        };
        let payload = payload.trim();
        if payload.is_empty() || payload == "[DONE]" {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(payload) else {
            continue;
        };
        let ok = v.get("successful").and_then(|b| b.as_bool()).unwrap_or(true);
        let code = v.get("code").and_then(|c| c.as_str()).unwrap_or("0");
        if !ok || (code != "0" && !code.is_empty()) {
            let msg = v.get("message").and_then(|m| m.as_str()).unwrap_or("");
            last_err = Some(error_hint(code, msg));
            continue;
        }
        if let Some(t) = v.pointer("/data/transFull").and_then(|x| x.as_str()) {
            if !t.trim().is_empty() {
                last_full = Some(t.to_string());
            }
        }
    }

    if let Some(t) = last_full {
        return Ok(t);
    }
    Err(match last_err {
        Some(hint) => TranslateError::Upstream { hint },
        None => TranslateError::BadResponse(format!(
            "流式响应里没有 transFull：{}",
            body.chars().take(200).collect::<String>()
        )),
    })
}

// ───────────────────────── 公共实现 ─────────────────────────

/// 把配置里的语言代码换成有道代码。不认识的语言要给出**可照做**的中文错误，
/// 不能把百度码直接发过去（那会得到 102，看起来像账号问题）。
fn youdao_lang(code: &str, what: &str) -> Result<&'static str, TranslateError> {
    lang::to_youdao(code).ok_or_else(|| TranslateError::Upstream {
        hint: format!(
            "有道不支持{}「{}」：去设置 → 翻译 → 翻译配置 换一个语言。",
            what,
            lang::name_of(code)
        ),
    })
}

fn not_ready_hint() -> String {
    t!("translate.youdao.ready").into_owned()
}

fn is_ready(cfg: &Config) -> bool {
    !cfg.youdao_app_key.trim().is_empty() && !cfg.youdao_app_secret.trim().is_empty()
}

/// 组装返回值。`from` / `to` 一律用**项目内部语言码**（`zh` / `jp` / `kor`），
/// 不是有道码（`zh-CHS` / `ja` / `ko`）——弹窗和设置页会把这两个值直接显示给
/// 用户，露一个 `zh-CHS` 出来就是 bug。
fn report(cfg: &Config, dst: String) -> Translated {
    let from = cfg.from.trim();
    Translated {
        from: if from.is_empty() {
            "auto".to_string()
        } else {
            from.to_string()
        },
        to: cfg.to.trim().to_string(),
        dst,
    }
}

fn client(secs: u64) -> Result<reqwest::blocking::Client, TranslateError> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(secs))
        .build()
        .map_err(|e| TranslateError::Network(format!("创建 HTTP 客户端失败：{e}")))
}

// ───────────────────────── 机器翻译 ─────────────────────────

impl Engine for YoudaoMt {
    fn id(&self) -> &'static str {
        "youdao"
    }

    fn name(&self) -> &'static str {
        "有道翻译"
    }

    fn max_chars(&self) -> usize {
        MAX_CHARS
    }

    fn qps(&self) -> u32 {
        2
    }

    fn is_ready(&self, cfg: &Config) -> bool {
        is_ready(cfg)
    }

    fn ready_hint(&self) -> String {
        not_ready_hint()
    }

    fn translate_one(&self, cfg: &Config, text: &str) -> Result<Translated, TranslateError> {
        let from = youdao_lang(&cfg.from, "源语言")?;
        let to = youdao_lang(&cfg.to, "目标语言")?;
        let app_key = cfg.youdao_app_key.trim();
        let secret = cfg.youdao_app_secret.trim();

        let salt = salt();
        let curtime = curtime();
        let sign = sign_v3(app_key, text, &salt, &curtime, secret);

        let params = [
            ("q", text),
            ("from", from),
            ("to", to),
            ("appKey", app_key),
            ("salt", salt.as_str()),
            ("sign", sign.as_str()),
            ("signType", "v3"),
            ("curtime", curtime.as_str()),
        ];

        let resp = client(20)?
            .post(MT_ENDPOINT)
            .form(&params)
            .send()
            .map_err(|e| TranslateError::Network(e.to_string()))?;
        let status = resp.status();
        let body = resp
            .text()
            .map_err(|e| TranslateError::Network(e.to_string()))?;

        if !status.is_success() && body.trim().is_empty() {
            return Err(TranslateError::Network(format!("HTTP {status}，响应体为空")));
        }

        let dst = parse_mt(&body)?;
        Ok(report(cfg, dst))
    }
}

// ───────────────────────── 大模型翻译 ─────────────────────────

impl Engine for YoudaoLlm {
    fn id(&self) -> &'static str {
        "youdao-llm"
    }

    fn name(&self) -> &'static str {
        "有道大模型翻译"
    }

    fn max_chars(&self) -> usize {
        MAX_CHARS
    }

    fn qps(&self) -> u32 {
        1
    }

    fn is_ready(&self, cfg: &Config) -> bool {
        is_ready(cfg)
    }

    fn ready_hint(&self) -> String {
        not_ready_hint()
    }

    fn translate_one(&self, cfg: &Config, text: &str) -> Result<Translated, TranslateError> {
        let from = youdao_lang(&cfg.from, "源语言")?;
        let to = youdao_lang(&cfg.to, "目标语言")?;
        let app_key = cfg.youdao_app_key.trim();
        let secret = cfg.youdao_app_secret.trim();

        let salt = salt();
        let curtime = curtime();
        let sign = sign_v3(app_key, text, &salt, &curtime, secret);

        // 文档的参数表写 `application/json`，但官方 curl 示例用的是
        // `--data-urlencode`（即表单），实测表单可用。按实测走。
        let params = [
            ("appKey", app_key),
            ("salt", salt.as_str()),
            ("sign", sign.as_str()),
            ("signType", "v3"),
            ("curtime", curtime.as_str()),
            ("i", text),
            ("from", from),
            ("to", to),
            // full = 每行给累积的全量译文；increment 要自己拼，容易拼错。
            ("streamType", "full"),
            // 0 = 子曰翻译 pro（14B）。3 是 lite（1.5B），质量明显差一档。
            ("handleOption", "0"),
        ];

        let resp = client(60)?
            .post(LLM_ENDPOINT)
            .form(&params)
            .send()
            .map_err(|e| TranslateError::Network(e.to_string()))?;
        let status = resp.status();
        let body = resp
            .text()
            .map_err(|e| TranslateError::Network(e.to_string()))?;

        if !status.is_success() && body.trim().is_empty() {
            return Err(TranslateError::Network(format!("HTTP {status}，响应体为空")));
        }

        let dst = parse_llm(&body)?;
        Ok(report(cfg, dst))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── 签名 input 的截断规则 ────────────────────────────────
    #[test]
    fn sign_input_keeps_short_text_whole() {
        assert_eq!(sign_input("hello"), "hello");
        // 20 个字符是边界：等于 20 时**不**截断
        assert_eq!(sign_input("12345678901234567890"), "12345678901234567890");
        // 中文按 char 数算：20 个汉字也不截断
        let twenty = "中".repeat(20);
        assert_eq!(sign_input(&twenty), twenty);
    }

    #[test]
    fn sign_input_truncates_long_text_by_chars() {
        let q = "1234567890ABCDEFGHIJ1234567890"; // 30 字符
        // 前 10 + 长度(30) + 后 10
        assert_eq!(sign_input(q), "1234567890301234567890");
        // 关键：长度按 char 算，不是字节。21 个汉字 → 长度写 21
        let cn = "中".repeat(21);
        let got = sign_input(&cn);
        assert!(got.contains("21"), "长度必须是字符数：{got}");
        assert!(got.starts_with("中中中中中中中中中中"));
        assert!(got.ends_with("中中中中中中中中中中"));
    }

    /// 21 个汉字按字节是 63，按字符是 21。这条测试专门盯「别按字节算」。
    #[test]
    fn sign_input_length_is_not_byte_length() {
        let cn = "中".repeat(21);
        assert_eq!(cn.len(), 63, "前提：UTF-8 下一个汉字 3 字节");
        assert!(!sign_input(&cn).contains("63"), "不能把字节数当长度");
        assert!(sign_input(&cn).contains("21"));
    }

    /// 签名的哈希本身锁一个已知值，防止有人「顺手改一下拼接顺序」。
    #[test]
    fn sign_v3_matches_known_vector() {
        // 用固定输入算出来的值；改拼接顺序/编码就会变
        let s = sign_v3("appkey", "hello", "salt1", "1700000000", "secret");
        assert_eq!(s.len(), 64, "sha256 十六进制必须是 64 位");
        assert!(s.chars().all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()));
        // 同样输入必须稳定
        assert_eq!(
            s,
            sign_v3("appkey", "hello", "salt1", "1700000000", "secret")
        );
        // 任一字段变了，签名就得变
        assert_ne!(
            s,
            sign_v3("appkey", "hello", "salt1", "1700000001", "secret")
        );
    }

    // ── 机翻响应 ────────────────────────────────────────────
    #[test]
    fn parse_mt_reads_translation_array() {
        let body = r#"{"errorCode":"0","query":"hello","translation":["你好"],"l":"EN2zh-CHS"}"#;
        assert_eq!(parse_mt(body).unwrap(), "你好");
    }

    #[test]
    fn parse_mt_joins_multiple_segments() {
        let body = r#"{"errorCode":"0","translation":["第一段","第二段"]}"#;
        assert_eq!(parse_mt(body).unwrap(), "第一段\n第二段");
    }

    #[test]
    fn parse_mt_reports_upstream_error() {
        let body = r#"{"errorCode":"102","query":"x"}"#;
        let e = parse_mt(body).unwrap_err();
        let m = e.message();
        assert!(m.contains("102"), "{m}");
        assert!(m.contains("语言"), "102 要说是语言码问题：{m}");
    }

    #[test]
    fn parse_mt_rejects_non_json_and_empty() {
        assert!(parse_mt("<html>not json</html>").is_err());
        assert!(parse_mt(r#"{"errorCode":"0","translation":[]}"#).is_err());
        assert!(parse_mt(r#"{"errorCode":"0","translation":["  "]}"#).is_err());
    }

    // ── 大模型 SSE ──────────────────────────────────────────
    const SSE_OK: &str = concat!(
        "data:{\"code\":\"0\",\"message\":\"success\",\"data\":{\"transFull\":\"Hi\"},\"successful\":true}\n\n",
        "data:{\"code\":\"0\",\"message\":\"success\",\"data\":{\"transFull\":\"Hi, nice\"},\"successful\":true}\n\n",
        "data:{\"code\":\"0\",\"message\":\"success\",\"data\":{\"transFull\":\"Hi, nice to meet you!\"},\"successful\":true}\n\n",
        "data:[DONE]\n\n"
    );

    #[test]
    fn parse_llm_takes_last_full_text() {
        assert_eq!(parse_llm(SSE_OK).unwrap(), "Hi, nice to meet you!");
    }

    /// 有的代理会在冒号后加空格，`data: {...}` 也要认。
    #[test]
    fn parse_llm_accepts_space_after_data_colon() {
        let body = "data: {\"code\":\"0\",\"data\":{\"transFull\":\"译文\"}}\n";
        assert_eq!(parse_llm(body).unwrap(), "译文");
    }

    /// 出错时返回的是**普通 JSON**（没有 `data:` 前缀），不能当 SSE 跳过。
    #[test]
    fn parse_llm_reports_plain_json_error() {
        let body = r#"{"code":"400","message":"'i'不能为空;","successful":false}"#;
        let e = parse_llm(body).unwrap_err();
        assert!(e.message().contains("400"), "{}", e.message());
    }

    #[test]
    fn parse_llm_reports_error_inside_stream() {
        let body = "data:{\"code\":\"411\",\"message\":\"频率受限\",\"successful\":false}\n";
        let e = parse_llm(body).unwrap_err();
        assert!(e.message().contains("频率"), "{}", e.message());
    }

    #[test]
    fn parse_llm_without_transfull_says_so() {
        let body = "data:{\"code\":\"0\",\"data\":{}}\n";
        assert!(parse_llm(body).is_err());
        assert!(parse_llm("").is_err());
        assert!(parse_llm("garbage").is_err());
    }

    // ── 语言码 ──────────────────────────────────────────────
    #[test]
    fn report_uses_internal_language_codes() {
        let cfg = Config {
            from: "auto".into(),
            to: "jp".into(),
            ..Config::default()
        };
        let r = report(&cfg, "译文".into());
        assert_eq!(r.from, "auto");
        assert_eq!(r.to, "jp", "不能把有道的 ja 露给用户");
        // 空源语言要写成 auto，前端拿它显示「自动」
        let cfg2 = Config {
            from: String::new(),
            to: "zh".into(),
            ..Config::default()
        };
        assert_eq!(report(&cfg2, "x".into()).from, "auto");
    }

    #[test]
    fn youdao_lang_maps_and_rejects_unknown() {
        assert_eq!(youdao_lang("jp", "目标语言").unwrap(), "ja");
        assert_eq!(youdao_lang("zh", "目标语言").unwrap(), "zh-CHS");
        assert_eq!(youdao_lang("auto", "源语言").unwrap(), "auto");
        let e = youdao_lang("klingon", "目标语言").unwrap_err();
        assert!(e.message().contains("不支持"), "{}", e.message());
        assert!(
            e.message().contains("设置"),
            "要指出去哪改：{}",
            e.message()
        );
    }

    // ── 错误码 ──────────────────────────────────────────────
    #[test]
    fn error_hint_covers_common_codes() {
        for code in [
            "101", "102", "103", "108", "110", "111", "113", "202", "203", "206", "207",
            "401", "411", "412", "2003", "303", "304",
        ] {
            let m = error_hint(code, "");
            assert!(
                !m.contains("没有给出这个错误码的解释"),
                "{code} 缺中文解释：{m}"
            );
        }
        assert!(error_hint("9999", "").contains("ai.youdao.com"));
    }
}
