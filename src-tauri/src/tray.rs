//! 托盘图标与右键菜单。
//!
//! 菜单是固定的使用入口（会话 73 用户定稿），不再按工具注册表逐项列出：
//! 输入翻译 / 截图（全屏、区域、活动窗口）/ 工具箱（下载、互传）/
//! 设置 / 重启应用 / 退出程序。
//!
//! 工具在设置 → 常规 → 工具管理里停用后，对应菜单项置灰不可点（ADR-090）。
//!
//! 菜单项 = 固定文案 + **透明占位图标**（会话 81 用户要求去掉可见图标）。
//! 保留占位是为了让文字统一缩进——Windows 原生菜单只有在「有图标列」时才会
//! 给文字留出左侧间距；完全去掉图标会让文字贴到左边缘。占位图透明，视觉上
//! 看不到任何图标。
//!
//! 菜单 id 约定：叶子项用固定字符串，子菜单只是容器。

use crate::QUITTING;
use rust_i18n::t;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use tauri::menu::{IconMenuItem, Menu, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::AppHandle;

/// 受工具启停影响的叶子项：菜单 id → 菜单项。语言切换会重建菜单，
/// 每次 `build_menu` 整体替换这一份。
static TOOL_ITEMS: Mutex<Vec<(&'static str, IconMenuItem<tauri::Wry>)>> = Mutex::new(Vec::new());
/// 「检查更新」项：发现新版本时改文字，给用户一个不打开设置页也能看到的提示。
static UPDATE_ITEM: Mutex<Option<IconMenuItem<tauri::Wry>>> = Mutex::new(None);
/// 上一次更新提示的状态：换语言重建菜单后要按这个状态重新套一遍。
static UPDATE_HINT: Mutex<Option<(Option<String>, bool)>> = Mutex::new(None);

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

fn build_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let cfg = crate::config::load();

    let input = item(
        app,
        "input_translate",
        &t!("tray.inputTranslate"),
        cfg.enable_translate,
    )?;
    let shot_full = item(
        app,
        "shot-full",
        &t!("tray.fullScreen"),
        cfg.enable_screenshot,
    )?;
    let shot_region = item(app, "shot-region", &t!("tray.region"), cfg.enable_screenshot)?;
    let shot_window = item(
        app,
        "shot-window",
        &t!("tray.activeWindow"),
        cfg.enable_screenshot,
    )?;
    let shot_menu = Submenu::with_items(
        app,
        &t!("tray.screenshot"),
        true,
        &[&shot_full, &shot_region, &shot_window],
    )?;

    let downloads = item(app, "downloads", &t!("tray.downloads"), cfg.enable_download)?;
    let transfer = item(app, "transfer", &t!("tray.transfer"), cfg.enable_transfer)?;
    let task_menu =
        Submenu::with_items(app, &t!("tray.taskWindows"), true, &[&downloads, &transfer])?;

    let settings = item(app, "settings", &t!("tray.settings"), true)?;
    let check_update = item(app, "check-update", &t!("tray.checkUpdate"), true)?;
    *UPDATE_ITEM.lock().unwrap_or_else(|e| e.into_inner()) = Some(check_update.clone());
    let restart = item(app, "restart", &t!("tray.restart"), true)?;
    let quit = item(app, "quit", &t!("tray.quit"), true)?;

    *TOOL_ITEMS.lock().unwrap_or_else(|e| e.into_inner()) = vec![
        ("input_translate", input.clone()),
        ("shot-full", shot_full.clone()),
        ("shot-region", shot_region.clone()),
        ("shot-window", shot_window.clone()),
        ("downloads", downloads.clone()),
        ("transfer", transfer.clone()),
    ];

    let separator = PredefinedMenuItem::separator(app)?;
    let items: Vec<&dyn tauri::menu::IsMenuItem<tauri::Wry>> = vec![
        &input,
        &shot_menu,
        &task_menu,
        &separator,
        &settings,
        &check_update,
        &restart,
        &quit,
    ];
    Menu::with_items(app, &items)
}

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let menu = build_menu(app)?;
    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip(t!("tray.tooltip"))
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, ev| {
            let id = ev.id().as_ref().to_string();
            match id.as_str() {
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
                "check-update" => {
                    // 打开关于页再 spawn 检查：回调里不做网络 IO。
                    crate::open_settings_at(app, "about");
                    crate::update::spawn_check(app.clone());
                }
                // 更新包已下载：重启 / 退出都先启动安装器（装完会自动重启）。
                "restart" if !crate::update::install_if_ready(app) => restart_app(app),
                "quit" if !crate::update::install_if_ready(app) => {
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
    apply_update_hint_now(app);
    Ok(())
}

/// 语言切换后重建托盘菜单与 tooltip。
///
/// 菜单项 id 不变，事件回调挂在 tray icon 上，所以只换 menu 不会丢点击处理。
pub fn apply_locale(app: &AppHandle) {
    let handle = app.clone();
    if let Err(e) = app.run_on_main_thread(move || {
        let menu = match build_menu(&handle) {
            Ok(menu) => menu,
            Err(e) => {
                crate::dlog::warn(&format!("tray: 重建菜单失败：{e}"));
                return;
            }
        };
        let Some(tray) = handle.tray_by_id("main") else {
            crate::dlog::warn("tray: 找不到主托盘，无法切换语言");
            return;
        };
        if let Err(e) = tray.set_menu(Some(menu)) {
            crate::dlog::warn(&format!("tray: 换菜单失败：{e}"));
        }
        if let Err(e) = tray.set_tooltip(Some(t!("tray.tooltip"))) {
            crate::dlog::warn(&format!("tray: 换 tooltip 失败：{e}"));
        }
        apply_update_hint_now(&handle);
    }) {
        crate::dlog::warn(&format!("tray: 切主线程换语言失败：{e}"));
    }
}

/// 重启：起一个延迟 1 秒的 cmd 再退出当前进程。
///
/// 不能直接 `spawn` 自己：单实例插件会在旧进程还没退出时把新进程挡回去。
pub(crate) fn restart_app(app: &AppHandle) {
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
        // 必须用 raw_arg 整段传入：`args` 会按 C 运行时规则给引号加转义，
        // cmd.exe 不认那种转义，带空格的路径会被拆坏（实测「拒绝访问」）。
        match std::process::Command::new("cmd")
            .raw_arg(format!("/C {cmdline}"))
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

/// 菜单项是否可点：工具停用后置灰。
fn tool_item_enabled(id: &str, cfg: &crate::config::Config) -> bool {
    match id {
        "input_translate" => cfg.enable_translate,
        "shot-full" | "shot-region" | "shot-window" => cfg.enable_screenshot,
        "downloads" => cfg.enable_download,
        "transfer" => cfg.enable_transfer,
        _ => true,
    }
}

/// 工具启停后同步菜单项的可用状态。
pub fn refresh_tool_states() {
    let cfg = crate::config::load();
    let guard = TOOL_ITEMS.lock().unwrap_or_else(|e| e.into_inner());
    for (id, item) in guard.iter() {
        if let Err(e) = item.set_enabled(tool_item_enabled(id, &cfg)) {
            crate::dlog::warn(&format!("tray: 更新「{id}」可用状态失败：{e}"));
        }
    }
}

/// 「检查更新」项当前该显示的文字。
fn update_hint_text(version: Option<&str>, required: bool) -> String {
    match (version, required) {
        (Some(v), true) => t!("tray.updateNowVersion", version = v).into_owned(),
        (Some(v), false) => t!("tray.updateAvailableVersion", version = v).into_owned(),
        (None, _) => t!("tray.checkUpdate").into_owned(),
    }
}

/// 按最近一次状态刷新「检查更新」项；必须在主线程调用。
fn apply_update_hint_now(_app: &AppHandle) {
    let state = UPDATE_HINT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let Some((version, required)) = state else {
        return;
    };
    let text = update_hint_text(version.as_deref(), required);
    let guard = UPDATE_ITEM.lock().unwrap_or_else(|e| e.into_inner());
    let Some(item) = guard.as_ref() else {
        return;
    };
    if let Err(e) = item.set_text(text) {
        crate::dlog::warn(&format!("tray: 更新「检查更新」文案失败：{e}"));
    }
}

/// 发现新版本时把托盘菜单项改成「检查更新（x.y.z 可用）」；没有则还原。
///
/// 工作线程会调用它，菜单操作统一丢回主线程。
pub fn set_update_hint(app: &AppHandle, version: Option<&str>, required: bool) {
    *UPDATE_HINT.lock().unwrap_or_else(|e| e.into_inner()) =
        Some((version.map(str::to_string), required));
    let handle = app.clone();
    if let Err(e) = app.run_on_main_thread(move || apply_update_hint_now(&handle)) {
        crate::dlog::warn(&format!("tray: 切主线程更新菜单失败：{e}"));
    }
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

    /// 工具停用后对应入口必须置灰，其余入口不受影响。
    #[test]
    fn tool_items_follow_enable_flags() {
        let mut cfg = crate::config::Config::default();
        cfg.enable_translate = true;
        cfg.enable_screenshot = true;
        cfg.enable_download = true;
        cfg.enable_transfer = true;
        assert!(tool_item_enabled("input_translate", &cfg));

        cfg.enable_translate = false;
        assert!(!tool_item_enabled("input_translate", &cfg));
        assert!(tool_item_enabled("settings", &cfg));

        cfg.enable_screenshot = false;
        for id in ["shot-full", "shot-region", "shot-window"] {
            assert!(!tool_item_enabled(id, &cfg), "{id} 应随截图停用");
        }

        cfg.enable_download = false;
        assert!(!tool_item_enabled("downloads", &cfg));
        assert!(tool_item_enabled("transfer", &cfg));

        cfg.enable_transfer = false;
        assert!(!tool_item_enabled("transfer", &cfg));
    }
}
