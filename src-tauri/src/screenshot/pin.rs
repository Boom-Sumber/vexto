//! 钉屏：**一个窗口放多张图**（门 1 裁定，ADR-040 / S7 §8 #4）。
//!
//! 钉屏窗口只显示暂存文件（自定义 `vextoimg` 协议，ADR-044），不持有像素副本；
//! 切换图片是换 `<img>` 的 URL，不新开 WebView2 环境。窗口关闭走 `destroy()`，
//! 销毁事件里清状态并删除暂存文件（预览窗口正在用的那张除外）。

use crate::screenshot::preview;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::Emitter;

struct State {
    images: Vec<PathBuf>,
    index: usize,
}

static PIN: Mutex<State> = Mutex::new(State {
    images: Vec::new(),
    index: 0,
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
    match current() {
        Some((path, index, count)) => serde_json::json!({
            "url": preview::display_url(&path),
            "index": index,
            "count": count,
        }),
        None => serde_json::json!({ "url": null, "index": 0, "count": 0 }),
    }
}

/// 按当前图片等比设置窗口大小（物理像素 → 逻辑像素，避免 DPI 双算）。
pub fn fit_window(win: &tauri::WebviewWindow) {
    let Some((path, _, _)) = current() else {
        return;
    };
    let Ok((w, h)) = preview::image_size(&path) else {
        return;
    };
    let (_, _, vw, vh) = crate::ocr::virtual_screen();
    let scale = (vw as f64 * 0.7 / w as f64)
        .min(vh as f64 * 0.8 / h as f64)
        .min(1.0);
    let pw = (w as f64 * scale).max(160.0);
    let ph = (h as f64 * scale).max(120.0);
    let sf = win.scale_factor().unwrap_or(1.0);
    let _ = win.set_size(tauri::LogicalSize::new(pw / sf, ph / sf));
}

pub fn emit_state(win: &tauri::WebviewWindow) {
    let _ = win.emit("pin:show", state_value());
}

/// 窗口销毁：清空状态，并删除不再被预览使用的暂存文件。
pub fn on_window_destroyed() {
    let files = {
        let mut g = lock();
        g.index = 0;
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
