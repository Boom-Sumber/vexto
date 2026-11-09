//! 下载文件名：从链接推断、重名递增（`a(1).zip`）、断点续传判定。
//!
//! **为什么不用 aria2 的 `--auto-file-renaming`**：
//! 1. 它生成的是 `a.1.zip`，产品要求 `a(1).zip`；
//! 2. 它不区分「同名文件已存在但 `.aria2` 控制文件也在」——那是上次没下完，
//!    必须续传原名；改名会让每次重试都多出一个新文件。
//!
//! 所以入队前自己算名字，通过 `aria2.addUri` 的 `out` 选项指定。aria2 侧的
//! `auto-file-renaming=true` 只作为「文件名推断不出来」时的兜底（兜底会得到
//! `a.1.zip` 格式，但至少不会像 `false` 那样直接失败）。
//!
//! `exists` 一律用闭包注入：单测不碰真实文件系统，也方便把「本进程已分配、
//! 但文件还没落盘」的名字一起算进去（一批里两条同名链接不能拿到同一个名字）。

use std::path::Path;

/// Windows / aria2 都不接受的文件名字符，替换成 `_`。
pub(crate) fn sanitize(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| match c {
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if (c as u32) < 0x20 => '_',
            c => c,
        })
        .collect();
    // Windows 不允许文件名以空格或点结尾（资源管理器会拒绝创建）。
    while s.ends_with(' ') || s.ends_with('.') {
        s.pop();
    }
    s.trim_start().to_string()
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// `%E4%B8%AD.zip` → `中.zip`。
///
/// 只解 path 段：`+` 不当空格（那是 query 的规则，在文件名里它是合法字符）。
pub(crate) fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// 从下载链接推断文件名。推断不出来返回 `None`，交给 aria2 按响应头决定。
///
/// 只处理 http/https/ftp：magnet 要等 metadata 才有名字，`.torrent` 路径
/// 由 aria2 自己取种子里面的名字。
pub fn file_name_from_uri(uri: &str) -> Option<String> {
    let u = uri.trim();
    let lower = u.to_ascii_lowercase();
    if !(lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("ftp://"))
    {
        return None;
    }
    let after_scheme = &u[u.find("://")? + 3..];
    // 主机名之后必须有 `/`，否则这只是一个站点头（`https://x.com`），没有文件名。
    let path = &after_scheme[after_scheme.find('/')?..];
    let path = path.split('#').next().unwrap_or(path);
    let path = path.split('?').next().unwrap_or(path);
    let seg = path.rsplit('/').next().unwrap_or("");
    if seg.is_empty() {
        return None;
    }
    let name = sanitize(&percent_decode(seg));
    if name.is_empty() || name == "." || name == ".." {
        None
    } else {
        Some(name)
    }
}

/// 这个名字是不是「上次没下完」：同名文件与同名 `.aria2` 控制文件同时存在。
fn resumable(dir: &Path, name: &str, exists: &impl Fn(&Path) -> bool) -> bool {
    exists(&dir.join(name)) && exists(&dir.join(format!("{name}.aria2")))
}

/// 重名递增：`a.zip` 已占用时依次试 `a(1).zip`、`a(2).zip`……
///
/// 三种情况：
/// - 目标不存在 → 用原名；
/// - 目标存在且 `.aria2` 也在 → **用原名续传**，绝不改名；
/// - 目标存在但没有 `.aria2` → 已完成的历史文件，递增编号。
///
/// `exists` 要同时覆盖「磁盘上存在」与「本进程已分配给其它排队任务」，
/// 否则一批里两条同名链接会拿到同一个名字、后一条覆盖前一条。
pub fn unique_file_name(dir: &Path, name: &str, exists: impl Fn(&Path) -> bool) -> String {
    let name = sanitize(name.trim());
    if name.is_empty() {
        return String::new();
    }
    if !exists(&dir.join(&name)) || resumable(dir, &name, &exists) {
        return name;
    }
    let p = Path::new(&name);
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| name.clone());
    let ext = p.extension().map(|s| s.to_string_lossy().to_string());
    for i in 1..=9_999u32 {
        let cand = match &ext {
            Some(e) => format!("{stem}({i}).{e}"),
            None => format!("{stem}({i})"),
        };
        let cp = dir.join(&cand);
        if !exists(&cp) && !exists(&dir.join(format!("{cand}.aria2"))) {
            return cand;
        }
    }
    // 一万个编号都占用了：退回原名，让 aria2 的 auto-file-renaming 兜底。
    name
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::path::PathBuf;

    fn exists_set(items: &[&str]) -> impl Fn(&Path) -> bool {
        let set: HashSet<PathBuf> = items.iter().map(PathBuf::from).collect();
        move |p: &Path| set.contains(p)
    }

    #[test]
    fn uri_file_name_basics() {
        assert_eq!(
            file_name_from_uri("https://x.com/a.zip").as_deref(),
            Some("a.zip")
        );
        assert_eq!(
            file_name_from_uri("http://x.com/d/a.tar.gz?k=1#f").as_deref(),
            Some("a.tar.gz")
        );
        assert_eq!(
            file_name_from_uri("ftp://x.com/%E4%B8%AD%E6%96%87.zip").as_deref(),
            Some("中文.zip")
        );
        // 站点头 / 目录结尾 / 磁力链 / 本地种子：都推断不出文件名
        assert_eq!(file_name_from_uri("https://x.com"), None);
        assert_eq!(file_name_from_uri("https://x.com/"), None);
        assert_eq!(file_name_from_uri("magnet:?xt=urn:btih:abc"), None);
        assert_eq!(file_name_from_uri("D:\\a.torrent"), None);
    }

    #[test]
    fn uri_file_name_sanitizes() {
        assert_eq!(
            file_name_from_uri("https://x.com/a:b*c.zip").as_deref(),
            Some("a_b_c.zip")
        );
        // 百分号编码的斜杠不能变出目录层级
        assert_eq!(
            file_name_from_uri("https://x.com/a%2Fb.zip").as_deref(),
            Some("a_b.zip")
        );
    }

    #[test]
    fn unique_uses_plain_name_when_free() {
        let dir = Path::new("/dl");
        let e = exists_set(&[]);
        assert_eq!(unique_file_name(dir, "a.zip", e), "a.zip");
    }

    #[test]
    fn unique_increments_on_conflict() {
        let dir = Path::new("/dl");
        let e = exists_set(&["/dl/a.zip"]);
        assert_eq!(unique_file_name(dir, "a.zip", e), "a(1).zip");
        let e = exists_set(&["/dl/a.zip", "/dl/a(1).zip"]);
        assert_eq!(unique_file_name(dir, "a.zip", e), "a(2).zip");
    }

    #[test]
    fn unique_keeps_name_for_resume() {
        let dir = Path::new("/dl");
        // 文件与 .aria2 同时存在 = 上次没下完，必须续传原名
        let e = exists_set(&["/dl/a.zip", "/dl/a.zip.aria2"]);
        assert_eq!(unique_file_name(dir, "a.zip", e), "a.zip");
    }

    #[test]
    fn unique_handles_no_extension_and_multi_dot() {
        let dir = Path::new("/dl");
        let e = exists_set(&["/dl/README"]);
        assert_eq!(unique_file_name(dir, "README", e), "README(1)");
        // 只按最后一个扩展名拆分：a.tar.gz → a.tar(1).gz
        let e = exists_set(&["/dl/a.tar.gz"]);
        assert_eq!(unique_file_name(dir, "a.tar.gz", e), "a.tar(1).gz");
    }

    #[test]
    fn unique_skips_names_allocated_to_queued_tasks() {
        let dir = Path::new("/dl");
        // 文件还没落盘，但同批另一个任务已经分配了 a.zip（由调用方的 exists 捕获）
        let e = exists_set(&["/dl/a.zip"]);
        assert_eq!(unique_file_name(dir, "a.zip", e), "a(1).zip");
    }

    #[test]
    fn unique_sanitizes_user_input() {
        let dir = Path::new("/dl");
        let e = exists_set(&[]);
        // 用户手填的名字里带斜杠不能变出子目录
        assert_eq!(unique_file_name(dir, "a/b.zip", e), "a_b.zip");
    }
}
