//! aria2 JSON-RPC 客户端（S3a 最小集 + S3b 批量快照与四个操作）。
//!
//! 两条硬约束（任务书 §S3.5、06-TESTING §6.2.4）：
//! 1. `token:SECRET` 永远是 `params` 的第一个元素；`system.multicall` 的
//!    **每个子调用**也要各自带 token，否则整批返回 Unauthorized；
//! 2. aria2 返回的所有整数都是**十进制字符串**，坏值按 0 处理，绝不让一个
//!    字段毁掉整条任务；`totalLength` 可能为 0（BT 取 metadata 阶段），进度必须防除零。
//!
//! 监听口径：`--rpc-listen-all=false` 只约束 RPC 端口；BT / DHT 的对外监听
//! 见 `licenses/aria2-VERSION.txt`「本机实测（会话 12）」段。

use serde::Serialize;
use serde_json::{json, Value};
use std::time::Duration;

/// 单次 RPC 超时。就绪探测会连着发很多次，单次不能太长。
const TIMEOUT: Duration = Duration::from_secs(3);

/// 单任务状态。字段只取 S3a 用得到的，其余（files / 上传速度 / 连接数）留到 S3b 主窗口。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskStatus {
    pub gid: String,
    pub status: String,
    pub total_length: u64,
    pub completed_length: u64,
    pub download_speed: u64,
    pub error_message: Option<String>,
}

/// 主窗口一行要显示的数据。只看 `files[0]`，不序列化整个文件数组——
/// BT 种子可能有上万个文件，全量序列化会冻结窗口（任务书 §S3.5.5）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskRow {
    pub gid: String,
    pub status: String,
    pub name: String,
    pub total_length: u64,
    pub completed_length: u64,
    pub download_speed: u64,
    pub upload_speed: u64,
    /// 速度为 0 / 已完成 / 总长为 0 时没有可算的剩余时间。
    pub eta_seconds: Option<u64>,
    pub dir: String,
    /// `files[0].path`：`06` §6.4.2 第 8 条「打开目录并**选中文件**」要用。
    /// 只取第一个文件，BT 种子可能有上万个（§S3.5.5）。
    pub file_path: String,
    /// 任务已完成、但文件已经不在磁盘上（用户删了或移走了）。
    /// 只在 `status == "complete"` 时有意义，由 `mark_missing()` 填。
    /// 前端据此把整行变灰 + 文件名加删除线。
    pub file_missing: bool,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct GlobalStat {
    pub download_speed: u64,
    pub upload_speed: u64,
    pub num_active: u64,
    pub num_waiting: u64,
    pub num_stopped: u64,
}

/// 一次 `tasks:snapshot` 事件的载荷。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Snapshot {
    pub global: GlobalStat,
    pub tasks: Vec<TaskRow>,
}

/// aria2 的整数都是十进制字符串。解析失败按 0：宁可显示「0 B」也不要整条任务消失。
fn decimal(v: &Value, key: &str) -> u64 {
    v.get(key)
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0)
}

fn text(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// 从 `tellStatus` 的 result 对象解析。没有 `gid` 视为无效响应。
pub fn parse_status(v: &Value) -> Option<TaskStatus> {
    let gid = text(v, "gid");
    if gid.is_empty() {
        return None;
    }
    Some(TaskStatus {
        gid,
        status: text(v, "status"),
        total_length: decimal(v, "totalLength"),
        completed_length: decimal(v, "completedLength"),
        download_speed: decimal(v, "downloadSpeed"),
        error_message: v
            .get("errorMessage")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

/// 进度比例。`total == 0`（BT metadata / 媒体任务）返回 0.0，绝不除零。
pub fn progress_ratio(completed: u64, total: u64) -> f64 {
    if total == 0 {
        return 0.0;
    }
    (completed as f64 / total as f64).clamp(0.0, 1.0)
}

/// 剩余秒数。速度为 0 或 `totalLength` 为 0 时返回 None（前端显示「—」），绝不除零。
fn eta_seconds(completed: u64, total: u64, speed: u64) -> Option<u64> {
    if speed == 0 || total <= completed {
        return None;
    }
    Some((total - completed) / speed)
}

/// 任务名：BT 用 `bittorrent.info.name`，普通任务取第一个文件的文件名，再退回 URI。
fn task_name(v: &Value) -> String {
    if let Some(n) = v.pointer("/bittorrent/info/name").and_then(Value::as_str) {
        if !n.is_empty() {
            return n.to_string();
        }
    }
    if let Some(p) = v.pointer("/files/0/path").and_then(Value::as_str) {
        if let Some(base) = std::path::Path::new(p).file_name() {
            return base.to_string_lossy().to_string();
        }
    }
    if let Some(u) = v.pointer("/files/0/uris/0/uri").and_then(Value::as_str) {
        return u.to_string();
    }
    "未知文件".to_string()
}

/// 把 `tellActive` / `tellWaiting` / `tellStopped` 的一格解析成主窗口的一行。
pub fn parse_task_row(v: &Value) -> Option<TaskRow> {
    let gid = text(v, "gid");
    if gid.is_empty() {
        return None;
    }
    let total = decimal(v, "totalLength");
    let completed = decimal(v, "completedLength");
    let speed = decimal(v, "downloadSpeed");
    Some(TaskRow {
        gid,
        status: text(v, "status"),
        name: task_name(v),
        total_length: total,
        completed_length: completed,
        download_speed: speed,
        upload_speed: decimal(v, "uploadSpeed"),
        eta_seconds: eta_seconds(completed, total, speed),
        dir: text(v, "dir"),
        file_path: v
            .pointer("/files/0/path")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        // 解析阶段不碰文件系统（纯函数、单测不依赖磁盘），由 mark_missing 后填。
        file_missing: false,
        error_message: v
            .get("errorMessage")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
    })
}

/// 构造 `system.multicall` 请求：一次拿全局速度 + 三个任务桶（任务书 §S3.5.3）。
/// 抽成纯函数是为了能单测「每个子调用都带 token」，不必真发请求。
pub fn multicall_body(secret: &str) -> Value {
    let t = json!(format!("token:{secret}"));
    json!({
        "jsonrpc": "2.0",
        "id": "lightbox",
        "method": "system.multicall",
        "params": [[
            {"methodName": "aria2.getGlobalStat", "params": [t.clone()]},
            {"methodName": "aria2.tellActive",   "params": [t.clone()]},
            {"methodName": "aria2.tellWaiting",  "params": [t.clone(), 0, 100]},
            {"methodName": "aria2.tellStopped",  "params": [t, 0, 50]}
        ]]
    })
}

/// 取出一个子调用的真实结果。
///
/// **本机 aria2 1.37.0 实测**（会话 13）：`system.multicall` 的 result 是 4 个子结果，
/// 而且**每个子结果都被再包一层数组** —— `result[0] = [{global}]`、
/// `result[1] = [[task, ...]]`。这与标准 JSON-RPC multicall 的扁平数组不同，
/// 照标准猜会解出 0 条任务。失败的子调用是 `{"error":...}` 或 `[{"error":...}]`。
fn unwrap_subcall(v: &Value) -> Option<&Value> {
    if let Some(inner) = v.get(0) {
        if inner.get("error").is_some() {
            return None;
        }
        return Some(inner);
    }
    if v.get("error").is_some() {
        return None;
    }
    // 兼容某些版本不包装、直接把值放在这一格的情况。
    Some(v)
}

/// 删除任务该用哪个方法。
///
/// aria2 把「已完成 / 出错 / 已移除」放在结果记录里，不在活动队列，
/// 对它们调 `aria2.remove` 会报 `GID not found`，必须改用
/// `aria2.removeDownloadResult`。抽成纯函数是为了能单测这个分支。
pub fn removal_method(status: &str) -> &'static str {
    match status {
        "active" | "waiting" | "paused" => "aria2.remove",
        _ => "aria2.removeDownloadResult",
    }
}

/// 解析 multicall 的 result。**任一子调用失败不能毁掉整批**：跳过坏格，其余照常显示。
pub fn parse_snapshot(result: &Value) -> Snapshot {
    let Some(items) = result.as_array() else {
        return Snapshot::default();
    };
    let mut snap = Snapshot::default();
    if let Some(g) = items.first().and_then(|v| unwrap_subcall(v)) {
        if g.is_object() {
            snap.global = GlobalStat {
                download_speed: decimal(g, "downloadSpeed"),
                upload_speed: decimal(g, "uploadSpeed"),
                num_active: decimal(g, "numActive"),
                num_waiting: decimal(g, "numWaiting"),
                num_stopped: decimal(g, "numStopped"),
            };
        }
    }
    for bucket in items.iter().skip(1).take(3) {
        if let Some(rows) = unwrap_subcall(bucket).and_then(Value::as_array) {
            // 请求侧已限制每桶条数，这里再兜一层，防止异常响应把窗口撑爆。
            for v in rows.iter().take(64) {
                if let Some(row) = parse_task_row(v) {
                    snap.tasks.push(row);
                }
            }
        }
    }
    snap
}

/// 标记「已完成但文件不在了」的任务。
///
/// 只检查 `complete`：下载中的任务文件还没落盘是正常的，不能标成已删除。
/// `exists` 用闭包注入，单测不碰文件系统；`true` 表示「已删除」。
///
/// **判据只能是「路径不存在」**：文件被移动、改名也会落进这一类。要区分
/// 「删除」和「移动」就得挂文件系统日志，代价远大于收益。
pub fn mark_missing(snap: &mut Snapshot, exists: impl Fn(&str) -> bool) {
    for t in &mut snap.tasks {
        t.file_missing =
            t.status == "complete" && !t.file_path.is_empty() && !exists(&t.file_path);
    }
}

/// 从 JSON-RPC 响应里取 result。错误响应转成中文，面向用户。
pub fn extract_result(v: &Value) -> Result<&Value, String> {
    if let Some(err) = v.get("error") {
        let msg = err
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("未知错误");
        return Err(format!("aria2 报错：{msg}"));
    }
    v.get("result")
        .ok_or_else(|| "aria2 的响应里没有 result 字段".to_string())
}

/// 构造请求体。抽成纯函数是为了能单测「token 在第一位」，不必真发请求。
pub fn request_body(method: &str, secret: &str, extra: Vec<Value>) -> Value {
    let mut params = vec![json!(format!("token:{secret}"))];
    params.extend(extra);
    json!({
        "jsonrpc": "2.0",
        "id": "lightbox",
        "method": method,
        "params": params,
    })
}

/// 一个 RPC 端点。每次 spawn 后新建一个（端口和 secret 都会变）。
pub struct Rpc {
    url: String,
    secret: String,
    client: reqwest::blocking::Client,
}

impl Rpc {
    /// 端口固定 127.0.0.1：RPC 只对回环监听；BT / DHT 的对外监听与此参数无关。
    pub fn new(port: u16, secret: &str) -> Result<Self, String> {
        let client = reqwest::blocking::Client::builder()
            // RPC 只对 127.0.0.1 监听，绝不能走系统代理。
            // reqwest 默认读 HTTP_PROXY/HTTPS_PROXY，用户装了代理软件且没配 NO_PROXY 时，
            // 回环请求会被发到代理、拿到非 JSON 响应，表现为「aria2 返回的不是 JSON」。
            .no_proxy()
            .timeout(TIMEOUT)
            .build()
            .map_err(|e| format!("创建 HTTP 客户端失败：{e}"))?;
        Ok(Self {
            url: format!("http://127.0.0.1:{port}/jsonrpc"),
            secret: secret.to_string(),
            client,
        })
    }

    fn post(&self, body: &Value) -> Result<Value, String> {
        let resp = self
            .client
            .post(&self.url)
            .json(body)
            .send()
            .map_err(|e| format!("连接 aria2 RPC 失败：{e}"))?;
        let v: Value = resp
            .json()
            .map_err(|e| format!("aria2 返回的不是 JSON：{e}"))?;
        extract_result(&v).cloned()
    }

    fn call(&self, method: &str, extra: Vec<Value>) -> Result<Value, String> {
        self.post(&request_body(method, &self.secret, extra))
    }

    /// 就绪探测兼能力探测：能拿到 version 就说明 RPC 起来了。
    pub fn get_version(&self) -> Result<String, String> {
        let r = self.call("aria2.getVersion", vec![])?;
        Ok(text(&r, "version"))
    }

    /// 加一个 URI（http/https/ftp/magnet），返回 gid。
    ///
    /// `options` 是 aria2 的 per-task 选项（`out` / `dir` / 限速 / UA / Referer）：
    /// 新建任务允许逐条覆盖目录与文件名，所以不能再只靠 conf 里的全局 `dir`。
    pub fn add_uri(&self, uri: &str, options: Value) -> Result<String, String> {
        let r = self.call("aria2.addUri", vec![json!([uri]), options])?;
        r.as_str()
            .map(str::to_string)
            .ok_or_else(|| "aria2.addUri 没返回 gid".to_string())
    }

    /// 查单任务进度。
    pub fn tell_status(&self, gid: &str) -> Result<TaskStatus, String> {
        let r = self.call("aria2.tellStatus", vec![json!(gid)])?;
        parse_status(&r).ok_or_else(|| "aria2.tellStatus 的响应里没有 gid".to_string())
    }

    /// 一次拿全量快照。轮询 pump 每 500ms / 2s 调一次，不要一任务一请求。
    pub fn snapshot(&self) -> Result<Snapshot, String> {
        let r = self.post(&multicall_body(&self.secret))?;
        let mut snap = parse_snapshot(&r);
        // 对已完成任务做一次 stat：几十个任务也是微秒级，换来「用户删了文件，
        // 列表下一帧就能看出来」。只查 complete，下载中的不用查。
        mark_missing(&mut snap, |p| std::path::Path::new(p).exists());
        Ok(snap)
    }

    /// 暂停一个活动任务。已完成的 gid 会报错，由调用方转成中文提示。
    pub fn pause(&self, gid: &str) -> Result<(), String> {
        self.call("aria2.pause", vec![json!(gid)]).map(|_| ())
    }

    pub fn unpause(&self, gid: &str) -> Result<(), String> {
        self.call("aria2.unpause", vec![json!(gid)]).map(|_| ())
    }

    /// 删除活动 / 等待 / 暂停中的任务。已完成的任务要用 `remove_result()`。
    pub fn remove(&self, gid: &str) -> Result<(), String> {
        self.call("aria2.remove", vec![json!(gid)]).map(|_| ())
    }

    /// 删掉一条已完成 / 出错 / 已移除的结果记录（不删磁盘文件）。
    pub fn remove_result(&self, gid: &str) -> Result<(), String> {
        self.call("aria2.removeDownloadResult", vec![json!(gid)])
            .map(|_| ())
    }

    /// 清空已完成 / 出错的任务记录（不删磁盘文件）。
    pub fn purge(&self) -> Result<(), String> {
        self.call("aria2.purgeDownloadResult", vec![]).map(|_| ())
    }

    /// 优雅退出：让 aria2 把 session 写完。调用方仍需等进程退出并用句柄兜底 kill。
    pub fn shutdown(&self) -> Result<(), String> {
        self.call("aria2.shutdown", vec![]).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 06-TESTING §6.2.4：aria2 所有整数都是字符串，必须 parse 成数字。
    #[test]
    fn parse_tell_status_handles_decimal_strings() {
        let v = json!({
            "gid": "2089b05ecca3d829",
            "status": "active",
            "totalLength": "1024",
            "completedLength": "512",
            "downloadSpeed": "100"
        });
        let t = parse_status(&v).expect("应该能解析出任务状态");
        assert_eq!(t.gid, "2089b05ecca3d829");
        assert_eq!(t.status, "active");
        assert_eq!(t.total_length, 1024);
        assert_eq!(t.completed_length, 512);
        assert_eq!(t.download_speed, 100);
    }

    /// 坏字段不能让整条任务消失；缺字段按 0。
    #[test]
    fn parse_status_tolerates_bad_or_missing_numbers() {
        let v = json!({
            "gid": "g1", "status": "waiting",
            "totalLength": "not-a-number", "completedLength": "7"
        });
        let t = parse_status(&v).expect("坏字段不该让任务消失");
        assert_eq!(t.total_length, 0);
        assert_eq!(t.completed_length, 7);
        assert_eq!(t.download_speed, 0);
        assert!(parse_status(&json!({"status": "active"})).is_none());
    }

    /// 06-TESTING §6.2.4：媒体任务 / BT metadata 阶段 totalLength 恒为 0，不能除零。
    #[test]
    fn progress_handles_zero_total() {
        assert_eq!(progress_ratio(0, 100), 0.0);
        assert_eq!(progress_ratio(100, 0), 0.0);
        assert_eq!(progress_ratio(50, 100), 0.5);
        assert_eq!(progress_ratio(200, 100), 1.0);
    }

    /// `token:SECRET` 必须是 params[0]，否则 aria2 一律返回 Unauthorized。
    #[test]
    fn token_is_always_first_param() {
        let body = request_body("aria2.addUri", "s3cr3t", vec![json!(["https://x/f.zip"])]);
        assert_eq!(body["method"], "aria2.addUri");
        assert_eq!(body["params"][0], "token:s3cr3t");
        assert_eq!(body["params"][1][0], "https://x/f.zip");
    }

    /// 错误响应必须转成中文，而不是把 `{"error":...}` 原样抛给用户。
    #[test]
    fn error_response_becomes_chinese_message() {
        let v = json!({"error": {"code": 1, "message": "Unauthorized"}});
        let e = extract_result(&v).unwrap_err();
        assert!(e.contains("aria2 报错") && e.contains("Unauthorized"), "实际：{e}");
        let ok = json!({"result": "OK"});
        assert_eq!(extract_result(&ok).unwrap(), &json!("OK"));
        assert!(extract_result(&json!({})).is_err());
    }

    /// multicall 的每个子调用都必须自带 token，漏一个整批就 Unauthorized。
    #[test]
    fn multicall_body_puts_token_first_everywhere() {
        let b = multicall_body("s3cr3t");
        assert_eq!(b["method"], "system.multicall");
        let calls = b["params"][0].as_array().expect("params[0] 应是子调用数组");
        assert_eq!(calls.len(), 4);
        for c in calls {
            assert_eq!(c["params"][0], "token:s3cr3t");
        }
        assert_eq!(calls[2]["params"][1], 0);
        assert_eq!(calls[2]["params"][2], 100);
    }

    /// 三个桶都要进快照，整数字符串要 parse，ETA 要算对。
    /// 这里的数据形状来自 2026-11-09 本机 aria2 1.37.0 的 **原始响应**：
    /// `result = [ [{global}], [[task,...]], [[...]], [[...]] ]`，每个子结果再包一层。
    #[test]
    fn parse_snapshot_reads_all_three_buckets() {
        let row = |gid: &str, status: &str| {
            json!({
                "gid": gid, "status": status,
                "totalLength": "1000", "completedLength": "250",
                "downloadSpeed": "50", "uploadSpeed": "0",
                "dir": "/tmp/dl",
                "files": [{"path": "/tmp/dl/a.zip", "uris": [{"uri": "http://x/a.zip"}]}]
            })
        };
        let result = json!([
            [{"downloadSpeed": "500", "uploadSpeed": "10",
              "numActive": "1", "numWaiting": "1", "numStopped": "1"}],
            [[row("g1", "active")]],
            [[row("g2", "waiting")]],
            [[row("g3", "complete")]]
        ]);
        let s = parse_snapshot(&result);
        assert_eq!(s.global.download_speed, 500);
        assert_eq!(s.global.num_active, 1);
        assert_eq!(s.tasks.len(), 3);
        assert_eq!(s.tasks[0].name, "a.zip");
        assert_eq!(s.tasks[0].eta_seconds, Some(15)); // (1000-250)/50
        assert_eq!(s.tasks[0].file_path, "/tmp/dl/a.zip");
        assert_eq!(s.tasks[2].status, "complete");
    }

    /// aria2 把单个子调用失败写成 {"error":...}；不能让一格坏掉整批快照。
    #[test]
    fn parse_snapshot_tolerates_failed_subcall() {
        let result = json!([
            [{"error": {"code": 1, "message": "boom"}}],
            [[{"gid": "g1", "status": "active",
               "totalLength": "0", "completedLength": "0"}]],
            [{"error": {"code": 1, "message": "boom"}}],
            [[]]
        ]);
        let s = parse_snapshot(&result);
        assert_eq!(s.global, GlobalStat::default());
        assert_eq!(s.tasks.len(), 1);
        assert_eq!(s.tasks[0].eta_seconds, None, "总长 0 不该算出剩余时间");
        assert_eq!(s.tasks[0].name, "未知文件");
        assert_eq!(parse_snapshot(&json!({})), Snapshot::default());
    }

    /// `06` §6.4.2 第 5 条要能删掉已完成记录，否则队列永远不空、sidecar 不退出。
    #[test]
    fn removal_method_splits_active_and_result_records() {
        assert_eq!(removal_method("active"), "aria2.remove");
        assert_eq!(removal_method("waiting"), "aria2.remove");
        assert_eq!(removal_method("paused"), "aria2.remove");
        assert_eq!(removal_method("complete"), "aria2.removeDownloadResult");
        assert_eq!(removal_method("error"), "aria2.removeDownloadResult");
        assert_eq!(removal_method("removed"), "aria2.removeDownloadResult");
    }

    #[test]
    fn eta_is_none_when_speed_zero_or_total_unknown() {
        assert_eq!(eta_seconds(0, 100, 0), None);
        assert_eq!(eta_seconds(50, 0, 10), None);
        assert_eq!(eta_seconds(100, 100, 10), None);
        assert_eq!(eta_seconds(0, 100, 10), Some(10));
    }

    /// 删除态只标 `complete`：下载中 / 暂停的任务文件还没落盘是正常的。
    #[test]
    fn mark_missing_marks_only_complete() {
        let mk = |gid: &str, status: &str, path: &str| TaskRow {
            gid: gid.into(),
            status: status.into(),
            name: "x".into(),
            total_length: 1,
            completed_length: 1,
            download_speed: 0,
            upload_speed: 0,
            eta_seconds: None,
            dir: "/dl".into(),
            file_path: path.into(),
            file_missing: false,
            error_message: None,
        };
        let mut snap = Snapshot {
            global: GlobalStat::default(),
            tasks: vec![
                mk("g1", "complete", "/dl/gone.zip"),
                mk("g2", "complete", "/dl/here.zip"),
                mk("g3", "active", "/dl/gone2.zip"),
                mk("g4", "complete", ""),
            ],
        };
        mark_missing(&mut snap, |p| p == "/dl/here.zip");
        assert!(snap.tasks[0].file_missing, "文件不在了要标记");
        assert!(!snap.tasks[1].file_missing, "文件还在");
        assert!(!snap.tasks[2].file_missing, "下载中不查文件");
        assert!(!snap.tasks[3].file_missing, "没有路径不标记");
    }
}
