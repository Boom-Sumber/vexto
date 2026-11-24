//! 百度翻译：通用文本翻译（机器翻译）与大模型翻译。
//!
//! 两个端点、一套签名：
//! - 机器翻译 `POST /api/trans/vip/translate`
//! - 大模型   `POST /ait/api/aiTextTranslate`，多传 `model_type=llm`
//!
//! 三条硬规矩（错了就是 54001 签名错误或 54003 频率受限）：
//! 1. `sign = MD5(appid + q + salt + 密钥)`，32 位小写；**拼接时 q 不做 URL encode**。
//! 2. POST 必须 `application/x-www-form-urlencoded` —— `reqwest` 的 `.form()` 即是。
//! 3. 标准版 QPS = 1，分块请求之间必须限速（由上层 `translate()` 统一 sleep）。
//!
//! 两个端点的语言代码是**同一套**（`zh` / `cht` / `jp` / `kor` / `fra`…），
//! 所以这里不需要像有道那样换码。

use super::{md5_hex, now_ms, Engine, TranslateError, Translated};
use crate::config::Config;
use rust_i18n::t;
use serde_json::Value;
use std::time::Duration;

const MT_ENDPOINT: &str = "https://fanyi-api.baidu.com/api/trans/vip/translate";
const LLM_ENDPOINT: &str = "https://fanyi-api.baidu.com/ait/api/aiTextTranslate";

pub struct Baidu;
pub struct BaiduLlm;

/// 把百度的错误码翻译成用户能照做的中文。
///
/// 直接把 `error_msg` 甩给用户等于没说（"UNAUTHORIZED USER" 用户看不懂），
/// 所以必须逐个码给出「为什么 + 怎么办」。
pub fn error_hint(code: &str) -> String {
    let msg = match code {
        "52001" => "请求超时：文本可能过长，或 from/to 语种代码不在支持列表里",
        "52002" => "百度侧系统错误，稍后重试",
        "52003" => "未授权用户：APPID 不正确，或对应翻译服务未开通",
        "54000" => "必填参数为空：检查 APPID / 密钥是否留空",
        "54001" => "签名错误：通常是密钥不对，或文本在拼接前被 URL encode 过；用 API Key 鉴权时则是 Key 不对",
        "54003" => "访问频率受限：标准版 QPS 为 1，请稍等再试（或去认证升级）",
        "54004" => "账户余额不足：文档说标准版有免费额度，去控制台确认",
        "54005" => "长 query 请求过于频繁：稍等 3 秒再试",
        "58000" => "客户端 IP 非法：开发者信息页填了固定 IP，但当前不是那个 IP",
        "58001" => "译文语言方向不支持：检查目标语种代码（非常用语种需企业尊享版）",
        "58002" => "服务当前已关闭：去管理控制台开启对应服务",
        "58003" => "此 IP 已被封禁：同一 IP 当日用过多个 APPID，次日解封",
        "58004" => "模型参数错误：`model_type` 只能是 `llm` 或 `nmt`（大模型翻译专用）",
        "59002" => "翻译指令过长：自定义指令上限 500 字符",
        "59003" => "请求文本过长：大模型翻译单次上限 6000 字符",
        "59004" => "QPS 超限：大模型翻译接口的频率限制，等一秒再试",
        "59005" => "标签保持参数非法：只能是 0 或 1",
        "59006" => "标签解析失败：文本里的标签没有闭合",
        "59007" => "忽略标签数量超限：最多 20 个",
        "90107" => "认证未通过或未生效",
        "20003" => "请求内容存在安全风险",
        _ => "未知错误码，去百度翻译控制台查错误码表",
    };
    format!("百度返回 {code}：{msg}")
}

/// 解析两个端点共用的成功/失败结构：`trans_result[].dst`，错误时是 `error_code`。
pub fn parse_trans_result(
    body: &str,
    status: reqwest::StatusCode,
    from: &str,
    to: &str,
) -> Result<Translated, TranslateError> {
    let v: Value = serde_json::from_str(body).map_err(|_| {
        TranslateError::BadResponse(format!(
            "HTTP {status}，前 200 字符：{}",
            body.chars().take(200).collect::<String>()
        ))
    })?;

    // error_code 可能是字符串也可能是数字，两路都处理
    let err_code = v.get("error_code").and_then(|c| {
        c.as_str()
            .map(|s| s.to_string())
            .or_else(|| c.as_i64().map(|n| n.to_string()))
    });

    if let Some(code) = err_code {
        // 52000 表示"成功"，是百度历史遗留的怪癖，不能当错误
        if code != "52000" {
            return Err(TranslateError::Upstream {
                hint: error_hint(&code),
            });
        }
    }

    let arr = v
        .get("trans_result")
        .and_then(|a| a.as_array())
        .ok_or_else(|| {
            TranslateError::BadResponse(format!(
                "响应里没有 trans_result：{}",
                body.chars().take(200).collect::<String>()
            ))
        })?;

    let mut dst = String::new();
    for item in arr {
        if let Some(d) = item.get("dst").and_then(|d| d.as_str()) {
            if !dst.is_empty() {
                dst.push('\n');
            }
            dst.push_str(d);
        }
    }

    if dst.trim().is_empty() {
        return Err(TranslateError::BadResponse("返回了空译文".to_string()));
    }

    Ok(Translated {
        from: v
            .get("from")
            .and_then(|f| f.as_str())
            .unwrap_or(from)
            .to_string(),
        to: v
            .get("to")
            .and_then(|t| t.as_str())
            .unwrap_or(to)
            .to_string(),
        dst,
    })
}

/// 两个端点共用的请求逻辑。`model_type` 为 `None` 时不传（机器翻译接口不认这个参数）。
fn call(
    cfg: &Config,
    text: &str,
    endpoint: &str,
    model_type: Option<&str>,
) -> Result<Translated, TranslateError> {
    let salt = format!("{}", now_ms() % 100_000_000_000);
    let sign = md5_hex(&format!("{}{}{}{}", cfg.appid, text, salt, cfg.secret));

    let from = if cfg.from.trim().is_empty() {
        "auto"
    } else {
        cfg.from.trim()
    };
    let to = if cfg.to.trim().is_empty() {
        "zh"
    } else {
        cfg.to.trim()
    };

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| TranslateError::Network(format!("创建 HTTP 客户端失败：{e}")))?;

    // 用 Vec 而不是定长数组：大模型要多塞一个 model_type
    let mut params: Vec<(&str, &str)> = vec![
        ("q", text),
        ("from", from),
        ("to", to),
        ("appid", cfg.appid.trim()),
        ("salt", salt.as_str()),
        ("sign", sign.as_str()),
    ];
    if let Some(m) = model_type {
        params.push(("model_type", m));
    }

    let resp = client
        .post(endpoint)
        .form(&params)
        .send()
        .map_err(|e| TranslateError::Network(e.to_string()))?;

    let status = resp.status();
    let body = resp
        .text()
        .map_err(|e| TranslateError::Network(e.to_string()))?;

    parse_trans_result(&body, status, from, to)
}

fn ready_hint() -> String {
    t!("translate.baidu.ready").into_owned()
}

fn is_ready(cfg: &Config) -> bool {
    !cfg.appid.trim().is_empty() && !cfg.secret.trim().is_empty()
}

impl Engine for Baidu {
    fn id(&self) -> &'static str {
        "baidu"
    }

    fn name(&self) -> &'static str {
        "百度翻译"
    }

    fn max_chars(&self) -> usize {
        2000
    }

    fn qps(&self) -> u32 {
        1
    }

    fn is_ready(&self, cfg: &Config) -> bool {
        is_ready(cfg)
    }

    fn ready_hint(&self) -> String {
        ready_hint()
    }

    fn translate_one(&self, cfg: &Config, text: &str) -> Result<Translated, TranslateError> {
        call(cfg, text, MT_ENDPOINT, None)
    }
}

impl Engine for BaiduLlm {
    fn id(&self) -> &'static str {
        "baidu-llm"
    }

    fn name(&self) -> &'static str {
        "百度大模型翻译"
    }

    /// 文档建议单次 2000 字符以内（上限 6000，但长了质量会掉）。
    fn max_chars(&self) -> usize {
        2000
    }

    fn qps(&self) -> u32 {
        1
    }

    fn is_ready(&self, cfg: &Config) -> bool {
        is_ready(cfg)
    }

    fn ready_hint(&self) -> String {
        ready_hint()
    }

    fn translate_one(&self, cfg: &Config, text: &str) -> Result<Translated, TranslateError> {
        call(cfg, text, LLM_ENDPOINT, Some("llm"))
    }
}

#[cfg(test)]
mod tests {
    use super::{error_hint, parse_trans_result};

    // ── 错误码：文档列出的每个码都得有中文解释 ───────────────────
    #[test]
    fn error_hint_covers_all_documented_codes() {
        for code in [
            "52001", "52002", "52003", "54000", "54001", "54003", "54004", "54005", "58000",
            "58001", "58002", "58003", "58004", "59002", "59003", "59004", "59005", "59006",
            "59007", "90107", "20003",
        ] {
            let msg = error_hint(code);
            assert!(msg.contains("百度返回"), "code {code} 没有中文解释");
            assert!(!msg.contains("未知错误"), "code {code} 缺解释");
        }
    }

    // ── 错误码：未知码也不能说"未知错误"就完事 ───────────────────
    #[test]
    fn error_hint_unknown_code_tells_user_where_to_look() {
        let msg = error_hint("99999");
        assert!(msg.contains("控制台"), "未知码要指路，实际：{msg}");
    }

    // ── 响应解析（两个端点共用）────────────────────────────────
    #[test]
    fn parse_reads_trans_result_and_falls_back_to_request_langs() {
        let body = r#"{"from":"en","to":"zh","trans_result":[{"src":"a","dst":"甲"},{"src":"b","dst":"乙"}]}"#;
        let r = parse_trans_result(body, reqwest::StatusCode::OK, "en", "zh").unwrap();
        assert_eq!(r.dst, "甲\n乙");
        assert_eq!((r.from.as_str(), r.to.as_str()), ("en", "zh"));
    }

    /// 响应里没有 from/to 时用请求里的值兜底，不能让前端显示空字符串。
    #[test]
    fn parse_falls_back_when_response_omits_langs() {
        let body = r#"{"trans_result":[{"dst":"甲"}]}"#;
        let r = parse_trans_result(body, reqwest::StatusCode::OK, "auto", "ja").unwrap();
        assert_eq!((r.from.as_str(), r.to.as_str()), ("auto", "ja"));
    }

    #[test]
    fn parse_treats_52000_as_success() {
        let body = r#"{"error_code":"52000","trans_result":[{"dst":"甲"}]}"#;
        assert!(parse_trans_result(body, reqwest::StatusCode::OK, "en", "zh").is_ok());
    }

    #[test]
    fn parse_reports_upstream_error_code() {
        let body = r#"{"error_code":"58004","error_msg":"model_type invalid"}"#;
        let e = parse_trans_result(body, reqwest::StatusCode::OK, "en", "zh").unwrap_err();
        assert!(e.message().contains("58004"), "{}", e.message());
        assert!(e.message().contains("model_type"), "{}", e.message());
    }
}
