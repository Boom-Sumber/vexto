//! 钉屏：单窗多图（默认）或每图一窗（`pinscreen_single_window = false`）。
//!
//! 图片走自定义 `vextoimg` 协议（ADR-044），窗口不持有像素副本。
//! 每个窗口在 `PINS` 里有一份条目（图片列表 / 当前下标 / 缩放）；
//! 关闭窗口走 `destroy()`，销毁事件里按窗口 label 清理。
//!
//! 缩放模型（会话 67）：窗口在屏幕范围内按「适配尺寸 × 缩放」变大/变小；
//! 到达屏幕尺寸后窗口不再变大，改由前端把 `<img>` 缩放到目标尺寸（超出部分裁切），
//! 这样百分比和画面始终一致，也不会出现窗口被夹住后「数字在涨、图不动」。

use crate::screenshot::{output, preview};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use tauri::Emitter;

struct Entry {
    images: Vec<PathBuf>,
    index: usize,
    /// 相对「适配窗口」的缩放倍数；1.0 = 100%。
    zoom: f64,
}

static PINS: LazyLock<Mutex<HashMap<String, Entry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn lock() -> std::sync::MutexGuard<'static, HashMap<String, Entry>> {
    PINS.lock().unwrap_or_else(|e| e.into_inner())
}

fn stamp() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// 从一张原图钉屏（先暂存）。`single` = 单窗多图模式。
/// 返回 `(窗口 label, 当前下标, 总数)`。
pub fn add(src: &Path, single: bool) -> Result<(String, usize, usize), String> {
    let staged = preview::stage_file(src)?;
    add_staged(staged, single)
}

/// 已经在暂存目录里的图片（预览裁切结果）直接加入。
pub fn add_staged(staged: PathBuf, single: bool) -> Result<(String, usize, usize), String> {
    let label = if single {
        crate::popup::PIN_LABEL.to_string()
    } else {
        format!("pin-{}", stamp())
    };
    let mut g = lock();
    let e = g.entry(label.clone()).or_insert_with(|| Entry {
        images: Vec::new(),
        index: 0,
        zoom: 1.0,
    });
    if single {
        match e.images.iter().position(|p| p == &staged) {
            Some(i) => e.index = i,
            None => {
                e.images.push(staged);
                e.index = e.images.len() - 1;
            }
        }
    } else {
        e.images = vec![staged];
        e.index = 0;
    }
    e.zoom = 1.0;
    Ok((label, e.index, e.images.len()))
}

pub fn current(label: &str) -> Option<(PathBuf, usize, usize)> {
    let g = lock();
    let e = g.get(label)?;
    let path = e.images.get(e.index)?.clone();
    Some((path, e.index, e.images.len()))
}

pub fn step(label: &str, delta: i32) -> Option<(PathBuf, usize, usize)> {
    let mut g = lock();
    let e = g.get_mut(label)?;
    if e.images.is_empty() {
        return None;
    }
    e.index = step_index(e.index, e.images.len(), delta);
    e.zoom = 1.0;
    let path = e.images[e.index].clone();
    Some((path, e.index, e.images.len()))
}

/// 循环切换下标；单张时原地不动。
fn step_index(index: usize, count: usize, delta: i32) -> usize {
    if count == 0 {
        return 0;
    }
    let count_i = count as i32;
    (index as i32 + delta).rem_euclid(count_i) as usize
}

/// 是否还有任一钉屏窗口在用这张暂存文件（关闭一个窗口时不误删别人的图）。
pub fn owns(path: &Path) -> bool {
    lock()
        .values()
        .any(|e| e.images.iter().any(|p| p == path))
}

pub fn state_value(label: &str) -> serde_json::Value {
    let g = lock();
    let Some(e) = g.get(label) else {
        return serde_json::json!({
            "url": null, "index": 0, "count": 0, "zoom": 100, "baseW": 0, "baseH": 0
        });
    };
    let percent = (e.zoom * 100.0).round() as i64;
    let (base_w, base_h) = e
        .images
        .get(e.index)
        .and_then(|p| preview::image_size(p).ok())
        .map(|(w, h)| display_size(w as f64, h as f64))
        .unwrap_or((0.0, 0.0));
    match e.images.get(e.index) {
        Some(path) => serde_json::json!({
            "url": preview::display_url(path),
            "index": e.index,
            "count": e.images.len(),
            "zoom": percent,
            "baseW": base_w,
            "baseH": base_h,
        }),
        None => serde_json::json!({
            "url": null, "index": 0, "count": 0, "zoom": percent, "baseW": base_w, "baseH": base_h
        }),
    }
}

/// 图片在 100% 时的显示尺寸（物理像素）：最大占屏幕 70% × 80%，且不超过原图。
fn display_size(w: f64, h: f64) -> (f64, f64) {
    let (_, _, vw, vh) = crate::ocr::virtual_screen();
    let scale = (vw as f64 * 0.7 / w)
        .min(vh as f64 * 0.8 / h)
        .min(1.0);
    (w * scale, h * scale)
}

/// 窗口尺寸在图片显示尺寸上再保证最小宽高（顶部操作条要放得下）。
/// 小图的窗口会比图片大，图片居中显示，不拉伸。
fn window_size(w: f64, h: f64) -> (f64, f64) {
    (w.max(260.0), h.max(60.0))
}

/// 按当前图片等比设置窗口大小（物理像素 → 逻辑像素，避免 DPI 双算）。
pub fn fit_window(win: &tauri::WebviewWindow) {
    let Some((path, _, _)) = current(win.label()) else {
        return;
    };
    let Ok((w, h)) = preview::image_size(&path) else {
        return;
    };
    let (dw, dh) = display_size(w as f64, h as f64);
    let (pw, ph) = window_size(dw, dh);
    let sf = win.scale_factor().unwrap_or(1.0);
    let _ = win.set_size(tauri::LogicalSize::new(pw / sf, ph / sf));
}

/// 滚轮缩放：`delta > 0` 放大、`< 0` 缩小；`reset = true` 回到 100%。
/// 窗口只在屏幕范围内变化；超出后由前端把图片缩放到目标尺寸。
pub fn zoom_window(win: &tauri::WebviewWindow, delta: i32, reset: bool) -> Option<i64> {
    let label = win.label().to_string();
    let (path, zoom) = {
        let mut g = lock();
        let e = g.get_mut(&label)?;
        let path = e.images.get(e.index)?.clone();
        let next = if reset {
            1.0
        } else {
            let step = if delta > 0 { 1.1 } else { 1.0 / 1.1 };
            (e.zoom * step).clamp(0.2, 4.0)
        };
        e.zoom = next;
        (path, next)
    };
    let Ok((w, h)) = preview::image_size(&path) else {
        return None;
    };
    let (base_w, base_h) = display_size(w as f64, h as f64);
    let (vx, vy, vw, vh) = crate::ocr::virtual_screen();
    let (win_w, win_h) = window_size(base_w * zoom, base_h * zoom);
    let tw = win_w.min(vw as f64 * 0.98).max(160.0);
    let th = win_h.min(vh as f64 * 0.98).max(60.0);

    // 以中心为锚点；窗口几何拿不到就只改大小。
    if let (Ok(size), Ok(pos)) = (win.inner_size(), win.inner_position()) {
        let cx = pos.x as f64 + size.width as f64 / 2.0;
        let cy = pos.y as f64 + size.height as f64 / 2.0;
        let max_x = vx + vw - tw as i32;
        let max_y = vy + vh - th as i32;
        let nx = (cx - tw / 2.0).round() as i32;
        let ny = (cy - th / 2.0).round() as i32;
        let nx = if max_x < vx { vx } else { nx.clamp(vx, max_x) };
        let ny = if max_y < vy { vy } else { ny.clamp(vy, max_y) };
        let _ = win.set_size(tauri::PhysicalSize::new(tw as u32, th as u32));
        let _ = win.set_position(tauri::PhysicalPosition::new(nx, ny));
    } else {
        let _ = win.set_size(tauri::PhysicalSize::new(tw as u32, th as u32));
    }
    let percent = (zoom * 100.0).round() as i64;
    let _ = win.emit(
        "pin:zoom",
        serde_json::json!({
            "percent": percent,
            "zoom": percent,
            "baseW": base_w,
            "baseH": base_h,
        }),
    );
    Some(percent)
}

/// 复制当前钉屏图片到剪贴板（大图受 `MAX_DECODE_PIXELS` 限制）。
pub fn copy_current(label: &str) -> Result<String, String> {
    let (path, _, _) = current(label).ok_or_else(|| "钉屏里还没有图片".to_string())?;
    let (w, h) = preview::image_size(&path)?;
    let img = preview::crop_region_to_image(&path, 0, 0, w, h)?;
    output::copy_image(&img)?;
    Ok(format!("已复制图片（{}×{}）", img.width, img.height))
}

pub fn emit_state(win: &tauri::WebviewWindow) {
    let _ = win.emit("pin:show", state_value(win.label()));
}

/// 某个钉屏窗口销毁：删条目；暂存文件若没被其它窗口 / 预览使用就删除。
pub fn on_window_destroyed(label: &str) {
    let files = {
        let mut g = lock();
        g.remove(label).map(|e| e.images).unwrap_or_default()
    };
    let preview_source = preview::source();
    for f in files {
        if preview_source.as_deref() != Some(f.as_path()) && !owns(&f) {
            let _ = std::fs::remove_file(f);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_index_wraps_both_ways() {
        assert_eq!(step_index(0, 3, 1), 1);
        assert_eq!(step_index(2, 3, 1), 0);
        assert_eq!(step_index(0, 3, -1), 2);
        assert_eq!(step_index(1, 1, 1), 0);
    }
}
