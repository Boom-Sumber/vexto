//! 截图 / 滚动框选的快照与选区动作（会话 65 选框改版）。
//!
//! 打开浮层前先抓一张全屏快照，前端用自定义 `vextoimg` 协议显示它：
//! - 放大的取色镜和最终裁剪都基于同一张图，用户看到什么就裁什么；
//! - 选区不再「松手就保存」，动作由选区下方的操作栏显式触发；
//! - 图片始终走文件 + 协议，不走 IPC base64（ADR-044）。
//!
//! 快照只在浮层存活期间有效，窗口销毁时删除。

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

/// 抓取整个虚拟屏幕作为框选底图。**必须在创建浮层窗口之前调用**，
/// 否则会把浮层自己截进快照。
pub fn capture() -> Result<(), String> {
    clear();
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
    let border = crate::config::load().screenshot_border_width.clamp(3, 5);
    let (vx, vy, _, _) = crate::ocr::virtual_screen();
    let g = lock();
    match g.as_ref() {
        Some(s) => serde_json::json!({
            "url": preview::display_url(&s.path),
            "width": s.width,
            "height": s.height,
            "originX": vx,
            "originY": vy,
            "border": border,
        }),
        None => serde_json::json!({ "url": null, "border": border, "originX": vx, "originY": vy }),
    }
}

pub fn clear() {
    let taken = lock().take();
    if let Some(s) = taken {
        let _ = std::fs::remove_file(s.path);
    }
}

/// 浮层窗口销毁：快照只在它活着时有意义。
pub fn on_window_destroyed() {
    clear();
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
        "save" => {
            let dir = output::resolve_dir(&cfg);
            std::fs::create_dir_all(&dir).map_err(|e| format!("创建截图目录失败：{e}"))?;
            let dst = output::next_png_path(&dir);
            let (ow, oh) = preview::crop_region_png(&src, &dst, sx, sy, sw, sh)?;
            Ok(preview::ActionOutcome {
                message: format!("已保存（{ow}×{oh}）：{}", dst.display()),
                path: Some(dst),
                text: None,
            })
        }
        "path" => {
            let dir = output::resolve_dir(&cfg);
            std::fs::create_dir_all(&dir).map_err(|e| format!("创建截图目录失败：{e}"))?;
            let dst = output::next_png_path(&dir);
            let (ow, oh) = preview::crop_region_png(&src, &dst, sx, sy, sw, sh)?;
            if let Err(e) = output::copy_path(&dst) {
                return Err(format!("已保存（{ow}×{oh}）到 {}，但复制路径失败：{e}", dst.display()));
            }
            Ok(preview::ActionOutcome {
                message: format!("已保存并复制路径（{ow}×{oh}）：{}", dst.display()),
                path: Some(dst),
                text: None,
            })
        }
        "clipboard" => {
            let img = preview::crop_region_to_image(&src, sx, sy, sw, sh)?;
            output::copy_image(&img)?;
            Ok(preview::ActionOutcome {
                message: format!("已复制到剪贴板（{}×{}）", img.width, img.height),
                path: None,
                text: None,
            })
        }
        "default" => {
            let img = preview::crop_region_to_image(&src, sx, sy, sw, sh)?;
            let d = output::deliver(&img, &cfg)?;
            Ok(preview::ActionOutcome {
                message: d.message,
                path: d.path,
                text: None,
            })
        }
        "pin" => {
            let dst = staged_crop(&src, sx, sy, sw, sh)?;
            crate::screenshot::pin::add_staged(dst)?;
            Ok(preview::ActionOutcome {
                message: format!("已钉屏（{sw}×{sh}）"),
                path: None,
                text: None,
            })
        }
        "preview" => {
            let dst = staged_crop(&src, sx, sy, sw, sh)?;
            preview::set_source(dst);
            Ok(preview::ActionOutcome {
                message: format!("已打开预览（{sw}×{sh}）"),
                path: None,
                text: None,
            })
        }
        "ocr" => {
            let img = preview::crop_region_to_image(&src, sx, sy, sw, sh)?;
            let text = crate::ocr::ocr_bitmap(&img)?;
            let empty = text.trim().is_empty();
            Ok(preview::ActionOutcome {
                message: if empty {
                    "没有识别到文字".into()
                } else {
                    "识别完成".into()
                },
                path: None,
                text: Some(text),
            })
        }
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
