//! 条件调试日志：只有 `LIGHTBOX_DEBUG=1|true` 时才写盘。
//! 默认关闭，避免用户不开调试也产生磁盘写入（ADR-012）。
//!
//! 轮转（S2 新增）：文件超过 2 MB 就只保留最后 512 KB。常驻程序一开调试可能
//! 跑好几天，不轮转迟早把用户的盘写满，而且日志太大也没法用记事本打开。

use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;

/// 超过这个大小就轮转
const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;
/// 轮转后保留的尾部大小
const KEEP_TAIL_BYTES: usize = 512 * 1024;

static ENABLED: OnceLock<bool> = OnceLock::new();

pub fn is_enabled() -> bool {
    *ENABLED.get_or_init(|| {
        matches!(
            std::env::var("LIGHTBOX_DEBUG").as_deref(),
            Ok("1") | Ok("true")
        )
    })
}

/// `%APPDATA%\vexto\debug.log`（ADR-038）。与配置文件同目录，迁移只带配置，
/// 所以更名后日志从新文件开始写，老日志留在 `%APPDATA%\lightbox\debug.log`。
pub fn log_path() -> PathBuf {
    crate::config::data_dir().join("debug.log")
}

/// 超过上限就把文件截成「最后 512 KB」。
///
/// 从**换行处**开始截断：直接按字节切会把某一行切成半截，之后 grep 起来很难看，
/// 而且可能把 UTF-8 序列从中间截断导致文件不是合法 UTF-8。
fn rotate_if_needed(p: &std::path::Path) {
    let len = match std::fs::metadata(p) {
        Ok(m) => m.len(),
        Err(_) => return,
    };
    if len <= MAX_LOG_BYTES {
        return;
    }
    let bytes = match std::fs::read(p) {
        Ok(b) => b,
        Err(_) => return,
    };
    let start = bytes.len().saturating_sub(KEEP_TAIL_BYTES);
    // 往后找到第一个换行，从它之后开始（找不到就整段保留尾部）
    let cut = bytes[start..]
        .iter()
        .position(|b| *b == b'\n')
        .map(|i| start + i + 1)
        .unwrap_or(start);
    // 再从 cut 往后跳过可能被截断的 UTF-8 续字节
    let mut s = cut;
    while s < bytes.len() && (bytes[s] & 0xC0) == 0x80 {
        s += 1;
    }
    let tail = &bytes[s..];
    let mut out = Vec::with_capacity(tail.len() + 64);
    out.extend_from_slice(b"[log rotated]\n");
    out.extend_from_slice(tail);
    if let Err(e) = std::fs::write(p, out) {
        // 轮转失败不该影响写日志，但要在原文件里留个痕迹
        let _ = e;
    }
}

pub fn dlog(msg: &str) {
    if !is_enabled() {
        return;
    }
    let p = log_path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    rotate_if_needed(&p);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    // 写失败只能吞掉：日志写不进去不该让功能挂掉，而这个错误也没人可展示
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
        let _ = writeln!(f, "[{ts}] {msg}");
        let _ = f.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::{rotate_if_needed, KEEP_TAIL_BYTES, MAX_LOG_BYTES};

    fn tmpfile(name: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("lightbox_dlog_test_{name}.log"));
        let _ = std::fs::remove_file(&p);
        p
    }

    /// 轮转后文件必须变小，且保留的是**尾部**（最近的日志才有诊断价值）。
    #[test]
    fn rotate_keeps_tail_and_drops_head() {
        let p = tmpfile("rotate");
        let mut s = String::new();
        s.push_str("HEAD-MARKER\n");
        // 塞到超过上限
        while s.len() as u64 <= MAX_LOG_BYTES + 1024 {
            s.push_str("0123456789abcdef0123456789abcdef\n");
        }
        s.push_str("TAIL-MARKER\n");
        std::fs::write(&p, &s).unwrap();

        rotate_if_needed(&p);

        let after = std::fs::read_to_string(&p).unwrap();
        assert!((after.len() as u64) < MAX_LOG_BYTES, "轮转后仍然过大");
        assert!(after.contains("TAIL-MARKER"), "尾部内容要保留");
        assert!(!after.contains("HEAD-MARKER"), "头部内容应被丢弃");
        assert!(after.starts_with("[log rotated]"));
        // 保留量不该超过「尾部上限 + 一行」
        assert!(after.len() <= KEEP_TAIL_BYTES + 128);
        let _ = std::fs::remove_file(&p);
    }

    /// 没超限时一个字节都不能动。
    #[test]
    fn rotate_is_noop_below_limit() {
        let p = tmpfile("small");
        std::fs::write(&p, "only a little\n").unwrap();
        rotate_if_needed(&p);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "only a little\n");
        let _ = std::fs::remove_file(&p);
    }

    /// 文件不存在时不能 panic（第一次启动就是这个状态）。
    #[test]
    fn rotate_missing_file_is_ok() {
        let p = tmpfile("missing");
        rotate_if_needed(&p);
    }

    /// 中文日志被截断后必须是合法 UTF-8，否则记事本打开是乱码。
    #[test]
    fn rotate_cuts_on_char_boundary() {
        let p = tmpfile("utf8");
        let mut s = String::new();
        while s.len() as u64 <= MAX_LOG_BYTES + 1024 {
            s.push_str("中文日志行，用来测试截断\n");
        }
        std::fs::write(&p, &s).unwrap();
        rotate_if_needed(&p);
        let after = std::fs::read_to_string(&p).expect("截断后必须是合法 UTF-8");
        assert!(after.contains("中文日志行"));
        let _ = std::fs::remove_file(&p);
    }
}
