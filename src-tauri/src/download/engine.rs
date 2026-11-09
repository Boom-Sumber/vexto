//! aria2c sidecar 的生命周期（S3a）。
//!
//! 状态机：未运行 →（第一个任务）spawn → 运行中 →（程序退出 / 关掉工具）
//! `aria2.shutdown` → 等进程退出 → 句柄 `kill()` 兜底 → 未运行。
//! **空闲 60 秒自动退出属于 S3b**，本阶段只在退出时主动关闭。
//!
//! 三条不可动摇的约定：
//! - 杀进程只用 spawn 时拿到的 `Child` 句柄，绝不按 PID 反查（03 §3.5.5）。
//! - 长参数（下载目录、session 路径）写进临时 conf 文件，命令行只传一个短的
//!   `--conf-path`（03 §3.5.4）。**不传 `--no-conf`**：本机实测它会把
//!   `--conf-path` 一起废掉，详见 `licenses/aria2-VERSION.txt`。
//! - RPC 只监听回环（`--rpc-listen-all=false`），不弹 Windows 防火墙。
//!
//! 状态放在静态 `OnceLock<Mutex<..>>` 而不是 `app.manage()`：`Tool::status_line()`
//! 与 `Tool::set_enabled()` 拿不到 `AppHandle`，用 Tauri state 会让设置页读不到状态。

use crate::config;
use crate::dlog;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// 与本仓 `rustc --print host-tuple` 一致；`externalBin` 的源文件名要带这个后缀。
const TRIPLE: &str = "x86_64-pc-windows-msvc";
/// 避开 aria2 默认的 6800——用户机器上可能另有一个 aria2 占着它。
const PREFERRED_PORT: u16 = 16800;
const READY_TIMEOUT: Duration = Duration::from_secs(5);
const EXIT_TIMEOUT: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(100);

struct EngineState {
    child: Option<Child>,
    port: u16,
    secret: String,
    conf_path: Option<PathBuf>,
    /// 设置页「下载」分区的状态摘要。S3a 没有主窗口，这里就是用户能看到的反馈。
    message: String,
}

static ENGINE: OnceLock<Mutex<EngineState>> = OnceLock::new();

fn lock() -> MutexGuard<'static, EngineState> {
    ENGINE
        .get_or_init(|| {
            Mutex::new(EngineState {
                child: None,
                port: PREFERRED_PORT,
                secret: String::new(),
                conf_path: None,
                message: String::new(),
            })
        })
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// 设置页显示的一行状态。S3a 只有「运行中 / 已停止 / 最近一次结果」，没有任务数。
pub fn status_line() -> String {
    let g = lock();
    if g.message.is_empty() {
        "已启用 · 空闲（aria2c 未启动）".to_string()
    } else {
        g.message.clone()
    }
}

/// 记一条用户可见的状态（成功入队、剪贴板不是链接、启停等）。
pub fn set_message(msg: impl Into<String>) {
    let m = msg.into();
    dlog::dlog(&format!("engine: {m}"));
    lock().message = m;
}

/// 关掉 sidecar。先优雅退出让 aria2 写完 session，等 5 秒还在就用句柄强杀。
/// 返回 Err 只表示「连进程都没能收掉」，这是必须让用户知道的严重情况。
pub fn shutdown_all() -> Result<(), String> {
    let mut g = lock();
    let Some(mut child) = g.child.take() else {
        return Ok(());
    };
    // 优雅关闭失败不致命：后面还有句柄 kill 兜底，所以只记日志不返回错误。
    if let Err(e) = crate::download::rpc::Rpc::new(g.port, &g.secret).and_then(|r| r.shutdown()) {
        dlog::dlog(&format!("engine: 优雅关闭请求失败（将强制结束）：{e}"));
    }
    let exited = wait_exit(&mut child, EXIT_TIMEOUT)?;
    if !exited {
        dlog::dlog("engine: 等待 5 秒未退出，用 spawn 句柄强制结束");
        kill_child(&mut child)?;
    }
    remove_conf(g.conf_path.take());
    g.secret.clear();
    g.message = "已停止（aria2c 已退出）".to_string();
    dlog::dlog("engine: aria2c 已退出，无孤儿进程");
    Ok(())
}

/// 确保有一个活着的 aria2c，返回可直接用的 RPC 端点。
/// 已有进程且还在跑就复用；端口和 secret 存在全局状态里，重启后由 spawn 重新生成。
pub fn ensure_running(cfg: &config::Config) -> Result<crate::download::rpc::Rpc, String> {
    let mut g = lock();
    if let Some(ch) = g.child.as_mut() {
        if matches!(ch.try_wait(), Ok(None)) {
            return crate::download::rpc::Rpc::new(g.port, &g.secret);
        }
        // 上次那个已经退出了：丢掉僵尸句柄重新拉起。
        g.child = None;
    }
    let (child, rpc, port, secret, conf) = spawn(cfg)?;
    g.child = Some(child);
    g.port = port;
    g.secret = secret;
    g.conf_path = Some(conf);
    g.message = format!("aria2c 运行中（RPC 端口 {port}）");
    Ok(rpc)
}

/// 入队一个链接并立刻查一次状态。这是 S3a 的最小闭环，也是端到端测试的入口。
pub fn add_uri(cfg: &config::Config, uri: &str) -> Result<crate::download::rpc::TaskStatus, String> {
    let rpc = ensure_running(cfg)?;
    let gid = rpc.add_uri(uri)?;
    let st = rpc.tell_status(&gid)?;
    // `totalLength` 为 0 时 progress_ratio 返回 0，绝不会在这里除零（任务书 §S3.10 坑 2）。
    let pct = crate::download::rpc::progress_ratio(st.completed_length, st.total_length) * 100.0;
    set_message(format!(
        "已入队 {} · {} · {pct:.0}% · {}/{} B",
        st.gid, st.status, st.completed_length, st.total_length
    ));
    Ok(st)
}

/// 真正拉起进程。就绪探测失败时**必须把刚 spawn 的进程收掉**，否则就是孤儿。
#[allow(clippy::type_complexity)]
fn spawn(
    cfg: &config::Config,
) -> Result<
    (
        Child,
        crate::download::rpc::Rpc,
        u16,
        String,
        PathBuf,
    ),
    String,
> {
    let bin = sidecar_path()?;
    let port = choose_port(PREFERRED_PORT);
    let secret = make_secret();
    let conf = write_conf(cfg)?;
    let rpc = crate::download::rpc::Rpc::new(port, &secret)?;

    let mut cmd = Command::new(&bin);
    cmd.arg("--enable-rpc")
        .arg(format!("--rpc-listen-port={port}"))
        .arg(format!("--rpc-secret={secret}"))
        .arg("--rpc-listen-all=false")
        .arg("--rpc-allow-origin-all=false")
        .arg(format!("--stop-with-process={}", std::process::id()))
        .arg(format!("--conf-path={}", conf.display()))
        .arg("--summary-interval=0")
        .arg("--console-log-level=warn")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("启动 aria2c 失败：{e}"))?;

    let deadline = Instant::now() + READY_TIMEOUT;
    let mut errors: Vec<String> = Vec::new();
    loop {
        match rpc.get_version() {
            Ok(v) => {
                dlog::dlog(&format!("engine: aria2c 就绪 version={v} port={port}"));
                return Ok((child, rpc, port, secret, conf));
            }
            Err(e) => {
                errors.push(e);
                if matches!(child.try_wait(), Ok(Some(_))) {
                    break; // 进程自己退了，别再等 5 秒
                }
                if Instant::now() >= deadline {
                    break;
                }
                std::thread::sleep(POLL);
            }
        }
    }
    remove_conf(Some(conf));
    if matches!(child.try_wait(), Ok(None)) {
        kill_child(&mut child)?;
    }
    let detail = errors
        .last()
        .cloned()
        .unwrap_or_else(|| "尚未收到响应".to_string());
    Err(format!("aria2c 启动了但 RPC 没就绪：{detail}"))
}

/// 找 sidecar。构建后 `externalBin` 产物是**去掉三元组后缀**的 `aria2c.exe`
/// （本机实测），开发/测试时源码目录里则是带后缀的原名。
/// bundle 后的布局留到 S6 用安装包实测，这里先列出常见候选。
fn sidecar_path() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("取不到自身路径：{e}"))?;
    let dir = exe
        .parent()
        .ok_or_else(|| "取不到自身所在目录".to_string())?;
    let named = format!("aria2c-{TRIPLE}.exe");
    let candidates = [
        dir.join("aria2c.exe"),
        dir.join("binaries").join(&named),
        dir.join("resources").join("aria2c.exe"),
        dir.join("resources").join("binaries").join(&named),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("binaries")
            .join(&named),
    ];
    for p in &candidates {
        if p.is_file() {
            return Ok(p.clone());
        }
    }
    Err(format!(
        "找不到 aria2c.exe。请把官方 1.37.0 二进制放到 {}（与 lightbox.exe 同目录），或源码的 src-tauri/binaries/{named}。",
        dir.join("aria2c.exe").display()
    ))
}

/// 先等 2 秒让上次的 aria2 释放端口，还占着就让系统分配一个。
/// 绝不按端口去杀进程——那可能是用户自己的 aria2（任务书 §S3.10 坑 1）。
fn choose_port(preferred: u16) -> u16 {
    for _ in 0..20 {
        if TcpListener::bind(("127.0.0.1", preferred)).is_ok() {
            return preferred;
        }
        std::thread::sleep(POLL);
    }
    match TcpListener::bind(("127.0.0.1", 0)).and_then(|l| l.local_addr()) {
        Ok(a) => a.port(),
        // 任务书原版写 preferred + 1，u16 溢出会 panic；这里饱和加。
        Err(_) => preferred.saturating_add(1),
    }
}

/// 本机回环 RPC 的一次性密钥。不需要密码学强度：只挡本机误调，不对外暴露。
fn make_secret() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("lb-{}-{nanos:x}", std::process::id())
}

fn default_download_dir() -> String {
    let base = std::env::var("USERPROFILE").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(base)
        .join("Downloads")
        .to_string_lossy()
        .to_string()
}

/// 把长参数写进临时 conf，命令行只留一个短 `--conf-path`。
/// 值里的换行会破坏 `key=value` 格式，先剔掉。
fn write_conf(cfg: &config::Config) -> Result<PathBuf, String> {
    let p = std::env::temp_dir().join(format!("lightbox-aria2-{}.conf", std::process::id()));
    let dir = if cfg.download_dir.trim().is_empty() {
        default_download_dir()
    } else {
        cfg.download_dir.trim().to_string()
    }
    .replace(['\r', '\n'], "");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建下载目录失败：{e}"))?;
    let session = config::log_dir().join("aria2.session");
    let speed = (cfg.max_speed_kb as u64).saturating_mul(1024);
    let concurrent = cfg.max_concurrent.clamp(1, 16);
    let content = format!(
        "dir={dir}\ncontinue=true\nauto-file-renaming=false\nsave-session={}\nsave-session-interval=60\nmax-overall-download-limit={speed}\nmax-concurrent-downloads={concurrent}\n",
        session.display()
    );
    std::fs::write(&p, content).map_err(|e| format!("写 aria2 配置文件失败：{e}"))?;
    Ok(p)
}

fn wait_exit(child: &mut Child, timeout: Duration) -> Result<bool, String> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return Ok(true),
            Ok(None) if Instant::now() >= deadline => return Ok(false),
            Ok(None) => std::thread::sleep(POLL),
            Err(e) => return Err(format!("检查 aria2c 是否退出失败：{e}")),
        }
    }
}

/// 只用 spawn 时拿到的句柄结束进程；`wait()` 回收，不留僵尸。
fn kill_child(child: &mut Child) -> Result<(), String> {
    child
        .kill()
        .map_err(|e| format!("结束 aria2c 失败：{e}"))?;
    child
        .wait()
        .map_err(|e| format!("回收 aria2c 进程失败：{e}"))?;
    Ok(())
}

fn remove_conf(p: Option<PathBuf>) {
    if let Some(p) = p {
        if let Err(e) = std::fs::remove_file(&p) {
            dlog::dlog(&format!("engine: 删除临时配置 {} 失败：{e}", p.display()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    const BODY: &[u8] = b"lightbox-s3a-probe-payload";

    fn serve_one(s: &mut TcpStream) {
        let mut buf = [0u8; 1024];
        if let Err(e) = s.set_read_timeout(Some(Duration::from_secs(2))) {
            eprintln!("sidecar test: 设置读超时失败：{e}");
        }
        if let Err(e) = s.read(&mut buf) {
            eprintln!("sidecar test: 读请求失败：{e}");
        }
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
            BODY.len()
        );
        if let Err(e) = s
            .write_all(head.as_bytes())
            .and_then(|_| s.write_all(BODY))
        {
            eprintln!("sidecar test: 写响应失败：{e}");
        }
    }

    /// S3a 的端到端证据：真起一个 aria2c，spawn → getVersion → addUri →
    /// tellStatus（进度非零）→ shutdown → 进程确实退出。缺 exe 时打印说明并跳过，
    /// **不许把没跑过的闭环写成已验证**。
    #[test]
    fn sidecar_roundtrip_adds_and_reports_progress() {
        if sidecar_path().is_err() {
            eprintln!("跳过：本机没有 aria2c.exe，真实闭环未验证");
            return;
        }
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("绑定本地测试端口");
        let src_port = listener.local_addr().expect("取本地端口").port();
        listener.set_nonblocking(true).expect("设非阻塞");
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = Arc::clone(&stop);
        let server = std::thread::spawn(move || {
            while !stop2.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut s, _)) => serve_one(&mut s),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });

        let dir = std::env::temp_dir().join("lightbox-s3a-test");
        let cfg = config::Config {
            download_dir: dir.to_string_lossy().to_string(),
            ..Default::default()
        };
        let uri = format!("http://127.0.0.1:{src_port}/probe.bin");

        let result = (|| {
            let r = ensure_running(&cfg)?;
            let v = r.get_version()?;
            assert!(!v.is_empty(), "getVersion 不能返回空版本");
            let gid = r.add_uri(&uri)?;
            let mut st = r.tell_status(&gid)?;
            for _ in 0..50 {
                if st.status == "complete" {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
                st = r.tell_status(&gid)?;
            }
            Ok::<_, String>(st)
        })();

        stop.store(true, Ordering::Relaxed);
        server.join().expect("本地 HTTP 线程正常结束");
        let shutdown = shutdown_all();
        let st = result.expect("spawn → addUri → tellStatus 全链路应成功");
        shutdown.expect("退出时 aria2c 应已停止且无孤儿");
        assert_eq!(
            st.status, "complete",
            "实际状态：{} / 错误：{:?}",
            st.status, st.error_message
        );
        assert_eq!(st.total_length, BODY.len() as u64);
        assert_eq!(st.completed_length, BODY.len() as u64);
        eprintln!(
            "sidecar 真实闭环通过：gid={} status={} {}/{} B，shutdown 后进程已退出",
            st.gid, st.status, st.completed_length, st.total_length
        );
    }

    #[test]
    fn choose_port_returns_usable_port() {
        let p = choose_port(16900);
        assert!(p > 0, "端口必须可用");
    }
}
