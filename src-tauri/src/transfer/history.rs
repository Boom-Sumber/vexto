//! 传输历史：`%APPDATA%\vexto\transfer-history.json`（ADR-038）。
//!
//! 一条 = **一次会话**（收或发），最新的在最前面；最多留 [`MAX_ENTRIES`] 条，
//! 超了从尾部砍。写入用「临时文件 + rename」，避免写一半断电留下半个 JSON。
//!
//! 为什么不用 sqlite：历史只是「看和删」，量级几百条，一个 JSON 数组足够；
//! 引 rusqlite 要多带一个 C 依赖（03 §3.2 禁 CMake/NASM 那条精神的延伸）。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// 历史上限。500 条大约 300 KB，够翻很久，也不会让启动读文件变慢。
pub const MAX_ENTRIES: usize = 500;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryFile {
    pub name: String,
    pub size: u64,
    /// 本机路径（接收是落盘路径，发送是源文件路径）。拿不到就是空串。
    #[serde(default)]
    pub path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// 删除用的唯一 id（`会话id-毫秒`）。
    pub id: String,
    pub session_id: String,
    /// `send` / `recv`
    pub direction: String,
    /// 对端别名（拿不到是空串）。
    pub peer: String,
    /// `done` / `failed` / `cancelled`
    pub status: String,
    #[serde(default)]
    pub message: Option<String>,
    pub total_bytes: u64,
    /// 完成时间（毫秒时间戳）。
    pub at_ms: u64,
    pub files: Vec<HistoryFile>,
}

/// 历史文件路径（与 `config.json` 同目录）。
pub fn path() -> PathBuf {
    match crate::config::config_path().parent() {
        Some(p) => p.join("transfer-history.json"),
        None => PathBuf::from("transfer-history.json"),
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 读全部（最新的在前）。文件不存在 / 坏掉一律当空——历史坏了不该挡住传输。
fn load_from(p: &Path) -> Vec<HistoryEntry> {
    let Ok(text) = fs::read_to_string(p) else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<HistoryEntry>>(&text).unwrap_or_default()
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
    // Windows 上 rename 会覆盖已存在的目标（std 内部用 MoveFileEx 的 REPLACE 语义）
    if fs::rename(&tmp, p).is_err() {
        let _ = fs::remove_file(&tmp);
    }
}

/// 追加一条（放在最前），并裁到上限。
pub fn append(entry: HistoryEntry) {
    append_to(&path(), entry);
}

fn append_to(p: &Path, entry: HistoryEntry) {
    let mut items = load_from(p);
    items.insert(0, entry);
    if items.len() > MAX_ENTRIES {
        items.truncate(MAX_ENTRIES);
    }
    save_to(p, &items);
}

/// 分页读取。`page` 从 1 开始，越界自动收到最后一页。
pub fn page(page: usize, size: usize) -> (Vec<HistoryEntry>, usize) {
    page_from(&path(), page, size)
}

fn page_from(p: &Path, page: usize, size: usize) -> (Vec<HistoryEntry>, usize) {
    let items = load_from(p);
    let total = items.len();
    let size = size.clamp(1, 200);
    let page = page.max(1);
    let start = (page - 1) * size;
    if start >= total {
        // 越界：返回最后一页（删到最后一条时前端不用自己算）
        let last = if total == 0 { 0 } else { (total - 1) / size };
        let start = last * size;
        return (items.into_iter().skip(start).take(size).collect(), total);
    }
    (items.into_iter().skip(start).take(size).collect(), total)
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
    let p = path();
    save_to(&p, &[]);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("lb-history-test-{name}-{}.json", now_ms()));
        let _ = fs::remove_file(&p);
        p
    }

    fn entry(id: &str) -> HistoryEntry {
        HistoryEntry {
            id: id.to_string(),
            session_id: "s".into(),
            direction: "recv".into(),
            peer: "手机".into(),
            status: "done".into(),
            message: None,
            total_bytes: 10,
            at_ms: now_ms(),
            files: vec![HistoryFile {
                name: "a.bin".into(),
                size: 10,
                path: "C:/a.bin".into(),
            }],
        }
    }

    #[test]
    fn append_puts_newest_first_and_pages() {
        let p = tmp("page");
        for i in 1..=7 {
            append_to(&p, entry(&format!("e{i}")));
        }
        let (first, total) = page_from(&p, 1, 3);
        assert_eq!(total, 7);
        assert_eq!(first.len(), 3);
        assert_eq!(first[0].id, "e7", "最新的必须排最前");
        let (last, _) = page_from(&p, 3, 3);
        assert_eq!(last.len(), 1);
        assert_eq!(last[0].id, "e1");
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn page_out_of_range_falls_back_to_last_page() {
        let p = tmp("oob");
        for i in 1..=4 {
            append_to(&p, entry(&format!("e{i}")));
        }
        let (items, total) = page_from(&p, 99, 3);
        assert_eq!(total, 4);
        assert_eq!(items.len(), 1, "越界应回到最后一页而不是空列表");
        assert_eq!(items[0].id, "e1");
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn remove_and_truncate() {
        let p = tmp("remove");
        for i in 1..=3 {
            append_to(&p, entry(&format!("e{i}")));
        }
        assert!(remove_from(&p, "e2"));
        assert!(!remove_from(&p, "nope"));
        let items = load_from(&p);
        assert_eq!(items.len(), 2);
        assert!(items.iter().all(|e| e.id != "e2"));

        // 超上限只留 MAX_ENTRIES 条，且留的是最新的
        for i in 1..=(MAX_ENTRIES + 5) {
            append_to(&p, entry(&format!("x{i}")));
        }
        let items = load_from(&p);
        assert_eq!(items.len(), MAX_ENTRIES);
        assert_eq!(items[0].id, format!("x{}", MAX_ENTRIES + 5));
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn broken_file_is_treated_as_empty() {
        let p = tmp("broken");
        fs::write(&p, b"{not json").unwrap();
        assert!(load_from(&p).is_empty(), "坏文件不能让历史接口崩掉");
        let _ = fs::remove_file(&p);
    }
}
