//! 弹窗与设置窗口的生命周期。
//!
//! **窗口一律运行时创建**：`tauri.conf.json` 里 `app.windows` 是 `[]`，启动时不建任何
//! WebView —— 这是"轻"的核心（启动即建窗会立刻拉起 6 个 msedgewebview2.exe，约 269 MB）。
//!
//! 两个窗口的关闭语义都是 **destroy 而不是 hide**（ADR-007）：`hide()` 只是把窗口藏起来，
//! WebView2 的 6 个子进程会一直驻留，实测约 300 MB。

use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::Duration;

use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetWindowRect, IsWindow};

pub const POPUP_LABEL: &str = "popup";
pub const SETTINGS_LABEL: &str = "settings";

const POPUP_W: f64 = 420.0;
const POPUP_H_MIN: f64 = 110.0;
const POPUP_H_MAX: f64 = 640.0;

/// 点击外部关闭的轮询间隔。30ms 人感知不到，又不会像 10ms 那样白烧 CPU。
const OUTSIDE_POLL_MS: u64 = 30;

/// 替换掉 wry 的默认参数，所以必须**原样**带上它默认的那串
/// （msWebOOUI 去掉迷你菜单、msPdfOOUI、msSmartScreenProtection）。
/// 后面几个是为"小卡片"专门削的：静态文字卡片用不上 GPU 合成，
/// 关掉 gpu-process 与后台节流相关的进程可以少几十 MB。
const WEBVIEW_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection \
--disable-gpu --disable-software-rasterizer --renderer-process-limit=1 \
--disable-background-timer-throttling --disable-backgrounding-occluded-windows \
--disable-renderer-backgrounding --disable-ipc-flooding-protection";

/// 最近一次要渲染的内容。前端加载完成前发的 `popup-render` 会丢，留一份给它补发。
static PENDING: std::sync::Mutex<Option<Value>> = std::sync::Mutex::new(None);

/// 当前正在被"点击外部关闭"监视的弹窗句柄。0 = 没有弹窗。
///
/// 用 `AtomicIsize` 而不是 `bool`：`show()` 在一次弹窗生命周期里会被调用多次
/// （先 loading、再 done），每次都得判断"这个窗口的监视器起了没有"。
static WATCHED_HWND: AtomicIsize = AtomicIsize::new(0);

fn cursor() -> (i32, i32) {
    let mut p = POINT { x: 0, y: 0 };
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    (p.x, p.y)
}

/// 把弹窗摆到光标右下方，并保证整块不越出主屏。
fn clamp_pos(app: &AppHandle, x: i32, y: i32) -> (i32, i32) {
    let w = POPUP_W as i32;
    let h = POPUP_H_MIN as i32;
    if let Ok(Some(m)) = app.primary_monitor() {
        let size = m.size();
        let pos = m.position();
        let mut nx = x;
        let mut ny = y;
        if nx + w > pos.x + size.width as i32 {
            nx = pos.x + size.width as i32 - w - 8;
        }
        if ny + h > pos.y + size.height as i32 {
            ny = y - h - 20;
        }
        if nx < pos.x + 8 {
            nx = pos.x + 8;
        }
        if ny < pos.y + 8 {
            ny = pos.y + 8;
        }
        return (nx, ny);
    }
    (x, y)
}

fn create_popup(app: &AppHandle) -> tauri::Result<tauri::WebviewWindow> {
    let (cx, cy) = cursor();
    let (x, y) = clamp_pos(app, cx + 14, cy + 18);
    WebviewWindowBuilder::new(app, POPUP_LABEL, WebviewUrl::App("popup.html".into()))
        .title("LightBox")
        .inner_size(POPUP_W, POPUP_H_MIN)
        .position(x as f64, y as f64)
        .decorations(false)
        .transparent(true)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .skip_taskbar(true)
        .shadow(false)
        .always_on_top(true)
        // focusable(false) 在 Windows 上等价于 WS_EX_NOACTIVATE：
        // 窗口弹出时不抢走当前程序的输入焦点，用户选中的文字不会被清掉。
        // 代价是它永远收不到「失焦」事件 —— 所以"点击外部关闭"只能自己轮询鼠标。
        .focusable(false)
        .focused(false)
        .visible(false)
        .additional_browser_args(WEBVIEW_ARGS)
        .build()
}

// ---------- 点击外部关闭 ----------

fn is_window(hwnd: isize) -> bool {
    // windows 0.62 里 IsWindow 收的是 Option<HWND>（None 表示空句柄）
    unsafe { IsWindow(Some(HWND(hwnd as *mut core::ffi::c_void))).as_bool() }
}

/// 鼠标左键**当前**是否按下。
///
/// 只取 `GetAsyncKeyState` 的最高位（当前按下状态），不用最低位（"自上次调用以来按过"）——
/// 那个位是进程内共享的，任何别处调用 `GetAsyncKeyState` 都会把它清掉。
fn left_button_down() -> bool {
    unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) < 0 }
}

fn point_in_window(hwnd: isize, x: i32, y: i32) -> bool {
    let mut r = RECT::default();
    let ok = unsafe { GetWindowRect(HWND(hwnd as *mut core::ffi::c_void), &mut r) }.is_ok();
    if !ok {
        return false;
    }
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

/// 弹窗是 `focusable(false)`（不抢焦点，才保得住用户选中的文字），
/// 因此它**收不到 `Focused(false)` 事件**，没法用失焦来实现"点外面关掉"。
///
/// 这里改用 30ms 轮询：左键从抬起变按下的那一刻，若光标不在弹窗矩形内就关闭。
/// 选轮询而不是 `WH_MOUSE_LL` 全局钩子的理由：钩子会让全系统每次鼠标移动都进我们的
/// 回调，回调稍慢一点 Windows 就静默把钩子摘掉，出问题时极难排查；轮询最多慢 30ms，
/// 而且**点击照常穿透给下面的程序**，用户点哪就激活哪，体验上跟没有这个窗口一样。
fn watch_outside_click(app: AppHandle, hwnd: isize) {
    // 初始状态就按下的左键要吞掉：热键触发时用户的手指可能正按着不动，
    // 那不叫"点击外部"。
    let mut prev_down = left_button_down();
    loop {
        std::thread::sleep(Duration::from_millis(OUTSIDE_POLL_MS));

        // 换弹窗了，或者窗口已经销毁，监视器就该退场
        if WATCHED_HWND.load(Ordering::SeqCst) != hwnd || !is_window(hwnd) {
            break;
        }

        let down = left_button_down();
        if down && !prev_down {
            let (cx, cy) = cursor();
            let inside = point_in_window(hwnd, cx, cy);
            crate::dlog::dlog(&format!(
                "popup: 左键按下 at ({cx},{cy}) inside={inside} hwnd={hwnd}"
            ));
            if !inside {
                crate::dlog::dlog("popup: 点在弹窗外部，关闭");
                let a = app.clone();
                let _ = app.run_on_main_thread(move || close(&a));
                break;
            }
        }
        prev_down = down;
    }

    // 只清掉自己的登记，别把后来者的覆盖掉
    let _ = WATCHED_HWND.compare_exchange(hwnd, 0, Ordering::SeqCst, Ordering::SeqCst);
}

/// 保证当前弹窗有一个监视器在跑。重复调用不会重复起线程。
fn ensure_outside_watcher(app: &AppHandle, win: &tauri::WebviewWindow) {
    let Ok(h) = win.hwnd() else {
        crate::dlog::dlog("popup: 拿不到窗口句柄，点击外部关闭不可用");
        return;
    };
    let hwnd = h.0 as isize;
    let prev = WATCHED_HWND.swap(hwnd, Ordering::SeqCst);
    if prev == hwnd {
        return; // 已经在看了
    }
    let a = app.clone();
    std::thread::spawn(move || watch_outside_click(a, hwnd));
}

// ---------- 弹窗 ----------

/// 渲染并显示弹窗。**必须在主线程调用。**
pub fn show(app: &AppHandle, payload: Value) {
    if let Ok(mut g) = PENDING.lock() {
        *g = Some(payload.clone());
    }
    let is_loading = payload.get("state").and_then(|s| s.as_str()) == Some("loading");

    let existing = app.get_webview_window(POPUP_LABEL);
    let just_created = existing.is_none();
    let win = match existing {
        Some(w) => w,
        None => match create_popup(app) {
            Ok(w) => w,
            Err(e) => {
                crate::dlog::dlog(&format!("popup: 创建失败: {e}"));
                return;
            }
        },
    };

    if is_loading {
        // 新一轮翻译：先把高度收回去，再挪到当前光标附近（用户可能已换了位置）
        let (cx, cy) = cursor();
        let (x, y) = clamp_pos(app, cx + 14, cy + 18);
        let _ = win.set_size(tauri::PhysicalSize::new(POPUP_W as u32, POPUP_H_MIN as u32));
        let _ = win.set_position(tauri::PhysicalPosition::new(x, y));
    }

    crate::dlog::dlog("popup::show: emit");
    let _ = win.emit("popup-render", payload);

    if just_created {
        // 窗口刚建出来，前端还在加载。现在 show() 会闪一下空白卡片，
        // 所以等 popup_ready 回调时再显示（那时 PENDING 会被补发）。
        return;
    }
    let _ = win.show();
    ensure_outside_watcher(app, &win);
}

/// 前端加载完成后回调：补发最近一次内容并把窗口显示出来。
pub fn ready(app: &AppHandle) {
    crate::dlog::dlog("popup::ready: frontend ready");
    if let Some(w) = app.get_webview_window(POPUP_LABEL) {
        let p = PENDING.lock().ok().and_then(|g| g.clone());
        if let Some(p) = p {
            let _ = w.emit("popup-render", p);
        }
        let _ = w.show();
        ensure_outside_watcher(app, &w);
    }
}

/// 关闭弹窗。**必须在主线程调用。**
///
/// 用 `destroy` 而不是 `hide`：`hide` 之后 6 个 msedgewebview2 子进程
/// （实测合计约 280 MB）会一直驻留；`destroy` 之后整个 WebView2 进程组退出，
/// 主进程回到纯 Rust 的几 MB。
pub fn close(app: &AppHandle) {
    // 先让监视器退场，避免它对着正在销毁的句柄继续轮询
    WATCHED_HWND.store(0, Ordering::SeqCst);
    if let Some(w) = app.get_webview_window(POPUP_LABEL) {
        crate::dlog::dlog("popup::close: destroy");
        let _ = w.destroy();
        // 等 WebView2 进程组收尾后再把主进程的工作集换出去：
        // 立刻调只会把还没回收的页又换出去一次，等 1.2s 更稳。
        crate::mem::trim_soon(1200);
    }
}

/// 由前端报告内容高度。原生窗口高度必须后端设置（ADR-011），
/// 前端算出来的 CSS 高度后端不知道。
pub fn resize(win: &tauri::WebviewWindow, height: f64) {
    let h = height.clamp(60.0, POPUP_H_MAX);
    let _ = win.set_size(tauri::PhysicalSize::new(POPUP_W as u32, h as u32));
}

// ---------- 设置窗口 ----------

/// 打开设置窗口。已存在则聚焦，不重复创建。**必须在主线程调用。**
pub fn open_settings(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(SETTINGS_LABEL) {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
        return;
    }

    let r = WebviewWindowBuilder::new(app, SETTINGS_LABEL, WebviewUrl::App("settings.html".into()))
        .title("LightBox 设置")
        .inner_size(560.0, 620.0)
        .min_inner_size(460.0, 480.0)
        .resizable(true)
        .center()
        .additional_browser_args(WEBVIEW_ARGS)
        .build();

    match r {
        Ok(win) => {
            // 窗口销毁后收缩工作集。必须是 destroy 语义（关闭即销毁），不能用 hide，
            // 否则 6 个 msedgewebview2.exe 会一直驻留（ADR-007）。
            win.on_window_event(|ev| {
                if matches!(ev, tauri::WindowEvent::Destroyed) {
                    crate::dlog::dlog("popup: 设置窗口已销毁，1.2s 后收缩工作集");
                    crate::mem::trim_soon(1200);
                }
            });
        }
        Err(e) => crate::dlog::dlog(&format!("create settings failed: {e}")),
    }
}
