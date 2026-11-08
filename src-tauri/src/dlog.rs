//! 条件调试日志：只有 `LIGHTBOX_DEBUG=1|true` 时才写盘。
//! 默认关闭，避免用户不开调试也产生磁盘写入（ADR-012）。

use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;

static ENABLED: OnceLock<bool> = OnceLock::new();

fn enabled() -> bool {
    *ENABLED.get_or_init(|| {
        matches!(
            std::env::var("LIGHTBOX_DEBUG").as_deref(),
            Ok("1") | Ok("true")
        )
    })
}

pub fn log_path() -> PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(base).join("lightbox").join("debug.log")
}

pub fn dlog(msg: &str) {
    if !enabled() {
        return;
    }
    let p = log_path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
        let _ = writeln!(f, "[{ts}] {msg}");
        let _ = f.flush();
    }
}
