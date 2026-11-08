//! 托盘图标与右键菜单。
//!
//! 三项：翻译选中文字（带当前热键提示）/ 设置… / 退出 LightBox。
//! 菜单里的「翻译选中文字」不只是给鼠标用的——热键注册失败时它是唯一的手动入口。

use crate::QUITTING;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::AppHandle;

/// 留存「翻译选中文字」这一项，热键改动后要跟着改它的文案。
/// 泛型参数是 Tauri 的 Runtime（桌面端固定 `Wry`），静态量里必须写全。
static TRANSLATE_ITEM: Mutex<Option<MenuItem<tauri::Wry>>> = Mutex::new(None);

pub fn build(app: &AppHandle, hotkey: &str) -> tauri::Result<()> {
    let translate_item = MenuItem::with_id(
        app,
        "translate",
        translate_label(hotkey).as_str(),
        true,
        None::<&str>,
    )?;
    let settings_item = MenuItem::with_id(app, "settings", "设置…", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "退出 LightBox", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&translate_item, &settings_item, &quit_item])?;

    *TRANSLATE_ITEM.lock().unwrap_or_else(|e| e.into_inner()) = Some(translate_item);

    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip("LightBox — 划词翻译")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, ev| match ev.id().as_ref() {
            "translate" => crate::translate_selection(app.clone()),
            "settings" => crate::open_settings(app),
            "quit" => {
                // 顺序不能反：先置位、再 exit，否则 prevent_exit 会把这次退出也挡掉。
                QUITTING.store(true, Ordering::SeqCst);
                app.exit(0);
            }
            _ => {}
        });

    // 必须带图标：没有图标时 Windows 上托盘位置是空白的，用户会以为程序没起来。
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }

    builder.build(app)?;
    Ok(())
}

fn translate_label(hotkey: &str) -> String {
    format!("翻译选中文字（{hotkey}）")
}

/// 热键改动后更新菜单文案。改不了只记日志——菜单写着旧热键不影响功能。
pub fn update_hotkey_label(app: &AppHandle, hotkey: &str) {
    let guard = TRANSLATE_ITEM.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(item) = guard.as_ref() {
        if let Err(e) = item.set_text(translate_label(hotkey)) {
            crate::dlog::dlog(&format!("tray: 更新热键文案失败：{e}"));
        }
    }
    let _ = app;
}
