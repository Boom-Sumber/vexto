//! 文件名清洗与重名递增（任务书 §S4.5，安全关键）。
//!
//! 两个用途：
//! 1. **发送侧**：对外广播的文件名先过一遍（截到 basename、去控制字符），
//!    不把本机完整路径泄露给对端；
//! 2. **接收侧展示**：确认框里显示的名字用它过一遍（落盘名由 lanflare-core
//!    的 `storage::path` 内部再清洗一次，双保险不嫌多）。
//!
//! §S4.9.2 的单测锁行为。**改这个文件必须同步看单测。**

use std::path::Path;

/// 清洗单个文件名（不含路径）。任务书 §S4.5 的算法：
/// 取最后一段 → 去 `.`/`..`/空 → 控制字符换 `_` → Windows 保留名加前缀 → 去尾部点空格。
pub fn sanitize_filename(name: &str) -> String {
    // 1. 只取最后一段（去掉路径）
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);

    // 2. 去掉 . 和 .. 以及空
    if base.is_empty() || base == "." || base == ".." {
        return "unnamed_file".to_string();
    }

    // 3. 替换控制字符和 \0
    let cleaned: String = base
        .chars()
        .map(|c| if c.is_control() || c == '\0' { '_' } else { c })
        .collect();

    // 4. 去掉 Windows 保留名（按去后缀的 stem 判，CON.txt 也是保留名）
    let stem = cleaned.split('.').next().unwrap_or("").to_uppercase();
    const RESERVED: &[&str] = &[
        "CON", "PRN", "AUX", "NUL",
        "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9",
        "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if RESERVED.contains(&stem.as_str()) {
        return format!("_{cleaned}");
    }

    // 5. 去掉尾部的点和空格（Windows 不允许）
    let trimmed = cleaned.trim_end_matches(['.', ' ']);
    if trimmed.is_empty() {
        "unnamed_file".to_string()
    } else {
        trimmed.to_string()
    }
}

/// 重名递增：`a.zip` → `a (1).zip`（不覆盖，任务书 §S4.5）。
///
/// 接收侧落盘由 lanflare 的 `resolve_unique_destination_path` 内部做同样的事；
/// 这份是 §S4.5 的契约实现（行为由单测锁定），当前版本没有生产调用点——
/// 将来自研接收写入时直接换上。
#[allow(dead_code)]
pub fn unique_path(dir: &Path, name: &str) -> std::path::PathBuf {
    let mut p = dir.join(name);
    if !p.exists() {
        return p;
    }
    let stem = Path::new(name)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let ext = Path::new(name)
        .extension()
        .map(|e| e.to_string_lossy().to_string());
    for i in 1..10000 {
        let candidate = match &ext {
            Some(e) => format!("{stem} ({i}).{e}"),
            None => format!("{stem} ({i})"),
        };
        p = dir.join(candidate);
        if !p.exists() {
            return p;
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- §S4.9.2：sanitize_filename 五个必过用例 ----

    #[test]
    fn sanitize_rejects_absolute_path() {
        assert_eq!(sanitize_filename("C:\\Windows\\System32\\cmd.exe"), "cmd.exe");
        assert_eq!(sanitize_filename("/etc/passwd"), "passwd");
    }

    #[test]
    fn sanitize_keeps_unicode() {
        assert_eq!(sanitize_filename("中文文件名.zip"), "中文文件名.zip");
    }

    #[test]
    fn sanitize_rejects_traversal() {
        assert_eq!(sanitize_filename("../../evil"), "evil");
        assert_eq!(sanitize_filename("..\\..\\windows\\system32\\evil.dll"), "evil.dll");
    }

    #[test]
    fn sanitize_neutralizes_reserved_and_control() {
        assert_eq!(sanitize_filename("CON"), "_CON");
        assert_eq!(sanitize_filename("con.txt"), "_con.txt");
        assert_eq!(sanitize_filename("a\u{0}b"), "a_b");
        assert_eq!(sanitize_filename("... "), "unnamed_file");
    }

    #[test]
    fn sanitize_trims_trailing_dots_and_spaces() {
        assert_eq!(sanitize_filename("report.pdf. "), "report.pdf");
        assert_eq!(sanitize_filename(""), "unnamed_file");
        assert_eq!(sanitize_filename("."), "unnamed_file");
    }

    // ---- unique_path ----

    #[test]
    fn unique_path_appends_counter_without_overwrite() {
        let dir = std::env::temp_dir().join(format!("lightbox-s4b-uniq-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p1 = unique_path(&dir, "a.zip");
        assert_eq!(p1, dir.join("a.zip"));
        std::fs::write(&p1, b"x").unwrap();
        let p2 = unique_path(&dir, "a.zip");
        assert_eq!(p2, dir.join("a (1).zip"));
        std::fs::write(&p2, b"x").unwrap();
        let p3 = unique_path(&dir, "a.zip");
        assert_eq!(p3, dir.join("a (2).zip"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unique_path_handles_no_extension() {
        let dir = std::env::temp_dir().join(format!("lightbox-s4b-uniq2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("LICENSE"), b"x").unwrap();
        let p = unique_path(&dir, "LICENSE");
        assert_eq!(p, dir.join("LICENSE (1)"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
