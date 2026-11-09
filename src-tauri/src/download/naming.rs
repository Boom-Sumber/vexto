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

/// Windows 保留设备名。带扩展名也一样被占用（`CON.txt` 也打不开），
/// 所以判断时只看第一个点之前的部分。
const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// 文件名长度上限（字符数，不是字节）。
///
/// Windows 单段上限 255 个字符，但整个路径还有 260 的经典限制；下载目录本身
/// 往往已经有几十个字符，留 180 给文件名足够安全，也不至于把长名字截得太狠。
const MAX_NAME_CHARS: usize = 180;
/// 截断时给扩展名留的预算：`.tar.gz` 这类双扩展名也要保住。
const MAX_EXT_CHARS: usize = 16;

/// Windows / aria2 都不接受的文件名字符，替换成 `_`。
pub(crate) fn sanitize(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| match c {
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            // 控制字符（含 0x7f DEL）：会破坏 conf 的 key=value 行
            c if (c as u32) < 0x20 || c as u32 == 0x7f => '_',
            c => c,
        })
        .collect();
    // Windows 不允许文件名以空格或点结尾（资源管理器会拒绝创建）。
    // 全是点的名字（`.` / `..`）在这里会被削成空串，由调用方当「没名字」处理。
    while s.ends_with(' ') || s.ends_with('.') {
        s.pop();
    }
    s = s.trim_start().to_string();
    if s.is_empty() {
        return s;
    }
    // 保留设备名：`CON.zip` 在 Windows 上根本创建不出来。加前缀保留用户可读性，
    // 比整串替换成 `_` 好——用户还能认出原文件名。
    let stem = s.split('.').next().unwrap_or("");
    if RESERVED.iter().any(|r| stem.eq_ignore_ascii_case(r)) {
        s.insert(0, '_');
    }
    truncate_keep_ext(&s, MAX_NAME_CHARS)
}

/// 按**字符**截断，尽量保住扩展名。
///
/// 用字符而不是字节：中文名按字节截会把一个字劈成两半，得到非法 UTF-8。
fn truncate_keep_ext(name: &str, max: usize) -> String {
    let total = name.chars().count();
    if total <= max {
        return name.to_string();
    }
    let p = Path::new(name);
    let ext = p
        .extension()
        .map(|e| e.to_string_lossy().to_string())
        .filter(|e| e.chars().count() <= MAX_EXT_CHARS);
    match ext {
        Some(e) => {
            let stem_len = max.saturating_sub(e.chars().count() + 1);
            let stem: String = name.chars().take(stem_len).collect();
            format!("{stem}.{e}")
        }
        None => name.chars().take(max).collect(),
    }
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
///
/// 解出来的字节**不保证是 UTF-8**：不少老站点（尤其国内的 GBK 页面）把文件名
/// 按本地代码页编码后直接百分号转义，`%D6%D0.zip` 是 GBK 的「中」。所以这里
/// 先按 UTF-8 解，失败再退到系统 ANSI 代码页——否则用户会看到 `��.zip`。
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
    decode_bytes(&out)
}

/// 字节 → 字符串：先 UTF-8，失败再按系统 ANSI 代码页。
pub(crate) fn decode_bytes(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => ansi_to_string(bytes),
    }
}

/// 用系统 ANSI 代码页解码（简体中文 Windows 是 936/GBK，繁体是 950）。
///
/// 不用 `from_utf8_lossy`：它会把每个非法字节变成 U+FFFD，GBK 中文名会变成
/// 一串 `���`，比没有名字还糟。`MultiByteToWideChar` 用的是 Windows 自带的
/// 码表，零依赖。
#[cfg(windows)]
fn ansi_to_string(bytes: &[u8]) -> String {
    use windows::Win32::Globalization::{MultiByteToWideChar, CP_ACP, MULTI_BYTE_TO_WIDE_CHAR_FLAGS};

    if bytes.is_empty() {
        return String::new();
    }
    let flags = MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0);
    let need = unsafe { MultiByteToWideChar(CP_ACP, flags, bytes, None) };
    if need <= 0 {
        return String::from_utf8_lossy(bytes).to_string();
    }
    let mut buf = vec![0u16; need as usize];
    let got = unsafe { MultiByteToWideChar(CP_ACP, flags, bytes, Some(&mut buf)) };
    if got <= 0 {
        return String::from_utf8_lossy(bytes).to_string();
    }
    buf.truncate(got as usize);
    String::from_utf16_lossy(&buf)
}

#[cfg(not(windows))]
fn ansi_to_string(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_string()
}

/// 从 `Content-Disposition` 的原始值里取文件名。
///
/// 三个来源，按 RFC 的优先级：
/// 1. `filename*=UTF-8''%E4%B8%AD.zip`（RFC 5987：带 charset，能正确表示非 ASCII）；
/// 2. `filename="中文.zip"`（普通值，也可能被百分号编码过）；
/// 3. `filename="=?UTF-8?B?5Lit5paHLnppcA==?="`（RFC 2047 编码字，老 PHP 站点常见）。
///
/// 解析失败返回 `None`，由调用方退回「从 URL 猜文件名」。**不要返回空串**——
/// 空串会被当成「用户想用空名字」，最后变成一个无名的 `.aria2` 文件。
pub fn from_content_disposition(raw: &str) -> Option<String> {
    let mut extended: Option<String> = None;
    let mut plain: Option<String> = None;
    for part in raw.split(';') {
        let Some((k, v)) = part.split_once('=') else {
            continue;
        };
        let key = k.trim().to_ascii_lowercase();
        let v = v.trim().trim_matches('"').trim();
        match key.as_str() {
            "filename*" => {
                if extended.is_none() {
                    extended = parse_ext_value(v);
                }
            }
            "filename" if plain.is_none() && !v.is_empty() => {
                plain = Some(v.to_string());
            }
            _ => {}
        }
    }
    extended
        .or_else(|| plain.map(|p| decode_plain_value(&p)))
        .map(|s| {
            // 解码后可能才冒出 `/`（`%2F`）：只留基名，绝不允许变出目录层级
            let base = s.rsplit(['/', '\\']).next().unwrap_or(s.as_str());
            sanitize(base)
        })
        .filter(|s| !s.is_empty())
}

/// `UTF-8''%E4%B8%AD.zip` → `中.zip`。没有 charset 段时整串当值。
fn parse_ext_value(v: &str) -> Option<String> {
    let v = v.trim();
    if v.is_empty() {
        return None;
    }
    match v.split_once('\'') {
        // `charset'lang'value`：lang 一般为空，但有些站点会填 `en`
        Some((_charset, rest)) => match rest.split_once('\'') {
            Some((_lang, value)) => Some(percent_decode(value)),
            None => Some(percent_decode(rest)),
        },
        None => Some(percent_decode(v)),
    }
}

/// 普通 `filename=` 的值：可能是 RFC 2047 编码字，也可能被百分号编码过。
fn decode_plain_value(v: &str) -> String {
    if let Some(s) = decode_rfc2047(v) {
        return s;
    }
    if v.contains('%') {
        return percent_decode(v);
    }
    v.to_string()
}

/// `=?charset?B?base64?=` / `=?charset?Q?quoted?=` → 原文。
fn decode_rfc2047(s: &str) -> Option<String> {
    let inner = s.strip_prefix("=?")?.strip_suffix("?=")?;
    let mut it = inner.splitn(3, '?');
    let _charset = it.next()?;
    let enc = it.next()?.to_ascii_uppercase();
    let text = it.next()?;
    match enc.as_str() {
        "B" => base64_decode(text).map(|b| decode_bytes(&b)),
        "Q" => Some(decode_q(text)),
        _ => None,
    }
}

/// RFC 2047 的 Q 编码：`_` 是空格，`=XX` 是字节，其余原样。
fn decode_q(s: &str) -> String {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'_' => {
                out.push(b' ');
                i += 1;
            }
            b'=' if i + 2 < b.len() => match (hex(b[i + 1]), hex(b[i + 2])) {
                (Some(h), Some(l)) => {
                    out.push(h * 16 + l);
                    i += 3;
                }
                _ => {
                    out.push(b[i]);
                    i += 1;
                }
            },
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    decode_bytes(&out)
}

/// 标准 base64 解码（只处理文件名会出现的短串，不做流式）。
fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let mut buf: u32 = 0;
    let mut bits = 0u32;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            // 换行/空格/fill：跳过。遇到其它字符说明不是 base64
            b'=' | b'\r' | b'\n' | b' ' => continue,
            _ => return None,
        };
        buf = (buf << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
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

    #[test]
    fn sanitize_escapes_windows_reserved_names() {
        // 这些名字在 Windows 上根本创建不出来，加前缀保留可读性
        assert_eq!(sanitize("CON"), "_CON");
        assert_eq!(sanitize("con.txt"), "_con.txt");
        assert_eq!(sanitize("NUL.zip"), "_NUL.zip");
        assert_eq!(sanitize("COM1"), "_COM1");
        // 只是以此开头的正常名字不能误伤
        assert_eq!(sanitize("CONSOLE.txt"), "CONSOLE.txt");
        assert_eq!(sanitize("COM10.txt"), "COM10.txt");
    }

    #[test]
    fn sanitize_truncates_but_keeps_extension() {
        let long = format!("{}.zip", "a".repeat(300));
        let out = sanitize(&long);
        assert_eq!(out.chars().count(), MAX_NAME_CHARS);
        assert!(out.ends_with(".zip"));
        // 中文按字符截断，不能把字劈成半个（劈了就不是合法 UTF-8）
        let cn = format!("{}.txt", "中".repeat(300));
        let out = sanitize(&cn);
        assert_eq!(out.chars().count(), MAX_NAME_CHARS);
        assert!(out.ends_with(".txt"));
    }

    #[test]
    fn percent_decode_handles_utf8_and_ansi_fallback() {
        assert_eq!(percent_decode("%E4%B8%AD.zip"), "中.zip");
        // GBK 的「中」不是合法 UTF-8：必须走系统 ANSI 代码页，
        // 而不是变成一串 U+FFFD（那等于把名字毁了）
        let gbk = percent_decode("%D6%D0.zip");
        assert!(!gbk.contains('\u{fffd}'), "出现替换字符：{gbk}");
        assert!(gbk.ends_with(".zip"));
    }

    #[test]
    fn content_disposition_plain_and_rfc5987() {
        assert_eq!(
            from_content_disposition(r#"attachment; filename="a.zip""#).as_deref(),
            Some("a.zip")
        );
        assert_eq!(
            from_content_disposition("attachment; filename*=UTF-8''%E4%B8%AD.zip").as_deref(),
            Some("中.zip")
        );
        // `filename*` 优先级高于 `filename`（RFC 6266）
        assert_eq!(
            from_content_disposition(
                "attachment; filename=\"fallback.zip\"; filename*=UTF-8''%E4%B8%AD.zip"
            )
            .as_deref(),
            Some("中.zip")
        );
        assert_eq!(from_content_disposition("inline"), None);
    }

    #[test]
    fn content_disposition_decodes_rfc2047() {
        // 老 PHP 站点还在用编码字
        assert_eq!(
            from_content_disposition(r#"attachment; filename="=?UTF-8?B?5Lit5paHLnppcA==?=""#)
                .as_deref(),
            Some("中文.zip")
        );
        assert_eq!(
            from_content_disposition(r#"attachment; filename="=?UTF-8?Q?a_b.zip?=""#).as_deref(),
            Some("a b.zip")
        );
    }

    #[test]
    fn content_disposition_never_escapes_the_directory() {
        // 明文路径穿越
        assert_eq!(
            from_content_disposition(r#"attachment; filename="../../etc/passwd""#).as_deref(),
            Some("passwd")
        );
        // 编码过的路径穿越：解码后才出现斜杠
        assert_eq!(
            from_content_disposition("attachment; filename*=UTF-8''..%2F..%2Fetc%2Fpasswd")
                .as_deref(),
            Some("passwd")
        );
        // 只有路径没有文件名 → 当没名字，交给 URL 猜
        assert_eq!(
            from_content_disposition(r#"attachment; filename="../""#),
            None
        );
    }
}
