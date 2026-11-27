//! 翻译历史：`%APPDATA%\vexto\translate-history.json`（ADR-096）。
//!
//! 一条 = 一次成功翻译（划词 / 输入 / 预览 / OCR 都算）。最新的在最前面；
//! 同引擎 + 同语向 + 同原文再次翻译时，把旧记录提到最前并更新译文与时间，
//! 不重复占位。最多留 [`MAX_ENTRIES`] 条，超了从尾部砍。
//!
//! 写入用「临时文件 + rename」，避免写一半断电留下半个 JSON。
//! 与下载 / 互传历史同款格式，不引 sqlite（理由见 `transfer/history.rs` 文件头）。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::Translated;
use crate::config::Config;

/// 历史上限。500 条约几百 KB，够翻很久，也不会让启动读文件变慢。
pub const MAX_ENTRIES: usize = 500;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// 删除用的唯一 id。
    pub id: String,
    /// 原文（已 trim）。
    pub src: String,
    pub dst: String,
    /// 引擎实际使用的语向。
    pub from: String,
    pub to: String,
    /// 引擎 id（`microsoft` / `baidu` / `baidu-llm` / `openai` …）。
    pub engine: String,
    /// 界面显示名（OpenAI 兼容会带模型名）。
    pub engine_name: String,
    /// 完成时间（毫秒时间戳）。
    pub at_ms: u64,
}

/// 历史文件路径（与 `config.json` 同目录）。
pub fn path() -> PathBuf {
    match crate::config::config_path().parent() {
        Some(p) => p.join("translate-history.json"),
        None => PathBuf::from("translate-history.json"),
    }
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 读全部（最新的在前）。文件不存在 / 坏掉一律当空——历史坏了不该挡住翻译。
fn load_from(p: &Path) -> Vec<HistoryEntry> {
    let Ok(text) = fs::read_to_string(p) else {
        return Vec::new();
    };
    let mut items = serde_json::from_str::<Vec<HistoryEntry>>(&text).unwrap_or_default();
    items.sort_by_key(|a| std::cmp::Reverse(a.at_ms));
    items
}

fn save_to(p: &Path, items: &[HistoryEntry]) {
    if let Some(dir) = p.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let Ok(text) = serde_json::to_string(items) else {
        return;
    };
    let tmp = p.with_extension("json.tmp");
    if fs::write(&tmp, text.as_bytes()).is_err() {
        return;
    }
    if fs::rename(&tmp, p).is_err() {
        let _ = fs::remove_file(&tmp);
    }
}

/// 记一条成功翻译。关闭历史时什么都不做（文件原样保留）。
pub fn record(cfg: &Config, src: &str, r: &Translated) {
    if !cfg.translate_history_enabled {
        return;
    }
    let src = src.trim();
    if src.is_empty() || r.dst.trim().is_empty() {
        return;
    }
    let engine = super::engine_id_of(cfg);
    let engine_name = super::engine_by_id(&engine)
        .map(|e| e.display_name(&super::prepared_config(cfg)))
        .unwrap_or_else(|| engine.clone());
    record_to(&path(), src, r, &engine, &engine_name);
}

fn record_to(p: &Path, src: &str, r: &Translated, engine: &str, engine_name: &str) {
    let mut items = load_from(p);
    let at = now_ms();

    let mut entry = HistoryEntry {
        id: uuid::Uuid::new_v4().to_string(),
        src: src.to_string(),
        dst: r.dst.clone(),
        from: r.from.clone(),
        to: r.to.clone(),
        engine: engine.to_string(),
        engine_name: engine_name.to_string(),
        at_ms: at,
    };

    // 合并：同引擎 + 同语向 + 同原文，只更新时间与译文，id 保持不变
    if let Some(i) = items.iter().position(|e| {
        e.engine == engine && e.from == r.from && e.to == r.to && e.src == entry.src
    }) {
        let mut old = items.remove(i);
        old.dst = entry.dst.clone();
        old.engine_name = entry.engine_name.clone();
        old.at_ms = at;
        entry = old;
    }

    items.insert(0, entry);
    if items.len() > MAX_ENTRIES {
        items.truncate(MAX_ENTRIES);
    }
    save_to(p, &items);
}

/// 全部记录（最新在前）。
pub fn list() -> Vec<HistoryEntry> {
    load_from(&path())
}

/// 删一条。返回是否删到了。
pub fn remove(id: &str) -> bool {
    remove_from(&path(), id)
}

fn remove_from(p: &Path, id: &str) -> bool {
    let mut items = load_from(p);
    let before = items.len();
    items.retain(|e| e.id != id);
    if items.len() == before {
        return false;
    }
    save_to(p, &items);
    true
}

/// 清空。
pub fn clear() {
    save_to(&path(), &[]);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("lb-tr-history-{name}-{}.json", now_ms()));
        let _ = fs::remove_file(&p);
        p
    }

    fn translated(from: &str, to: &str, dst: &str) -> Translated {
        Translated {
            from: from.into(),
            to: to.into(),
            dst: dst.into(),
        }
    }

    #[test]
    fn record_inserts_newest_first_and_lists() {
        let p = tmp("order");
        record_to(&p, "hello", &translated("en", "zh", "你好"), "microsoft", "微软翻译");
        record_to(&p, "world", &translated("en", "zh", "世界"), "baidu", "百度");
        let items = load_from(&p);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].src, "world");
        assert_eq!(items[1].src, "hello");
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn same_text_merges_and_moves_to_front() {
        let p = tmp("merge");
        record_to(&p, "hello", &translated("en", "zh", "你好"), "microsoft", "微软翻译");
        record_to(&p, "world", &translated("en", "zh", "世界"), "baidu", "百度");
        let first_id = load_from(&p)[1].id.clone();
        record_to(&p, "hello", &translated("en", "zh", "您好"), "microsoft", "微软翻译");
        let items = load_from(&p);
        assert_eq!(items.len(), 2, "相同内容不应新增一条");
        assert_eq!(items[0].src, "hello");
        assert_eq!(items[0].dst, "您好");
        assert_eq!(items[0].id, first_id, "合并应保留原 id，删除才稳定");
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn caps_at_max_entries() {
        let p = tmp("cap");
        for i in 0..(MAX_ENTRIES + 5) {
            record_to(
                &p,
                &format!("text-{i}"),
                &translated("en", "zh", "译文"),
                "microsoft",
                "微软翻译",
            );
        }
        let items = load_from(&p);
        assert_eq!(items.len(), MAX_ENTRIES);
        assert_eq!(items[0].src, format!("text-{}", MAX_ENTRIES + 4));
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn remove_and_clear() {
        let p = tmp("remove");
        record_to(&p, "a", &translated("en", "zh", "甲"), "microsoft", "微软翻译");
        record_to(&p, "b", &translated("en", "zh", "乙"), "microsoft", "微软翻译");
        let id = load_from(&p)[0].id.clone();
        assert!(remove_from(&p, &id));
        assert_eq!(load_from(&p).len(), 1);
        assert!(!remove_from(&p, "missing"));
        save_to(&p, &[]);
        assert!(load_from(&p).is_empty());
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn broken_file_is_treated_as_empty() {
        let p = tmp("broken");
        fs::write(&p, b"{not json").unwrap();
        assert!(load_from(&p).is_empty());
        let _ = fs::remove_file(&p);
    }
}
