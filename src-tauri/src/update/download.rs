//! 安装包下载：线路选择、128 KB 分块流式写盘、sha256 校验、取消。
//!
//! 红线：安装包不许整块进内存（docs/03 §3.8）。这里只用一个 128 KB 缓冲，
//! 一边写文件一边喂哈希。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::dlog;
use crate::update::github;

/// 取消是正常路径，不是错误文案。
pub const CANCELLED: &str = "已取消下载";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub label: String,
    pub url: String,
}

/// `%LOCALAPPDATA%\vexto\updates`；拿不到 LOCALAPPDATA 时退回临时目录。
pub fn updates_dir() -> PathBuf {
    std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir())
        .join("vexto")
        .join("updates")
}

/// 按配置给出下载候选，顺序即尝试顺序。
///
/// 未知配置值回落到 `auto`：手改配置写错不该让更新彻底不可用。
pub fn candidates(
    source: &str,
    custom: &str,
    asset_url: &str,
) -> Result<Vec<Candidate>, String> {
    let src = source.trim().to_ascii_lowercase();
    match src.as_str() {
        "direct" => Ok(vec![Candidate {
            label: "直连".to_string(),
            url: asset_url.to_string(),
        }]),
        "custom" => {
            let prefix = normalize_prefix(custom).ok_or_else(|| {
                "自定义代理地址需要以 http:// 或 https:// 开头".to_string()
            })?;
            Ok(vec![Candidate {
                label: "自定义代理".to_string(),
                url: format!("{prefix}{asset_url}"),
            }])
        }
        _ => {
            if !matches!(src.as_str(), "auto" | "") {
                dlog::warn(&format!("update: 未知下载线路「{source}」，回落到 auto"));
            }
            let mut out: Vec<Candidate> = github::PROXIES
                .iter()
                .map(|p| Candidate {
                    label: p
                        .trim_start_matches("https://")
                        .trim_end_matches('/')
                        .to_string(),
                    url: format!("{p}{asset_url}"),
                })
                .collect();
            out.push(Candidate {
                label: "直连".to_string(),
                url: asset_url.to_string(),
            });
            Ok(out)
        }
    }
}

fn normalize_prefix(raw: &str) -> Option<String> {
    let s = raw.trim();
    if !(s.starts_with("https://") || s.starts_with("http://")) {
        return None;
    }
    let mut out = s.to_string();
    if !out.ends_with('/') {
        out.push('/');
    }
    Some(out)
}

/// 解析 GitHub 的 `sha256:<64 hex>`。格式不对返回 `None`，调用方退回 size 校验。
pub fn parse_digest(raw: &str) -> Option<[u8; 32]> {
    let s = raw.trim();
    let s = s
        .strip_prefix("sha256:")
        .or_else(|| s.strip_prefix("SHA256:"))
        .unwrap_or(s);
    if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// 校验已有文件：size 必须匹配（已知时），digest 必须匹配（已知时）。
pub fn verify_file(
    path: &Path,
    digest: Option<&[u8; 32]>,
    expected_size: u64,
) -> Result<(), String> {
    let meta = std::fs::metadata(path).map_err(|e| {
        dlog::dlog(&format!("update: 读取安装包信息失败：{e}"));
        "读取安装包失败".to_string()
    })?;
    if expected_size > 0 && meta.len() != expected_size {
        return Err("安装包大小不符".to_string());
    }
    let Some(expected) = digest else {
        return Ok(());
    };
    let mut file = std::fs::File::open(path).map_err(|e| {
        dlog::dlog(&format!("update: 打开安装包失败：{e}"));
        "读取安装包失败".to_string()
    })?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 128 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(|e| {
            dlog::dlog(&format!("update: 校验安装包时读取失败：{e}"));
            "读取安装包失败".to_string()
        })?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    if hasher.finalize().as_slice() != expected.as_slice() {
        return Err("安装包校验失败".to_string());
    }
    Ok(())
}

/// 从 `url` 下载到 `dest`（覆盖写）。校验失败时保留文件由调用方删除。
pub fn download_one(
    url: &str,
    dest: &Path,
    expected_size: u64,
    digest: Option<&[u8; 32]>,
    on_progress: &mut dyn FnMut(u64, u64),
    cancel: &AtomicBool,
) -> Result<(), String> {
    if cancel.load(Ordering::SeqCst) {
        return Err(CANCELLED.to_string());
    }
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(10 * 60))
        .user_agent(github::user_agent())
        .build()
        .map_err(|e| {
            dlog::warn(&format!("update: 创建下载客户端失败：{e}"));
            "无法创建下载请求".to_string()
        })?;

    let mut resp = client.get(url).send().map_err(|e| {
        dlog::dlog(&format!("update: 连接下载地址失败：{e}"));
        "无法连接下载服务器".to_string()
    })?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("下载服务器返回 HTTP {}", status.as_u16()));
    }
    let total = resp.content_length().unwrap_or(expected_size);

    let mut file = std::fs::File::create(dest).map_err(|e| {
        dlog::warn(&format!("update: 创建安装包文件失败：{e}"));
        "无法创建安装包文件".to_string()
    })?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 128 * 1024];
    let mut got = 0u64;
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(CANCELLED.to_string());
        }
        let n = resp.read(&mut buf).map_err(|e| {
            dlog::dlog(&format!("update: 下载中断：{e}"));
            "下载中断".to_string()
        })?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| {
            dlog::warn(&format!("update: 写入安装包失败：{e}"));
            "写入安装包失败".to_string()
        })?;
        hasher.update(&buf[..n]);
        got += n as u64;
        on_progress(got, total);
    }
    file.sync_all().map_err(|e| {
        dlog::warn(&format!("update: 安装包落盘失败：{e}"));
        "写入安装包失败".to_string()
    })?;
    drop(file);

    if expected_size > 0 && got != expected_size {
        return Err(format!("安装包大小不符（{got} / {expected_size} 字节）"));
    }
    if let Some(expected) = digest {
        if hasher.finalize().as_slice() != expected.as_slice() {
            return Err("安装包校验失败".to_string());
        }
    } else if total > 0 && got != total {
        return Err("安装包大小不符".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::atomic::AtomicU32;
    use std::thread;

    static SEQ: AtomicU32 = AtomicU32::new(0);

    fn temp_dir(tag: &str) -> PathBuf {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "vexto-update-test-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 一次性 HTTP 服务：读到请求后把 `body` 写回去。
    fn serve_once(body: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut req = [0u8; 1024];
                let _ = stream.read(&mut req);
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        format!("http://{addr}/asset.exe")
    }

    fn sha(body: &[u8]) -> [u8; 32] {
        let mut h = Sha256::new();
        h.update(body);
        h.finalize().into()
    }

    #[test]
    fn parse_digest_accepts_prefixed_and_plain_hex() {
        let hex = "acd7081b729453de5d2524e6564f4166b906621fd588a29d8708e7294d06f749";
        assert_eq!(
            parse_digest(&format!("sha256:{hex}")).unwrap()[0],
            0xac
        );
        assert!(parse_digest(hex).is_some());
        assert!(parse_digest("sha256:xyz").is_none());
        assert!(parse_digest(&hex[..62]).is_none());
    }

    #[test]
    fn candidates_follow_configured_source() {
        let auto = candidates("auto", "", "https://github.com/x/y.exe").unwrap();
        assert_eq!(auto.len(), 3);
        assert!(auto[0].url.starts_with("https://v4.gh-proxy.org/"));
        assert_eq!(auto[2].url, "https://github.com/x/y.exe");

        let direct = candidates("direct", "", "https://github.com/x/y.exe").unwrap();
        assert_eq!(direct.len(), 1);
        assert_eq!(direct[0].url, "https://github.com/x/y.exe");

        // 未知值回落 auto，不报错
        assert_eq!(
            candidates("typo", "", "https://github.com/x/y.exe")
                .unwrap()
                .len(),
            3
        );
    }

    #[test]
    fn custom_proxy_prefix_is_normalized() {
        let list = candidates("custom", "https://example.com", "https://github.com/x/y.exe")
            .unwrap();
        assert_eq!(list[0].url, "https://example.com/https://github.com/x/y.exe");
        assert!(candidates("custom", "example.com", "https://github.com/x/y.exe").is_err());
        assert!(candidates("custom", "", "https://github.com/x/y.exe").is_err());
    }

    #[test]
    fn download_writes_streams_and_verifies_digest() {
        let body = vec![7u8; 300 * 1024];
        let url = serve_once(body.clone());
        let dir = temp_dir("ok");
        let dest = dir.join("a.exe");
        let cancel = AtomicBool::new(false);
        let expected = sha(&body);
        let mut last = 0u64;
        let mut calls = 0;
        download_one(
            &url,
            &dest,
            body.len() as u64,
            Some(&expected),
            &mut |got, _total| {
                last = got;
                calls += 1;
            },
            &cancel,
        )
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert_eq!(last, body.len() as u64);
        assert!(calls >= 2, "应至少分两块回调，实际 {calls}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn download_rejects_wrong_digest() {
        let body = vec![1u8; 1024];
        let url = serve_once(body.clone());
        let dir = temp_dir("bad");
        let dest = dir.join("a.exe");
        let cancel = AtomicBool::new(false);
        let wrong = [0u8; 32];
        let err = download_one(
            &url,
            &dest,
            body.len() as u64,
            Some(&wrong),
            &mut |_, _| {},
            &cancel,
        )
        .unwrap_err();
        assert!(err.contains("校验"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn download_rejects_wrong_size_when_digest_missing() {
        let body = vec![1u8; 100];
        let url = serve_once(body);
        let dir = temp_dir("size");
        let dest = dir.join("a.exe");
        let cancel = AtomicBool::new(false);
        let err = download_one(&url, &dest, 999, None, &mut |_, _| {}, &cancel).unwrap_err();
        assert!(err.contains("大小"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancelled_download_stops_before_writing() {
        let dir = temp_dir("cancel");
        let dest = dir.join("a.exe");
        let cancel = AtomicBool::new(true);
        let err = download_one(
            "http://127.0.0.1:1/never",
            &dest,
            1,
            None,
            &mut |_, _| {},
            &cancel,
        )
        .unwrap_err();
        assert_eq!(err, CANCELLED);
        assert!(!dest.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_file_checks_size_and_digest() {
        let dir = temp_dir("verify");
        let path = dir.join("a.exe");
        let body = b"hello";
        std::fs::write(&path, body).unwrap();
        let digest = sha(body);
        assert!(verify_file(&path, Some(&digest), body.len() as u64).is_ok());
        assert!(verify_file(&path, Some(&[0u8; 32]), body.len() as u64).is_err());
        assert!(verify_file(&path, None, 99).is_err());
        assert!(verify_file(&path, None, body.len() as u64).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
