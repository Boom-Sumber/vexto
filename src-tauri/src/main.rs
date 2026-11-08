#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! LightBox 入口（S0 版：只有托盘，没有任何窗口、热键、取词、翻译）。
//!
//! S0 的退出标准是「能编译、能起托盘、内存测得出来」，所以这里刻意只做三件事：
//! 建托盘、把调试日志接上、拦住意外退出。功能都在 S1 之后加。

mod dlog;
mod mem;
// S0 只有接口没有实现，bin 里确实没人调用。S2 接上第一个 Tool 后删掉这行 allow。
#[allow(dead_code)]
mod tools;
mod tray;

use std::sync::atomic::{AtomicBool, Ordering};

/// 托盘「退出」置位。没有它，`ExitRequested` 里的 `prevent_exit()` 会让进程永远关不掉。
pub static QUITTING: AtomicBool = AtomicBool::new(false);

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let handle = app.handle().clone();
            tray::build(&handle)?;
            dlog::dlog("setup: 托盘已创建");
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("构建 Tauri 应用失败")
        .run(|_app, event| {
            if let tauri::RunEvent::ExitRequested { api, .. } = event {
                // 常驻程序要挡住「最后一个窗口关掉就退出」的默认行为。
                if !QUITTING.load(Ordering::SeqCst) {
                    api.prevent_exit();
                }
            }
        });
}
