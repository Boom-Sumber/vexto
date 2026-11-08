#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! LightBox 入口。
//!
//! S1a 范围：全局热键能注册能触发（触发只写日志）、托盘菜单接上、
//! 设置页能填 APPID / 密钥 / 热键并存盘。
//!
//! **明确没有的**（属 S1b）：取词、百度翻译、弹窗渲染。
//! 所以 `translate_selection()` 现在只打一行日志，不做别的事——
//! 这样热键链路可以独立验证，取词坏了不会被误判成热键坏了。

mod config;
mod dlog;
mod hotkey;
mod mem;
// S0 只有接口没有实现，bin 里确实没人调用。S2 接上第一个 Tool 后删掉这行 allow。
#[allow(dead_code)]
mod tools;
mod popup;
mod tray;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use serde::Serialize;
use tauri::AppHandle;

/// 托盘「退出」置位。没有它，`ExitRequested` 里的 `prevent_exit()` 会让进程永远关不掉。
pub static QUITTING: AtomicBool = AtomicBool::new(false);

pub struct AppState {
    /// 上一次触发的原文，供 S1b 的弹窗「重试」用。S1a 只写入不读取。
    pub last_src: Mutex<Option<String>>,
}

#[derive(Serialize)]
struct ConfigOut {
    appid: String,
    secret: String,
    hotkey: String,
    config_path: String,
    /// 热键注册失败的原因（空串表示正常）
    hotkey_error: String,
}

#[derive(Serialize)]
struct SimpleOut {
    ok: bool,
    message: String,
}

// ---------- 窗口操作统一走主线程 ----------

pub fn open_settings(app: &AppHandle) {
    let a = app.clone();
    // 窗口必须在事件循环线程创建（S0 实测：在 setup() 里建窗直接 os error 5）
    let _ = app.run_on_main_thread(move || popup::open_settings(&a));
}

// ---------- 核心流程 ----------

/// 热键 / 托盘菜单触发的入口。
///
/// S1a 只记日志。热键回调跑在事件循环线程上，**不能**在这里做 UIA / 网络 / 文件 IO，
/// 所以即使只是写日志也挪到新线程——S1b 把取词接进来时这个结构不用改。
pub fn translate_selection(app: AppHandle) {
    std::thread::spawn(move || {
        dlog::dlog("translate_selection: 热键已触发（S1a：取词与翻译属 S1b）");
        // 热键能被按下但配置还没填，是首次使用最常见的情况，直接给出可执行提示。
        let cfg = config::load();
        if cfg.appid.trim().is_empty() || cfg.secret.trim().is_empty() {
            dlog::dlog("translate_selection: 还没填百度翻译 APPID / 密钥，打开设置页");
            open_settings(&app);
        }
    });
}

// ---------- 命令 ----------

#[tauri::command]
fn get_config() -> ConfigOut {
    let c = config::load();
    ConfigOut {
        appid: c.appid,
        secret: c.secret,
        hotkey: c.hotkey,
        config_path: config::config_path().display().to_string(),
        hotkey_error: hotkey::last_error(),
    }
}

#[tauri::command]
fn save_config(config: config::Config, app: AppHandle) -> SimpleOut {
    // 后端必须自己校验一次热键：不许只信前端（06-TESTING §6.4.1 第 11j 条）。
    let normalized = match hotkey::validate_hotkey(&config.hotkey) {
        Ok(h) => h,
        Err(e) => return SimpleOut { ok: false, message: e },
    };

    let mut cfg = config;
    cfg.hotkey = normalized.clone();

    if let Err(e) = config::save(&cfg) {
        return SimpleOut { ok: false, message: e };
    }

    let mut msg = String::from("已保存");
    match hotkey::register_hotkey(&app, &normalized, |a| translate_selection(a.clone())) {
        Ok(()) => msg.push_str(&format!("，热键 {normalized} 已生效")),
        Err(e) => msg.push_str(&format!("；但{e}")),
    }
    // 托盘菜单上的热键提示要跟着变，否则改完热键菜单还写着旧的
    tray::update_hotkey_label(&app, &normalized);

    SimpleOut { ok: true, message: msg }
}

#[tauri::command]
fn open_settings_cmd(app: AppHandle) {
    open_settings(&app);
}

// ---------- 入口 ----------

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(AppState {
            last_src: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![
            get_config,
            save_config,
            open_settings_cmd
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            let cfg = config::load();

            tray::build(&handle, &cfg.hotkey)?;

            // 热键注册失败不阻断启动：托盘菜单仍然可用，设置页会显示红字原因。
            match hotkey::register_hotkey(&handle, &cfg.hotkey, |a| {
                translate_selection(a.clone())
            }) {
                Ok(()) => dlog::dlog(&format!("setup: 热键 {} 注册成功", cfg.hotkey)),
                Err(e) => dlog::dlog(&format!("setup: {e}")),
            }

            dlog::dlog("setup: 托盘已创建");
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("构建 Tauri 应用失败")
        .run(|app, event| match event {
            // 窗口必须等事件循环跑起来才能创建：在 setup() 里建窗会 os error 5。
            tauri::RunEvent::Ready => {
                let cfg = config::load();
                // 第一次运行没配密钥：直接把设置页打开，否则用户不知道该干什么。
                if cfg.appid.trim().is_empty() || cfg.secret.trim().is_empty() {
                    open_settings(app);
                }
            }
            // 常驻程序要挡住「最后一个窗口关掉就退出」的默认行为。
            tauri::RunEvent::ExitRequested { api, .. }
                if !QUITTING.load(Ordering::SeqCst) =>
            {
                api.prevent_exit();
            }
            _ => {}
        });
}
