//! 钉屏：**一个窗口放多张图**（门 1 裁定，ADR-040 / S7 §8 #4）。
//!
//! 钉屏窗口只显示暂存文件（自定义 `vextoimg` 协议，ADR-044），不持有像素副本；
//! 切换图片是换 `<img>` 的 URL，不新开 WebView2 环境。窗口关闭走 `destroy()`，
//! 销毁事件里清状态并删除暂存文件（预览窗口正在用的那张除外）。
//!
//! 会话 66 起：悬停时窗口上方出现操作条（复制 / 缩放比例 / 重置 / 关闭），
//! 滚轮以窗口中心为锚点缩放；2px 边框由前端画。

use crate::screenshot::{output, preview};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::Emitter;

struct State {
    images: Vec<PathBuf>,
    index: usize,
    /// 相对「适配窗口」的缩放倍数；1.0 = 100%。
    zoom: f64,
}

static PIN: Mutex<State> = Mutex::new(State {
    images: Vec::new(),
    index: 0,
    zoom: 1.0,
});

fn lock() -> std::sync::MutexGuard<'static, State> {
    PIN.lock().unwrap_or_else(|e| e.into_inner())
}

/// 从一张原图钉屏（先暂存）。
pub fn add(src: &Path) -> Result<(PathBuf, usize, usize), String> {
    let staged = preview::stage_file(src)?;
    add_staged(staged)
}

/// 已经在暂存目录里的图片（预览裁切结果）直接加入。
pub fn add_staged(staged: PathBuf) -> Result<(PathBuf, usize, usize), String> {
    let mut g = lock();
    match g.images.iter().position(|p| p == &staged) {
        Some(i) => g.index = i,
        None => {
            g.images.push(staged.clone());
            g.index = g.images.len() - 1;
        }
    }
    g.zoom = 1.0;
    Ok((staged, g.index, g.images.len()))
}

pub fn current() -> Option<(PathBuf, usize, usize)> {
    let g = lock();
    let path = g.images.get(g.index)?.clone();
    Some((path, g.index, g.images.len()))
}

pub fn step(delta: i32) -> Option<(PathBuf, usize, usize)> {
    let mut g = lock();
    if g.images.is_empty() {
        return None;
    }
    g.index = step_index(g.index, g.images.len(), delta);
    g.zoom = 1.0;
    let path = g.images[g.index].clone();
    Some((path, g.index, g.images.len()))
}

/// 循环切换下标；单张时原地不动。
fn step_index(index: usize, count: usize, delta: i32) -> usize {
    if count == 0 {
        return 0;
    }
    let count_i = count as i32;
    (index as i32 + delta).rem_euclid(count_i) as usize
}

pub fn owns(path: &Path) -> bool {
    lock().images.iter().any(|p| p == path)
}

pub fn state_value() -> serde_json::Value {
    let g = lock();
    let percent = (g.zoom * 100.0).round() as i64;
    match g.images.get(g.index) {
        Some(path) => serde_json::json!({
            "url": preview::display_url(path),
            "index": g.index,
            "count": g.images.len(),
            "zoom": percent,
        }),
        None => serde_json::json!({ "url": null, "index": 0, "count": 0, "zoom": percent }),
    }
}

/// 适配窗口的基准尺寸（物理像素）：最大占屏幕 70% × 80%，且不超过原图。
/// 最小宽度保证顶部操作条放得下。
fn fit_size(w: f64, h: f64) -> (f64, f64) {
    let (_, _, vw, vh) = crate::ocr::virtual_screen();
    let scale = (vw as f64 * 0.7 / w)
        .min(vh as f64 * 0.8 / h)
        .min(1.0);
    ((w * scale).max(260.0), (h * scale).max(60.0))
}

/// 按当前图片等比设置窗口大小（物理像素 → 逻辑像素，避免 DPI 双算）。
pub fn fit_window(win: &tauri::WebviewWindow) {
    let Some((path, _, _)) = current() else {
        return;
    };
    let Ok((w, h)) = preview::image_size(&path) else {
        return;
    };
    let (pw, ph) = fit_size(w as f64, h as f64);
    let sf = win.scale_factor().unwrap_or(1.0);
    let _ = win.set_size(tauri::LogicalSize::new(pw / sf, ph / sf));
}

/// 滚轮缩放：`delta > 0` 放大、`< 0` 缩小；`reset = true` 回到 100%。
/// 以窗口中心为锚点调整大小与位置，返回新的百分比。
pub fn zoom_window(
    win: &tauri::WebviewWindow,
    delta: i32,
    reset: bool,
) -> Option<i64> {
    let (path, zoom) = {
        let mut g = lock();
        let path = g.images.get(g.index)?.clone();
        let next = if reset {
            1.0
        } else {
            let step = if delta > 0 { 1.1 } else { 1.0 / 1.1 };
            (g.zoom * step).clamp(0.2, 4.0)
        };
        g.zoom = next;
        (path, next)
    };
    let Ok((w, h)) = preview::image_size(&path) else {
        return None;
    };
    let (base_w, base_h) = fit_size(w as f64, h as f64);
    let (vx, vy, vw, vh) = crate::ocr::virtual_screen();
    let tw = (base_w * zoom).min(vw as f64 * 0.98).max(160.0);
    let th = (base_h * zoom).min(vh as f64 * 0.98).max(60.0);

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
    let _ = win.emit("pin:zoom", serde_json::json!({ "percent": percent }));
    Some(percent)
}

/// 复制当前钉屏图片到剪贴板（大图受 `MAX_DECODE_PIXELS` 限制）。
pub fn copy_current() -> Result<String, String> {
    let (path, _, _) = current().ok_or_else(|| "钉屏里还没有图片".to_string())?;
    let (w, h) = preview::image_size(&path)?;
    let img = preview::crop_region_to_image(&path, 0, 0, w, h)?;
    output::copy_image(&img)?;
    Ok(format!("已复制图片（{}×{}）", img.width, img.height))
}

pub fn emit_state(win: &tauri::WebviewWindow) {
    let _ = win.emit("pin:show", state_value());
}

/// 窗口销毁：清空状态，并删除不再被预览使用的暂存文件。
pub fn on_window_destroyed() {
    let files = {
        let mut g = lock();
        g.index = 0;
        g.zoom = 1.0;
        std::mem::take(&mut g.images)
    };
    let preview_source = preview::source();
    for f in files {
        if preview_source.as_deref() != Some(f.as_path()) {
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
