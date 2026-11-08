//! 翻译引擎层：trait + 实现 + 缓存 + 分块。
//!
//! 一条链路：`translate(cfg, text)` → 按 `cfg.engine` 选引擎 → 检查 `is_ready`
//! → 查缓存 → 分块 → 逐块 `translate_one` → 拼起来 → 回填缓存。
//!
//! 加一个新引擎只需要：①新建 `xxx.rs` 实现 `Engine` ②`engine_by_id()` 里加一个
//! 分支 ③设置页的引擎下拉框加一项（在 `tools/translate_tool.rs` 里）。

pub mod baidu;
pub mod openai;

use crate::config::Config;
use md5::{Digest, Md5};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// 一次翻译的结果。`from`/`to` 是引擎**实际**用的语向（`auto` 会被解析成具体语言）。
#[derive(Debug, Clone)]
pub struct Translated {
    pub from: String,
    pub to: String,
    pub dst: String,
}

/// 引擎级错误。
#[derive(Debug, Clone)]
pub enum TranslateError {
    /// 配置不全（缺密钥之类）。用户能自己修。
    NotReady(String),
    /// 网络/HTTP 层失败
    Network(String),
    /// 上游返回了错误码。`hint` 里已经带上了错误码原文，所以不单独存 code 字段。
    Upstream { hint: String },
    /// 上游成功但内容是空的，或响应结构不认识
    BadResponse(String),
}

impl TranslateError {
    pub fn message(&self) -> String {
        match self {
            TranslateError::NotReady(s) => s.clone(),
            TranslateError::Network(s) => format!("网络请求失败：{s}"),
            TranslateError::Upstream { hint } => hint.clone(),
            TranslateError::BadResponse(s) => format!("引擎返回了无法解析的内容：{s}"),
        }
    }
}

/// 一个翻译引擎。
///
/// `cfg` 作为参数传进 `translate_one` 而不是塞进 `Engine` 的实现里：配置随时会改
/// （用户在设置页改完立刻生效），引擎实例本身应该是无状态的，否则每次改配置都要
/// 重建实例。
pub trait Engine: Send + Sync {
    fn id(&self) -> &'static str;
    fn name(&self) -> &'static str;
    /// 单次请求的字符上限
    fn max_chars(&self) -> usize;
    /// 每秒允许的请求数，用来算块间间隔
    fn qps(&self) -> u32;
    fn translate_one(&self, cfg: &Config, text: &str) -> Result<Translated, TranslateError>;
    fn is_ready(&self, cfg: &Config) -> bool;
    /// `is_ready() == false` 时告诉用户缺什么、去哪填
    fn ready_hint(&self) -> &'static str;
}

/// 引擎 id → 实例。未知 id 返回 `None`，由调用方给出中文错误。
pub fn engine_by_id(id: &str) -> Option<Box<dyn Engine>> {
    match id.trim().to_ascii_lowercase().as_str() {
        "baidu" | "" => Some(Box::new(baidu::Baidu)),
        "openai" => Some(Box::new(openai::OpenAiCompat)),
        _ => None,
    }
}

/// 设置页下拉框用的引擎清单。加引擎时这里和 `engine_by_id` 一起改。
pub const ENGINE_OPTIONS: &[(&str, &str)] = &[("baidu", "百度翻译"), ("openai", "OpenAI 兼容")];

pub const MAX_CHUNKS: usize = 4;
/// 缓存条数上限。200 条 × 平均几百字 ≈ 几十 KB，对常驻内存无压力
pub const CACHE_CAP: usize = 200;
/// 缓存有效期。翻译结果不会变，设 TTL 只是为了别让内存无限期占着旧数据的槽位
const CACHE_TTL_SECS: u64 = 600;

// ───────────────────────── 工具函数 ─────────────────────────

/// 32 位小写 MD5。百度签名和缓存 key 都用它。
///
/// md-5 0.11 的 `finalize()` 返回 `hybrid_array::Array`，不再实现 `LowerHex`，
/// 所以手动逐字节转十六进制——直接 `format!("{:x}")` 编译不过。
pub(crate) fn md5_hex(s: &str) -> String {
    let mut h = Md5::new();
    h.update(s.as_bytes());
    let digest = h.finalize();
    let mut out = String::with_capacity(32);
    for b in digest.iter() {
        out.push_str(&format!("{:02x}", b));
    }
    out
}

pub(crate) fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// 按字符边界切块，优先在换行/句末断开。
///
/// 必须按 `char` 切而不是按字节：中文每字 3 字节，按字节切会把 UTF-8 序列从
/// 中间截断，直接 panic 或者送出一堆乱码。
pub fn chunk(text: &str, max: usize, max_chunks: usize) -> Vec<String> {
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
            let low = start + max / 2;
            for i in (low..hard_end).rev() {
                let c = chars[i];
                if matches!(c, '\n' | '。' | '！' | '？' | '.' | '!' | '?') {
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

// ───────────────────────── 缓存 ─────────────────────────

struct Entry {
    dst: String,
    at: Instant,
    /// 单调递增序号，用来淘汰最旧的一条。不用 `Instant` 排序是因为同一毫秒内
    /// 连插几条时 `Instant` 可能相等，淘汰谁就不确定了（`cache_evicts_oldest` 会挂）。
    seq: u64,
}

/// 内存缓存，**只存不落盘**（隐私：用户查的可能是合同、病历）。
pub struct Cache {
    map: Mutex<HashMap<String, Entry>>,
    cap: usize,
    seq: Mutex<u64>,
}

impl Cache {
    pub fn new(cap: usize) -> Self {
        Self {
            map: Mutex::new(HashMap::new()),
            cap,
            seq: Mutex::new(0),
        }
    }

    pub fn get(&self, key: &str) -> Option<String> {
        let mut m = self.map.lock().ok()?;
        let e = m.get(key)?;
        if e.at.elapsed() > Duration::from_secs(CACHE_TTL_SECS) {
            m.remove(key);
            return None;
        }
        Some(e.dst.clone())
    }

    pub fn put(&self, key: String, dst: String) {
        let seq = {
            let mut s = match self.seq.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            *s += 1;
            *s
        };
        let mut m = match self.map.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        m.insert(
            key,
            Entry {
                dst,
                at: Instant::now(),
                seq,
            },
        );
        // 超上限就淘汰最旧的一条（每次最多删一条，插入是单条操作，不需要批量清理）
        while m.len() > self.cap {
            if let Some(oldest) = m.iter().min_by_key(|(_, e)| e.seq).map(|(k, _)| k.clone()) {
                m.remove(&oldest);
            } else {
                break;
            }
        }
    }

    pub fn clear(&self) {
        if let Ok(mut m) = self.map.lock() {
            m.clear();
        }
    }

    pub fn len(&self) -> usize {
        self.map.lock().map(|m| m.len()).unwrap_or(0)
    }
}

static CACHE: OnceLock<Cache> = OnceLock::new();

pub fn cache() -> &'static Cache {
    CACHE.get_or_init(|| Cache::new(CACHE_CAP))
}

fn cache_key(engine: &str, cfg: &Config, text: &str) -> String {
    md5_hex(&format!("{engine}\u{1}{}\u{1}{}\u{1}{text}", cfg.from, cfg.to))
}

// ───────────────────────── 入口 ─────────────────────────

/// 翻译一段文本。超长会按引擎自己的上限分块（最多 4 块），块间自动限速。
///
/// `from` 为空/`auto` 时由引擎自动检测；返回值里的 `from` 是检测出来的实际语种。
pub fn translate(cfg: &Config, text: &str) -> Result<Translated, String> {
    let engine = engine_by_id(&cfg.engine)
        .ok_or_else(|| format!("未知的翻译引擎「{}」，去设置 → 翻译 → 引擎 里选一个。", cfg.engine))?;

    if !engine.is_ready(cfg) {
        return Err(engine.ready_hint().to_string());
    }

    let text = text.trim();
    if text.is_empty() {
        return Err("没有可翻译的文本".to_string());
    }

    let use_cache = cfg.cache_enabled;
    let key = cache_key(engine.id(), cfg, text);
    if use_cache {
        if let Some(dst) = cache().get(&key) {
            let to = if cfg.to.trim().is_empty() { "zh" } else { cfg.to.trim() };
            return Ok(Translated {
                from: cfg.from.clone(),
                to: to.to_string(),
                dst,
            });
        }
    }

    let parts = chunk(text, engine.max_chars(), MAX_CHUNKS);
    if parts.is_empty() {
        return Err("没有可翻译的文本".to_string());
    }

    // qps=1 时块间隔 1000ms，但百度标准版用 1100ms 更稳（留 100ms 余量）
    let gap_ms: u64 = if engine.qps() == 0 {
        1100
    } else {
        u64::from((1000 / engine.qps()).max(1100))
    };

    let mut dsts: Vec<String> = Vec::new();
    let mut from_lang = cfg.from.clone();
    let mut to_lang = cfg.to.clone();

    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            std::thread::sleep(Duration::from_millis(gap_ms));
        }
        let r = engine.translate_one(cfg, part).map_err(|e| e.message())?;
        from_lang = r.from;
        to_lang = r.to;
        dsts.push(r.dst);
    }

    let dst = dsts.join("\n");
    if use_cache {
        cache().put(key, dst.clone());
        crate::dlog::dlog(&format!("translate: 缓存 {} 条", cache().len()));
    }

    Ok(Translated {
        from: from_lang,
        to: to_lang,
        dst,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    // ── 分块 ────────────────────────────────────────────────
    #[test]
    fn chunk_short_text_returns_single() {
        assert_eq!(chunk("hello", 2000, 4), vec!["hello"]);
    }

    #[test]
    fn chunk_respects_multibyte_boundary() {
        let s = "中".repeat(5000);
        let parts = chunk(&s, 2000, 4);
        assert!(!parts.is_empty());
        for p in &parts {
            assert!(p.chars().count() <= 2000);
            assert!(!p.is_empty());
        }
    }

    #[test]
    fn chunk_prefers_sentence_boundary() {
        let s = format!("{}{}{}", "a".repeat(1500), "。", "b".repeat(500));
        let parts = chunk(&s, 1800, 4);
        assert!(
            parts[0].ends_with('。'),
            "应该在句号后断开，实际尾部: {:?}",
            parts[0].chars().rev().take(5).collect::<String>()
        );
    }

    #[test]
    fn chunk_caps_at_max_chunks() {
        let s = "x".repeat(20000);
        let parts = chunk(&s, 2000, 4);
        assert_eq!(parts.len(), 4);
    }

    // ── 缓存 ────────────────────────────────────────────────
    #[test]
    fn cache_evicts_oldest() {
        let c = Cache::new(3);
        c.put("a".into(), "1".into());
        c.put("b".into(), "2".into());
        c.put("c".into(), "3".into());
        c.put("d".into(), "4".into());
        assert!(c.get("a").is_none(), "最旧的 a 应该被淘汰");
        assert_eq!(c.get("d").as_deref(), Some("4"));
        assert_eq!(c.len(), 3);
    }

    #[test]
    fn cache_hit_returns_value() {
        let c = Cache::new(10);
        c.put("k".into(), "译文".into());
        assert_eq!(c.get("k").as_deref(), Some("译文"));
        assert!(c.get("nope").is_none());
    }

    // ── 引擎路由 ────────────────────────────────────────────
    #[test]
    fn engine_by_id_routes_known_ids() {
        assert_eq!(engine_by_id("baidu").unwrap().id(), "baidu");
        assert_eq!(engine_by_id("openai").unwrap().id(), "openai");
        // 空字符串当作默认的百度，避免老配置里没有 engine 字段时炸掉
        assert_eq!(engine_by_id("").unwrap().id(), "baidu");
        assert!(engine_by_id("youdao").is_none());
    }

    #[test]
    fn engine_options_cover_every_routable_id() {
        for (id, name) in ENGINE_OPTIONS {
            let e = engine_by_id(id).unwrap_or_else(|| panic!("下拉框里的 {id} 没有实现"));
            assert_eq!(e.id(), *id);
            assert_eq!(e.name(), *name);
        }
    }

    // ── 未就绪要给出可照做的中文提示 ─────────────────────────
    #[test]
    fn translate_without_credentials_says_where_to_fill() {
        let cfg = Config {
            appid: String::new(),
            secret: String::new(),
            ..Config::default()
        };
        let err = translate(&cfg, "hello").unwrap_err();
        assert!(err.contains("APPID"), "错误里要指出缺什么：{err}");
        assert!(err.contains("设置"), "错误里要指出去哪填：{err}");
    }

    #[test]
    fn translate_unknown_engine_says_where_to_change() {
        let cfg = Config {
            engine: "youdao".into(),
            ..Config::default()
        };
        let err = translate(&cfg, "hello").unwrap_err();
        assert!(err.contains("引擎"), "实际：{err}");
        assert!(err.contains("设置"), "实际：{err}");
    }

    #[test]
    fn translate_empty_text_is_rejected() {
        let cfg = Config {
            appid: "a".into(),
            secret: "s".into(),
            ..Config::default()
        };
        assert!(translate(&cfg, "   ").is_err());
    }

    #[test]
    fn cache_key_varies_with_direction_and_engine() {
        let a = Config::default();
        let b = Config {
            to: "en".into(),
            ..Config::default()
        };
        assert_ne!(
            cache_key("baidu", &a, "hello"),
            cache_key("baidu", &b, "hello")
        );
        assert_ne!(
            cache_key("baidu", &a, "hello"),
            cache_key("openai", &a, "hello")
        );
        // 同样的输入必须得到同样的 key，否则缓存永远不命中
        assert_eq!(
            cache_key("baidu", &a, "hello"),
            cache_key("baidu", &a, "hello")
        );
    }
}
