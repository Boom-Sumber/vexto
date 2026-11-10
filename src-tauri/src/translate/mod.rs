//! 翻译引擎层：trait + 实现 + 缓存 + 分块。
//!
//! 一条链路：`translate(cfg, text)` → 按 `cfg.engine_mode` + `cfg.vendor` 选引擎
//! → 检查 `is_ready` → 查缓存 → 分块 → 逐块 `translate_one` → 拼起来 → 回填缓存。
//!
//! 加一个新引擎只需要：①新建 `xxx.rs` 实现 `Engine` ②`engine_by_id()` 里加一个
//! 分支 ③`engine_id_of()` 里接上模式/厂商的组合。
//!
//! ## 会话 29 的「三模式 + 厂商」
//!
//! 以前只有 `engine = baidu | openai` 一个字段，加了大模型之后它表达不了
//! 「百度 + 大模型」这种组合（加个 `baidu-llm` 也行，但下拉框会变成五项平铺，
//! 用户看不出「模式」和「厂商」两个维度）。现在拆成：
//!
//! ```text
//! engine_mode = vendor_mt | vendor_llm | openai
//! vendor      = baidu | youdao
//! ```
//!
//! 组合出五个实现：`baidu` / `baidu-llm` / `youdao` / `youdao-llm` / `openai`。

pub mod baidu;
pub mod openai;
pub mod youdao;

use crate::config::{self, Config};
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
    /// 界面上显示的名字。默认就是 `name()`；OpenAI 兼容会带上用户起的模型名，
    /// 否则配了五套也看不出现在用的是哪套。
    fn display_name(&self, _cfg: &Config) -> String {
        self.name().to_string()
    }
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
        "baidu-llm" => Some(Box::new(baidu::BaiduLlm)),
        "youdao" => Some(Box::new(youdao::YoudaoMt)),
        "youdao-llm" => Some(Box::new(youdao::YoudaoLlm)),
        "openai" => Some(Box::new(openai::OpenAiCompat)),
        _ => None,
    }
}

/// 厂商字段 → 引擎 id 前缀。不认识的值一律当百度（配置被手改坏时也能用）。
fn vendor_id(cfg: &Config) -> &'static str {
    if cfg.vendor.trim() == config::VENDOR_YOUDAO {
        "youdao"
    } else {
        "baidu"
    }
}

/// 配置 → 实际引擎 id。这是**唯一**的「模式 + 厂商」换算入口。
pub fn engine_id_of(cfg: &Config) -> String {
    match cfg.engine_mode.trim() {
        config::MODE_OPENAI => "openai".to_string(),
        config::MODE_VENDOR_LLM => format!("{}-llm", vendor_id(cfg)),
        // 空串与 vendor_mt 都走机器翻译：空串只会在还没迁移的配置上出现
        _ => vendor_id(cfg).to_string(),
    }
}

/// 设置页「翻译模式」下拉框。
pub const MODE_OPTIONS: &[(&str, &str)] = &[
    (config::MODE_VENDOR_MT, "厂商机器翻译"),
    (config::MODE_VENDOR_LLM, "厂商大模型翻译"),
    (config::MODE_OPENAI, "OpenAI 兼容大模型"),
];

/// 设置页「厂商」下拉框。只在两种厂商模式下显示。
pub const VENDOR_OPTIONS: &[(&str, &str)] = &[
    (config::VENDOR_BAIDU, "百度"),
    (config::VENDOR_YOUDAO, "有道"),
];

/// 翻译弹窗的引擎切换菜单用：五个引擎平铺。
///
/// 弹窗是「随手切一下」的场景，不套模式/厂商两层——列表短，直接列全。
pub const ENGINE_CHOICES: &[(&str, &str)] = &[
    ("baidu", "百度翻译"),
    ("baidu-llm", "百度大模型翻译"),
    ("youdao", "有道翻译"),
    ("youdao-llm", "有道大模型翻译"),
    ("openai", "OpenAI 兼容"),
];

/// 引擎 id → `(engine_mode, vendor)`。弹窗切引擎时用它写回配置。
/// 未知 id 返回 `None`。
pub fn engine_parts(id: &str) -> Option<(&'static str, &'static str)> {
    match id.trim().to_ascii_lowercase().as_str() {
        "baidu" => Some((config::MODE_VENDOR_MT, config::VENDOR_BAIDU)),
        "baidu-llm" => Some((config::MODE_VENDOR_LLM, config::VENDOR_BAIDU)),
        "youdao" => Some((config::MODE_VENDOR_MT, config::VENDOR_YOUDAO)),
        "youdao-llm" => Some((config::MODE_VENDOR_LLM, config::VENDOR_YOUDAO)),
        "openai" => Some((config::MODE_OPENAI, config::VENDOR_BAIDU)),
        _ => None,
    }
}

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

fn cache_key(engine: &str, from: &str, to: &str, text: &str) -> String {
    md5_hex(&format!("{engine}\u{1}{from}\u{1}{to}\u{1}{text}"))
}

// ───────────────────────── 语向解析 ─────────────────────────

/// 本次翻译该往哪个方向走：返回 `(请求里用的 from, 目标语言)`。
///
/// 规则（会话 28 用户定，替换掉之前「中文特例」那一版）：
/// 1. **一律检测**语种，跟 `from` 怎么设无关；
/// 2. 检测结果 == 第一目标语言 → 译到第二目标语言，否则译到第一目标语言
///    （「我看不懂的翻成中文，我写得出的翻成外语」的通用写法）；
/// 3. 请求里的 `from` 用设置里的源语言值——**设成 auto 就发 auto**，
///    让厂商自己再判一次，等于二次确认；
/// 4. 但用户把源语言固定成某一种、检测结果却明显不同时，以**检测为准**：
///    否则「源语言=en + 文本其实是中文」会发出 en→zh 这种自相矛盾的请求。
///
/// 检测失败不算错误：`from` 用设置值、目标用第一目标语言。宁可少一层智能，
/// 也不能因为探测失败就整个翻译不可用。
pub fn resolve_direction(cfg: &Config, text: &str) -> (String, String) {
    let to1 = pick(&cfg.to, "zh");
    let to2 = pick(&cfg.to2, "en");
    let configured = cfg.from.trim();
    let request_from = if configured.is_empty() {
        "auto".to_string()
    } else {
        configured.to_string()
    };

    let detected = match crate::lang::detect(cfg, text) {
        Ok(d) => d,
        Err(e) => {
            crate::dlog::dlog(&format!("translate: 语种检测失败，交给引擎自己判（{e}）"));
            return (request_from, to1);
        }
    };

    let target = if detected == to1 { to2 } else { to1 };
    // auto 保持 auto（让厂商二次确认）；否则用检测结果——两者矛盾时检测更可信
    let from = if request_from.eq_ignore_ascii_case("auto") {
        request_from
    } else {
        detected.clone()
    };
    crate::dlog::dlog(&format!(
        "translate: 检测到 {}（{}）→ {}（请求 from={}）",
        detected,
        crate::lang::name_of(&detected),
        target,
        from
    ));
    (from, target)
}

/// 空字符串用兜底值。用户把目标语言清空也不该让翻译发出一个非法请求。
fn pick(v: &str, fallback: &str) -> String {
    let t = v.trim();
    if t.is_empty() {
        fallback.to_string()
    } else {
        t.to_string()
    }
}

// ───────────────────────── 入口 ─────────────────────────

/// 翻译一段文本。超长会按引擎自己的上限分块（最多 4 块），块间自动限速。
///
/// `from` 为空/`auto` 时由引擎自动检测；返回值里的 `from` 是检测出来的实际语种。
pub fn translate(cfg: &Config, text: &str) -> Result<Translated, String> {
    let engine_id = engine_id_of(cfg);
    let engine = engine_by_id(&engine_id).ok_or_else(|| {
        format!(
            "未知的翻译引擎「{engine_id}」，去设置 → 翻译 → 引擎 里重选一下模式与厂商。"
        )
    })?;

    if !engine.is_ready(cfg) {
        return Err(engine.ready_hint().to_string());
    }

    let text = text.trim();
    if text.is_empty() {
        return Err("没有可翻译的文本".to_string());
    }

    // 先定方向：这一步可能触发语种检测（本地或网络），所以必须在查缓存之前做，
    // 让缓存 key 用的是**解析后的**语向。否则同一段中文在 to2 改动后会命中旧缓存。
    let (from, to) = resolve_direction(cfg, text);
    let mut call_cfg = cfg.clone();
    call_cfg.from = from.clone();
    call_cfg.to = to.clone();

    let use_cache = cfg.cache_enabled;
    let key = cache_key(engine.id(), &from, &to, text);
    if use_cache {
        if let Some(dst) = cache().get(&key) {
            return Ok(Translated { from, to, dst });
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
    let mut from_lang = from;
    let mut to_lang = to;

    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            std::thread::sleep(Duration::from_millis(gap_ms));
        }
        let r = engine
            .translate_one(&call_cfg, part)
            .map_err(|e| e.message())?;
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
        for (id, name) in [
            ("baidu", "百度翻译"),
            ("baidu-llm", "百度大模型翻译"),
            ("youdao", "有道翻译"),
            ("youdao-llm", "有道大模型翻译"),
            ("openai", "OpenAI 兼容"),
        ] {
            let e = engine_by_id(id).unwrap_or_else(|| panic!("{id} 没实现"));
            assert_eq!(e.id(), id);
            assert_eq!(e.name(), name);
        }
        // 空字符串当作默认的百度，避免老配置里没有引擎字段时炸掉
        assert_eq!(engine_by_id("").unwrap().id(), "baidu");
        assert!(engine_by_id("deepl").is_none());
    }

    /// 模式 × 厂商的所有组合都必须路由到一个**已实现**的引擎。
    #[test]
    fn every_mode_vendor_combination_resolves() {
        let mut seen = Vec::new();
        for (mode, _) in MODE_OPTIONS {
            for (vendor, _) in VENDOR_OPTIONS {
                let cfg = Config {
                    engine_mode: mode.to_string(),
                    vendor: vendor.to_string(),
                    ..Config::default()
                };
                let id = engine_id_of(&cfg);
                let e = engine_by_id(&id)
                    .unwrap_or_else(|| panic!("{mode}+{vendor} → {id} 没有实现"));
                assert_eq!(e.id(), id);
                seen.push(id);
            }
        }
        // OpenAI 兼容不看厂商，两次都会算出同一个 id —— 这是预期
        assert!(seen.contains(&"baidu".to_string()));
        assert!(seen.contains(&"baidu-llm".to_string()));
        assert!(seen.contains(&"youdao".to_string()));
        assert!(seen.contains(&"youdao-llm".to_string()));
        assert!(seen.contains(&"openai".to_string()));
    }

    #[test]
    fn engine_id_of_maps_all_five_shapes() {
        let mk = |mode: &str, vendor: &str| {
            engine_id_of(&Config {
                engine_mode: mode.to_string(),
                vendor: vendor.to_string(),
                ..Config::default()
            })
        };
        assert_eq!(mk(config::MODE_VENDOR_MT, config::VENDOR_BAIDU), "baidu");
        assert_eq!(mk(config::MODE_VENDOR_LLM, config::VENDOR_BAIDU), "baidu-llm");
        assert_eq!(mk(config::MODE_VENDOR_MT, config::VENDOR_YOUDAO), "youdao");
        assert_eq!(
            mk(config::MODE_VENDOR_LLM, config::VENDOR_YOUDAO),
            "youdao-llm"
        );
        assert_eq!(mk(config::MODE_OPENAI, config::VENDOR_BAIDU), "openai");
        // 配置被手改坏时也要能用：不认识的厂商当百度、不认识的模式当机器翻译
        assert_eq!(mk("vendor_x", "klingon"), "baidu");
        assert_eq!(mk("", ""), "baidu");
    }

    /// 弹窗切引擎会把 id 写回 mode + vendor，必须能无损往返。
    #[test]
    fn engine_parts_roundtrips_with_engine_id_of() {
        for (id, name) in ENGINE_CHOICES {
            let (mode, vendor) = engine_parts(id)
                .unwrap_or_else(|| panic!("{id}（{name}）没有反解成 mode + vendor"));
            let cfg = Config {
                engine_mode: mode.to_string(),
                vendor: vendor.to_string(),
                ..Config::default()
            };
            assert_eq!(engine_id_of(&cfg), *id, "{id} 往返不一致");
            assert!(engine_by_id(id).is_some(), "{id} 没有实现");
        }
        assert!(engine_parts("deepl").is_none());
    }

    #[test]
    fn mode_and_vendor_options_cover_config_constants() {        for (id, name) in MODE_OPTIONS {
            assert!(!name.is_empty());
            // `openai` 恰好既是模式值也是引擎 id，其余模式值不该是引擎 id
            if *id != config::MODE_OPENAI {
                assert!(engine_by_id(id).is_none(), "{id} 是模式不是引擎 id");
            }
        }
        for (id, name) in VENDOR_OPTIONS {
            assert!(!name.is_empty());
            assert!(matches!(*id, config::VENDOR_BAIDU | config::VENDOR_YOUDAO));
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
        // 模式值写坏 → 路由到 baidu（兜底），但引擎 id 本身不可能未知；
        // 这里直接测 `engine_by_id` 的兜底分支的提示语由 translate 给出。
        let cfg = Config {
            engine_mode: config::MODE_VENDOR_MT.into(),
            vendor: config::VENDOR_YOUDAO.into(),
            youdao_app_key: String::new(),
            youdao_app_secret: String::new(),
            ..Config::default()
        };
        let err = translate(&cfg, "hello").unwrap_err();
        assert!(err.contains("有道"), "未配置时要说是哪家缺凭据：{err}");
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
        assert_ne!(
            cache_key("baidu", "auto", "zh", "hello"),
            cache_key("baidu", "auto", "en", "hello"),
            "目标语言不同必须不命中"
        );
        assert_ne!(
            cache_key("baidu", "auto", "zh", "hello"),
            cache_key("openai", "auto", "zh", "hello"),
            "引擎不同必须不命中"
        );
        assert_ne!(
            cache_key("baidu", "zh", "en", "hello"),
            cache_key("baidu", "en", "en", "hello"),
            "源语言不同必须不命中"
        );
        // 同样的输入必须得到同样的 key，否则缓存永远不命中
        assert_eq!(
            cache_key("baidu", "auto", "zh", "hello"),
            cache_key("baidu", "auto", "zh", "hello")
        );
    }

    // ── 语向解析（会话 28 改规则）────────────────────────────
    /// 固定了源语言、且与检测一致：from 用它，目标用第一目标语言。
    #[test]
    fn explicit_from_is_kept_when_it_matches_detection() {
        let cfg = Config {
            from: "zh".into(),
            to: "ja".into(),
            to2: "en".into(),
            ..Config::default()
        };
        let (from, to) = resolve_direction(&cfg, "你好世界");
        assert_eq!((from.as_str(), to.as_str()), ("zh", "ja"));
    }

    /// 「源语言=en，文本其实是中文，第一目标=中文」——矛盾时以检测为准，
    /// 否则会发出 en→en 这种自相矛盾的请求。
    #[test]
    fn contradictory_from_is_overridden_by_detection() {
        let cfg = Config {
            from: "en".into(),
            to: "zh".into(),
            to2: "en".into(),
            ..Config::default()
        };
        let (from, to) = resolve_direction(&cfg, "这是一段明确的中文");
        assert_eq!(from, "zh", "检测到中文就不能再发 en");
        assert_eq!(to, "en", "检测结果等于第一目标（中文）→ 换第二目标");
    }

    /// auto 模式下 from 保持 auto：交给厂商再判一次，等于二次确认。
    #[test]
    fn non_chinese_text_goes_to_primary_target() {
        let cfg = Config {
            from: "auto".into(),
            to: "zh".into(),
            to2: "en".into(),
            ..Config::default()
        };
        let (from, to) = resolve_direction(&cfg, "The quick brown fox jumps over the lazy dog");
        assert_eq!(from, "auto", "auto 要原样发出去");
        assert_eq!(to, "zh", "英文本该译成第一目标语言");
    }

    /// 这是本功能的核心规则：检测结果等于第一目标语言时换第二目标。
    #[test]
    fn chinese_text_goes_to_second_target() {
        let cfg = Config {
            from: "auto".into(),
            to: "zh".into(),
            to2: "en".into(),
            ..Config::default()
        };
        let (from, to) = resolve_direction(&cfg, "这是一段中文，应该翻译成英文");
        assert_eq!(from, "auto");
        assert_eq!(to, "en", "检测到中文（=第一目标）→ 译成第二目标");
    }

    /// 规则不再写死「中文」：第一目标设成英语时，英文文本要换到第二目标。
    #[test]
    fn rule_is_general_not_chinese_specific() {
        let cfg = Config {
            from: "auto".into(),
            to: "en".into(),
            to2: "zh".into(),
            ..Config::default()
        };
        let (_, to) = resolve_direction(&cfg, "This is clearly English text");
        assert_eq!(to, "zh", "英文 = 第一目标 → 换第二目标（中文）");
        let (_, to2) = resolve_direction(&cfg, "这是一段中文");
        assert_eq!(to2, "en", "中文 ≠ 第一目标 → 译成第一目标（英文）");
    }

    #[test]
    fn empty_targets_fall_back_to_sane_defaults() {
        let cfg = Config {
            from: "auto".into(),
            to: String::new(),
            to2: String::new(),
            ..Config::default()
        };
        let (from, to) = resolve_direction(&cfg, "hello world this is english text");
        assert_eq!(from, "auto");
        assert_eq!(to, "zh", "目标语言被清空时要兜底成中文，不能发出空 to 请求");
    }

    /// 检测不出来时：from 用设置值、目标用第一目标，不猜语言。
    #[test]
    fn undetectable_text_falls_back_to_first_target() {
        let cfg = Config {
            from: "auto".into(),
            to: "zh".into(),
            ..Config::default()
        };
        let (from, to) = resolve_direction(&cfg, "12345 --- !!!");
        assert_eq!(from, "auto", "判不出来就交给引擎，别硬猜");
        assert_eq!(to, "zh");
    }
}
