//! aria2 JSON-RPC 客户端（S3a 最小集）。
//!
//! 只实现本阶段闭环需要的四个方法：`getVersion` / `addUri` / `tellStatus` /
//! `shutdown`。`system.multicall` 与轮询 pump 属于 S3b，这里不预先铺路。
//!
//! 两条硬约束（任务书 §S3.5、06-TESTING §6.2.4）：
//! 1. `token:SECRET` 永远是 `params` 的第一个元素；
//! 2. aria2 返回的所有整数都是**十进制字符串**，坏值按 0 处理，绝不让一个
//!    字段毁掉整条任务；`totalLength` 可能为 0（BT 取 metadata 阶段），进度必须防除零。

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
    /// 端口固定 127.0.0.1：RPC 不对外监听，不弹 Windows 防火墙。
    pub fn new(port: u16, secret: &str) -> Result<Self, String> {
        let client = reqwest::blocking::Client::builder()
            .timeout(TIMEOUT)
            .build()
            .map_err(|e| format!("创建 HTTP 客户端失败：{e}"))?;
        Ok(Self {
            url: format!("http://127.0.0.1:{port}/jsonrpc"),
            secret: secret.to_string(),
            client,
        })
    }

    fn call(&self, method: &str, extra: Vec<Value>) -> Result<Value, String> {
        let body = request_body(method, &self.secret, extra);
        let resp = self
            .client
            .post(&self.url)
            .json(&body)
            .send()
            .map_err(|e| format!("连接 aria2 RPC 失败：{e}"))?;
        let v: Value = resp
            .json()
            .map_err(|e| format!("aria2 返回的不是 JSON：{e}"))?;
        extract_result(&v).cloned()
    }

    /// 就绪探测兼能力探测：能拿到 version 就说明 RPC 起来了。
    pub fn get_version(&self) -> Result<String, String> {
        let r = self.call("aria2.getVersion", vec![])?;
        Ok(text(&r, "version"))
    }

    /// 加一个 URI（http/https/ftp/magnet），返回 gid。
    /// 保存目录写在 conf 文件里，这里不传 options，避免长路径进命令行（03 §3.5.4）。
    pub fn add_uri(&self, uri: &str) -> Result<String, String> {
        let r = self.call("aria2.addUri", vec![json!([uri]), json!({})])?;
        r.as_str()
            .map(str::to_string)
            .ok_or_else(|| "aria2.addUri 没返回 gid".to_string())
    }

    /// 查单任务进度。
    pub fn tell_status(&self, gid: &str) -> Result<TaskStatus, String> {
        let r = self.call("aria2.tellStatus", vec![json!(gid)])?;
        parse_status(&r).ok_or_else(|| "aria2.tellStatus 的响应里没有 gid".to_string())
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
}
