//! 全屏截图右下角缩略图反馈（S7 会话 72）。
//!
//! 抓图 / 自动交付 / 缩略图窗口共用；PrintScreen 热键、设置页按钮、托盘全屏
//! 三个入口都走 `screenshot::capture_full_feedback`。图片只在暂存目录，
//! 前端用 `vextoimg` 协议显示，不走 IPC base64（ADR-040 / 044）。

use crate::config::Config;
use crate::screenshot::{output, preview};
use std::path::PathBuf;
use std::sync::Mutex;

struct Source {
    path: PathBuf,
    width: u32,
    height: u32,
}

static SOURCE: Mutex<Option<Source>> = Mutex::new(None);

fn lock() -> std::sync::MutexGuard<'static, Option<Source>> {
    SOURCE.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn set_source(path: PathBuf, width: u32, height: u32) {
    let mut g = lock();
    if let Some(old) = g.take() {
        let _ = std::fs::remove_file(old.path);
    }
    *g = Some(Source {
        path,
        width,
        height,
    });
}

/// 缩略图页面的状态：显示 URL、尺寸、倒计时秒数。
pub fn state_value() -> serde_json::Value {
    let cfg = crate::config::load();
    let seconds = cfg.screenshot_feedback_seconds.clamp(3, 5);
    let g = lock();
    match g.as_ref() {
        Some(s) => serde_json::json!({
            "url": preview::display_url(&s.path),
            "width": s.width,
            "height": s.height,
            "seconds": seconds,
        }),
        None => serde_json::json!({ "url": null, "width": 0, "height": 0, "seconds": seconds }),
    }
}

pub fn clear() {
    let taken = lock().take();
    if let Some(s) = taken {
        let _ = std::fs::remove_file(s.path);
    }
}

/// 缩略图悬停按钮：复制图片 / 钉住 / 预览。
pub fn apply(action: &str, cfg: &Config) -> Result<preview::ActionOutcome, String> {
    let (src, w, h) = {
        let g = lock();
        let s = g.as_ref().ok_or_else(|| "没有截图反馈图".to_string())?;
        (s.path.clone(), s.width, s.height)
    };
    match action {
        "copy" => {
            let img = preview::crop_region_to_image(&src, 0, 0, w, h)?;
            output::copy_image(&img)?;
            Ok(preview::ActionOutcome {
                pin_label: None,
                message: format!("已复制图片（{}×{}）", img.width, img.height),
                path: None,
                text: None,
            })
        }
        "pin" => {
            let (label, _, _) =
                crate::screenshot::pin::add_staged(src, cfg.pinscreen_single_window)?;
            Ok(preview::ActionOutcome {
                pin_label: Some(label),
                message: format!("已钉屏（{w}×{h}）"),
                path: None,
                text: None,
            })
        }
        "preview" => {
            // 预览窗用独立的暂存副本，反馈窗关闭时删自己的文件不会影响它。
            let staged = preview::stage_file(&src)?;
            preview::set_source(staged);
            Ok(preview::ActionOutcome {
                pin_label: None,
                message: format!("已打开预览（{w}×{h}）"),
                path: None,
                text: None,
            })
        }
        other => Err(format!("未知的缩略图动作：{other}")),
    }
}
