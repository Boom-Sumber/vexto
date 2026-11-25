//! OpenAI 兼容接口（DeepSeek / 硅基流动 / Ollama / 本地 vLLM 都能用）。
//!
//! 只用 `/v1/chat/completions` 这一个端点——它是「OpenAI 兼容」事实上的最小公约数。
//! 不用 `/v1/translations`（那是 OpenAI 自家的、别家没有）。
//!
//! 关键设计：**不要求模型输出 JSON**。让模型只吐译文，然后原样取 `choices[0].message.content`。
//! 要求结构化输出会引入解析失败这一类新故障，而这里根本不需要结构。

use super::{Engine, TranslateError, Translated};
use crate::config::Config;
use rust_i18n::t;
use serde_json::{json, Value};
use std::time::Duration;

pub struct OpenAiCompat;

/// 把用户填的 base 拼成完整的 chat completions 地址。
///
/// 用户可能填三种东西，都要能work：
/// - `https://api.deepseek.com`         → 补 `/v1/chat/completions`
/// - `https://api.deepseek.com/v1`      → 补 `/chat/completions`
/// - `https://xxx/v1/chat/completions`  → 原样用
fn endpoint(base: &str) -> String {
    let b = base.trim().trim_end_matches('/');
    if b.is_empty() {
        return String::new();
    }
    if b.ends_with("/chat/completions") {
        return b.to_string();
    }
    if b.ends_with("/v1") {
        return format!("{b}/chat/completions");
    }
    format!("{b}/v1/chat/completions")
}

/// 系统提示词。
///
/// **语言名必须用中文名**（`中文`/`英语`/`日语`），不能把 `zh`/`jp`/`kor` 这类代码
/// 塞给模型：这是一套「百度代码」，模型看不懂，会照字母照做翻译。
fn system_prompt(profile: &crate::config::ModelProfile, cfg: &Config) -> String {
    let custom = profile.prompt.trim();
    if !custom.is_empty() {
        return custom.to_string();
    }
    let to = crate::lang::name_of(&cfg.to);
    format!(
        "你是翻译引擎。把用户输入翻译成目标语言（{to}），\
         只输出译文本身：不要解释、不要加引号、不要重复原文、不要 Markdown 代码块。\
         保留原文的换行与专有名词，代码和命令不翻译。"
    )
}

fn user_prompt(cfg: &Config, text: &str) -> String {
    let from = cfg.from.trim();
    if from.is_empty() || from == "auto" {
        text.to_string()
    } else {
        // 指定了源语言时明确告诉模型，比让它自己猜准确。
        // 同样转成中文名——`源语言：jp` 对模型来说是噪音。
        format!("源语言：{}\n\n{text}", crate::lang::name_of(from))
    }
}

impl Engine for OpenAiCompat {
    fn id(&self) -> &'static str {
        "openai"
    }

    fn name(&self) -> &'static str {
        "OpenAI 兼容"
    }

    fn max_chars(&self) -> usize {
        // 主流模型上下文都远超这个数；限制它是为了让分块逻辑有界，
        // 而不是因为模型读不下
        6000
    }

    fn qps(&self) -> u32 {
        2
    }

    fn is_ready(&self, cfg: &Config) -> bool {
        cfg.active_openai_profile().is_some_and(|p| {
            !p.key.trim().is_empty() && !p.base.trim().is_empty() && !p.model.trim().is_empty()
        })
    }

    fn ready_hint(&self) -> String {
        t!("translate.openai.ready").into_owned()
    }

    /// 带上用户起的模型名（「OpenAI 兼容 · DeepSeek」），否则弹窗和状态行
    /// 只显示「OpenAI 兼容」，配了五套也看不出当前用的是哪套。
    fn display_name(&self, cfg: &Config) -> String {
        match cfg.active_openai_profile() {
            Some(p) if !p.name.trim().is_empty() => format!("{} · {}", self.name(), p.name.trim()),
            _ => self.name().to_string(),
        }
    }

    fn translate_one(&self, cfg: &Config, text: &str) -> Result<Translated, TranslateError> {
        let profile = cfg.active_openai_profile().ok_or_else(|| {
            TranslateError::NotReady(t!("translate.openai.noModel").into_owned())
        })?;
        let url = endpoint(&profile.base);
        if url.is_empty() {
            return Err(TranslateError::NotReady(
                t!("error.translate.openai.baseUrlEmpty", name = profile.name)
                    .into_owned(),
            ));
        }

        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| {
                TranslateError::Network(
                    t!("error.translate.client", detail = e).into_owned(),
                )
            })?;

        let body = json!({
            "model": profile.model.trim(),
            "messages": [
                {"role": "system", "content": system_prompt(profile, cfg)},
                {"role": "user", "content": user_prompt(cfg, text)},
            ],
            "temperature": 0.2,
            "stream": false,
        });

        let resp = client
            .post(&url)
            .bearer_auth(profile.key.trim())
            .json(&body)
            .send()
            .map_err(|e| TranslateError::Network(format!("{url}：{e}")))?;

        let status = resp.status();
        let raw = resp
            .text()
            .map_err(|e| TranslateError::Network(e.to_string()))?;

        let v: Value = serde_json::from_str(&raw).map_err(|_| {
            TranslateError::BadResponse(
                t!(
                    "error.translate.openai.httpStatus",
                    status = status,
                    detail = raw.chars().take(200).collect::<String>()
                )
                .into_owned(),
            )
        })?;

        // 上游错误先看 error 字段：OpenAI 兼容层的报错格式比百度统一
        if let Some(err) = v.get("error") {
            let code = err
                .get("code")
                .and_then(|c| c.as_str())
                .or_else(|| err.get("type").and_then(|t| t.as_str()))
                .unwrap_or(status.as_str());
            let msg = err
                .get("message")
                .and_then(|m| m.as_str())
                .map(str::to_string)
                .unwrap_or_else(|| t!("error.translate.openai.noReason").into_owned());
            return Err(TranslateError::Upstream {
                hint: t!(
                    "error.translate.upstream",
                    code = code,
                    msg = msg
                )
                .into_owned(),
            });
        }

        let content = v
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first())
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .ok_or_else(|| {
                TranslateError::BadResponse(format!(
                    "响应里没有 choices[0].message.content：{}",
                    raw.chars().take(200).collect::<String>()
                ))
            })?;

        let dst = strip_wrapper(content);
        if dst.trim().is_empty() {
            return Err(TranslateError::BadResponse(
                t!("error.translate.emptyTranslation", detail = "").into_owned(),
            ));
        }

        Ok(Translated {
            from: if cfg.from.trim().is_empty() {
                "auto".to_string()
            } else {
                cfg.from.clone()
            },
            to: cfg.to.clone(),
            dst,
        })
    }
}

/// 剥掉模型爱加的包装：整段包在 ```…``` 里、或用引号括起来。
///
/// 这不是为了"容错"而容错：实测这类包装出现频率不低，直接显示给用户很难看。
fn strip_wrapper(s: &str) -> String {
    let t = s.trim();

    // ```lang\n...\n```
    if let Some(rest) = t.strip_prefix("```") {
        let body = match rest.find('\n') {
            Some(i) => &rest[i + 1..],
            None => rest,
        };
        if let Some(end) = body.rfind("```") {
            return body[..end].trim().to_string();
        }
    }

    // 整体被一对引号包住，且中间没有别的同类引号。
    // 注意中文引号的开闭不是同一个字符，「」『』要成对匹配。
    const PAIRS: [(char, char); 4] = [('"', '"'), ('“', '”'), ('「', '」'), ('『', '』')];
    for (open, close) in PAIRS {
        if t.starts_with(open) && t.ends_with(close) && t.chars().count() > 2 {
            let inner: String = t.chars().skip(1).take(t.chars().count() - 2).collect();
            if !inner.contains(open) && !inner.contains(close) {
                return inner.trim().to_string();
            }
        }
    }
    t.to_string()
}

#[cfg(test)]
mod tests {
    use super::{endpoint, strip_wrapper, OpenAiCompat};
    use crate::config::{Config, ModelProfile};
    // trait 方法（is_ready / display_name）要在作用域里才能直接调
    use crate::translate::Engine;

    #[test]
    fn endpoint_accepts_all_three_shapes() {
        assert_eq!(
            endpoint("https://api.deepseek.com"),
            "https://api.deepseek.com/v1/chat/completions"
        );
        assert_eq!(
            endpoint("https://api.deepseek.com/v1"),
            "https://api.deepseek.com/v1/chat/completions"
        );
        assert_eq!(
            endpoint("https://api.deepseek.com/v1/chat/completions"),
            "https://api.deepseek.com/v1/chat/completions"
        );
        // 尾部斜杠不能拼出 //v1
        assert_eq!(
            endpoint("https://api.deepseek.com/"),
            "https://api.deepseek.com/v1/chat/completions"
        );
        assert_eq!(endpoint("   "), "");
    }

    #[test]
    fn strip_wrapper_removes_code_fence() {
        assert_eq!(strip_wrapper("```\n译文\n```"), "译文");
        assert_eq!(strip_wrapper("```text\n译文\n```"), "译文");
    }

    #[test]
    fn strip_wrapper_removes_matching_quotes() {
        assert_eq!(strip_wrapper("\"译文\""), "译文");
        assert_eq!(strip_wrapper("「译文」"), "译文");
    }

    // ── 多套模型（会话 30）────────────────────────────────────

    fn profile(id: &str, name: &str) -> ModelProfile {
        ModelProfile {
            id: id.into(),
            name: name.into(),
            base: "https://api.deepseek.com".into(),
            model: "deepseek-chat".into(),
            key: "sk-x".into(),
            prompt: String::new(),
            enabled: true,
        }
    }

    /// 弹窗和状态行要显示「OpenAI 兼容 · 用户起的名字」，否则配了五套
    /// 也看不出现在用的是哪套。
    #[test]
    fn display_name_appends_profile_name() {
        let mut cfg = Config::default();
        assert_eq!(
            OpenAiCompat.display_name(&cfg),
            "OpenAI 兼容",
            "还没添加模型时只显示引擎名"
        );
        cfg.openai_profiles = vec![profile("a", "DeepSeek")];
        cfg.active_engine = "openai:a".into();
        assert_eq!(OpenAiCompat.display_name(&cfg), "OpenAI 兼容 · DeepSeek");
        // 切到厂商实例后不再是「当前」，名字退回引擎名（不拿别的实例的名字冒充）
        cfg.active_engine = "vendor:v1".into();
        assert_eq!(OpenAiCompat.display_name(&cfg), "OpenAI 兼容");
    }

    /// 就绪判断改成看「当前那一套」，不是看历史遗留的单字段。
    #[test]
    fn is_ready_follows_active_profile() {
        let mut cfg = Config::default();
        assert!(!OpenAiCompat.is_ready(&cfg), "列表为空时必须判未就绪");
        cfg.openai_profiles = vec![profile("a", "A")];
        cfg.active_engine = "openai:a".into();
        assert!(OpenAiCompat.is_ready(&cfg));
        cfg.openai_profiles[0].key.clear();
        assert!(!OpenAiCompat.is_ready(&cfg), "Key 空 → 未就绪");
        cfg.openai_profiles[0].key = "k".into();
        cfg.openai_profiles[0].base.clear();
        assert!(!OpenAiCompat.is_ready(&cfg), "BaseURL 空 → 未就绪");
        cfg.openai_profiles[0].base = "https://x".into();
        cfg.openai_profiles[0].model.clear();
        assert!(!OpenAiCompat.is_ready(&cfg), "模型名空 → 未就绪");
    }

    /// 自定义指令取的是**当前那套**的 prompt，不是全局那个。
    #[test]
    fn system_prompt_uses_active_profile_prompt() {
        let mut cfg = Config::default();
        cfg.to = "en".into();
        let mut p = profile("a", "A");
        p.prompt = "只输出译文，不要解释".into();
        assert_eq!(super::system_prompt(&p, &cfg), "只输出译文，不要解释");
        // 留空 → 内置指令里要带上目标语言的中文名
        p.prompt.clear();
        let builtin = super::system_prompt(&p, &cfg);
        assert!(builtin.contains("英语"), "内置指令要写目标语言：{builtin}");
    }

    #[test]
    fn strip_wrapper_keeps_plain_text() {
        assert_eq!(strip_wrapper("  译文  "), "译文");
        // 只有一侧有引号时不动它，避免把正常文本吃掉
        assert_eq!(strip_wrapper("\"hello"), "\"hello");
    }
}
