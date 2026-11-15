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

// 20×20 菜单图标（透明底，黑白两套，按系统菜单明暗自动选）。
// 源图在 src-tauri/icons/menu/{light,dark}/；用户提供的 SVG 按菜单文本色重绘。
struct IconSet {
    translate: &'static [u8],
    input: &'static [u8],
    shot_full: &'static [u8],
    shot_region: &'static [u8],
    shot_window: &'static [u8],
    download: &'static [u8],
    transfer: &'static [u8],
    settings: &'static [u8],
    restart: &'static [u8],
    quit: &'static [u8],
}

const LIGHT: IconSet = IconSet {
    translate: include_bytes!("../icons/menu/light/translate.png"),
    input: include_bytes!("../icons/menu/light/input.png"),
    shot_full: include_bytes!("../icons/menu/light/shot-full.png"),
    shot_region: include_bytes!("../icons/menu/light/shot-region.png"),
    shot_window: include_bytes!("../icons/menu/light/shot-window.png"),
    download: include_bytes!("../icons/menu/light/download.png"),
    transfer: include_bytes!("../icons/menu/light/transfer.png"),
    settings: include_bytes!("../icons/menu/light/settings.png"),
    restart: include_bytes!("../icons/menu/light/restart.png"),
    quit: include_bytes!("../icons/menu/light/quit.png"),
};

const DARK: IconSet = IconSet {
    translate: include_bytes!("../icons/menu/dark/translate.png"),
    input: include_bytes!("../icons/menu/dark/input.png"),
    shot_full: include_bytes!("../icons/menu/dark/shot-full.png"),
    shot_region: include_bytes!("../icons/menu/dark/shot-region.png"),
    shot_window: include_bytes!("../icons/menu/dark/shot-window.png"),
    download: include_bytes!("../icons/menu/dark/download.png"),
    transfer: include_bytes!("../icons/menu/dark/transfer.png"),
    settings: include_bytes!("../icons/menu/dark/settings.png"),
    restart: include_bytes!("../icons/menu/dark/restart.png"),
    quit: include_bytes!("../icons/menu/dark/quit.png"),
};

/// 系统菜单当前是浅色还是深色。菜单图标必须跟菜单文字同色，所以不能固定用
/// 品牌色；Windows 把这项存在 `AppsUseLightTheme` 里。
fn system_menu_is_light() -> bool {
    #[cfg(windows)]
    {
        use windows::core::w;
        use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
        let mut value: u32 = 0;
        let mut size = std::mem::size_of::<u32>() as u32;
        let rc = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
                w!("AppsUseLightTheme"),
                RRF_RT_REG_DWORD,
                None,
                Some(&mut value as *mut u32 as *mut core::ffi::c_void),
                Some(&mut size),
            )
        };
        rc.0 == 0 && value != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn icons() -> &'static IconSet {
    if system_menu_is_light() {
        &LIGHT
    } else {
        &DARK
    }
}

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

    let ic = icons();
    let translate = icon_item(app, "translate", "划词翻译", translate_on, ic.translate)?;
    let input = icon_item(app, "input_translate", "输入翻译", translate_on, ic.input)?;
    *TRANSLATE_ITEM.lock().unwrap_or_else(|e| e.into_inner()) = Some(translate.clone());
    *INPUT_ITEM.lock().unwrap_or_else(|e| e.into_inner()) = Some(input.clone());

    let shot_full = icon_item(app, "shot-full", "全屏截图", true, ic.shot_full)?;
    let shot_region = icon_item(app, "shot-region", "区域截图", true, ic.shot_region)?;
    let shot_window = icon_item(app, "shot-window", "活动窗口截图", true, ic.shot_window)?;
    let shot_menu = Submenu::with_items(
        app,
        "截图",
        true,
        &[&shot_full, &shot_region, &shot_window],
    )?;

    let downloads = icon_item(app, "downloads", "下载", true, ic.download)?;
    let transfer = icon_item(app, "transfer", "互传", true, ic.transfer)?;
    let task_menu = Submenu::with_items(app, "任务窗口", true, &[&downloads, &transfer])?;

    let settings = icon_item(app, "settings", "设置", true, ic.settings)?;
    let restart = icon_item(app, "restart", "重启应用", true, ic.restart)?;
    let quit = icon_item(app, "quit", "退出程序", true, ic.quit)?;

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

    /// 打包前就确认黑白两套菜单图标都能被 Tauri 解码；坏一张会让整个托盘菜单变空白。
    #[test]
    fn menu_icons_decode() {
        for set in [&LIGHT, &DARK] {
            for (name, bytes) in [
                ("translate", set.translate),
                ("input", set.input),
                ("shot-full", set.shot_full),
                ("shot-region", set.shot_region),
                ("shot-window", set.shot_window),
                ("download", set.download),
                ("transfer", set.transfer),
                ("settings", set.settings),
                ("restart", set.restart),
                ("quit", set.quit),
            ] {
                let img = menu_icon(bytes).unwrap_or_else(|| panic!("菜单图标 {name} 解码失败"));
                assert!(img.width() >= 16 && img.height() >= 16, "{name} 图标太小");
            }
        }
    }
}
