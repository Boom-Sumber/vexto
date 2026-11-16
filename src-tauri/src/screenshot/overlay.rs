//! 截图 / 滚动框选的快照与选区动作（会话 65 选框改版）。
//!
//! 打开浮层前先抓一张全屏快照，前端用自定义 `vextoimg` 协议显示它：
//! - 放大的取色镜和最终裁剪都基于同一张图，用户看到什么就裁什么；
//! - 选区不再「松手就保存」，动作由选区下方的操作栏显式触发；
//! - 图片始终走文件 + 协议，不走 IPC base64（ADR-044）。
//!
//! 快照只在浮层存活期间有效，窗口销毁时删除。

use crate::config::Config;
use crate::screenshot::{output, preview};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

struct Snapshot {
    path: PathBuf,
    width: u32,
    height: u32,
}

static SNAPSHOT: Mutex<Option<Snapshot>> = Mutex::new(None);

fn lock() -> std::sync::MutexGuard<'static, Option<Snapshot>> {
    SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner())
}

fn stamp() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// 滚动 / 全屏截图的待确认结果。
///
/// 与「选框快照」分开：快照是裁剪源，结果文件已经是一张完整 PNG。
/// 结果先落在 `preview::stage_dir()`，用户点保存才复制到输出目录；
/// 长图不经过内存，动作全部按文件路径处理（ADR-040 / ADR-043）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultKind {
    Full,
    Scroll,
}

struct ResultSource {
    path: PathBuf,
    width: u32,
    height: u32,
    kind: ResultKind,
}

static RESULT: Mutex<Option<ResultSource>> = Mutex::new(None);

pub fn set_result(path: PathBuf, width: u32, height: u32, kind: ResultKind) {
    let mut g = RESULT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(old) = g.take() {
        let _ = std::fs::remove_file(old.path);
    }
    *g = Some(ResultSource {
        path,
        width,
        height,
        kind,
    });
}

pub fn result_state() -> serde_json::Value {
    let g = RESULT.lock().unwrap_or_else(|e| e.into_inner());
    match g.as_ref() {
        Some(r) => serde_json::json!({
            "kind": match r.kind { ResultKind::Full => "full", ResultKind::Scroll => "scroll" },
            "url": preview::display_url(&r.path),
            "width": r.width,
            "height": r.height,
        }),
        None => serde_json::Value::Null,
    }
}

pub fn clear_result() {
    let taken = RESULT.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(r) = taken {
        let _ = std::fs::remove_file(r.path);
    }
}

/// 对滚动 / 全屏结果执行动作栏操作。结果文件已经在暂存目录，按路径复制 /
/// 钉屏 / 预览；只有全屏图允许复制图片，长图按 ADR-043 只保存或复制路径。
pub fn apply_result(action: &str, cfg: &Config) -> Result<preview::ActionOutcome, String> {
    let (src, w, h, kind) = {
        let g = RESULT.lock().unwrap_or_else(|e| e.into_inner());
        let r = g.as_ref().ok_or_else(|| "没有可交付的截图结果".to_string())?;
        (r.path.clone(), r.width, r.height, r.kind)
    };
    match action {
        "save_as" => {
            let dir = output::resolve_dir(cfg);
            let _ = std::fs::create_dir_all(&dir);
            let def = output::next_png_path(&dir, cfg);
            match crate::fileops::pick_save_png(&def)? {
                Some(dst) => {
                    crate::fileops::move_file(&src, &dst)?;
                    Ok(preview::ActionOutcome {
                        pin_label: None,
                        message: format!("已另存为（{w}×{h}）：{}", dst.display()),
                        path: Some(dst),
                        text: None,
                    })
                }
                None => Ok(preview::ActionOutcome {
                    pin_label: None,
                    message: "已取消保存".into(),
                    path: None,
                    text: None,
                }),
            }
        }
        "save" => {
            let dst = output::copy_file_to_dir(&src, &output::resolve_dir(cfg), cfg)?;
            Ok(preview::ActionOutcome {
                pin_label: None,
                message: format!("已保存（{w}×{h}）：{}", dst.display()),
                path: Some(dst),
                text: None,
            })
        }
        "path" => {
            let dst = output::copy_file_to_dir(&src, &output::resolve_dir(cfg), cfg)?;
            if let Err(e) = output::copy_path(&dst) {
                return Err(format!(
                    "已保存（{w}×{h}）到 {}，但复制路径失败：{e}",
                    dst.display()
                ));
            }
            Ok(preview::ActionOutcome {
                pin_label: None,
                message: format!("已保存并复制路径（{w}×{h}）：{}", dst.display()),
                path: Some(dst),
                text: None,
            })
        }
        "clipboard" => {
            if kind == ResultKind::Scroll {
                return Err("长图太大，不能复制到剪贴板；请保存或用预览裁切".into());
            }
            let img = preview::crop_region_to_image(&src, 0, 0, w, h)?;
            output::copy_image(&img)?;
            Ok(preview::ActionOutcome {
                pin_label: None,
                message: format!("已复制到剪贴板（{}×{}）", img.width, img.height),
                path: None,
                text: None,
            })
        }
        "default" => {
            let dst = output::copy_file_to_dir(&src, &output::resolve_dir(cfg), cfg)?;
            let after = cfg.screenshot_after_action.trim();
            let mut message = format!("已保存（{w}×{h}）：{}", dst.display());
            if matches!(after, "both" | "path") {
                match output::copy_path(&dst) {
                    Ok(()) => message.push_str("；文件路径已复制"),
                    Err(e) => message.push_str(&format!("；复制路径失败：{e}")),
                }
            }
            let copy_image = after == "clipboard" || after == "both" || after.is_empty();
            if copy_image {
                if kind == ResultKind::Scroll {
                    message.push_str("；长图不支持复制图片，已仅保存");
                } else if let Ok(img) = preview::crop_region_to_image(&src, 0, 0, w, h) {
                    match output::copy_image(&img) {
                        Ok(()) => message.push_str("；已复制到剪贴板"),
                        Err(e) => message.push_str(&format!("；复制到剪贴板失败：{e}")),
                    }
                }
            }
            Ok(preview::ActionOutcome {
                pin_label: None,
                message,
                path: Some(dst),
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
            let staged = preview::stage_file(&src)?;
            preview::set_source(staged);
            Ok(preview::ActionOutcome {
                pin_label: None,
                message: format!("已打开预览（{w}×{h}）"),
                path: None,
                text: None,
            })
        }
        "close" => Ok(preview::ActionOutcome {
            pin_label: None,
            message: String::new(),
            path: None,
            text: None,
        }),
        other => Err(format!("未知的截图结果动作：{other}")),
    }
}

/// 抓取整个虚拟屏幕作为框选底图。**必须在创建浮层窗口之前调用**，
/// 否则会把浮层自己截进快照。
pub fn capture() -> Result<(), String> {
    clear();
    clear_result();
    // 托盘菜单 / 上一次浮层退场，给合成器留一拍。
    std::thread::sleep(std::time::Duration::from_millis(150));
    let img = crate::screenshot::capture::capture_full_screen()?;
    let dir = preview::stage_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建暂存目录失败：{e}"))?;
    // save_png 用中文前缀命名；显示 URL 只接受安全 ASCII 文件名，所以改名。
    let tmp = output::save_png(&img, &dir)?;
    let dst = dir.join(format!("snap-{}.png", stamp()));
    if let Err(e) = std::fs::rename(&tmp, &dst) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("暂存截图快照失败：{e}"));
    }
    *lock() = Some(Snapshot {
        path: dst,
        width: img.width as u32,
        height: img.height as u32,
    });
    Ok(())
}

/// 给浮层页面的状态：快照 URL、物理尺寸、虚拟屏原点、边框宽度。
pub fn state_value() -> serde_json::Value {
    let cfg = crate::config::load();
    let border = cfg.screenshot_border_width.clamp(3, 5);
    let zoom = cfg.screenshot_magnifier_zoom.clamp(2, 10);
    let (vx, vy, _, _) = crate::ocr::virtual_screen();
    let result = result_state();
    let mode = result
        .get("kind")
        .and_then(|v| v.as_str())
        .unwrap_or("shot");
    let g = lock();
    match g.as_ref() {
        Some(s) => serde_json::json!({
            "url": preview::display_url(&s.path),
            "width": s.width,
            "height": s.height,
            "originX": vx,
            "originY": vy,
            "border": border,
            "zoom": zoom,
            "mode": mode,
            "result": result,
        }),
        None => serde_json::json!({
            "url": null,
            "border": border,
            "zoom": zoom,
            "originX": vx,
            "originY": vy,
            "mode": mode,
            "result": result,
        }),
    }
}

pub fn clear() {
    let taken = lock().take();
    if let Some(s) = taken {
        let _ = std::fs::remove_file(s.path);
    }
}

/// 浮层窗口销毁：快照与待确认结果都只在它活着时有意义。
pub fn on_window_destroyed() {
    clear();
    clear_result();
}

fn crop_rect(
    img_w: u32,
    img_h: u32,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    dpr: Option<f64>,
) -> Result<(u32, u32, u32, u32), String> {
    let dpr = dpr.filter(|d| d.is_finite() && *d > 0.0).unwrap_or(1.0);
    let sx = (x * dpr).round().clamp(0.0, img_w as f64) as u32;
    let sy = (y * dpr).round().clamp(0.0, img_h as f64) as u32;
    let sw = ((w * dpr).round() as i64).clamp(0, (img_w - sx) as i64) as u32;
    let sh = ((h * dpr).round() as i64).clamp(0, (img_h - sy) as i64) as u32;
    if sw < 2 || sh < 2 {
        return Err("选区太小，请重新框选".into());
    }
    Ok((sx, sy, sw, sh))
}

fn staged_crop(src: &Path, sx: u32, sy: u32, sw: u32, sh: u32) -> Result<PathBuf, String> {
    let dir = preview::stage_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建暂存目录失败：{e}"))?;
    let dst = dir.join(format!("sel-{}.png", stamp()));
    preview::crop_region_png(src, &dst, sx, sy, sw, sh)?;
    Ok(dst)
}

/// 对当前选区执行动作。坐标是浮层内的 CSS 像素，`dpr` 由前端上报。
pub fn apply(
    action: &str,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    dpr: Option<f64>,
) -> Result<preview::ActionOutcome, String> {
    let (src, img_w, img_h) = {
        let g = lock();
        let s = g.as_ref().ok_or_else(|| "没有截图快照，请重新截图".to_string())?;
        (s.path.clone(), s.width, s.height)
    };
    let (sx, sy, sw, sh) = crop_rect(img_w, img_h, x, y, w, h, dpr)?;
    let cfg = crate::config::load();
    match action {
        "save_as" => {
            let tmp = staged_crop(&src, sx, sy, sw, sh)?;
            let dir = output::resolve_dir(&cfg);
            let _ = std::fs::create_dir_all(&dir);
            let def = output::next_png_path(&dir, &cfg);
            match crate::fileops::pick_save_png(&def)? {
                Some(dst) => {
                    crate::fileops::move_file(&tmp, &dst)?;
                    Ok(preview::ActionOutcome {
                        pin_label: None,
                        message: format!("已另存为（{sw}×{sh}）：{}", dst.display()),
                        path: Some(dst),
                        text: None,
                    })
                }
                None => {
                    let _ = std::fs::remove_file(&tmp);
                    Ok(preview::ActionOutcome {
                        pin_label: None,
                        message: "已取消保存".into(),
                        path: None,
                        text: None,
                    })
                }
            }
        }
        "save" => {
            let dir = output::resolve_dir(&cfg);
            std::fs::create_dir_all(&dir).map_err(|e| format!("创建截图目录失败：{e}"))?;
            let dst = output::next_png_path(&dir, &cfg);
            let (ow, oh) = preview::crop_region_png(&src, &dst, sx, sy, sw, sh)?;
            Ok(preview::ActionOutcome {
                pin_label: None,
                message: format!("已保存（{ow}×{oh}）：{}", dst.display()),
                path: Some(dst),
                text: None,
            })
        }
        "path" => {
            let dir = output::resolve_dir(&cfg);
            std::fs::create_dir_all(&dir).map_err(|e| format!("创建截图目录失败：{e}"))?;
            let dst = output::next_png_path(&dir, &cfg);
            let (ow, oh) = preview::crop_region_png(&src, &dst, sx, sy, sw, sh)?;
            if let Err(e) = output::copy_path(&dst) {
                return Err(format!("已保存（{ow}×{oh}）到 {}，但复制路径失败：{e}", dst.display()));
            }
            Ok(preview::ActionOutcome {
                pin_label: None,
                message: format!("已保存并复制路径（{ow}×{oh}）：{}", dst.display()),
                path: Some(dst),
                text: None,
            })
        }
        "clipboard" => {
            let img = preview::crop_region_to_image(&src, sx, sy, sw, sh)?;
            output::copy_image(&img)?;
            Ok(preview::ActionOutcome {
                pin_label: None,
                message: format!("已复制到剪贴板（{}×{}）", img.width, img.height),
                path: None,
                text: None,
            })
        }
        "default" => {
            let img = preview::crop_region_to_image(&src, sx, sy, sw, sh)?;
            let d = output::deliver(&img, &cfg)?;
            Ok(preview::ActionOutcome {
                pin_label: None,
                message: d.message,
                path: d.path,
                text: None,
            })
        }
        "pin" => {
            let dst = staged_crop(&src, sx, sy, sw, sh)?;
            let (label, _, _) = crate::screenshot::pin::add_staged(dst, cfg.pinscreen_single_window)?;
            Ok(preview::ActionOutcome {
                pin_label: Some(label),
                message: format!("已钉屏（{sw}×{sh}）"),
                path: None,
                text: None,
            })
        }
        "preview" => {
            let dst = staged_crop(&src, sx, sy, sw, sh)?;
            preview::set_source(dst);
            Ok(preview::ActionOutcome {
                pin_label: None,
                message: format!("已打开预览（{sw}×{sh}）"),
                path: None,
                text: None,
            })
        }
        "ocr" => Err("截图选框的动作栏不再提供识别文字，请在「预览」里识别".into()),
        other => Err(format!("未知的选区动作：{other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crop_rect_maps_css_to_physical() {
        assert_eq!(crop_rect(100, 80, 10.0, 20.0, 30.0, 40.0, Some(1.0)), Ok((10, 20, 30, 40)));
        // 150% 缩放
        assert_eq!(crop_rect(200, 120, 10.0, 20.0, 30.0, 40.0, Some(1.5)), Ok((15, 30, 45, 60)));
        // 超出边界会被夹回
        assert_eq!(crop_rect(100, 80, 90.0, 70.0, 30.0, 40.0, Some(1.0)), Ok((90, 70, 10, 10)));
        // 太小
        assert!(crop_rect(100, 80, 0.0, 0.0, 1.0, 1.0, Some(1.0)).is_err());
        // 非法 dpr 按 1 处理
        assert_eq!(crop_rect(100, 80, 0.0, 0.0, 10.0, 10.0, Some(f64::NAN)), Ok((0, 0, 10, 10)));
    }
}
