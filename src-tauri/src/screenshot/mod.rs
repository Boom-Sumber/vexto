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

/// 托盘 / 设置页触发的全屏 / 活动窗口截图。
///
/// 跑在工作线程（GDI + PNG 都不该占事件循环）；抓完不直接交付，先暂存并打开
/// 居中结果框，由用户确认保存 / 复制 / 钉屏 / 预览。建结果窗必须等 300ms
/// 跨过当前事件循环 tick（IPC / 托盘回调里同步建第二个 WebView 会卡 0x0）。
pub fn tray_capture(app: &tauri::AppHandle, mode: Mode) {
    if !crate::config::load().enable_screenshot {
        crate::open_settings_at(app, "screenshot");
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || match grab(mode) {
        Ok(img) => {
            let (w, h) = (img.width as u32, img.height as u32);
            match preview::stage_image(&img) {
                Ok(path) => {
                    overlay::set_result(path, w, h, overlay::ResultKind::Full);
                    let a = app.clone();
                    std::thread::sleep(std::time::Duration::from_millis(300));
                    let _ = app.run_on_main_thread(move || {
                        crate::region_select::open_result(
                            &a,
                            crate::region_select::Purpose::Shot,
                        );
                    });
                }
                Err(e) => {
                    crate::dlog::dlog(&format!("screenshot: 暂存失败：{e}"));
                    show_error(&e);
                }
            }
        }
        Err(e) => {
            crate::dlog::dlog(&format!("screenshot: 失败：{e}"));
            show_error(&e);
        }
    });
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
