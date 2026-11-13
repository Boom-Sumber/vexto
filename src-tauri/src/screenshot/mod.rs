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
pub mod pin;
pub mod preview;
pub mod scroll;
pub mod stitch;

use crate::config::Config;
use crate::ocr::CapturedImage;

pub use output::Delivered;

/// 一次抓图的模式。区域模式带的是**物理像素**坐标。
#[derive(Debug, Clone, Copy)]
pub enum Mode {
    Region {
        x: i32,
        y: i32,
        w: i32,
        h: i32,
    },
    FullScreen,
    ActiveWindow,
}

/// 按模式抓一张图并交付。
pub fn run(mode: Mode) -> Result<Delivered, String> {
    let cfg = crate::config::load();
    run_with(mode, &cfg)
}

/// 给测试 / 调用方一个显式的配置入口（避免测试读用户真实配置）。
pub fn run_with(mode: Mode, cfg: &Config) -> Result<Delivered, String> {
    let img = grab(mode)?;
    output::deliver(&img, cfg)
}

/// 只抓图不交付（会话 63 的预览 / 钉屏会直接用）。
pub fn grab(mode: Mode) -> Result<CapturedImage, String> {
    match mode {
        Mode::Region { x, y, w, h } => capture::capture_region_checked(x, y, w, h),
        Mode::FullScreen => capture::capture_full_screen(),
        Mode::ActiveWindow => capture::capture_active_window(),
    }
}

/// 托盘 / 设置页触发的全屏 / 活动窗口截图。
///
/// 跑在工作线程（GDI + PNG + 剪贴板都不该占事件循环）；成功不弹窗，
/// 剪贴板 / 文件就是反馈；失败用原生消息框说明原因——没有浮层可显示，
/// 不能只写日志让用户以为按了没反应。
pub fn tray_capture(app: &tauri::AppHandle, mode: Mode) {
    if !crate::config::load().enable_screenshot {
        crate::open_settings_at(app, "screenshot");
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || match run(mode) {
        Ok(d) => {
            crate::dlog::dlog(&format!("screenshot: {}", d.message));
            // 托盘 / 设置页没有浮层可显示结果，用系统通知确认一下。
            // 通知被系统策略挡掉不影响交付（剪贴板 / 文件已经好了）。
            use tauri_plugin_notification::NotificationExt;
            if let Err(e) = app
                .notification()
                .builder()
                .title("Vexto 截图")
                .body(&d.message)
                .show()
            {
                crate::dlog::dlog(&format!("screenshot: 通知失败：{e}"));
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
