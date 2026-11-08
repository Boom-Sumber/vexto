//! 设置窗口的生命周期。
//!
//! **窗口一律运行时创建**：`tauri.conf.json` 里 `app.windows` 是 `[]`，启动时不建任何
//! WebView —— 这是"轻"的核心（启动即建窗会立刻拉起 6 个 msedgewebview2.exe，约 269 MB）。
//!
//! S1a 只做设置窗口。弹窗（popup.html）的 show / ready / close / resize 属 S1b，
//! 这次不写：没人调用的 `#[allow(dead_code)]` 是债，台账里明确记着要尽快还。

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

pub const SETTINGS_LABEL: &str = "settings";

/// 替换掉 wry 的默认参数，所以必须**原样**带上它默认的那串
/// （msWebOOUI 去掉迷你菜单、msPdfOOUI、msSmartScreenProtection）。
const WEBVIEW_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection \
--disable-gpu --disable-software-rasterizer --renderer-process-limit=1 \
--disable-background-timer-throttling --disable-backgrounding-occluded-windows \
--disable-renderer-backgrounding --disable-ipc-flooding-protection";

/// 打开设置窗口。已存在则聚焦，不重复创建。**必须在主线程调用。**
pub fn open_settings(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(SETTINGS_LABEL) {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
        return;
    }

    let r = WebviewWindowBuilder::new(app, SETTINGS_LABEL, WebviewUrl::App("settings.html".into()))
        .title("LightBox 设置")
        .inner_size(560.0, 620.0)
        .min_inner_size(460.0, 480.0)
        .resizable(true)
        .center()
        .additional_browser_args(WEBVIEW_ARGS)
        .build();

    match r {
        Ok(win) => {
            // 窗口销毁后收缩工作集。必须是 destroy 语义（关闭即销毁），不能用 hide，
            // 否则 6 个 msedgewebview2.exe 会一直驻留（ADR-007）。
            win.on_window_event(|ev| {
                if matches!(ev, tauri::WindowEvent::Destroyed) {
                    crate::dlog::dlog("popup: 设置窗口已销毁，1.2s 后收缩工作集");
                    crate::mem::trim_soon(1200);
                }
            });
        }
        Err(e) => crate::dlog::dlog(&format!("create settings failed: {e}")),
    }
}
