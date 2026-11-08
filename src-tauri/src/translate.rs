//! 百度通用文本翻译 API。
//!
//! 三条硬规矩（错了就是 54001 签名错误或 54003 频率受限）：
//! 1. `sign = MD5(appid + q + salt + 密钥)`，32 位小写；**拼接时 q 不做 URL encode**。
//! 2. POST 必须 `application/x-www-form-urlencoded` —— `reqwest` 的 `.form()` 即是。
//! 3. 标准版 QPS = 1，分块请求之间必须限速。

use crate::config::Config;
use md5::{Digest, Md5};
use serde_json::Value;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const ENDPOINT: &str = "https://fanyi-api.baidu.com/api/trans/vip/translate";
const MAX_CHUNK_CHARS: usize = 2000;
const MAX_CHUNKS: usize = 4;
/// 标准版 QPS=1，两次请求之间至少隔这么久
const QPS_GAP_MS: u64 = 1100;

pub struct Translated {
    pub from: String,
    pub to: String,
    pub dst: String,
}

fn md5_hex(s: &str) -> String {
    let mut h = Md5::new();
    h.update(s.as_bytes());
    // md-5 0.11 的 finalize() 返回 hybrid_array::Array，不再直接实现 LowerHex，
    // 这里手动逐字节转十六进制，保证 32 位小写（百度要求）。
    let digest = h.finalize();
    let mut out = String::with_capacity(32);
    for b in digest.iter() {
        out.push_str(&format!("{:02x}", b));
    }
    out
}

fn salt() -> String {
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("{}", n % 100_000_000_000)
}

/// 按字符边界切块，优先在换行/句末断开。
///
/// 必须按 `char` 切而不是按字节：中文每字 3 字节，按字节切会把 UTF-8
/// 序列从中间截断，直接 panic 或者送出一堆乱码。
fn chunk(text: &str, max: usize, max_chunks: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max {
        return vec![text.to_string()];
    }

    let mut out: Vec<String> = Vec::new();
    let mut start = 0usize;
    while start < chars.len() && out.len() < max_chunks {
        let hard_end = (start + max).min(chars.len());
        let mut end = hard_end;
        if hard_end < chars.len() {
            // 在 [start+max/2, hard_end] 里往回找一个好断点，避免把句子劈成两半
            let low = start + max / 2;
            for i in (low..hard_end).rev() {
                let c = chars[i];
                if c == '\n' || c == '。' || c == '！' || c == '？' || c == '.' || c == '!' || c == '?' {
                    end = i + 1;
                    break;
                }
            }
        }
        let seg: String = chars[start..end].iter().collect();
        let seg = seg.trim();
        if !seg.is_empty() {
            out.push(seg.to_string());
        }
        if end <= start {
            break;
        }
        start = end;
    }
    out
}

/// 把百度的错误码翻译成用户能照做的中文。
///
/// 直接把 `error_msg` 甩给用户等于没说（"UNAUTHORIZED USER" 用户看不懂），
/// 所以必须逐个码给出「为什么 + 怎么办」。
fn error_hint(code: &str) -> String {
    let msg = match code {
        "52001" => "请求超时：文本可能过长，或 from/to 语种代码不在支持列表里",
        "52002" => "百度侧系统错误，稍后重试",
        "52003" => "未授权用户：APPID 不正确，或「通用文本翻译」服务未开通",
        "54000" => "必填参数为空：检查 APPID / 密钥是否留空",
        "54001" => "签名错误：通常是密钥不对，或文本在拼接前被 URL encode 过",
        "54003" => "访问频率受限：标准版 QPS 为 1，请稍等再试（或去认证升级）",
        "54004" => "账户余额不足：文档说标准版有免费额度，去控制台确认",
        "54005" => "长 query 请求过于频繁：稍等 3 秒再试",
        "58000" => "客户端 IP 非法：开发者信息页填了固定 IP，但当前不是那个 IP",
        "58001" => "译文语言方向不支持：检查目标语种代码（非常用语种需企业尊享版）",
        "58002" => "服务当前已关闭：去管理控制台开启「通用文本翻译」",
        "58003" => "此 IP 已被封禁：同一 IP 当日用过多个 APPID，次日解封",
        "90107" => "认证未通过或未生效",
        "20003" => "请求内容存在安全风险",
        _ => "未知错误码，去百度翻译控制台查错误码表",
    };
    format!("百度返回 {code}：{msg}")
}

fn translate_one(cfg: &Config, text: &str) -> Result<Translated, String> {
    let salt = salt();
    let sign = md5_hex(&format!("{}{}{}{}", cfg.appid, text, salt, cfg.secret));

    let from = if cfg.from.trim().is_empty() { "auto" } else { cfg.from.trim() };
    let to = if cfg.to.trim().is_empty() { "zh" } else { cfg.to.trim() };

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败：{e}"))?;

    let params = [
        ("q", text),
        ("from", from),
        ("to", to),
        ("appid", cfg.appid.trim()),
        ("salt", salt.as_str()),
        ("sign", sign.as_str()),
    ];

    let resp = client
        .post(ENDPOINT)
        .form(&params)
        .send()
        .map_err(|e| format!("请求百度翻译失败（网络）：{e}"))?;

    let status = resp.status();
    let body = resp.text().map_err(|e| format!("读取百度响应失败：{e}"))?;

    let v: Value = serde_json::from_str(&body).map_err(|_| {
        format!(
            "百度返回了无法解析的内容（HTTP {status}）：{}",
            body.chars().take(200).collect::<String>()
        )
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
            return Err(error_hint(&code));
        }
    }

    let arr = v
        .get("trans_result")
        .and_then(|a| a.as_array())
        .ok_or_else(|| format!("百度响应里没有 trans_result：{body}"))?;

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
        return Err("百度返回了空译文".to_string());
    }

    Ok(Translated {
        from: v.get("from").and_then(|f| f.as_str()).unwrap_or(from).to_string(),
        to: v.get("to").and_then(|t| t.as_str()).unwrap_or(to).to_string(),
        dst,
    })
}

/// 翻译一段文本。超长文本会分块（最多 4 块，约 8000 字），块间自动限速。
pub fn translate(cfg: &Config, text: &str) -> Result<Translated, String> {
    if cfg.appid.trim().is_empty() || cfg.secret.trim().is_empty() {
        return Err("还没填百度翻译的 APPID / 密钥。\n右键托盘图标 → 设置…".to_string());
    }

    let parts = chunk(text, MAX_CHUNK_CHARS, MAX_CHUNKS);
    if parts.is_empty() {
        return Err("没有可翻译的文本".to_string());
    }

    let mut dsts: Vec<String> = Vec::new();
    let mut from_lang = "auto".to_string();
    let mut to_lang = cfg.to.clone();

    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            std::thread::sleep(Duration::from_millis(QPS_GAP_MS));
        }
        let r = translate_one(cfg, part)?;
        from_lang = r.from;
        to_lang = r.to;
        dsts.push(r.dst);
    }

    Ok(Translated {
        from: from_lang,
        to: to_lang,
        dst: dsts.join("\n"),
    })
}

#[cfg(test)]
mod tests {
    use super::{chunk, error_hint, md5_hex};

    // ── 签名：百度对字节级细节很挑，钉死一个已知向量 ──────────────
    #[test]
    fn md5_hex_is_32_lowercase_chars() {
        let h = md5_hex("2015063000000001apple143566028812345678");
        // 百度官方文档里的示例签名，改任何一位都会挂
        assert_eq!(h, "f89f9594663708c1605f3d736d01d2d4");
        assert!(h.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    // ── 分块：短文本不分块 ──────────────────────────────────────
    #[test]
    fn chunk_short_text_returns_single() {
        assert_eq!(chunk("hello", 2000, 4), vec!["hello"]);
    }

    // ── 分块：不能从 UTF-8 序列中间切开 ─────────────────────────
    #[test]
    fn chunk_respects_multibyte_boundary() {
        // 全中文（每字 3 字节），按字节切会破坏编码
        let s = "中".repeat(5000);
        let parts = chunk(&s, 2000, 4);
        assert!(!parts.is_empty());
        for p in &parts {
            assert!(p.chars().count() <= 2000);
            // 能成功解析说明没切坏
            assert!(!p.is_empty());
        }
    }

    // ── 分块：优先在句末断开，不要劈断句子 ───────────────────────
    #[test]
    fn chunk_prefers_sentence_boundary() {
        let s = format!("{}{}", "a".repeat(1500), "。".repeat(1));
        let s = format!("{}{}", s, "b".repeat(500));
        let parts = chunk(&s, 1800, 4);
        assert!(
            parts[0].ends_with('。'),
            "应该在句号后断开，实际尾部: {:?}",
            parts[0].chars().rev().take(5).collect::<String>()
        );
    }

    // ── 分块：块数有上限，不能无限发请求 ─────────────────────────
    #[test]
    fn chunk_caps_at_max_chunks() {
        let s = "x".repeat(20000);
        let parts = chunk(&s, 2000, 4);
        assert_eq!(parts.len(), 4);
    }

    // ── 错误码：文档列出的每个码都得有中文解释 ───────────────────
    #[test]
    fn error_hint_covers_all_documented_codes() {
        for code in [
            "52001", "52002", "52003", "54000", "54001", "54003", "54004", "54005", "58000",
            "58001", "58002", "58003", "90107", "20003",
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
}
