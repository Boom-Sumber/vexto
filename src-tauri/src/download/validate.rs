//! 新建任务高级选项的校验与规范化（S3.5 第三轮起，会话 25 补 Authorization）。
//!
//! 这些都是「用户手填 → aria2 option」，必须在后端再校验一遍：前端可能绕过
//! （脚本直接调命令），而 aria2 对非法值的报错又很难懂（一个 RPC error 字符串）。
//! 保持纯函数、不碰文件系统，方便单测锁住边界。

/// aria2 `checksum` 允许的类型与对应十六进制摘要长度（字符数）。
/// 见 aria2 `--checksum=TYPE=DIGEST`；**只对 HTTP/FTP 生效，BT 有自己的分片校验**。
fn digest_len(kind: &str) -> Option<usize> {
    match kind {
        "md5" => Some(32),
        "sha-1" => Some(40),
        "sha-224" => Some(56),
        "sha-256" => Some(64),
        "sha-384" => Some(96),
        "sha-512" => Some(128),
        _ => None,
    }
}

/// `sha-256=ABCD…` → 规范化的 aria2 `checksum` option 值。
///
/// - 空串返回 `Ok(None)`（用户没填，不写这个 option）；
/// - 大小写都接受，输出统一小写，避免用户从别处粘来的大写摘要被 aria2 拒；
/// - 非法格式 / 不支持的类型 / 摘要长度不对，一律返回中文错误，**不静默丢弃**。
pub fn checksum_option(raw: &str) -> Result<Option<String>, String> {
    let s = raw.trim();
    if s.is_empty() {
        return Ok(None);
    }
    let Some((kind, digest)) = s.split_once('=') else {
        return Err("校验和格式应为「类型=摘要」，例如 sha-256=abcd…".into());
    };
    let kind = kind.trim().to_ascii_lowercase();
    let digest = digest.trim();
    let Some(n) = digest_len(&kind) else {
        return Err("校验和类型只支持 md5 / sha-1 / sha-224 / sha-256 / sha-384 / sha-512".into());
    };
    if digest.len() != n || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("{kind} 摘要必须是 {n} 位十六进制字符"));
    }
    Ok(Some(format!("{kind}={}", digest.to_ascii_lowercase())))
}

/// `Authorization` 头值的长度上限。
///
/// 4 KB：JWT 一般 1–2 KB，再长基本是粘错了东西。这不是安全边界（换行已经剔掉），
/// 只是别让一段误粘的网页正文被塞进 aria2 的参数里。
const MAX_AUTH_LEN: usize = 4 * 1024;

/// 用户填的 `Authorization` → aria2 `header` 数组里的一行。
///
/// - 空串返回 `Ok(None)`（用户没填，不写这个头）；
/// - 换行一并剔掉：aria2 把 header 值按行拼进请求，留一行就等于请求头注入
///   （与 Cookie 同口径，见 `engine::cookie_line`）；
/// - 不校验是不是 `Bearer xxx`：有的站点用 `Basic`、`Token` 或自定义方案，
///   强行只放 Bearer 会把合法用法挡掉。
///
/// **调用方负责不把它写进日志**：令牌等价于密码。
pub fn authorization_header(raw: &str) -> Result<Option<String>, String> {
    let v = raw.replace(['\r', '\n'], "");
    let v = v.trim();
    if v.is_empty() {
        return Ok(None);
    }
    if v.len() > MAX_AUTH_LEN {
        return Err("认证令牌过长（上限 4 KB）".into());
    }
    if v.bytes().any(|b| b < 0x20 && b != b'\t') {
        return Err("认证令牌里含有不可见控制字符".into());
    }
    Ok(Some(format!("Authorization: {v}")))
}

/// base64 字符数上限：约 4.5 MB 的种子文件。
///
/// `.torrent` 通常只有几十 KB；设上限是为了防止脚本往 IPC 里塞一大串把
/// WebView2 / serde_json 拖垮，而不是限制真实种子。前端也按同样的量级拦一次。
const MAX_TORRENT_B64: usize = 6 * 1024 * 1024;

/// 校验前端读出来的 `.torrent` base64，去掉空白后原样返回。
///
/// **不在这里判断它是不是合法的 bencode**：前端读文件时已检查首尾字节，
/// 真正解析交给 aria2（它会给出明确的 RPC 错误）。这里只挡住「空 / 超长 /
/// 不是 base64」这类会让请求白跑一趟的输入。
pub fn torrent_base64(raw: &str) -> Result<String, String> {
    let cleaned: String = raw.chars().filter(|c| !c.is_whitespace()).collect();
    if cleaned.is_empty() {
        return Err("种子文件内容为空".into());
    }
    if cleaned.len() > MAX_TORRENT_B64 {
        return Err("种子文件过大（上限约 4 MB）".into());
    }
    if !cleaned.len().is_multiple_of(4)
        || !cleaned
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='))
    {
        return Err("种子文件内容不是合法的 base64".into());
    }
    Ok(cleaned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_checksum_is_absent() {
        assert_eq!(checksum_option("").unwrap(), None);
        assert_eq!(checksum_option("   ").unwrap(), None);
    }

    #[test]
    fn checksum_normalizes_case() {
        let sha256 = "a".repeat(64);
        assert_eq!(
            checksum_option(&format!("SHA-256={}", sha256.to_uppercase())).unwrap(),
            Some(format!("sha-256={sha256}"))
        );
    }

    #[test]
    fn checksum_covers_every_supported_type() {
        for (kind, n) in [
            ("md5", 32),
            ("sha-1", 40),
            ("sha-224", 56),
            ("sha-256", 64),
            ("sha-384", 96),
            ("sha-512", 128),
        ] {
            let v = "f".repeat(n);
            assert_eq!(
                checksum_option(&format!("{kind}={v}")).unwrap(),
                Some(format!("{kind}={v}"))
            );
        }
    }

    #[test]
    fn checksum_rejects_bad_input() {
        assert!(checksum_option("abcd").unwrap_err().contains("类型=摘要"));
        assert!(checksum_option("crc32=abcd").unwrap_err().contains("只支持"));
        assert!(checksum_option("sha-256=zz").unwrap_err().contains("十六进制"));
        assert!(checksum_option("sha-256=abcd").unwrap_err().contains("64 位"));
        assert!(checksum_option("md5=").unwrap_err().contains("32 位"));
    }

    #[test]
    fn authorization_empty_is_absent() {
        assert_eq!(authorization_header("").unwrap(), None);
        assert_eq!(authorization_header("  \r\n\t").unwrap(), None);
    }

    #[test]
    fn authorization_keeps_the_raw_scheme() {
        // 不强行加 Bearer：Basic / Token / 自定义方案都要能透传
        assert_eq!(
            authorization_header("  Bearer abc.def  ").unwrap().as_deref(),
            Some("Authorization: Bearer abc.def")
        );
        assert_eq!(
            authorization_header("Basic dXNlcjpwdw==").unwrap().as_deref(),
            Some("Authorization: Basic dXNlcjpwdw==")
        );
    }

    /// 请求头注入：换行必须被剔掉，不能变成第二个头。
    #[test]
    fn authorization_strips_newlines() {
        assert_eq!(
            authorization_header("Bearer a\r\nX-Evil: 1").unwrap().as_deref(),
            Some("Authorization: Bearer aX-Evil: 1")
        );
    }

    #[test]
    fn authorization_rejects_oversize_and_control_chars() {
        let too_long = "a".repeat(MAX_AUTH_LEN + 1);
        assert!(authorization_header(&too_long).unwrap_err().contains("过长"));
        assert!(authorization_header("Bearer a\u{0}b").unwrap_err().contains("控制字符"));
    }

    #[test]
    fn torrent_base64_accepts_clean_and_whitespace() {
        // "d8:announcee" 之类的 bencode 片段，这里只测编码形态
        assert_eq!(torrent_base64("ZDg6YW5ub3VuY2Vl").unwrap(), "ZDg6YW5ub3VuY2Vl");
        assert_eq!(
            torrent_base64("ZDg6YW5ub3Vu\nY2Vl ").unwrap(),
            "ZDg6YW5ub3VuY2Vl"
        );
    }

    #[test]
    fn torrent_base64_rejects_invalid() {
        assert!(torrent_base64("").unwrap_err().contains("为空"));
        assert!(torrent_base64("   ").unwrap_err().contains("为空"));
        assert!(torrent_base64("abc").unwrap_err().contains("base64"));
        assert!(torrent_base64("%%%%").unwrap_err().contains("base64"));
        let too_big = "a".repeat(MAX_TORRENT_B64 + 4);
        assert!(torrent_base64(&too_big).unwrap_err().contains("过大"));
    }
}
