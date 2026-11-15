//! 下载历史：aria2 空闲退出后，列表里仍然要看得到下过的任务。
//!
//! 为什么不引 SQLite：条目量小（上限 300）、只有本进程写、没有查询需求，
//! 一个 JSON 文件 + 内存缓存就够；省一个依赖也省一次编译。
//!
//! 写入时机：pump 每轮快照后调 `observe()`，只在下述情况落盘——
//! 任务第一次进入终态（complete / error），或终态发生变化。
//! 所以 500 ms 的轮询不会变成 500 ms 一次的磁盘写。
//!
//! 合并时机：`download_snapshot` 命令与 pump 的 `tasks:snapshot` 事件都会把
//! 「历史里有、aria2 里没有」的条目补进快照，前端不需要区分两套数据源。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::rpc::{Snapshot, TaskRow};

/// 历史最多保留多少条。超出按时间从旧到新丢，避免文件无限长。
const MAX_ENTRIES: usize = 300;

/// 设置里的「已完成记录保留」→ 条数上限。`None` = 永久保留（不裁剪）。
///
/// 认不出来的值退回默认 300：配置被手改坏时宁可少存几条，也不能一下子
/// 把用户的历史全裁掉（`None` 会让文件无限长，那更糟）。
pub fn keep_limit(raw: &str) -> Option<usize> {
    match raw.trim() {
        "100" => Some(100),
        "300" => Some(MAX_ENTRIES),
        "forever" => None,
        _ => Some(MAX_ENTRIES),
    }
}

/// 上一次「清理丢失记录」的时间。合并快照是 500 ms 一次，不对它节流就是
/// 每秒对 300 个路径做 600 次 stat。
static LAST_SWEEP: Mutex<Option<std::time::Instant>> = Mutex::new(None);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub gid: String,
    #[serde(default)]
    pub url: String,
    pub name: String,
    #[serde(default)]
    pub dir: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub size: u64,
    /// 只存终态：`complete` / `error`。
    pub status: String,
    pub finished_at: u64,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
struct Store {
    #[serde(default = "version_1")]
    version: u32,
    #[serde(default)]
    entries: Vec<HistoryEntry>,
}

fn version_1() -> u32 {
    1
}

/// 内存缓存。`None` = 还没读过盘。
static CACHE: Mutex<Option<Vec<HistoryEntry>>> = Mutex::new(None);

fn path() -> PathBuf {
    crate::config::config_path().with_file_name("download-history.json")
}

fn load() -> Vec<HistoryEntry> {
    let mut g = match CACHE.lock() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    };
    if let Some(v) = g.as_ref() {
        return v.clone();
    }
    let v = std::fs::read_to_string(path())
        .ok()
        .and_then(|s| serde_json::from_str::<Store>(&s).ok())
        .map(|s| s.entries)
        .unwrap_or_default();
    *g = Some(v.clone());
    v
}

fn store(entries: Vec<HistoryEntry>) {
    let mut g = match CACHE.lock() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    };
    *g = Some(entries.clone());
    drop(g);
    let body = Store {
        version: 1,
        entries,
    };
    if let Ok(text) = serde_json::to_string_pretty(&body) {
        // 先写临时文件再 rename：写一半崩掉时，旧历史不会被截断成半个 JSON
        // （load 解析失败会当成空历史，等于把用户记录清了）。
        let p = path();
        let tmp = p.with_extension("json.tmp");
        if std::fs::write(&tmp, text).is_err() || std::fs::rename(&tmp, &p).is_err() {
            crate::dlog::warn("history: 写盘失败");
        }
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn entry_from_row(t: &TaskRow) -> Option<HistoryEntry> {
    if !matches!(t.status.as_str(), "complete" | "error") {
        return None;
    }
    if t.gid.is_empty() {
        return None;
    }
    let name = if t.name.trim().is_empty() {
        Path::new(&t.file_path)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| t.gid.clone())
    } else {
        t.name.clone()
    };
    Some(HistoryEntry {
        gid: t.gid.clone(),
        url: t.url.clone(),
        name,
        dir: t.dir.clone(),
        path: t.file_path.clone(),
        size: if t.total_length > 0 {
            t.total_length
        } else {
            t.completed_length
        },
        status: t.status.clone(),
        finished_at: now(),
        error: t.error_message.clone(),
    })
}

/// 观察一轮快照：把新进入终态的任务记进历史。
///
/// 返回**本轮新记录**的条目，调用方据此发系统通知。已记录过的不会重复返回。
pub fn observe(tasks: &[TaskRow]) -> Vec<HistoryEntry> {
    let mut entries = load();
    let mut fresh: Vec<HistoryEntry> = Vec::new();
    let mut changed = false;

    for t in tasks {
        let Some(rec) = entry_from_row(t) else {
            continue;
        };
        match entries.iter().position(|e| e.gid == rec.gid) {
            Some(i) if entries[i].status == rec.status => {}
            Some(i) => {
                entries.remove(i);
                entries.insert(0, rec.clone());
                fresh.push(rec);
                changed = true;
            }
            None => {
                entries.insert(0, rec.clone());
                fresh.push(rec);
                changed = true;
            }
        }
    }

    if let Some(limit) = keep_limit(&crate::config::load().history_keep) {
        if entries.len() > limit {
            entries.truncate(limit);
            changed = true;
        }
    }
    if changed {
        store(entries);
    }
    fresh
}

/// 「文件丢失时移除记录」：把落盘文件已经不在的**已完成**记录删掉。
///
/// 只删 `complete`：`error` 的任务本来就可能没有文件（403 时连文件都没建），
/// 拿「文件存在」当判据会把所有失败记录误删。
///
/// `force` 给用户主动点「清理」用；平时合并快照会以 60 秒为间隔节流调用。
pub fn sweep_missing(force: bool) {
    {
        let mut g = match LAST_SWEEP.lock() {
            Ok(g) => g,
            Err(e) => e.into_inner(),
        };
        let now = std::time::Instant::now();
        if !force {
            if let Some(prev) = *g {
                if now.saturating_duration_since(prev) < std::time::Duration::from_secs(60) {
                    return;
                }
            }
        }
        *g = Some(now);
    }
    let mut entries = load();
    let before = entries.len();
    entries.retain(|e| {
        e.status != "complete" || e.path.trim().is_empty() || Path::new(&e.path).exists()
    });
    if entries.len() != before {
        store(entries);
    }
}

/// 把历史补进快照（纯函数，便于单测）。
///
/// 只补「aria2 当前没有的 gid」；同 gid 以实时任务为准（进度、状态都更新）。
pub fn merge_entries(snap: &mut Snapshot, entries: &[HistoryEntry], exists: impl Fn(&str) -> bool) {
    // 用拥有所有权的 HashSet：下面要往 `snap.tasks` push，借用会打架。
    let live: HashSet<String> = snap.tasks.iter().map(|t| t.gid.clone()).collect();
    for e in entries {
        if live.contains(e.gid.as_str()) {
            continue;
        }
        let missing = !e.path.is_empty() && !exists(&e.path);
        snap.tasks.push(TaskRow {
            gid: e.gid.clone(),
            status: e.status.clone(),
            name: e.name.clone(),
            total_length: e.size,
            completed_length: e.size,
            download_speed: 0,
            upload_speed: 0,
            eta_seconds: None,
            dir: e.dir.clone(),
            file_path: e.path.clone(),
            file_missing: missing,
            url: e.url.clone(),
            historical: true,
            error_message: e.error.clone(),
        });
    }
}

/// 把历史补进快照（读缓存 + 碰一次文件系统）。
pub fn merge(snap: &mut Snapshot) {
    let cfg = crate::config::load();
    if cfg.remove_missing_records {
        sweep_missing(false);
    }
    let entries = load();
    // 保留策略是配置项，但历史文件是老版本写的：这里顺带裁一次，
    // 用户把 300 改成 100 之后不必等下一次下载完才生效。
    if let Some(limit) = keep_limit(&cfg.history_keep) {
        if entries.len() > limit {
            let kept = entries[..limit].to_vec();
            store(kept.clone());
            merge_entries(snap, &kept, |p| Path::new(p).exists());
            return;
        }
    }
    merge_entries(snap, &entries, |p| Path::new(p).exists());
}

/// 按 gid 找一条历史，转成 `TaskRow`（「打开目录」这类操作要用）。
pub fn find(gid: &str) -> Option<TaskRow> {
    let entries = load();
    let mut snap = Snapshot::default();
    merge_entries(&mut snap, &entries, |p| Path::new(p).exists());
    snap.tasks.into_iter().find(|t| t.gid == gid)
}

/// 用户删掉一条任务：历史里也删掉，否则下次快照又会把它补回来。
/// 返回是否命中（用于判断「这只是一条历史记录」）。
pub fn remove(gid: &str) -> bool {
    let mut entries = load();
    let before = entries.len();
    entries.retain(|e| e.gid != gid);
    if entries.len() == before {
        return false;
    }
    store(entries);
    true
}

/// 「清空已完成」：终态记录（complete / error）一起清掉。
pub fn clear_finished() {
    let mut entries = load();
    let before = entries.len();
    entries.retain(|e| !matches!(e.status.as_str(), "complete" | "error"));
    if entries.len() != before {
        store(entries);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(gid: &str, status: &str, name: &str) -> TaskRow {
        TaskRow {
            gid: gid.into(),
            status: status.into(),
            name: name.into(),
            total_length: 100,
            completed_length: 100,
            download_speed: 0,
            upload_speed: 0,
            eta_seconds: None,
            dir: r"D:\dl".into(),
            file_path: format!(r"D:\dl\{name}"),
            file_missing: false,
            url: "http://x/a.zip".into(),
            historical: false,
            error_message: None,
        }
    }

    fn entry(gid: &str, status: &str, path: &str) -> HistoryEntry {
        HistoryEntry {
            gid: gid.into(),
            url: "http://x/a.zip".into(),
            name: "a.zip".into(),
            dir: r"D:\dl".into(),
            path: path.into(),
            size: 100,
            status: status.into(),
            finished_at: 1,
            error: None,
        }
    }

    #[test]
    fn only_finished_tasks_are_recorded() {
        assert!(entry_from_row(&row("g1", "active", "a.zip")).is_none());
        assert!(entry_from_row(&row("g1", "paused", "a.zip")).is_none());
        assert!(entry_from_row(&row("g1", "complete", "a.zip")).is_some());
        assert!(entry_from_row(&row("g1", "error", "a.zip")).is_some());
    }

    /// 保留策略：认不出来的值必须退回默认 300，绝不能变成「永久保留」——
    /// 那会让配置文件无限长。
    #[test]
    fn keep_limit_falls_back_to_default() {
        assert_eq!(keep_limit("100"), Some(100));
        assert_eq!(keep_limit("300"), Some(300));
        assert_eq!(keep_limit("forever"), None);
        assert_eq!(keep_limit(""), Some(300));
        assert_eq!(keep_limit("手改坏了"), Some(300));
    }

    #[test]
    fn entry_falls_back_to_file_name() {
        let mut r = row("g1", "complete", "");
        r.file_path = r"D:\dl\落盘.bin".into();
        let e = entry_from_row(&r).unwrap();
        assert_eq!(e.name, "落盘.bin");
    }

    #[test]
    fn merge_skips_live_and_marks_missing() {
        let mut snap = Snapshot::default();
        snap.tasks.push(row("live", "active", "live.bin"));
        let entries = vec![
            entry("live", "complete", r"D:\dl\live.bin"),
            entry("gone", "complete", r"D:\dl\gone.bin"),
            entry("del", "complete", r"D:\dl\del.bin"),
        ];
        merge_entries(&mut snap, &entries, |p| p.ends_with("gone.bin"));
        assert_eq!(snap.tasks.len(), 3);
        let gone = snap.tasks.iter().find(|t| t.gid == "gone").unwrap();
        assert!(gone.historical);
        assert!(!gone.file_missing);
        let del = snap.tasks.iter().find(|t| t.gid == "del").unwrap();
        assert!(del.file_missing);
        // 同 gid 以实时行为准：不能被历史覆盖成 complete
        let live = snap.tasks.iter().find(|t| t.gid == "live").unwrap();
        assert_eq!(live.status, "active");
        assert!(!live.historical);
    }

    #[test]
    fn merge_keeps_error_message() {
        let mut snap = Snapshot::default();
        let mut e = entry("bad", "error", r"D:\dl\bad.bin");
        e.error = Some("403 Forbidden".into());
        merge_entries(&mut snap, &[e], |_| false);
        assert_eq!(snap.tasks[0].error_message.as_deref(), Some("403 Forbidden"));
        assert_eq!(snap.tasks[0].status, "error");
    }
}
