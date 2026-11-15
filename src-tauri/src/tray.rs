//! 托盘图标与右键菜单。
//!
//! 菜单是固定的使用入口（会话 73 用户定稿），不再按工具注册表逐项列出：
//! 划词翻译 / 输入翻译 / 截图（全屏、区域、活动窗口）/ 任务窗口（下载、互传）/
//! 设置 / 重启应用 / 退出程序。
//!
//! 菜单项 = 固定文案 + 图标；没有「…」后缀。工具配置统一从「设置」进。
//! 菜单 id 约定：叶子项用固定字符串，子菜单只是容器。

use crate::QUITTING;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use tauri::menu::{IconMenuItem, Menu, PredefinedMenuItem, Submenu};
use tauri::tray::TrayIconBuilder;
use tauri::AppHandle;

/// 翻译 / 输入翻译两项在工具停用时置灰。
static TRANSLATE_ITEM: Mutex<Option<IconMenuItem<tauri::Wry>>> = Mutex::new(None);
static INPUT_ITEM: Mutex<Option<IconMenuItem<tauri::Wry>>> = Mutex::new(None);

// 16×16 菜单图标（品牌色，透明底）；源文件在 src-tauri/icons/menu/。
// 新增图标时保持同样的 16px 画布与透明背景，docs/13-UI-DESIGN-SYSTEM.md §13.5。
const ICON_TRANSLATE: &[u8] = include_bytes!("../icons/menu/translate.png");
const ICON_INPUT: &[u8] = include_bytes!("../icons/menu/input.png");
const ICON_SHOT_FULL: &[u8] = include_bytes!("../icons/menu/shot-full.png");
const ICON_SHOT_REGION: &[u8] = include_bytes!("../icons/menu/shot-region.png");
const ICON_SHOT_WINDOW: &[u8] = include_bytes!("../icons/menu/shot-window.png");
const ICON_DOWNLOAD: &[u8] = include_bytes!("../icons/menu/download.png");
const ICON_TRANSFER: &[u8] = include_bytes!("../icons/menu/transfer.png");
const ICON_SETTINGS: &[u8] = include_bytes!("../icons/menu/settings.png");
const ICON_RESTART: &[u8] = include_bytes!("../icons/menu/restart.png");
const ICON_QUIT: &[u8] = include_bytes!("../icons/menu/quit.png");

fn menu_icon(bytes: &'static [u8]) -> Option<tauri::image::Image<'static>> {
    tauri::image::Image::from_bytes(bytes).ok()
}

fn icon_item(
    app: &AppHandle,
    id: &str,
    text: &str,
    enabled: bool,
    bytes: &'static [u8],
) -> tauri::Result<IconMenuItem<tauri::Wry>> {
    IconMenuItem::with_id(app, id, text, enabled, menu_icon(bytes), None::<&str>)
}

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let translate_on = crate::tools::registry::by_id("translate")
        .map(|t| t.enabled())
        .unwrap_or(true);

    let translate = icon_item(app, "translate", "划词翻译", translate_on, ICON_TRANSLATE)?;
    let input = icon_item(app, "input_translate", "输入翻译", translate_on, ICON_INPUT)?;
    *TRANSLATE_ITEM.lock().unwrap_or_else(|e| e.into_inner()) = Some(translate.clone());
    *INPUT_ITEM.lock().unwrap_or_else(|e| e.into_inner()) = Some(input.clone());

    let shot_full = icon_item(app, "shot-full", "全屏截图", true, ICON_SHOT_FULL)?;
    let shot_region = icon_item(app, "shot-region", "区域截图", true, ICON_SHOT_REGION)?;
    let shot_window = icon_item(app, "shot-window", "活动窗口截图", true, ICON_SHOT_WINDOW)?;
    let shot_menu = Submenu::with_items(
        app,
        "截图",
        true,
        &[&shot_full, &shot_region, &shot_window],
    )?;

    let downloads = icon_item(app, "downloads", "下载", true, ICON_DOWNLOAD)?;
    let transfer = icon_item(app, "transfer", "互传", true, ICON_TRANSFER)?;
    let task_menu = Submenu::with_items(app, "任务窗口", true, &[&downloads, &transfer])?;

    let settings = icon_item(app, "settings", "设置", true, ICON_SETTINGS)?;
    let restart = icon_item(app, "restart", "重启应用", true, ICON_RESTART)?;
    let quit = icon_item(app, "quit", "退出程序", true, ICON_QUIT)?;

    let separator = PredefinedMenuItem::separator(app)?;
    let items: Vec<&dyn tauri::menu::IsMenuItem<tauri::Wry>> = vec![
        &translate,
        &input,
        &shot_menu,
        &task_menu,
        &separator,
        &settings,
        &restart,
        &quit,
    ];
    let menu = Menu::with_items(app, &items)?;

    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip("Vexto — 划词翻译")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, ev| {
            let id = ev.id().as_ref().to_string();
            match id.as_str() {
                "translate" => crate::translate_selection(app.clone()),
                "input_translate" => crate::open_input_translate(app.clone()),
                "shot-region" => {
                    if crate::config::load().enable_screenshot {
                        crate::popup::open_shot_overlay(app);
                    } else {
                        crate::open_settings_at(app, "screenshot");
                    }
                }
                "shot-full" => crate::screenshot::tray_capture(
                    app,
                    crate::screenshot::Mode::FullScreen,
                ),
                "shot-window" => crate::screenshot::tray_capture(
                    app,
                    crate::screenshot::Mode::ActiveWindow,
                ),
                "downloads" => crate::open_download_window(app),
                "transfer" => crate::open_transfer_window(app),
                "settings" => crate::open_settings(app),
                "restart" => restart_app(app),
                "quit" => {
                    // 顺序不能反：先置位、再 exit，否则 prevent_exit 会把这次退出也挡掉。
                    QUITTING.store(true, Ordering::SeqCst);
                    app.exit(0);
                }
                _ => {}
            }
        });

    // 必须带图标：没有图标时 Windows 上托盘位置是空白的，用户会以为程序没起来。
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }

    builder.build(app)?;
    Ok(())
}

/// 重启：起一个延迟 1 秒的 cmd 再退出当前进程。
///
/// 不能直接 `spawn` 自己：单实例插件会在旧进程还没退出时把新进程挡回去。
fn restart_app(app: &AppHandle) {
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    if exe.is_empty() {
        crate::dlog::warn("tray: 取不到当前 exe 路径，取消重启");
        return;
    }
    let cmdline = format!(
        "ping -n 2 127.0.0.1 >NUL & start \"\" \"{}\"",
        exe.replace('"', "")
    );
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        match std::process::Command::new("cmd")
            .args(["/C", cmdline.as_str()])
            .creation_flags(0x0800_0000)
            .spawn()
        {
            Ok(_) => {}
            Err(e) => {
                crate::dlog::warn(&format!("tray: 启动重启命令失败：{e}"));
                return;
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = cmdline;
        return;
    }
    QUITTING.store(true, Ordering::SeqCst);
    app.exit(0);
}

/// 翻译工具启停后同步两个入口的可用状态。
pub fn set_translate_enabled(app: &AppHandle, on: bool) {
    for slot in [&TRANSLATE_ITEM, &INPUT_ITEM] {
        let guard = slot.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(item) = guard.as_ref() {
            if let Err(e) = item.set_enabled(on) {
                crate::dlog::warn(&format!("tray: 更新翻译项可用状态失败：{e}"));
            }
        }
    }
    let _ = app;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 打包前就确认 10 个菜单图标能被 Tauri 解码；坏一张会让整个托盘菜单变空白。
    #[test]
    fn menu_icons_decode() {
        for (name, bytes) in [
            ("translate", ICON_TRANSLATE),
            ("input", ICON_INPUT),
            ("shot-full", ICON_SHOT_FULL),
            ("shot-region", ICON_SHOT_REGION),
            ("shot-window", ICON_SHOT_WINDOW),
            ("download", ICON_DOWNLOAD),
            ("transfer", ICON_TRANSFER),
            ("settings", ICON_SETTINGS),
            ("restart", ICON_RESTART),
            ("quit", ICON_QUIT),
        ] {
            assert!(menu_icon(bytes).is_some(), "菜单图标 {name} 解码失败");
        }
    }
}
