//! 浏览器扩展接管：兼容 Rayburst-Connect 的本地 HTTP 契约（protocol v3）。
//!
//! 用户在浏览器里点下载时，扩展会先探测本机接口，然后把「任务」POST 过来：
//!
//! | 方法 | 路径 | 作用 |
//! |---|---|---|
//! | GET | `/ping` | 产品发现（无鉴权） |
//! | GET | `/stat` | 就绪探针（读 pump 缓存，不现取 aria2） |
//! | GET | `/downloads/capabilities` | 声明 `protocolVersion = 3` |
//! | POST | `/add` | 交接下载：立刻回 `accepted`，并把任务预填进新建页 |
//!
//! 为什么 `/ping` 必须自称 `product:"rayburst"`：扩展用这个字段做产品门，
//! 不认识的产品直接判「装最新版 Rayburst 再来」。这是**协议兼容标签**，
//! 不是品牌替换，设置页与文档都按这个口径写。
//!
//! 安全边界（与 Rayburst 同口径）：
//! - 只绑 `127.0.0.1`，任何情况下都不绑 `0.0.0.0`；
//! - 除 `/ping` 与预检外，必须带 `x-rayburst-client: rayburst-connect`；
//! - CORS 只放 `chrome-extension://` 与 `moz-extension://` 源；
//! - 配了密钥就必须带 `Authorization: Bearer <secret>`（空 = 不校验）；
//! - Cookie 只进 aria2 的请求头，**不写日志、不进界面**。
//!
//! 唤醒限制：扩展只会用 native messaging 唤起 Rayburst / Motrix 的主机名。
//! 我们不冒充别人的身份，所以 Vexto 没运行时扩展唤不起来，浏览器会自己
//! 下载 —— 要接管就保持托盘常驻（可开机自启）。

use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use serde_json::{json, Value};
use tauri::AppHandle;

/// 请求头上限。扩展只会发几百字节，8 KiB 足够且能挡住畸形请求。
const MAX_HEAD: usize = 8 * 1024;
/// 请求体上限。`/add` 里最多是 cookie + 少量请求头，256 KiB 已经很宽松。
const MAX_BODY: usize = 256 * 1024;

/// 每次 `start` / `stop` 递增；服务线程只在代数没变时继续 accept。
/// 比单个 bool 可靠：重启时旧线程即使还卡在 accept 里，也会因代数变化退出，
/// 不会和新线程同时对外服务。
static GENERATION: AtomicU64 = AtomicU64::new(0);
/// 当前是否真的绑上了端口（绑定失败时为 false，设置页据此显示红字）。
static LISTENING: AtomicBool = AtomicBool::new(false);
/// 已接收的下载数，只用于设置页状态行。
static ACCEPTED: AtomicU64 = AtomicU64::new(0);
static LAST_ERROR: Mutex<Option<String>> = Mutex::new(None);

/// 启动（或按当前配置启动）监听。重复调用是安全的。
pub fn start(app: AppHandle) {
    let cfg = crate::config::load();
    if !cfg.extension_bridge_enabled {
        stop();
        set_error(None);
        return;
    }
    let port = cfg.extension_bridge_port;
    let secret = cfg.extension_bridge_secret.clone();
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    std::thread::spawn(move || serve(app, generation, port, secret));
}

/// 配置改了（开关 / 端口 / 密钥）：停掉旧监听，稍等它释放端口，再起新的。
pub fn restart(app: AppHandle) {
    stop();
    std::thread::sleep(Duration::from_millis(250));
    start(app);
}

/// 停止监听。线程在下一轮轮询（≤120 ms）自行退出。
pub fn stop() {
    GENERATION.fetch_add(1, Ordering::SeqCst);
    LISTENING.store(false, Ordering::SeqCst);
}

/// 设置页状态行。
pub fn status_line() -> String {
    let cfg = crate::config::load();
    if !cfg.extension_bridge_enabled {
        return "浏览器插件接管：已关闭".to_string();
    }
    if let Ok(g) = LAST_ERROR.lock() {
        if let Some(e) = g.as_ref() {
            return format!("浏览器插件接管：{e}");
        }
    }
    if LISTENING.load(Ordering::SeqCst) {
        format!(
            "浏览器插件接管：监听 127.0.0.1:{} · 已接收 {} 个下载",
            cfg.extension_bridge_port,
            ACCEPTED.load(Ordering::SeqCst)
        )
    } else {
        "浏览器插件接管：正在启动…".to_string()
    }
}

fn set_error(msg: Option<String>) {
    if let Ok(mut g) = LAST_ERROR.lock() {
        *g = msg;
    }
}

// ── 服务循环 ────────────────────────────────────────────────────────────

fn serve(app: AppHandle, generation: u64, port: u16, secret: String) {
    // 重启时旧监听可能还没释放端口，重试几次比直接失败友好。
    let mut listener = None;
    let mut last_err = String::new();
    for _ in 0..6 {
        if GENERATION.load(Ordering::SeqCst) != generation {
            return;
        }
        match TcpListener::bind(("127.0.0.1", port)) {
            Ok(l) => {
                listener = Some(l);
                break;
            }
            Err(e) => {
                last_err = e.to_string();
                std::thread::sleep(Duration::from_millis(120));
            }
        }
    }
    let Some(listener) = listener else {
        LISTENING.store(false, Ordering::SeqCst);
        let msg = format!("端口 {port} 监听失败：{last_err}");
        crate::dlog::dlog(&format!("bridge: {msg}"));
        set_error(Some(msg));
        return;
    };
    if listener.set_nonblocking(true).is_err() {
        crate::dlog::dlog("bridge: 设置非阻塞失败，仍继续（accept 会阻塞）");
    }
    LISTENING.store(true, Ordering::SeqCst);
    set_error(None);
    crate::dlog::dlog(&format!("bridge: 监听 127.0.0.1:{port}"));

    while GENERATION.load(Ordering::SeqCst) == generation {
        match listener.accept() {
            Ok((stream, _)) => {
                let app = app.clone();
                let secret = secret.clone();
                std::thread::spawn(move || {
                    if generation != GENERATION.load(Ordering::SeqCst) {
                        return;
                    }
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(8)));
                    let _ = stream.set_write_timeout(Some(Duration::from_secs(8)));
                    handle(stream, app, secret);
                });
            }
            Err(ref e) if e.kind() == ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(120));
            }
            Err(e) => {
                crate::dlog::error(&format!("bridge: accept 失败：{e}"));
                std::thread::sleep(Duration::from_millis(300));
            }
        }
    }
    LISTENING.store(false, Ordering::SeqCst);
    crate::dlog::dlog("bridge: 已停止监听");
}

// ── 单条请求 ────────────────────────────────────────────────────────────

struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

fn handle(mut stream: TcpStream, app: AppHandle, secret: String) {
    let req = match read_request(&mut stream) {
        Ok(r) => r,
        Err(e) => {
            let _ = write_json(&mut stream, 400, None, &json!({ "error": e }));
            return;
        }
    };

    // CORS：只允许扩展源；没有 Origin 的是本机脚本/测试，放行但不回 CORS 头。
    let origin = match req.header("origin") {
        None => None,
        Some(o) => match cors_origin(o) {
            Some(o) => Some(o),
            None => {
                let _ = write_json(&mut stream, 403, None, &json!({ "error": "origin not allowed" }));
                return;
            }
        },
    };

    // 预检不带自定义头，必须在产品头校验之前处理。
    if req.method == "OPTIONS" {
        let _ = write_preflight(&mut stream, origin);
        return;
    }

    if !product_ok(&req) {
        let _ = write_json(
            &mut stream,
            403,
            origin,
            &json!({ "error": "missing or wrong x-rayburst-client" }),
        );
        return;
    }
    if !bearer_ok(&req, &secret) {
        let _ = write_json(&mut stream, 401, origin, &json!({ "error": "unauthorized" }));
        return;
    }

    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/ping") => {
            let _ = write_json(
                &mut stream,
                200,
                origin,
                &json!({
                    "product": "rayburst",
                    "status": "ok",
                    "version": env!("CARGO_PKG_VERSION"),
                }),
            );
        }
        ("GET", "/stat") => {
            let _ = write_json(&mut stream, 200, origin, &stat_json());
        }
        ("GET", "/downloads/capabilities") => {
            let _ = write_json(
                &mut stream,
                200,
                origin,
                &json!({ "product": "rayburst", "protocolVersion": 3 }),
            );
        }
        // Rayburst Connect 2.2.x 在下载前会先 `GET /version` 判断桌面引擎
        // 是否 ready（`engine === "running"`）。没有这个端点时 isReady 永远
        // false，扩展会判定 desktop-unavailable 并把下载还给浏览器。
        // Vexto 的 aria2 是按需启动的，只要下载工具启用就视为可用。
        ("GET", "/version") => {
            let enabled = crate::config::load().enable_download;
            let _ = write_json(
                &mut stream,
                200,
                origin,
                &json!({
                    "app": "Vexto",
                    "engine": if enabled { "running" } else { "failed" },
                }),
            );
        }
        ("POST", "/add") => add_download(&mut stream, &app, origin, &req.body),
        _ => {
            let _ = write_json(&mut stream, 404, origin, &json!({ "error": "not found" }));
        }
    }
}

fn add_download(stream: &mut TcpStream, app: &AppHandle, origin: Option<&str>, body: &[u8]) {
    // 工具没开时不要假装接管：回 fallback 让浏览器自己下，比「接管了又不干活」好。
    if !crate::config::load().enable_download {
        set_error(Some("下载工具未启用，已拒绝扩展请求".into()));
        let _ = write_json(
            stream,
            200,
            origin,
            &json!({ "action": "fallback", "reason": "rejected" }),
        );
        return;
    }

    let entry = match prefill_from_add(body) {
        Ok(e) => e,
        Err(e) => {
            crate::dlog::dlog(&format!("bridge: 拒绝一个下载请求：{e}"));
            let _ = write_json(
                stream,
                200,
                origin,
                &json!({ "action": "fallback", "reason": "rejected" }),
            );
            return;
        }
    };

    // 先把任务填进新建页，再回 accepted。扩展收到 accepted 就会取消浏览器
    // 自己那份下载，所以「用户选了立即接管」的前提下，顺序必须是先开窗。
    let a = app.clone();
    let _ = app.run_on_main_thread(move || {
        crate::popup::open_new_task(&a, Some(entry));
    });
    ACCEPTED.fetch_add(1, Ordering::SeqCst);
    set_error(None);
    let _ = write_json(stream, 200, origin, &json!({ "action": "accepted" }));
}

/// `GET /stat`：返回 pump 缓存的全局统计。
///
/// **不能现取**：扩展给这个请求的超时只有 500 ms，aria2 忙时一次 RPC 就可能
/// 超过，探针会误判桌面端不可用。缓存为空（aria2 没起）时按 0 返回，就绪检查
/// 只要求字段存在且是字符串。
fn stat_json() -> Value {
    let snap = crate::download::pump::last_snapshot();
    let g = snap.as_ref().map(|s| &s.global);
    let get = |f: fn(&crate::download::rpc::GlobalStat) -> u64| {
        g.map(f).unwrap_or(0).to_string()
    };
    json!({
        "downloadSpeed": get(|g| g.download_speed),
        "uploadSpeed": get(|g| g.upload_speed),
        "numActive": get(|g| g.num_active),
        "numWaiting": get(|g| g.num_waiting),
        "numStopped": get(|g| g.num_stopped),
        "numStoppedTotal": get(|g| g.num_stopped),
    })
}

// ── 纯函数（全部有单测） ─────────────────────────────────────────────────

fn cors_origin(origin: &str) -> Option<&str> {
    if origin.starts_with("chrome-extension://") || origin.starts_with("moz-extension://") {
        Some(origin)
    } else {
        None
    }
}

/// `/ping` 是产品发现，扩展不会带产品头；`/version` 是引擎就绪探针，
/// Rayburst Connect 2.2.x 调用 `engineState()` 时**也不带**产品头/密钥。
/// 其余接口必须带。
fn product_ok(req: &Request) -> bool {
    req.path == "/ping"
        || req.path == "/version"
        || req
            .header("x-rayburst-client")
            .map(|v| v.eq_ignore_ascii_case("rayburst-connect"))
            .unwrap_or(false)
}

/// 密钥为空 = 不校验（与 Rayburst 语义一致）。
fn bearer_ok(req: &Request, secret: &str) -> bool {
    if secret.is_empty() || req.path == "/ping" || req.path == "/version" {
        return true;
    }
    req.header("authorization")
        .map(|v| v == format!("Bearer {secret}"))
        .unwrap_or(false)
}

/// 扩展给的文件名可能是完整路径或带非法字符，这里只留一个安全的基名。
/// 扩展交过来的文件名只取基名，清洗规则与下载引擎**共用一份**
/// （`naming::sanitize`：非法字符、保留名、长度上限都在那里）。
///
/// 不能各写一套：这里松一点、那边严一点，会出现「预填显示 `CON`、真正入队
/// 变成 `_CON`」这种对不上的现象。
fn sanitize_filename(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or(raw);
    crate::download::naming::sanitize(base)
}

fn prefill_from_add(body: &[u8]) -> Result<crate::popup::NewTaskPrefill, String> {
    let v: Value = serde_json::from_slice(body).map_err(|e| format!("JSON 解析失败：{e}"))?;
    let url = v
        .get("url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "缺少 url".to_string())?;
    // 支持哪些协议只认一处：`download::looks_like_url`。
    // 磁力现在被那边关着（ALLOW_MAGNET = false），所以扩展交来磁力会走 fallback
    // 让浏览器自己下——比「接管了却下不了」诚实。
    if !crate::download::looks_like_url(url) {
        return Err(format!("不支持的链接类型：{url}"));
    }

    let text = |key: &str| {
        v.get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };

    Ok(crate::popup::NewTaskPrefill {
        url: url.to_string(),
        // 文件名只取基名；空串当没有。
        filename: text("filename")
            .map(|s| sanitize_filename(&s))
            .filter(|s| !s.is_empty()),
        // 扩展给的 size 是浏览器已看到的总字节数；0 当未知。
        size: v.get("size").and_then(Value::as_u64).filter(|n| *n > 0),
        // Referer 用于过防盗链；缺失就不传（不要拿 finalUrl 顶替）。
        referer: text("referer"),
        user_agent: text("userAgent"),
        cookie: text("cookie"),
    })
}

// ── HTTP 读写 ───────────────────────────────────────────────────────────

fn read_request(stream: &mut TcpStream) -> Result<Request, String> {
    let mut buf: Vec<u8> = Vec::with_capacity(2048);
    let mut chunk = [0u8; 2048];
    let head_end = loop {
        if let Some(p) = find(&buf, b"\r\n\r\n") {
            break p;
        }
        if buf.len() > MAX_HEAD {
            return Err("请求头过大".into());
        }
        let n = stream.read(&mut chunk).map_err(|e| format!("读取失败：{e}"))?;
        if n == 0 {
            return Err("连接在请求头结束前关闭".into());
        }
        buf.extend_from_slice(&chunk[..n]);
    };

    let head = String::from_utf8_lossy(&buf[..head_end]);
    let mut req = parse_head(&head)?;

    let want = req
        .header("content-length")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);
    if want > MAX_BODY {
        return Err("请求体过大".into());
    }
    req.body.extend_from_slice(&buf[head_end + 4..]);
    while req.body.len() < want {
        let n = stream.read(&mut chunk).map_err(|e| format!("读取请求体失败：{e}"))?;
        if n == 0 {
            break;
        }
        req.body.extend_from_slice(&chunk[..n]);
    }
    req.body.truncate(want);
    Ok(req)
}

/// 解析请求行 + 头。单独拆出来是为了能直接跑单测（不需要真开 socket）。
fn parse_head(head: &str) -> Result<Request, String> {
    let mut lines = head.split("\r\n");
    let first = lines.next().ok_or_else(|| "空请求".to_string())?;
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("").to_ascii_uppercase();
    let target = parts.next().unwrap_or("/");
    if method.is_empty() {
        return Err("缺少请求方法".into());
    }
    let path = target.split('?').next().unwrap_or("/").to_string();
    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    Ok(Request {
        method,
        path,
        headers,
        body: Vec::new(),
    })
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn write_json(
    stream: &mut TcpStream,
    status: u16,
    origin: Option<&str>,
    body: &Value,
) -> std::io::Result<()> {
    let text = body.to_string();
    let reason = match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        _ => "Error",
    };
    let mut head = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: application/json; charset=utf-8\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n",
        text.len()
    );
    if let Some(o) = origin {
        head.push_str(&format!(
            "Access-Control-Allow-Origin: {o}\r\nVary: Origin\r\n"
        ));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.write_all(text.as_bytes())?;
    stream.flush()
}

fn write_preflight(stream: &mut TcpStream, origin: Option<&str>) -> std::io::Result<()> {
    let mut head = String::from("HTTP/1.1 204 No Content\r\n");
    if let Some(o) = origin {
        head.push_str(&format!(
            "Access-Control-Allow-Origin: {o}\r\nVary: Origin\r\n"
        ));
    }
    head.push_str(
        "Access-Control-Allow-Methods: GET, POST, OPTIONS\r\n\
         Access-Control-Allow-Headers: content-type, x-rayburst-client, authorization\r\n\
         Access-Control-Max-Age: 600\r\n\
         Content-Length: 0\r\n\
         Connection: close\r\n\r\n",
    );
    stream.write_all(head.as_bytes())?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(method: &str, path: &str, headers: &[(&str, &str)]) -> Request {
        Request {
            method: method.into(),
            path: path.into(),
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            body: Vec::new(),
        }
    }

    #[test]
    fn parses_request_line_and_headers() {
        let head = "POST /add?x=1 HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Rayburst-Client: rayburst-connect\r\nContent-Length: 42";
        let r = parse_head(head).unwrap();
        assert_eq!(r.method, "POST");
        assert_eq!(r.path, "/add");
        assert_eq!(r.header("x-rayburst-client"), Some("rayburst-connect"));
        assert_eq!(r.header("CONTENT-LENGTH"), Some("42"));
    }

    #[test]
    fn cors_only_allows_extension_origins() {
        assert!(cors_origin("chrome-extension://abc").is_some());
        assert!(cors_origin("moz-extension://abc").is_some());
        assert!(cors_origin("https://evil.test").is_none());
        assert!(cors_origin("http://127.0.0.1").is_none());
    }

    #[test]
    fn product_header_required_except_ping_and_version() {
        assert!(product_ok(&req("GET", "/ping", &[])));
        assert!(product_ok(&req("GET", "/version", &[])));
        assert!(!product_ok(&req("GET", "/stat", &[])));
        assert!(product_ok(&req(
            "GET",
            "/stat",
            &[("x-rayburst-client", "rayburst-connect")]
        )));
        // 值必须精确匹配，大小写不敏感但多了别的字不行
        assert!(!product_ok(&req(
            "GET",
            "/stat",
            &[("x-rayburst-client", "rayburst-connect-x")]
        )));
    }

    #[test]
    fn bearer_is_optional_until_secret_set() {
        assert!(bearer_ok(&req("GET", "/stat", &[]), ""));
        assert!(!bearer_ok(&req("GET", "/stat", &[]), "s3cret"));
        assert!(bearer_ok(
            &req("GET", "/stat", &[("authorization", "Bearer s3cret")]),
            "s3cret"
        ));
        assert!(bearer_ok(&req("GET", "/ping", &[]), "s3cret"));
        // engineState() 不带 Authorization：密钥启用时也不能把它挡掉。
        assert!(bearer_ok(&req("GET", "/version", &[]), "s3cret"));
    }

    #[test]
    fn filename_is_reduced_to_a_safe_basename() {
        assert_eq!(sanitize_filename(r"C:\tmp\a.zip"), "a.zip");
        assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
        // 与下载引擎共用 naming::sanitize：非法字符**替换成 `_`** 而不是删掉，
        // 否则 `a:b` 会粘成 `ab`，用户认不出原文件名
        assert_eq!(sanitize_filename("a:b?.zip"), "a_b_.zip");
        assert_eq!(sanitize_filename("   "), "");
    }

    #[test]
    fn add_request_maps_to_prefill() {
        let body = br#"{
            "url":"https://example.test/a.zip",
            "finalUrl":"https://example.test/a.zip",
            "referer":"https://example.test/page",
            "cookie":"sid=1; theme=dark",
            "filename":"C:\\Users\\x\\Downloads\\a.zip",
            "userAgent":"Mozilla/5.0",
            "size":1331,
            "ack":"early"
        }"#;
        let p = prefill_from_add(body).unwrap();
        assert_eq!(p.url, "https://example.test/a.zip");
        assert_eq!(p.filename.as_deref(), Some("a.zip"));
        assert_eq!(p.size, Some(1331));
        assert_eq!(p.referer.as_deref(), Some("https://example.test/page"));
        assert_eq!(p.cookie.as_deref(), Some("sid=1; theme=dark"));
        assert_eq!(p.user_agent.as_deref(), Some("Mozilla/5.0"));
    }

    #[test]
    fn add_request_rejects_bad_input() {
        assert!(prefill_from_add(br#"{"url":"file:///etc/passwd"}"#).is_err());
        assert!(prefill_from_add(br#"{}"#).is_err());
        assert!(prefill_from_add(b"not json").is_err());
    }

    /// 磁力链在会话 25 被关掉（BT 入口未开放）：扩展交过来必须走 fallback，
    /// 不能「收下了但下不了」。开关在 `download::ALLOW_MAGNET`，与剪贴板同一处。
    #[test]
    fn add_request_rejects_magnet_while_bt_is_closed() {
        let body = br#"{"url":"magnet:?xt=urn:btih:abc123"}"#;
        assert_eq!(
            prefill_from_add(body).is_err(),
            !crate::download::ALLOW_MAGNET
        );
    }

    #[test]
    fn stat_is_all_strings_even_without_engine() {
        let v = stat_json();
        for k in [
            "downloadSpeed",
            "uploadSpeed",
            "numActive",
            "numWaiting",
            "numStopped",
            "numStoppedTotal",
        ] {
            assert!(v.get(k).and_then(Value::as_str).is_some(), "{k} 必须是字符串");
        }
    }
}
