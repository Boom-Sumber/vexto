//! 下载代理：直连 / 跟随系统 / 手动（S3.5 第二轮）。
//!
//! **aria2 不会读 Windows 的 WinINET 代理设置**——`engine.rs` 的 spawn 参数里
//! 没有任何代理项，所以公司代理 / 本地 Clash 这类环境里，所有任务都会连接超时，
//! 而 aria2 报的还是「连接被重置」这种看不出原因的错误。这里把「用户选的那一档」
//! 翻译成 aria2 的选项：
//!
//! - `direct`：什么都不设（aria2 默认直连）
//! - `system`：读 `HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings`
//!   的 `ProxyEnable` / `ProxyServer`
//! - `manual`：用户填的地址，没写 scheme 时补 `http://`
//!
//! 两个必须注意的点：
//! 1. **aria2 不解析 `all-proxy` 里的 userinfo**。`http://u:p@h:8080` 整串塞进去
//!    会连不上，必须拆成 `all-proxy-user` / `all-proxy-passwd`。
//! 2. 代理一旦生效，**本机回环必须排除**（`no-proxy`）。否则连 `127.0.0.1` 的
//!    自建下载源、局域网 NAS 都会被绕到代理上，表现为「本地文件都下不动」。

use crate::config;
use crate::dlog;

/// 一条解析好的代理配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxySpec {
    /// 不含 userinfo 的代理地址，例如 `http://127.0.0.1:7890`
    pub url: String,
    pub user: Option<String>,
    pub passwd: Option<String>,
}

/// 把用户填的地址规范化成 aria2 能吃的形式。
///
/// 返回 `None` 表示「这串东西没法当代理用」——空串、只有 scheme、协议不认识。
/// 此时**保持直连**并让调用方写一条日志，比把非法值塞给 aria2 让它起不来强。
pub fn normalize(raw: &str) -> Option<ProxySpec> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    let (scheme, rest) = match s.split_once("://") {
        Some((sch, r)) => (sch.trim().to_ascii_lowercase(), r.trim().to_string()),
        // 只填了 `127.0.0.1:7890`：按最常用的 HTTP 代理处理
        None => ("http".to_string(), s.to_string()),
    };
    // 只放 aria2 真正支持的三类；`socks5h` 之类是 curl 的方言，aria2 不认
    let scheme = match scheme.as_str() {
        "http" | "https" | "socks5" | "socks4" => scheme,
        _ => return None,
    };
    let rest = rest.trim_end_matches('/');
    if rest.is_empty() {
        return None;
    }
    // 用 rsplit：密码里可能含 `@`（未转义时），最后一个 `@` 才是分隔符
    let (userinfo, host) = match rest.rsplit_once('@') {
        Some((u, h)) => (Some(u), h),
        None => (None, rest),
    };
    if host.is_empty() {
        return None;
    }
    let (user, passwd) = match userinfo {
        Some(u) if !u.is_empty() => match u.split_once(':') {
            Some((a, b)) => (Some(a.to_string()), Some(b.to_string())),
            None => (Some(u.to_string()), None),
        },
        _ => (None, None),
    };
    Some(ProxySpec {
        url: format!("{scheme}://{host}"),
        user,
        passwd,
    })
}

/// 解析 WinINET 的 `ProxyServer` 值。
///
/// 两种形态：
/// - `127.0.0.1:7890` —— 所有协议共用一个代理；
/// - `http=1.2.3.4:8080;https=1.2.3.4:8080;socks=1.2.3.4:1080`
///   —— 按协议分开，优先 `http`（aria2 下 http/https 都走 all-proxy 一个口）。
///
/// 分段里出现 `socks=` 时要补 `socks5://` 前缀，否则会当成 http 代理去连。
pub fn pick_from_wininet(server: &str) -> Option<ProxySpec> {
    let server = server.trim();
    if server.is_empty() {
        return None;
    }
    if !server.contains('=') {
        return normalize(server);
    }
    let mut http: Option<&str> = None;
    let mut https: Option<&str> = None;
    let mut socks: Option<&str> = None;
    for part in server.split(';') {
        let Some((k, v)) = part.split_once('=') else {
            continue;
        };
        let v = v.trim();
        if v.is_empty() {
            continue;
        }
        match k.trim().to_ascii_lowercase().as_str() {
            "http" => http = Some(v),
            "https" => https = Some(v),
            "socks" | "socks5" => socks = Some(v),
            _ => {}
        }
    }
    if let Some(v) = http.or(https) {
        return normalize(v);
    }
    normalize(&format!("socks5://{}", socks?))
}

/// 读系统代理。`ProxyEnable` 为 0 时返回 `None`（= 直连）。
#[cfg(windows)]
pub fn from_system() -> Option<ProxySpec> {
    if !reg_dword("ProxyEnable").is_some_and(|v| v != 0) {
        return None;
    }
    let server = reg_string("ProxyServer")?;
    let spec = pick_from_wininet(&server);
    if spec.is_none() {
        dlog::dlog(&format!("proxy: 系统代理地址无法解析，已按直连处理：{server}"));
    }
    spec
}

#[cfg(not(windows))]
pub fn from_system() -> Option<ProxySpec> {
    None
}

#[cfg(windows)]
fn reg_string(name: &str) -> Option<String> {
    let raw = reg_value_bytes(name, REG_SZ_TYPE)?;
    // REG_SZ 是 UTF-16LE，末尾带一个 `\0`
    if raw.len() < 2 || raw.len() % 2 != 0 {
        return None;
    }
    // `as_chunks::<2>()` 直接给出 `&[[u8; 2]]`，避免 clippy 的
    // `chunks_exact_to_as_chunks` 提示，也省掉一次逐字节下标取值。
    let wide: Vec<u16> = raw
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .take_while(|c| *c != 0)
        .collect();
    Some(String::from_utf16_lossy(&wide))
}

#[cfg(windows)]
fn reg_dword(name: &str) -> Option<u32> {
    let raw = reg_value_bytes(name, REG_DWORD_TYPE)?;
    let b: [u8; 4] = raw.get(..4)?.try_into().ok()?;
    Some(u32::from_le_bytes(b))
}

#[cfg(windows)]
const REG_SZ_TYPE: u32 = 1;
#[cfg(windows)]
const REG_DWORD_TYPE: u32 = 4;

/// 读 `HKCU\...\Internet Settings\<name>` 的原始字节。
///
/// 用 `RegGetValueW` 而不是 `RegOpenKeyEx` + `RegQueryValueEx`：后者要自己
/// 管关句柄，一个 return 忘了关就是句柄泄漏；前者一次调用搞定。
#[cfg(windows)]
fn reg_value_bytes(name: &str, expect_type: u32) -> Option<Vec<u8>> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RRF_RT_REG_SZ};

    let sub = HSTRING::from(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings");
    let val = HSTRING::from(name);
    let flags = if expect_type == REG_DWORD_TYPE {
        RRF_RT_REG_DWORD
    } else {
        RRF_RT_REG_SZ
    };

    // 先问需要多少字节：传 NULL 缓冲区 + 长度为 0
    let mut size: u32 = 0;
    let probe = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(sub.as_ptr()),
            PCWSTR(val.as_ptr()),
            flags,
            None,
            None,
            Some(&mut size),
        )
    };
    if probe.is_err() || size == 0 || size > 64 * 1024 {
        return None;
    }
    let mut buf = vec![0u8; size as usize];
    let got = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(sub.as_ptr()),
            PCWSTR(val.as_ptr()),
            flags,
            None,
            Some(buf.as_mut_ptr() as *mut core::ffi::c_void),
            Some(&mut size),
        )
    };
    if got.is_err() {
        return None;
    }
    buf.truncate(size as usize);
    Some(buf)
}

/// 当前配置下应该用的代理。`None` = 直连。
pub fn resolve(cfg: &config::Config) -> Option<ProxySpec> {
    match cfg.proxy_mode.trim() {
        "system" => from_system(),
        "manual" => {
            let spec = normalize(&cfg.proxy_url);
            if spec.is_none() && !cfg.proxy_url.trim().is_empty() {
                dlog::dlog(&format!(
                    "proxy: 手动代理地址无法解析，已按直连处理：{}",
                    cfg.proxy_url
                ));
            }
            spec
        }
        // `direct` 与任何未知值都按直连：配置被手改成乱码时也不能下不动
        _ => None,
    }
}

/// 写进 aria2 conf 的若干行（`key=value`）。直连时返回空。
pub fn conf_lines(cfg: &config::Config) -> Vec<String> {
    let Some(p) = resolve(cfg) else {
        return Vec::new();
    };
    let mut v = vec![format!("all-proxy={}", p.url)];
    if let Some(u) = p.user {
        v.push(format!("all-proxy-user={u}"));
    }
    if let Some(pw) = p.passwd {
        v.push(format!("all-proxy-passwd={pw}"));
    }
    // 本机与内网必须直连：否则 127.0.0.1 的测试源、局域网 NAS 都走代理
    v.push("no-proxy=localhost,127.0.0.1,::1".to_string());
    v
}

/// 状态行用的一句话摘要。
pub fn describe(cfg: &config::Config) -> String {
    match cfg.proxy_mode.trim() {
        "system" => match from_system() {
            Some(p) => format!("跟随系统代理（{}）", p.url),
            None => "跟随系统代理（系统未启用，当前直连）".to_string(),
        },
        "manual" => match normalize(&cfg.proxy_url) {
            Some(p) => format!("手动代理（{}）", p.url),
            None => "手动代理（地址无效，当前直连）".to_string(),
        },
        _ => "直连".to_string(),
    }
}

/// 改完设置后立刻让运行中的 aria2 换代理，不用重启 sidecar。
///
/// `all-proxy` 是 aria2 明确支持热改的选项（`aria2.changeGlobalOption`）。
/// 换成「直连」时传空串——aria2 用它表示「清空该选项」。
/// 引擎没在跑时直接返回，下次 spawn 会从 conf 里读到新值。
pub fn apply_runtime(cfg: &config::Config) {
    let Some(rpc) = crate::download::engine::rpc_handle() else {
        return;
    };
    let mut opt = serde_json::Map::new();
    match resolve(cfg) {
        Some(p) => {
            opt.insert("all-proxy".into(), serde_json::json!(p.url));
            opt.insert(
                "all-proxy-user".into(),
                serde_json::json!(p.user.unwrap_or_default()),
            );
            opt.insert(
                "all-proxy-passwd".into(),
                serde_json::json!(p.passwd.unwrap_or_default()),
            );
        }
        None => {
            opt.insert("all-proxy".into(), serde_json::json!(""));
            opt.insert("all-proxy-user".into(), serde_json::json!(""));
            opt.insert("all-proxy-passwd".into(), serde_json::json!(""));
        }
    }
    match rpc.change_global_option(serde_json::Value::Object(opt)) {
        Ok(()) => dlog::dlog(&format!("proxy: 运行时已切换为 {}", describe(cfg))),
        Err(e) => dlog::dlog(&format!("proxy: 运行时切换失败（重启 aria2c 后生效）：{e}")),
    }
}

/// 把代理相关的环境变量从 aria2 子进程里剔掉。
///
/// **aria2 会读 `http_proxy` / `https_proxy` / `all_proxy` / `no_proxy` 环境变量，
/// 而且优先级高于我们写进 conf 的 `all-proxy`。** 本机实测：conf 里写
/// `all-proxy=http://127.0.0.1:9`（一个没人监听的端口），任务照样下载成功，
/// 自建代理一条请求都没收到——因为 shell 里的 `http_proxy` 把它盖掉了。
///
/// 用户在我们的设置里选了代理或直连，就该以设置为准；被上层的残留变量悄悄
/// 改掉是最难排查的一类问题。所以子进程一律不带这些变量。
pub fn clear_env(cmd: &mut std::process::Command) {
    for k in [
        "http_proxy",
        "https_proxy",
        "ftp_proxy",
        "all_proxy",
        "no_proxy",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "FTP_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
    ] {
        cmd.env_remove(k);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_adds_default_scheme() {
        let p = normalize("127.0.0.1:7890").unwrap();
        assert_eq!(p.url, "http://127.0.0.1:7890");
        assert!(p.user.is_none());
    }

    #[test]
    fn normalize_splits_userinfo() {
        let p = normalize("http://u:p%40ss@10.0.0.1:8080").unwrap();
        assert_eq!(p.url, "http://10.0.0.1:8080");
        assert_eq!(p.user.as_deref(), Some("u"));
        assert_eq!(p.passwd.as_deref(), Some("p%40ss"));
    }

    #[test]
    fn normalize_keeps_socks_and_rejects_unknown() {
        assert_eq!(
            normalize("socks5://127.0.0.1:1080").unwrap().url,
            "socks5://127.0.0.1:1080"
        );
        // curl 方言，aria2 不认，必须当无效处理而不是硬塞
        assert!(normalize("socks5h://127.0.0.1:1080").is_none());
        assert!(normalize("").is_none());
        assert!(normalize("http://").is_none());
    }

    #[test]
    fn wininet_plain_and_per_protocol() {
        assert_eq!(
            pick_from_wininet("127.0.0.1:7890").unwrap().url,
            "http://127.0.0.1:7890"
        );
        assert_eq!(
            pick_from_wininet("http=1.2.3.4:8080;https=1.2.3.4:8443")
                .unwrap()
                .url,
            "http://1.2.3.4:8080"
        );
        // 只有 socks 时要补 scheme，否则会当 http 代理连过去
        assert_eq!(
            pick_from_wininet("socks=1.2.3.4:1080").unwrap().url,
            "socks5://1.2.3.4:1080"
        );
        assert!(pick_from_wininet("   ").is_none());
    }

    fn cfg_with(mode: &str, url: &str) -> config::Config {
        config::Config {
            proxy_mode: mode.to_string(),
            proxy_url: url.to_string(),
            ..config::Config::default()
        }
    }

    #[test]
    fn conf_lines_only_when_proxy_configured() {
        assert!(conf_lines(&cfg_with("direct", "http://x:1")).is_empty());

        let lines = conf_lines(&cfg_with("manual", "http://127.0.0.1:7890"));
        assert_eq!(lines[0], "all-proxy=http://127.0.0.1:7890");
        assert!(!lines.iter().any(|l| l.starts_with("all-proxy-user=")));
        // 本机必须排除，否则本地下载源也被绕到代理上
        assert!(lines.iter().any(|l| l.starts_with("no-proxy=")));
    }

    #[test]
    fn conf_lines_carry_credentials_separately() {
        let lines = conf_lines(&cfg_with("manual", "http://u:p@1.2.3.4:8080"));
        assert!(lines.contains(&"all-proxy=http://1.2.3.4:8080".to_string()));
        assert!(lines.contains(&"all-proxy-user=u".to_string()));
        assert!(lines.contains(&"all-proxy-passwd=p".to_string()));
    }

    #[test]
    fn describe_is_honest_about_invalid_manual() {
        assert_eq!(describe(&cfg_with("direct", "")), "直连");
        assert!(describe(&cfg_with("manual", "ftp://x")).contains("地址无效"));
    }
}
