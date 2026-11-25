//! 微软 Edge 免鉴权翻译。
//!
//! 端点来源见 `requests.http`（2026-11-08 实测）：
//! `POST https://edge.microsoft.com/translate/translatetext`
//! - body 是字符串数组，不是旧版的 `[{"Text":"..."}]`；
//! - 必须带浏览器 User-Agent，否则 400；
//! - `from` 留空 = 自动检测，不能传 `auto-detect`；
//! - 响应 `Content-Type` 是 `text/plain`，但内容是 JSON。
//!
//! 这是微软 Edge 自带的免鉴权端点，不是官方开放平台 API：没有 SLA，可能变更或
//! 限流（ADR-058）。失败时把 HTTP 状态和响应片段直接交给用户。

use super::{Engine, TranslateError, Translated};
use crate::config::Config;
use crate::lang;
use rust_i18n::t;
use serde_json::Value;
use std::time::Duration;

const ENDPOINT: &str = "https://edge.microsoft.com/translate/translatetext";
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                  (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 Edg/131.0.0.0";

pub struct Microsoft;

fn snippet(s: &str) -> String {
    s.chars().take(200).collect()
}

/// 解析微软响应：数组第一项的 `translations[].text`，`detectedLanguage.language`
/// 是可选的（指定 `from` 时响应里没有）。
pub fn parse_response(
    body: &str,
    status: reqwest::StatusCode,
    requested_from: &str,
    requested_to: &str,
) -> Result<Translated, TranslateError> {
    if !status.is_success() {
        return Err(TranslateError::Upstream {
            hint: t!(
                "error.translate.httpStatus",
                engine = t!("translate.microsoft.name"),
                status = status,
                detail = snippet(body)
            )
            .into_owned(),
        });
    }
    let v: Value = serde_json::from_str(body).map_err(|_| {
        TranslateError::BadResponse(
            t!("error.translate.badJson", detail = snippet(body)).into_owned(),
        )
    })?;
    let first = v.as_array().and_then(|a| a.first()).ok_or_else(|| {
        TranslateError::BadResponse(
            t!("error.translate.emptyArray", detail = snippet(body)).into_owned(),
        )
    })?;

    let dst = first
        .get("translations")
        .and_then(|t| t.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|t| t.get("text").and_then(|x| x.as_str()))
                .filter(|s| !s.trim().is_empty())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    if dst.trim().is_empty() {
        return Err(TranslateError::BadResponse(
            t!("error.translate.emptyTranslation", detail = snippet(body)).into_owned(),
        ));
    }

    let detected = first
        .get("detectedLanguage")
        .and_then(|d| d.get("language"))
        .and_then(|l| l.as_str())
        .and_then(lang::from_microsoft);
    let from = if let Some(d) = detected {
        d.to_string()
    } else {
        let f = requested_from.trim();
        if f.is_empty() || f.eq_ignore_ascii_case("auto") {
            "auto".to_string()
        } else {
            lang::from_microsoft(f).unwrap_or(f).to_string()
        }
    };
    let to = lang::from_microsoft(requested_to)
        .unwrap_or(requested_to)
        .to_string();

    Ok(Translated { from, to, dst })
}

impl Engine for Microsoft {
    fn id(&self) -> &'static str {
        "microsoft"
    }

    fn name(&self) -> &'static str {
        "微软翻译"
    }

    fn max_chars(&self) -> usize {
        1800
    }

    fn qps(&self) -> u32 {
        2
    }

    fn is_ready(&self, _cfg: &Config) -> bool {
        true
    }

    fn ready_hint(&self) -> String {
        t!("translate.microsoft.ready").into_owned()
    }

    fn translate_one(&self, cfg: &Config, text: &str) -> Result<Translated, TranslateError> {
        let from = lang::to_microsoft(&cfg.from).ok_or_else(|| {
            TranslateError::NotReady(
                t!(
                    "error.translate.unsupportedFrom",
                    engine = t!("translate.microsoft.name"),
                    lang = cfg.from
                )
                .into_owned(),
            )
        })?;

        let to_app = if cfg.to.trim().is_empty() || cfg.to.eq_ignore_ascii_case("auto") {
            "zh"
        } else {
            cfg.to.trim()
        };
        let to = lang::to_microsoft(to_app)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                TranslateError::NotReady(
                    t!(
                        "error.translate.unsupportedTo",
                        engine = t!("translate.microsoft.name"),
                        lang = to_app
                    )
                    .into_owned(),
                )
            })?;

        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|e| {
                TranslateError::Network(
                    t!("error.translate.client", detail = e).into_owned(),
                )
            })?;

        // 语种码只包含字母、数字和 `-`，直接拼进 query 是安全的；reqwest 0.13 的
        // blocking RequestBuilder 没有 `.query()`（只有 form），这里不引 URL 编码库。
        let url = format!("{ENDPOINT}?from={from}&to={to}&isEnterpriseClient=false");
        let resp = client
            .post(url)
            .header(reqwest::header::USER_AGENT, UA)
            .json(&[text])
            .send()
            .map_err(|e| TranslateError::Network(e.to_string()))?;

        let status = resp.status();
        let body = resp
            .text()
            .map_err(|e| TranslateError::Network(e.to_string()))?;
        parse_response(&body, status, from, to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_i18n::t;

    #[test]
    fn parses_detected_language_and_translation() {
        let body = r#"[{"detectedLanguage":{"language":"en","score":0.9},"translations":[{"text":"你好，世界","to":"zh-Hans"}]}]"#;
        let r = parse_response(body, reqwest::StatusCode::OK, "", "zh-Hans").unwrap();
        assert_eq!(r.from, "en");
        assert_eq!(r.to, "zh");
        assert_eq!(r.dst, "你好，世界");
    }

    #[test]
    fn specified_from_has_no_detected_language() {
        let body = r#"[{"translations":[{"text":"Hello","to":"en"}]}]"#;
        let r = parse_response(body, reqwest::StatusCode::OK, "jp", "en").unwrap();
        assert_eq!(r.from, "jp");
        assert_eq!(r.to, "en");
        assert_eq!(r.dst, "Hello");
    }

    #[test]
    fn maps_chinese_target_to_cht() {
        let body = r#"[{"translations":[{"text":"你好","to":"zh-Hant"}]}]"#;
        let r = parse_response(body, reqwest::StatusCode::OK, "", "zh-Hant").unwrap();
        assert_eq!(r.to, "cht");
        assert_eq!(r.from, "auto");
    }

    #[test]
    fn rejects_http_error_and_empty_payload() {
        let e = parse_response(
            "Client Browser Version not supported",
            reqwest::StatusCode::BAD_REQUEST,
            "",
            "zh-Hans",
        )
        .unwrap_err();
        assert!(e.message().contains("400"));
        let e = parse_response("[]", reqwest::StatusCode::OK, "", "zh-Hans").unwrap_err();
        assert!(e
            .message()
            .contains(t!("error.translate.emptyArray", detail = "").as_ref()));
        let e = parse_response("not json", reqwest::StatusCode::OK, "", "zh-Hans").unwrap_err();
        assert!(e
            .message()
            .contains(t!("error.translate.badJson", detail = "").as_ref()));
    }

    #[test]
    fn engine_is_always_ready() {
        let e = Microsoft;
        assert!(e.is_ready(&Config::default()));
        assert_eq!(e.id(), "microsoft");
        assert_eq!(e.name(), "微软翻译");
    }

    /// 真机联网冒烟：默认 `cargo test` 不跑，避免离线环境变红。
    #[test]
    #[ignore]
    fn live_microsoft_translate() {
        let cfg = Config {
            from: "auto".into(),
            to: "zh".into(),
            ..Config::default()
        };
        let r = Microsoft.translate_one(&cfg, "Hello world").unwrap();
        assert!(!r.dst.trim().is_empty());
        assert_eq!(r.from, "en");
        assert_eq!(r.to, "zh");
    }
}
