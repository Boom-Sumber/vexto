//! S7 截图工具：抓图（区域 / 全屏 / 活动窗口）+ 输出（PNG / 剪贴板 / 路径）。
//!
//! 入口与生命周期：
//! - 区域：热键 / 托盘 → 全屏框选浮层（`ui/screenshot_overlay.html`）→
//!   `shot_submit_region` → 本模块抓图并交付 → 成功即销毁浮层。
//! - 全屏 / 活动窗口：托盘或设置页按钮 → 直接抓图交付，不建浮层。
//!
//! **未启用时零资源**（ADR-040）：本模块没有常驻线程 / 进程 / 监听；
//! 只有热键或托盘动作真正发生时才跑一次。

pub mod capture;
pub mod feedback;
pub mod input;
pub mod output;
pub mod overlay;
pub mod pin;
pub mod preview;
pub mod scroll;
pub mod stitch;

use crate::ocr::CapturedImage;

/// 一次抓图的模式（区域截图已改走框选快照，不再经过这里）。
#[derive(Debug, Clone, Copy)]
pub enum Mode {
    FullScreen,
    ActiveWindow,
}

/// 只抓图不交付（结果框 / 预览 / 钉屏会直接用）。
pub fn grab(mode: Mode) -> Result<CapturedImage, String> {
    match mode {
        Mode::FullScreen => capture::capture_full_screen(),
        Mode::ActiveWindow => capture::capture_active_window(),
    }
}

/// 全屏截图统一入口：抓图 → 按「截图后」配置自动交付 → 右下角缩略图反馈。
/// PrintScreen 热键、设置页按钮、托盘全屏都走这里。
pub fn capture_full_feedback(app: &tauri::AppHandle) -> Result<String, String> {
    let img = grab(Mode::FullScreen)?;
    let (w, h) = (img.width as u32, img.height as u32);
    let staged = preview::stage_image(&img)?;
    let cfg = crate::config::load();
    let delivered = output::deliver(&img, &cfg);
    feedback::set_source(staged, w, h);

    let a = app.clone();
    std::thread::spawn(move || {
        // 建第二个 WebView 必须跨过当前事件循环 tick（坑 11）。
        std::thread::sleep(std::time::Duration::from_millis(300));
        let b = a.clone();
        let _ = a.run_on_main_thread(move || crate::popup::open_feedback_window(&b));
    });

    match delivered {
        Ok(d) => Ok(d.message),
        Err(e) => Ok(format!(
            "已截图，但自动交付失败：{e}（右下角缩略图可手动复制）"
        )),
    }
}

/// 托盘 / 设置页触发的全屏 / 活动窗口截图。
///
/// 全屏走 `capture_full_feedback`；活动窗口仍走居中结果框。
pub fn tray_capture(app: &tauri::AppHandle, mode: Mode) {
    if !crate::config::load().enable_screenshot {
        crate::open_settings_at(app, "screenshot");
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let r = match mode {
            Mode::FullScreen => capture_full_feedback(&app),
            Mode::ActiveWindow => capture_active_preview(&app).map(|_| String::new()),
        };
        if let Err(e) = r {
            crate::dlog::dlog(&format!("screenshot: 失败：{e}"));
            show_error(&e);
        }
    });
}

/// 活动窗口截图：暂存后打开居中结果框（与 S7 会话 70/71 一致）。
fn capture_active_preview(app: &tauri::AppHandle) -> Result<(), String> {
    let img = grab(Mode::ActiveWindow)?;
    let (w, h) = (img.width as u32, img.height as u32);
    let path = preview::stage_image(&img)?;
    overlay::set_result(path, w, h, overlay::ResultKind::Full);
    let a = app.clone();
    std::thread::sleep(std::time::Duration::from_millis(300));
    let _ = app.run_on_main_thread(move || {
        crate::region_select::open_result(&a, crate::region_select::Purpose::Shot);
    });
    Ok(())
}

/// 原生错误框。只在没有浮层可用的路径（托盘 / 设置页按钮）使用。
pub fn show_error(msg: &str) {
    use windows::core::HSTRING;
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR};
    let text = HSTRING::from(msg);
    let title = HSTRING::from("Vexto 截图");
    unsafe {
        MessageBoxW(None, &text, &title, MB_ICONERROR);
    }
}
