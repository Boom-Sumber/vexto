//! 通用框选浮层（S7）：OCR 与截图共用一套窗口、坐标换算与取消语义。
//!
//! 两种用途只有三处不同：窗口 label、页面、事件/命令名。
//! - OCR：`ocr_overlay.html`，`ocr_submit_region` / `ocr_cancel` / `ocr:result`
//! - 截图：`screenshot_overlay.html`，`shot_submit_region` / `shot_cancel` / `shot:result`
//!
//! **OCR 的行为与事件名不变**（ADR-041 / 风险 109）：本模块只是把 S5 的建窗
//! 代码搬过来，参数、藏窗时序、坐标换算逐字保留。

use rust_i18n::t;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use windows::Win32::UI::WindowsAndMessaging::{
    SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WDA_NONE,
};

use crate::popup::{OCR_LABEL, SCROLL_LABEL, SHOT_LABEL, WEBVIEW_ARGS};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    Ocr,
    Shot,
    Scroll,
}

impl Purpose {
    pub fn label(self) -> &'static str {
        match self {
            Purpose::Ocr => OCR_LABEL,
            Purpose::Shot => SHOT_LABEL,
            Purpose::Scroll => SCROLL_LABEL,
        }
    }

    fn page(self) -> &'static str {
        match self {
            Purpose::Ocr => "ocr_overlay.html",
            Purpose::Shot => "screenshot_overlay.html",
            Purpose::Scroll => "scroll_overlay.html",
        }
    }

    fn title(self) -> String {
        match self {
            Purpose::Ocr => t!("window.ocrTranslate").into_owned(),
            Purpose::Shot => t!("window.feedback").into_owned(),
            Purpose::Scroll => t!("window.scroll").into_owned(),
        }
    }
}

/// 打开框选浮层。已存在则聚焦（正常流程关闭即销毁，ADR-007）。
/// **必须在主线程调用**（热键回调本身就在事件循环线程）。
///
/// 多显示器：用物理坐标摆位（虚拟屏幕原点可能是负数，逻辑坐标会摆错）。
/// 混合 DPI 的多屏是 MVP 之外的边界，先按单值 dpr 处理（前端上报）。
pub fn open(app: &AppHandle, purpose: Purpose) {
    open_with(app, purpose, true);
}

/// 打开「结果框」：不抓新快照，直接显示 `overlay::set_result` 里已经准备好的
/// 全屏结果图。前端从 `shot_overlay_state` 的 `mode:"full"` 进入结果态。
pub fn open_result(app: &AppHandle, purpose: Purpose) {
    if let Some(w) = app.get_webview_window(purpose.label()) {
        // 已有浮层：直接把结果推过去，不要 destroy——销毁会清掉刚设好的结果文件。
        let _ = w.show();
        let _ = w.set_focus();
        let _ = w.emit("shot:result", crate::screenshot::overlay::result_state());
        return;
    }
    open_with(app, purpose, false);
}

fn open_with(app: &AppHandle, purpose: Purpose, snapshot: bool) {
    let label = purpose.label();
    if let Some(w) = app.get_webview_window(label) {
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }

    // 截图 / 滚动：先抓一张全屏快照再做浮层。必须赶在窗口创建之前，
    // 否则快照里会有浮层自己；放大镜与最终裁剪都用这张图。
    // OCR（S5）保持原路径：透明浮层 + 实时屏幕，不受影响。
    // 全屏结果框（open_result）已经拿到结果图，不再抓快照。
    if snapshot && purpose != Purpose::Ocr {
        if let Err(e) = crate::screenshot::overlay::capture() {
            crate::dlog::dlog(&format!("region overlay snapshot failed: {e}"));
            crate::screenshot::show_error(
                &t!("error.screenshot.initFailed", detail = e).into_owned(),
            );
            return;
        }
    }

    let (vx, vy, vw, vh) = crate::ocr::virtual_screen();
    let r = WebviewWindowBuilder::new(app, label, WebviewUrl::App(purpose.page().into()))
        .title(purpose.title())
        .decorations(false)
        .shadow(false)
        .transparent(true)
        .always_on_top(true)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .skip_taskbar(true)
        // 会话 32 同款防白屏：先藏，前端 window_ready 再显示。
        .visible(false)
        .additional_browser_args(WEBVIEW_ARGS)
        .build();

    match r {
        Ok(win) => {
            if purpose != Purpose::Ocr {
                win.on_window_event(|ev| {
                    if matches!(ev, tauri::WindowEvent::Destroyed) {
                        crate::screenshot::overlay::on_window_destroyed();
                    }
                });
            }
            let _ = win.set_position(tauri::PhysicalPosition::new(vx, vy));
            let _ = win.set_size(tauri::PhysicalSize::new(vw.max(1) as u32, vh.max(1) as u32));
            let _ = win.show();
            let _ = win.set_focus();
        }
        Err(e) => crate::dlog::dlog(&format!("create region overlay failed: {e}")),
    }
}

/// 进入 / 退出滚动捕获态：
/// - 窗口点击穿透，目标窗口才能收到滚动消息；
/// - `WDA_EXCLUDEFROMCAPTURE` 让边框只给人看，不进入 GDI 截屏。
///
/// 退出时必须还原，否则结果框无法点击、也会被系统截图排除。
pub fn set_capture_mode(app: &AppHandle, purpose: Purpose, on: bool) {
    if purpose == Purpose::Ocr {
        return;
    }
    if let Some(win) = app.get_webview_window(purpose.label()) {
        let _ = win.set_ignore_cursor_events(on);
        if let Ok(hwnd) = win.hwnd() {
            let affinity = if on {
                WDA_EXCLUDEFROMCAPTURE
            } else {
                WDA_NONE
            };
            unsafe {
                let _ = SetWindowDisplayAffinity(hwnd, affinity);
            }
        }
    }
}

/// 销毁浮层。OCR 与截图共用同一套「关窗即销毁」语义。
pub fn close(app: &AppHandle, purpose: Purpose) {
    set_capture_mode(app, purpose, false);
    if let Some(w) = app.get_webview_window(purpose.label()) {
        let _ = w.destroy();
    }
}

/// CSS 坐标 → 屏幕物理像素。抽成纯函数方便单测。
///
/// `x/y/w/h` 是浮层窗口内的 CSS 像素，`dpr` 是设备像素比；
/// 结果加上虚拟屏幕原点（多显示器时可能是负数）。
pub fn scale_rect(
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    dpr: f64,
    vs: (i32, i32, i32, i32),
) -> (i32, i32, i32, i32) {
    let (vsx, vsy, vsw, vsh) = vs;
    let dpr = if dpr.is_finite() && dpr > 0.0 { dpr } else { 1.0 };
    let px = vsx + (x * dpr).round() as i32;
    let py = vsy + (y * dpr).round() as i32;
    let pw = ((w * dpr).round() as i32).min(vsw);
    let ph = ((h * dpr).round() as i32).min(vsh);
    (px, py, pw, ph)
}

/// 用真实虚拟屏幕原点换算。
pub fn physical_rect(x: f64, y: f64, w: f64, h: f64, dpr: Option<f64>) -> (i32, i32, i32, i32) {
    let vs = crate::ocr::virtual_screen();
    scale_rect(x, y, w, h, dpr.unwrap_or(1.0), vs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_rect_applies_dpr_and_origin() {
        // 100% 缩放、虚拟屏原点 (0,0)
        assert_eq!(
            scale_rect(10.0, 20.0, 30.0, 40.0, 1.0, (0, 0, 1920, 1080)),
            (10, 20, 30, 40)
        );
        // 150% 缩放：CSS 10 → 物理 15
        assert_eq!(
            scale_rect(10.0, 20.0, 30.0, 40.0, 1.5, (0, 0, 1920, 1080)),
            (15, 30, 45, 60)
        );
        // 副屏在左侧：虚拟屏原点为负
        assert_eq!(
            scale_rect(10.0, 10.0, 100.0, 100.0, 1.0, (-1920, 0, 3840, 1080)),
            (-1910, 10, 100, 100)
        );
    }

    #[test]
    fn scale_rect_clamps_to_virtual_screen() {
        let (_, _, w, h) = scale_rect(0.0, 0.0, 9999.0, 9999.0, 1.0, (0, 0, 1920, 1080));
        assert_eq!((w, h), (1920, 1080));
    }

    #[test]
    fn bad_dpr_falls_back_to_one() {
        assert_eq!(
            scale_rect(10.0, 10.0, 10.0, 10.0, f64::NAN, (0, 0, 1920, 1080)),
            (10, 10, 10, 10)
        );
        assert_eq!(
            scale_rect(10.0, 10.0, 10.0, 10.0, 0.0, (0, 0, 1920, 1080)),
            (10, 10, 10, 10)
        );
    }
}
