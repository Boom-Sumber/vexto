#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! LightBox 入口。
//!
//! S1 全链路：全局热键 → 取词 → 百度翻译 → 无焦点弹窗 → 点外部/超时关闭。
//!
//! 线程模型（三条边界，改动前先看 docs/03-ENGINEERING-STANDARDS.md §3.5）：
//! - 热键回调跑在**事件循环线程**上 → 只能 `std::thread::spawn` 出去，绝不能回调里做 UIA / 网络。
//! - 工作线程做完取词和翻译 → `app.run_on_main_thread()` 回主线程再碰窗口。
//! - `setup()` 里不能建窗（实测 os error 5），要等 `RunEvent::Ready`。

mod capture;
mod config;
mod dlog;
mod hotkey;
mod mem;
mod popup;
// S0 只有接口没有实现，bin 里确实没人调用。S2 接上第一个 Tool 后删掉这行 allow。
#[allow(dead_code)]
mod tools;
mod tray;
mod translate;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, Manager};

/// 托盘「退出」置位。没有它，`ExitRequested` 里的 `prevent_exit()` 会让进程永远关不掉。
pub static QUITTING: AtomicBool = AtomicBool::new(false);

pub struct AppState {
    /// 上一次取到的原文。弹窗「重试」用它，否则重试时又得再取一次词
    /// （用户那时可能已经把选中区取消了）。
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
    // 窗口必须在事件循环线程创建（实测：在 setup() 里建窗直接 os error 5）
    let _ = app.run_on_main_thread(move || popup::open_settings(&a));
}

fn show_popup(app: &AppHandle, payload: serde_json::Value) {
    let a = app.clone();
    let _ = app.run_on_main_thread(move || popup::show(&a, payload));
}

// ---------- 核心流程 ----------

/// 取词 → 翻译 → 弹窗，一次完整的翻译流程。
///
/// 调用方可能是热键回调（事件循环线程）或托盘菜单，所以这里第一步就先
/// `std::thread::spawn`，把后面所有阻塞操作（UIA、SendInput、HTTP）都挪出事件循环。
fn run_translate(app: AppHandle, reuse_last_src: bool) {
    std::thread::spawn(move || {
        dlog::dlog(&format!(
            "translate_selection: 开始（reuse={reuse_last_src}）"
        ));

        // 1) 取词
        let src = if reuse_last_src {
            app.state::<AppState>()
                .last_src
                .lock()
                .map(|g| g.clone())
                .ok()
                .flatten()
                .unwrap_or_default()
        } else {
            String::new()
        };

        let src = if src.trim().is_empty() {
            match capture::capture_selection() {
                Ok(t) => {
                    if let Ok(mut g) = app.state::<AppState>().last_src.lock() {
                        *g = Some(t.clone());
                    }
                    t
                }
                Err(e) => {
                    dlog::dlog(&format!("translate_selection: 取词失败：{e}"));
                    show_popup(
                        &app,
                        json!({ "state": "error", "src": "", "message": e }),
                    );
                    return;
                }
            }
        } else {
            src
        };

        // 2) 先弹「翻译中」，用户按下热键后 1 秒内必须有反应，
        //    等百度返回（可能几秒）再弹窗会让人以为热键没生效。
        show_popup(
            &app,
            json!({ "state": "loading", "src": src }),
        );

        // 3) 翻译
        let cfg = config::load();
        if cfg.appid.trim().is_empty() || cfg.secret.trim().is_empty() {
            dlog::dlog("translate_selection: 还没填百度翻译 APPID / 密钥，打开设置页");
            let a = app.clone();
            let _ = app.run_on_main_thread(move || {
                popup::close(&a);
                popup::open_settings(&a);
            });
            return;
        }

        match translate::translate(&cfg, &src) {
            Ok(r) => {
                dlog::dlog(&format!(
                    "translate_selection: ok {} -> {} chars",
                    r.from,
                    r.dst.chars().count()
                ));
                show_popup(
                    &app,
                    json!({
                        "state": "done",
                        "src": src,
                        "dst": r.dst,
                        "from": r.from,
                        "to": r.to,
                    }),
                );
            }
            Err(e) => {
                dlog::dlog(&format!("translate_selection: 翻译失败：{e}"));
                show_popup(
                    &app,
                    json!({ "state": "error", "src": src, "message": e }),
                );
            }
        }
    });
}

/// 热键 / 托盘「翻译选中文字」的入口。
pub fn translate_selection(app: AppHandle) {
    run_translate(app, false);
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

/// 前端通知「我准备好了，可以发内容了」。
///
/// 必须等前端 await 完 listen 后再调，否则补发的 popup-render 会丢
/// （现象：弹窗永远停在"翻译中"，见 docs/stages/S1 §S1.6 坑 1）。
#[tauri::command]
fn popup_ready(app: AppHandle) {
    popup::ready(&app);
}

#[tauri::command]
fn resize_popup(win: tauri::WebviewWindow, height: f64) {
    popup::resize(&win, height);
}

#[tauri::command]
fn close_popup(app: AppHandle) {
    popup::close(&app);
}

/// 「复制译文」的兜底路线：不可聚焦的窗口里 navigator.clipboard 可能被拒。
#[tauri::command]
fn copy_text(text: String) {
    capture::write_clipboard_text(&text);
}

/// 「重试」：复用上一次的原文重新翻译，不再取词
/// （用户点按钮时选中区多半已经没了，重新取词必然失败）。
#[tauri::command]
fn retry_translate(app: AppHandle) {
    run_translate(app, true);
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
            open_settings_cmd,
            popup_ready,
            resize_popup,
            close_popup,
            copy_text,
            retry_translate
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
