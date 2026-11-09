//! 托盘图标与右键菜单。
//!
//! 菜单**从注册表生成**（ADR-015）：每个工具一项「<工具名>…」，点了直接打开设置页
//! 并跳到该工具。加一个新工具不需要动这个文件。
//!
//! 特殊项有四个：「翻译选中文字」（带当前热键提示，热键注册失败时是唯一的手动
//! 入口）、「查看下载…」（S3b 主窗口）、「设置…」（打开设置页第一个工具）、
//! 「退出 LightBox」。工具项仍全部由注册表生成，配置入口不变。
//!
//! 菜单 id 约定：工具项是 `tool:<id>`，其余是固定字符串。

use crate::QUITTING;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::AppHandle;

/// 留存「翻译选中文字」这一项，热键改动后要跟着改它的文案。
/// 泛型参数是 Tauri 的 Runtime（桌面端固定 `Wry`），静态量里必须写全。
static TRANSLATE_ITEM: Mutex<Option<MenuItem<tauri::Wry>>> = Mutex::new(None);

/// 工具菜单项的 id 前缀。
pub const TOOL_PREFIX: &str = "tool:";

pub fn build(app: &AppHandle, hotkey: &str) -> tauri::Result<()> {
    let mut items: Vec<Box<dyn tauri::menu::IsMenuItem<tauri::Wry>>> = Vec::new();

    // 1) 翻译选中文字：按「翻译」工具是否启用决定可点不可点。
    let translate_on = crate::tools::registry::by_id("translate")
        .map(|t| t.enabled())
        .unwrap_or(true);
    let translate_item = MenuItem::with_id(
        app,
        "translate",
        translate_label(hotkey).as_str(),
        translate_on,
        None::<&str>,
    )?;
    *TRANSLATE_ITEM.lock().unwrap_or_else(|e| e.into_inner()) = Some(translate_item.clone());
    items.push(Box::new(translate_item));

    // 「查看下载…」是固定项，不是注册表里的工具项：`tool:download` 按 ADR-015
    // 仍然负责打开设置页的下载分区，主窗口单独给一个入口，两不冲突。
    items.push(Box::new(MenuItem::with_id(
        app,
        "downloads",
        "查看下载…",
        true,
        None::<&str>,
    )?));

    items.push(Box::new(tauri::menu::PredefinedMenuItem::separator(app)?));

    // 2) 每个工具一项。工具自己决定要不要出现在托盘（这里全都出现，
    //    因为用户可能想配置一个当前停用的工具）。
    for tool in crate::tools::registry::all() {
        let id = format!("{TOOL_PREFIX}{}", tool.id());
        let label = format!("{}…", tool.name());
        let item = MenuItem::with_id(app, id.as_str(), label.as_str(), true, None::<&str>)?;
        items.push(Box::new(item));
    }

    items.push(Box::new(tauri::menu::PredefinedMenuItem::separator(app)?));

    // 3) 设置…（打开设置页，不指定工具）与退出
    items.push(Box::new(MenuItem::with_id(
        app,
        "settings",
        "设置…",
        true,
        None::<&str>,
    )?));
    items.push(Box::new(MenuItem::with_id(
        app,
        "quit",
        "退出 LightBox",
        true,
        None::<&str>,
    )?));

    let refs: Vec<&dyn tauri::menu::IsMenuItem<tauri::Wry>> =
        items.iter().map(|b| b.as_ref()).collect();
    let menu = Menu::with_items(app, &refs)?;

    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip("LightBox — 划词翻译")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, ev| {
            let id = ev.id().as_ref().to_string();
            if let Some(tool_id) = id.strip_prefix(TOOL_PREFIX) {
                // 点工具项 = 打开设置页并停在那个工具
                crate::open_settings_at(app, tool_id);
                return;
            }
            match id.as_str() {
                "translate" => crate::translate_selection(app.clone()),
                "downloads" => crate::open_download_window(app),
                "settings" => crate::open_settings(app),
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

/// 翻译工具启停后同步托盘项的可用状态。菜单项建好后只能改文本/可用性，
/// 没有「隐藏」这个概念，所以停用时置灰。
pub fn set_translate_enabled(app: &AppHandle, on: bool) {
    let guard = TRANSLATE_ITEM.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(item) = guard.as_ref() {
        if let Err(e) = item.set_enabled(on) {
            crate::dlog::dlog(&format!("tray: 更新翻译项可用状态失败：{e}"));
        }
    }
    let _ = app;
}
