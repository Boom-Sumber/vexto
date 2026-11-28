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
pub mod history;
pub mod microsoft;
pub mod openai;
pub mod youdao;

use crate::config::{self, Config};
use md5::{Digest, Md5};
use rust_i18n::t;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::Emitter;

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
            TranslateError::Network(s) => {
                t!("error.translate.network", detail = s).into_owned()
            }
            TranslateError::Upstream { hint } => hint.clone(),
            TranslateError::BadResponse(s) => {
                t!("error.translate.badResponse", detail = s).into_owned()
            }
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
    fn ready_hint(&self) -> String;
}

/// 引擎 id → 实例。未知 id 返回 `None`，由调用方给出中文错误。
pub fn engine_by_id(id: &str) -> Option<Box<dyn Engine>> {
    match id.trim().to_ascii_lowercase().as_str() {
        "microsoft" => Some(Box::new(microsoft::Microsoft)),
        "baidu" | "" => Some(Box::new(baidu::Baidu)),
        "baidu-llm" => Some(Box::new(baidu::BaiduLlm)),
        "youdao" => Some(Box::new(youdao::YoudaoMt)),
        "youdao-llm" => Some(Box::new(youdao::YoudaoLlm)),
        "openai" => Some(Box::new(openai::OpenAiCompat)),
        _ => None,
    }
}

/// 当前配置 → 实际引擎 id。这是**唯一**的「哪个实例 → 哪个引擎」换算入口。
///
/// 空串表示「一个可用实例都没有」：调用方据此给出「先去添加一个」的中文提示，
/// 而不是让引擎报一句看不懂的错误。
pub fn engine_id_of(cfg: &Config) -> String {
    if cfg
        .active_engine
        .trim()
        .eq_ignore_ascii_case(config::MICROSOFT_ENGINE)
    {
        return "microsoft".to_string();
    }
    if let Some(p) = cfg.active_vendor_profile() {
        let base = if p.vendor == config::VENDOR_YOUDAO {
            "youdao"
        } else {
            "baidu"
        };
        // 机翻还是大模型由 `active_engine` 的 `:llm` 后缀说了算；
        // 实例的 use_llm 只决定下拉里要不要展示大模型选项（会话 52）。
        return if cfg.active_engine.trim().ends_with(config::LLM_ENGINE_SUFFIX) {
            format!("{base}-llm")
        } else {
            base.to_string()
        };
    }
    if cfg.active_openai_profile().is_some() {
        return "openai".to_string();
    }
    String::new()
}

/// 把当前厂商实例的凭据「投影」到 Config 的扁平字段，返回一份给引擎用的配置。
///
/// 引擎实现（`baidu.rs` / `youdao.rs`）读的一直是 `cfg.appid` 这类字段。让它们
/// 改成「先找实例再取字段」会把每个引擎都写一遍实例查找；投影一次，
/// 引擎代码一行都不用改，也更容易单测（构造 Config 就能测）。
///
/// **所有要问引擎 `is_ready` / `translate_one` 的入口都必须先过这里**：会话 31
/// 迁移后扁平字段在磁盘上是空的（凭据只存在实例里），直接拿 `config::load()`
/// 的结果去问会一直得到「未配置」。
pub fn prepared_config(cfg: &Config) -> Config {
    let mut out = cfg.clone();
    if let Some(p) = cfg.active_vendor_profile() {
        out.appid = p.appid.clone();
        out.secret = p.secret.clone();
        out.youdao_app_key = p.youdao_app_key.clone();
        out.youdao_app_secret = p.youdao_app_secret.clone();
    }
    out
}

/// 当前引擎是否可用（凭据已投影）。
///
/// 唯一的就绪判断入口：`translate()`、划词热键、启动时自动开设置页、设置页状态行
/// 都走它。别自己拿 `Config` 去问 `Engine::is_ready`，也别再读废弃的 `cfg.engine`。
pub fn engine_ready(cfg: &Config) -> bool {
    let id = engine_id_of(cfg);
    if id.is_empty() {
        return false;
    }
    let prepared = prepared_config(cfg);
    engine_by_id(&id).is_some_and(|e| e.is_ready(&prepared))
}

/// 设置页「添加厂商」弹窗里的厂商下拉。
pub const VENDOR_OPTIONS: &[(&str, &str)] = &[
    (config::VENDOR_BAIDU, "百度"),
    (config::VENDOR_YOUDAO, "有道"),
];

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
    if engine_id.is_empty() {
        return Err(t!("error.translate.noEngine").into_owned());
    }
    let engine = engine_by_id(&engine_id).ok_or_else(|| {
        t!("error.translate.unknownEngine", id = engine_id).into_owned()
    })?;

    // 凭据先投影到扁平字段：引擎实现只认 `cfg.appid` 那一套，不知道「实例」的存在
    let base_cfg = prepared_config(cfg);

    if !engine.is_ready(&base_cfg) {
        return Err(engine.ready_hint());
    }

    let text = text.trim();
    if text.is_empty() {
        return Err(t!("error.translate.emptyText").into_owned());
    }

    // 先定方向：这一步可能触发语种检测（本地或网络），所以必须在查缓存之前做，
    // 让缓存 key 用的是**解析后的**语向。否则同一段中文在 to2 改动后会命中旧缓存。
    let (from, to) = resolve_direction(&base_cfg, text);
    let mut call_cfg = base_cfg;
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
        return Err(t!("error.translate.emptyText").into_owned());
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

/// 翻译并记入历史。**所有用户入口都走这个**；
/// 设置页的引擎测试探针继续用 `translate()`，不污染历史。
pub fn translate_recorded(cfg: &Config, text: &str) -> Result<Translated, String> {
    let r = translate(cfg, text)?;
    history::record(cfg, text, &r);
    if cfg.translate_history_enabled {
        if let Some(app) = crate::app_handle() {
            let _ = app.emit("translate:recorded", ());
        }
    }
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use rust_i18n::t;

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
            ("microsoft", "微软翻译"),
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

    fn vendor_profile(id: &str, vendor: &str, use_llm: bool) -> config::VendorProfile {
        config::VendorProfile {
            id: id.into(),
            name: format!("{vendor}-{id}"),
            vendor: vendor.into(),
            use_llm,
            enabled: true,
            appid: "app".into(),
            secret: "sec".into(),
            youdao_app_key: "yk".into(),
            youdao_app_secret: "ys".into(),
        }
    }

    fn model_profile(id: &str) -> config::ModelProfile {
        config::ModelProfile {
            id: id.into(),
            name: format!("model-{id}"),
            base: "https://api.deepseek.com".into(),
            model: "deepseek-chat".into(),
            key: "sk".into(),
            prompt: String::new(),
            enabled: true,
        }
    }

    /// 厂商实例 × 机翻/大模型，加上 OpenAI 实例，五种形态都要路由到**已实现**的引擎。
    #[test]
    fn every_profile_shape_resolves_to_an_engine() {
        let mut seen = Vec::new();
        for vendor in [config::VENDOR_BAIDU, config::VENDOR_YOUDAO] {
            for llm in [false, true] {
                // 会话 52 语义：用不用大模型看 active_engine 的 :llm 后缀，
                // use_llm 只是「允不允许」；两种取值都试一遍，确保路由不受它影响。
                for use_llm in [false, true] {
                    let cfg = Config {
                        vendor_profiles: vec![vendor_profile("v1", vendor, use_llm)],
                        active_engine: if llm {
                            format!("vendor:v1{}", config::LLM_ENGINE_SUFFIX)
                        } else {
                            "vendor:v1".into()
                        },
                        ..Config::default()
                    };
                    let id = engine_id_of(&cfg);
                    let e = engine_by_id(&id).unwrap_or_else(|| {
                        panic!("{vendor}/suffix_llm={llm}/use_llm={use_llm} → {id} 没有实现")
                    });
                    assert_eq!(e.id(), id);
                    seen.push(id);
                }
            }
        }
        let cfg = Config {
            openai_profiles: vec![model_profile("m1")],
            active_engine: "openai:m1".into(),
            ..Config::default()
        };
        assert_eq!(engine_id_of(&cfg), "openai");
        assert!(engine_by_id("openai").is_some());

        for id in ["baidu", "baidu-llm", "youdao", "youdao-llm"] {
            assert!(seen.contains(&id.to_string()), "{id} 没被覆盖到");
        }
    }

    /// 一个实例都没配时返回空串：调用方据此给「先去添加」的提示，
    /// 而不是让引擎报一句看不懂的错误。
    #[test]
    fn engine_id_of_is_empty_without_profiles() {
        assert_eq!(engine_id_of(&Config::default()), "");
        // 有列表但 active 指向不存在的 id → 也是空（不会瞎猜第一条）
        let cfg = Config {
            vendor_profiles: vec![vendor_profile("v1", config::VENDOR_BAIDU, false)],
            active_engine: "vendor:nope".into(),
            ..Config::default()
        };
        assert_eq!(engine_id_of(&cfg), "");
    }

    /// 内置微软翻译不需要任何配置，active_engine 直接指向它。
    #[test]
    fn microsoft_engine_routes_and_is_ready_by_default() {
        let cfg = Config {
            active_engine: config::MICROSOFT_ENGINE.into(),
            ..Config::default()
        };
        assert_eq!(engine_id_of(&cfg), "microsoft");
        assert!(engine_ready(&cfg));
        assert_eq!(cfg.active_engine_label(), "微软翻译");
    }

    /// 未知厂商代码当百度处理（配置被手改坏也要能用）。
    #[test]
    fn unknown_vendor_falls_back_to_baidu() {
        let cfg = Config {
            vendor_profiles: vec![vendor_profile("v1", "klingon", false)],
            active_engine: "vendor:v1".into(),
            ..Config::default()
        };
        assert_eq!(engine_id_of(&cfg), "baidu");
    }

    /// 凭据投影：翻译前要把当前实例的四组凭据写到扁平字段上，
    /// 这样引擎实现不用知道「实例」这回事。
    #[test]
    fn credentials_are_projected_from_active_profile() {
        let mut p = vendor_profile("v1", config::VENDOR_YOUDAO, true);
        p.youdao_app_key = "KEY".into();
        p.youdao_app_secret = "SEC".into();
        let cfg = Config {
            vendor_profiles: vec![p],
            active_engine: "vendor:v1".into(),
            ..Config::default()
        };
        let out = prepared_config(&cfg);
        assert_eq!(out.youdao_app_key, "KEY");
        assert_eq!(out.youdao_app_secret, "SEC");
        // 选的是 OpenAI 时不做任何投影，扁平字段保持空
        let cfg2 = Config {
            openai_profiles: vec![model_profile("m1")],
            active_engine: "openai:m1".into(),
            ..Config::default()
        };
        assert!(prepared_config(&cfg2).appid.is_empty());
    }

    /// 会话 31 实机回归：迁移后扁平凭据字段是空的，所有就绪判断必须先投影。
    /// 曾经 `engine_ready()` 读废弃的 `cfg.engine`（恒空）→ 划词热键永远被引去设置页，
    /// 设置页状态行也一直显示「未配置」。
    #[test]
    fn engine_ready_uses_projected_credentials() {
        let mut p = vendor_profile("v1", config::VENDOR_BAIDU, false);
        p.appid = "APP".into();
        p.secret = "SEC".into();
        let mut cfg = Config {
            vendor_profiles: vec![p],
            active_engine: "vendor:v1".into(),
            ..Config::default()
        };
        // 迁移后的真实形状：扁平字段全空，凭据只在实例里
        assert!(cfg.appid.is_empty() && cfg.secret.is_empty());
        assert!(engine_ready(&cfg), "投影后应判已就绪");
        assert!(
            !engine_by_id("baidu").unwrap().is_ready(&cfg),
            "直接拿原始 cfg 问引擎会误判未配置"
        );

        cfg.vendor_profiles[0].secret.clear();
        assert!(!engine_ready(&cfg), "实例里缺密钥 → 未就绪");

        // 没有任何实例 → 不就绪（不能靠 engine_by_id(\"\") 的默认百度蒙混过关）
        assert!(!engine_ready(&Config::default()));

        // OpenAI 实例：key / base / model 齐了才算就绪
        let mut oa = Config {
            openai_profiles: vec![model_profile("m1")],
            active_engine: "openai:m1".into(),
            ..Config::default()
        };
        assert!(engine_ready(&oa));
        oa.openai_profiles[0].key.clear();
        assert!(!engine_ready(&oa));
    }

    #[test]
    fn vendor_options_match_config_constants() {
        for (id, name) in VENDOR_OPTIONS {
            assert!(!name.is_empty());
            assert!(matches!(*id, config::VENDOR_BAIDU | config::VENDOR_YOUDAO));
        }
        assert_eq!(VENDOR_OPTIONS.len(), 2, "现在只有两家，加第三家时同步这里");
    }

    // ── 未就绪要给出可照做的中文提示 ─────────────────────────

    /// 一个实例都没配：要指出去哪添加，而不是报一句「未知引擎」。
    #[test]
    fn translate_without_profiles_says_where_to_add() {
        let err = translate(&Config::default(), "hello").unwrap_err();
        assert!(
            err.contains(t!("error.translate.noEngine").as_ref()),
            "实际：{err}"
        );
    }

    /// 有实例但凭据没填：要指出缺什么、去哪填。
    #[test]
    fn translate_without_credentials_says_where_to_fill() {
        let cfg = Config {
            vendor_profiles: vec![vendor_profile("v1", config::VENDOR_BAIDU, false)],
            active_engine: "vendor:v1".into(),
            ..Config::default()
        };
        // vendor_profile() 里凭据是占位的，先清空
        let mut cfg = cfg;
        cfg.vendor_profiles[0].appid.clear();
        cfg.vendor_profiles[0].secret.clear();
        let err = translate(&cfg, "hello").unwrap_err();
        assert!(
            err.contains(t!("translate.baidu.ready").as_ref()),
            "错误里要指出缺什么、去哪填：{err}"
        );
    }

    /// 选的是有道、凭据没填 → 提示里要出现「有道」（不能一律说百度）。
    #[test]
    fn translate_youdao_without_credentials_names_the_vendor() {
        let mut cfg = Config {
            vendor_profiles: vec![vendor_profile("v1", config::VENDOR_YOUDAO, false)],
            active_engine: "vendor:v1".into(),
            ..Config::default()
        };
        cfg.vendor_profiles[0].youdao_app_key.clear();
        cfg.vendor_profiles[0].youdao_app_secret.clear();
        let err = translate(&cfg, "hello").unwrap_err();
        assert!(
            err.contains(t!("translate.youdao.ready").as_ref()),
            "要为具体那家指路：{err}"
        );
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
