//! 发送侧（S4b）：`TransferOrchestrator` 驱动 prepare-upload → upload，
//! 进度经 [`TransferEvent::Progress`] 推给前端，取消走 `CancellationToken`。
//!
//! SHA-256 在发送前整文件流式算一遍（协议要求 prepare 带摘要；lanflare 的
//! `OutgoingFile::with_computed_sha256` 是 128 KB 分块读，不进内存）。
//! 大文件会有一段「准备中」，前端按 `preparing` 状态显示。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use lanflare_core::client::transfer::{
    OutgoingFile, TransferOptions, TransferOrchestrator, TransferProgress,
};
use lanflare_core::client::{LsClient, LsClientConfig};
use lanflare_proto::discovery::{ProtocolType, RegisterDtoV2};
use tokio_util::sync::CancellationToken;

use super::backend::TransferHandle;
use super::events::{ProgressView, TransferEvent};
use super::server;
use crate::config;

static SEND_SEQ: AtomicU64 = AtomicU64::new(1);
static SEND_CANCELS: OnceLock<Mutex<HashMap<String, CancellationToken>>> = OnceLock::new();

fn cancels() -> &'static Mutex<HashMap<String, CancellationToken>> {
    SEND_CANCELS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 发送请求的总时长上限（`reqwest::ClientBuilder::timeout` 的口径是「整个请求」，
/// 从开始连接到上传 body 写完，不是空闲超时）。
///
/// 旧值固定 60 秒会把大文件腰斩：4 GB 在 5 MB/s 的 WiFi 上要 14 分钟，60 秒只能过
/// 约 300 MB（真机实测恰好在 7% 处报 `error sending request for url (…/upload…)`）。
/// 所以按体积给下限——假设最差 1 MB/s，另加 120 秒余量；封顶 6 小时。
///
/// 代价：对端假死时最长要等到这个值才报错，期间进度条不动，用户可以主动取消。
const MIN_SEND_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_SEND_TIMEOUT: Duration = Duration::from_secs(6 * 60 * 60);
/// 计算超时时假设的最差平均速率（字节/秒）。
const WORST_CASE_BPS: u64 = 1024 * 1024;

fn send_timeout(total_bytes: u64) -> Duration {
    let secs = total_bytes / WORST_CASE_BPS + MIN_SEND_TIMEOUT.as_secs();
    Duration::from_secs(secs).clamp(MIN_SEND_TIMEOUT, MAX_SEND_TIMEOUT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_timeout_keeps_floor_for_small_files() {
        assert_eq!(send_timeout(0), MIN_SEND_TIMEOUT);
        // 小文件按 1 MB/s 折算只多几秒，仍远小于旧值 60 秒之外的余量，够用
        assert_eq!(send_timeout(10 * 1024 * 1024), Duration::from_secs(130));
        assert_eq!(send_timeout(100 * 1024 * 1024), Duration::from_secs(220));
    }

    #[test]
    fn send_timeout_scales_with_size() {
        // 4 GB：4096 秒 + 120 秒余量，够 1 MB/s 跑完
        assert_eq!(send_timeout(4 * 1024 * 1024 * 1024), Duration::from_secs(4216));
        // 1 GB
        assert_eq!(send_timeout(1024 * 1024 * 1024), Duration::from_secs(1144));
    }

    #[test]
    fn send_timeout_is_capped() {
        assert_eq!(send_timeout(u64::MAX / 2), MAX_SEND_TIMEOUT);
    }
}

/// 局域网目标不走代理（风险 55 / 62 的同类坑）：reqwest 默认读 `HTTP_PROXY`
/// 环境变量，用户开着代理工具时，发往手机 / 局域网设备的请求会被劫持到代理。
/// lanflare 的 `LsClient` 不暴露 `.no_proxy()`，所以在 `main()` 最早处
/// （任何客户端构建之前）往 `NO_PROXY` 合并 LAN 段：**只豁免局域网与回环**，
/// 外网翻译等照常走代理。reqwest / hyper-util 构建客户端时读这个变量。
pub fn ensure_lan_no_proxy() {
    const LAN: &str =
        "127.0.0.1,localhost,::1,10.0.0.0/8,172.16.0.0/12,192.168.0.0/16,169.254.0.0/16";
    for key in ["NO_PROXY", "no_proxy"] {
        let merged = match std::env::var(key) {
            Ok(v) if !v.trim().is_empty() => format!("{v},{LAN}"),
            _ => LAN.to_string(),
        };
        std::env::set_var(key, merged);
    }
}

/// 发送文件给一台已发现的设备。
///
/// `peer_id` = 设备指纹（`Peer::fingerprint`）或 `ip:port`。立即返回本地
/// 发送 id，实际传输在服务线程的 tokio 上异步跑，进度走事件。
pub fn send_files(
    peer_id: &str,
    files: &[PathBuf],
    pin: Option<String>,
) -> Result<TransferHandle, String> {
    if files.is_empty() {
        return Err("没有选中文件".to_string());
    }
    let rt = server::runtime_handle().ok_or("请先启用局域网互传")?;
    let identity = server::identity().ok_or("传输身份未就绪，请重新启用模块")?;

    let peer = server::peers()
        .into_iter()
        .find(|p| {
            p.fingerprint == peer_id
                || format!("{}:{}", p.ip, p.port) == peer_id
                || p.alias == peer_id
        })
        .ok_or("找不到该设备（可能已离线，请刷新设备列表）")?;

    let base_url = format!("{}://{}:{}", peer.protocol, peer.ip, peer.port);
    let send_id = format!("send-{}-{}", std::process::id(), SEND_SEQ.fetch_add(1, Ordering::SeqCst));
    let cancel = CancellationToken::new();
    lock(cancels()).insert(send_id.clone(), cancel.clone());

    let files: Vec<PathBuf> = files.to_vec();
    let sid = send_id.clone();
    let fingerprint = peer.fingerprint.clone();
    let target_name = peer.alias.clone();

    rt.spawn(async move {
        let total: u64 = files
            .iter()
            .filter_map(|p| std::fs::metadata(p).ok())
            .map(|m| m.len())
            .sum();
        server::emit(TransferEvent::Progress {
            view: ProgressView {
                session_id: sid.clone(),
                direction: "send".into(),
                file_name: if files.len() == 1 {
                    super::sanitize::sanitize_filename(
                        &files[0].file_name().unwrap_or_default().to_string_lossy(),
                    )
                } else {
                    format!("{} 个文件", files.len())
                },
                bytes: 0,
                total,
                speed_bps: None,
                status: "preparing".into(),
                message: Some(format!("正在计算校验并连接 {target_name}…")),
            },
        });

        // 1) 逐个算 SHA-256（分块读，不进内存）
        let mut outgoing = Vec::with_capacity(files.len());
        for p in &files {
            match OutgoingFile::from_path(p).await {
                Ok(f) => match f.with_computed_sha256().await {
                    Ok(f) => outgoing.push(f),
                    Err(e) => {
                        fail(&sid, &target_name, format!("读取文件失败：{e}"));
                        lock(cancels()).remove(&sid);
                        return;
                    }
                },
                Err(e) => {
                    fail(&sid, &target_name, format!("读取文件失败：{e}"));
                    lock(cancels()).remove(&sid);
                    return;
                }
            }
        }

        // 2) 发起传输
        let cfg = config::load();
        let alias = if cfg.device_alias.trim().is_empty() {
            std::env::var("COMPUTERNAME").unwrap_or_else(|_| "LightBox".to_string())
        } else {
            cfg.device_alias.trim().to_string()
        };
        let sender_info = RegisterDtoV2 {
            alias,
            version: "2.2".to_string(),
            device_model: Some(if cfg.transfer_device_model.trim().is_empty() {
                "Windows".to_string()
            } else {
                cfg.transfer_device_model.trim().to_string()
            }),
            device_type: Some(super::server::parse_device_type(&cfg.transfer_device_type)),
            fingerprint: identity.fingerprint.clone(),
            port: if cfg.transfer_port == 0 {
                crate::transfer::TRANSFER_PORT
            } else {
                cfg.transfer_port.clamp(1024, 65535)
            },
            protocol: if cfg.transfer_use_https {
                ProtocolType::Https
            } else {
                ProtocolType::Http
            },
            download: false,
        };
        let client = LsClient::new(LsClientConfig {
            // 总时长上限按体积算，见 `send_timeout()`：固定值会把大文件腰斩。
            timeout: send_timeout(total),
            connect_timeout: std::time::Duration::from_secs(5),
            tls_identity: Some(identity),
            target_fingerprint: if fingerprint.is_empty() {
                None
            } else {
                Some(fingerprint)
            },
        });
        let client = match client {
            Ok(c) => c,
            Err(e) => {
                fail(&sid, &target_name, format!("创建传输客户端失败：{e}"));
                lock(cancels()).remove(&sid);
                return;
            }
        };

        let orchestrator = TransferOrchestrator::new(client, sender_info);
        let (ptx, mut prx) = tokio::sync::mpsc::channel::<TransferProgress>(256);
        let opts = TransferOptions {
            pin: pin.filter(|s| !s.trim().is_empty()),
            cancel_token: Some(cancel.clone()),
            ..Default::default()
        };

        // 进度转发任务：TransferProgress → 前端事件（以本地 send_id 为准）
        let fwd_id = sid.clone();
        let fwd_target = target_name.clone();
        let forwarder = tokio::spawn(async move {
            while let Some(p) = prx.recv().await {
                forward_progress(&fwd_id, &fwd_target, p);
            }
        });

        let res = orchestrator.transfer(&base_url, outgoing, opts, Some(ptx)).await;
        let _ = forwarder.await;
        lock(cancels()).remove(&sid);

        match res {
            Ok(()) => {
                server::emit(TransferEvent::Notice {
                    message: format!("已发送给 {target_name}"),
                });
            }
            Err(e) => {
                let cancelled = cancel.is_cancelled();
                let status = if cancelled { "cancelled" } else { "failed" };
                let msg = if cancelled {
                    "已取消发送".to_string()
                } else {
                    format!("发送失败：{e}")
                };
                server::emit(TransferEvent::Progress {
                    view: ProgressView {
                        session_id: sid.clone(),
                        direction: "send".into(),
                        file_name: String::new(),
                        bytes: 0,
                        total: 0,
                        speed_bps: None,
                        status: status.into(),
                        message: Some(msg.clone()),
                    },
                });
                server::emit(TransferEvent::Notice { message: msg });
            }
        }
    });

    Ok(TransferHandle { session_id: send_id })
}

/// 取消一次发送。只对发送侧有效（接收侧取消是对方在自己界面上点）。
pub fn cancel_send(send_id: &str) -> Result<(), String> {
    let token = lock(cancels()).get(send_id).cloned();
    match token {
        Some(t) => {
            t.cancel();
            Ok(())
        }
        None => Err("该传输已结束或不存在".to_string()),
    }
}

fn fail(sid: &str, target: &str, msg: String) {
    crate::dlog::dlog(&format!("transfer: 发送失败（{target}）：{msg}"));
    server::emit(TransferEvent::Progress {
        view: ProgressView {
            session_id: sid.to_string(),
            direction: "send".into(),
            file_name: String::new(),
            bytes: 0,
            total: 0,
            speed_bps: None,
            status: "failed".into(),
            message: Some(msg.clone()),
        },
    });
    server::emit(TransferEvent::Notice { message: msg });
}

fn forward_progress(sid: &str, target: &str, p: TransferProgress) {
    match p {
        TransferProgress::Started {
            accepted_files,
            total_bytes,
            ..
        } => {
            server::emit(TransferEvent::Progress {
                view: ProgressView {
                    session_id: sid.to_string(),
                    direction: "send".into(),
                    file_name: format!("{} 个文件", accepted_files.len()),
                    bytes: 0,
                    total: total_bytes,
                    speed_bps: None,
                    status: "active".into(),
                    message: Some(format!("对方已接受，开始发送给 {target}")),
                },
            });
        }
        TransferProgress::FileProgress {
            file_name,
            file_bytes_sent,
            file_total_bytes,
            bytes_per_second,
            ..
        } => {
            server::emit(TransferEvent::Progress {
                view: ProgressView {
                    session_id: sid.to_string(),
                    direction: "send".into(),
                    file_name,
                    bytes: file_bytes_sent,
                    total: file_total_bytes,
                    speed_bps: Some(bytes_per_second),
                    status: "active".into(),
                    message: None,
                },
            });
        }
        TransferProgress::FileFinished { file_name, .. } => {
            server::emit(TransferEvent::Progress {
                view: ProgressView {
                    session_id: sid.to_string(),
                    direction: "send".into(),
                    file_name,
                    bytes: 0,
                    total: 0,
                    speed_bps: None,
                    status: "done".into(),
                    message: None,
                },
            });
        }
        TransferProgress::FileFailed {
            file_name, error, ..
        } => {
            server::emit(TransferEvent::Progress {
                view: ProgressView {
                    session_id: sid.to_string(),
                    direction: "send".into(),
                    file_name,
                    bytes: 0,
                    total: 0,
                    speed_bps: None,
                    status: "failed".into(),
                    message: Some(error),
                },
            });
        }
        TransferProgress::SessionCompleted {
            total_files,
            total_bytes,
            duration,
            ..
        } => {
            let secs = duration.as_secs_f64().max(0.001);
            server::emit(TransferEvent::Progress {
                view: ProgressView {
                    session_id: sid.to_string(),
                    direction: "send".into(),
                    file_name: format!("{total_files} 个文件"),
                    bytes: total_bytes,
                    total: total_bytes,
                    speed_bps: Some((total_bytes as f64 / secs) as u64),
                    status: "done".into(),
                    message: Some(format!("发送完成：{} 个文件", total_files)),
                },
            });
        }
        TransferProgress::SessionCancelled { reason, .. } => {
            server::emit(TransferEvent::Progress {
                view: ProgressView {
                    session_id: sid.to_string(),
                    direction: "send".into(),
                    file_name: String::new(),
                    bytes: 0,
                    total: 0,
                    speed_bps: None,
                    status: "cancelled".into(),
                    message: Some(reason),
                },
            });
        }
    }
}
