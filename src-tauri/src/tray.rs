//! 托盘图标与右键菜单。
//!
//! 菜单是固定的使用入口（会话 73 用户定稿），不再按工具注册表逐项列出：
//! 划词翻译 / 输入翻译 / 截图（全屏、区域、活动窗口）/ 任务窗口（下载、互传）/
//! 设置 / 重启应用 / 退出程序。
//!
//! 菜单项 = 固定文案 + **透明占位图标**（会话 81 用户要求去掉可见图标）。
//! 保留占位是为了让文字统一缩进——Windows 原生菜单只有在「有图标列」时才会
//! 给文字留出左侧间距；完全去掉图标会让文字贴到左边缘。占位图透明，视觉上
//! 看不到任何图标。
//!
//! 菜单 id 约定：叶子项用固定字符串，子菜单只是容器。

use crate::QUITTING;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use tauri::menu::{IconMenuItem, Menu, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::AppHandle;

/// 翻译 / 输入翻译两项在工具停用时置灰。
static TRANSLATE_ITEM: Mutex<Option<IconMenuItem<tauri::Wry>>> = Mutex::new(None);
static INPUT_ITEM: Mutex<Option<IconMenuItem<tauri::Wry>>> = Mutex::new(None);

/// 20×20 全透明占位图：只为撑出菜单的文字缩进。
fn spacer() -> Option<tauri::image::Image<'static>> {
    Some(tauri::image::Image::new_owned(vec![0; 20 * 20 * 4], 20, 20))
}

fn item(
    app: &AppHandle,
    id: &str,
    text: &str,
    enabled: bool,
) -> tauri::Result<IconMenuItem<tauri::Wry>> {
    IconMenuItem::with_id(app, id, text, enabled, spacer(), None::<&str>)
}

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let translate_on = crate::tools::registry::by_id("translate")
        .map(|t| t.enabled())
        .unwrap_or(true);

    let translate = item(app, "translate", "划词翻译", translate_on)?;
    let input = item(app, "input_translate", "输入翻译", translate_on)?;
    *TRANSLATE_ITEM.lock().unwrap_or_else(|e| e.into_inner()) = Some(translate.clone());
    *INPUT_ITEM.lock().unwrap_or_else(|e| e.into_inner()) = Some(input.clone());

    let shot_full = item(app, "shot-full", "全屏截图", true)?;
    let shot_region = item(app, "shot-region", "区域截图", true)?;
    let shot_window = item(app, "shot-window", "活动窗口截图", true)?;
    let shot_menu = Submenu::with_items(
        app,
        "截图",
        true,
        &[&shot_full, &shot_region, &shot_window],
    )?;

    let downloads = item(app, "downloads", "下载", true)?;
    let transfer = item(app, "transfer", "互传", true)?;
    let task_menu = Submenu::with_items(app, "任务窗口", true, &[&downloads, &transfer])?;

    let settings = item(app, "settings", "设置", true)?;
    let restart = item(app, "restart", "重启应用", true)?;
    let quit = item(app, "quit", "退出程序", true)?;

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
        })
        // 左键功能由设置决定（会话 81，默认打开设置页）。右键菜单不受影响：
        // `show_menu_on_left_click(false)` 已经让左键不弹菜单。
        .on_tray_icon_event(|tray, event| {
            let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            else {
                return;
            };
            let app = tray.app_handle().clone();
            match crate::config::load().tray_left_click.trim() {
                "tasks" => crate::open_download_window(&app),
                "input" => crate::open_input_translate(app.clone()),
                "screenshot" => {
                    if crate::config::load().enable_screenshot {
                        crate::popup::open_shot_overlay(&app);
                    } else {
                        crate::open_settings_at(&app, "screenshot");
                    }
                }
                // settings / 空值 / 手改坏的值都回落到设置页
                _ => crate::open_settings(&app),
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

    /// 占位图必须是 20×20 全透明：菜单文字靠它缩进，不能画出一个黑块。
    #[test]
    fn spacer_is_fully_transparent() {
        let img = spacer().expect("占位图必须能创建");
        assert_eq!((img.width(), img.height()), (20, 20));
        assert!(img.rgba().iter().all(|b| *b == 0), "占位图必须全透明");
    }
}
