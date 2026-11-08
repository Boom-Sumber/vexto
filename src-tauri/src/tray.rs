//! 托盘图标与右键菜单。
//!
//! S0 只有一项「退出 LightBox」：菜单项一多就必须接具体功能，那是 S1/S2 的事。

use crate::QUITTING;
use std::sync::atomic::Ordering;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::AppHandle;

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let quit_item = MenuItem::with_id(app, "quit", "退出 LightBox", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&quit_item])?;

    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip("LightBox")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, ev| {
            if ev.id().as_ref() == "quit" {
                // 顺序不能反：先置位、再 exit，否则 prevent_exit 会把这次退出也挡掉。
                QUITTING.store(true, Ordering::SeqCst);
                app.exit(0);
            }
        });

    // 必须带图标：没有图标时 Windows 上托盘位置是空白的，用户会以为程序没起来。
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }

    builder.build(app)?;
    Ok(())
}
