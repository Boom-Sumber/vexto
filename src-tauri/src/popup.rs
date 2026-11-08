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

/// 卡片（可见部分）的左右内边距，即 CSS `#card` 的 `margin` 左右值。
///
/// 窗口是透明的，可见卡片比窗口小一圈，那一圈是给 `box-shadow` 留的画布。
/// **窗口矩形以外的阴影会被裁掉**，所以这组数字必须 ≥ 阴影扩散范围：
/// `box-shadow: 0 4px 12px` → 左右各 12、上 8（12-4）、下 16（12+4）。
///
/// 必须与 `ui/popup.css` 的 `#card { margin: 8px 12px 16px }` 和
/// `ui/popup.js` 的 `PAD_TOP / PAD_X / PAD_BOTTOM` 三处保持一致。
const CARD_PAD_X: i32 = 12;
const CARD_PAD_TOP: i32 = 8;
const CARD_PAD_BOTTOM: i32 = 16;

/// 卡片默认宽度。窗口宽度 = 这个值 + 左右边距。
const CARD_W: f64 = 420.0;
/// 卡片高度范围（不含阴影边距）。
const CARD_H_MIN: f64 = 110.0;
const CARD_H_MAX: f64 = 640.0;

const POPUP_W: f64 = CARD_W + (CARD_PAD_X * 2) as f64;
const POPUP_H_MIN: f64 = CARD_H_MIN + (CARD_PAD_TOP + CARD_PAD_BOTTOM) as f64;
const POPUP_H_MAX: f64 = CARD_H_MAX + (CARD_PAD_TOP + CARD_PAD_BOTTOM) as f64;

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

/// 弹窗是否被图钉钉住。
///
/// 钉住 = 用户明确说"这个别自己消失"：不自动关闭、不响应点击外部、
/// 也不需要一直置顶（窗口本来就 `always_on_top(true)`）。
/// 放在 Rust 侧而不是只放前端，是因为「点击外部关闭」的轮询线程在 Rust 里，
/// 前端改个变量它看不见。
static PINNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 当前弹窗是不是「输入翻译」模式（可编辑、要能收键盘）。
///
/// 划词模式必须 `focusable(false)`（Windows 上是 `WS_EX_NOACTIVATE`）：
/// 窗口弹出不抢焦点，用户选中的文字才不会被清掉——06-TESTING §6.4.1 第 4 条。
/// 输入模式则相反，必须能收键盘，否则用户没法打字。
/// 同一个窗口按模式切换，所以每次 `show()` 都要重新设一遍，不能只在建窗时设。
static INPUT_MODE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn is_pinned() -> bool {
    PINNED.load(Ordering::SeqCst)
}

/// 设置图钉状态。**必须在主线程调用**（会改窗口标志）。
pub fn set_pinned(app: &AppHandle, pinned: bool) {
    PINNED.store(pinned, Ordering::SeqCst);
    crate::dlog::dlog(&format!("popup: 图钉 = {pinned}"));
    // 钉住时把窗口的 always_on_top 再确认一次：用户可能拖到别的置顶窗口后面，
    // 取消置顶再置顶一次能把它拉回最上层。
    if pinned {
        if let Some(w) = app.get_webview_window(POPUP_LABEL) {
            let _ = w.set_always_on_top(false);
            let _ = w.set_always_on_top(true);
        }
    }
}

/// 切换弹窗的「可聚焦」状态。**必须在主线程调用。**
///
/// 划词模式传 `false`（不抢焦点，保住选中高亮）；输入模式传 `true`（要收键盘）。
/// 从输入模式切回划词模式时必须显式设回 `false`，
/// 否则下一次划词弹窗会带着可聚焦状态出现，把用户的选中清掉。
pub fn set_focusable(app: &AppHandle, on: bool) {
    INPUT_MODE.store(on, Ordering::SeqCst);
    let Some(w) = app.get_webview_window(POPUP_LABEL) else {
        return;
    };
    if let Err(e) = w.set_focusable(on) {
        crate::dlog::dlog(&format!("popup: set_focusable({on}) 失败：{e}"));
        return;
    }
    if on {
        // 可聚焦之后还得主动要一次焦点，否则窗口只是"允许被聚焦"，
        // 键盘输入仍然进不到输入框。
        let _ = w.set_focus();
    }
}

fn cursor() -> (i32, i32) {
    let mut p = POINT { x: 0, y: 0 };
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    (p.x, p.y)
}

/// 把弹窗摆到光标右下方，并保证整块不越出主屏。
///
/// `h` 必须是**即将使用的真实窗口高度**，不能拿最小高度糊弄：
/// 弹窗内容一变高就会冲出屏幕底部（S1b 实测出现过 bottom=1106 > 屏高 1080，
/// 底部 26 px 跑到屏幕外，用户看不到也点不到）。
fn clamp_pos(app: &AppHandle, x: i32, y: i32, h: i32) -> (i32, i32) {
    let w = POPUP_W as i32;
    if let Ok(Some(m)) = app.primary_monitor() {
        let size = m.size();
        let pos = m.position();
        let mut nx = x;
        let mut ny = y;
        if nx + w > pos.x + size.width as i32 {
            nx = pos.x + size.width as i32 - w - 8;
        }
        if ny + h > pos.y + size.height as i32 {
            // 下方放不下就翻到光标上方
            ny = y - h - 20;
        }
        if nx < pos.x + 8 {
            nx = pos.x + 8;
        }
        if ny < pos.y + 8 {
            ny = pos.y + 8;
        }
        // 屏幕太矮、上下都放不下时，至少保证底边不越界
        if ny + h > pos.y + size.height as i32 {
            ny = (pos.y + size.height as i32 - h).max(pos.y);
        }
        return (nx, ny);
    }
    (x, y)
}

/// 划词模式下用户主动点进源文本框 → 这时才允许窗口抢焦点。
///
/// 门 3 选项 A（用户 m03141 选的）：划词弹窗默认不抢焦点，保住选中高亮
/// （06-TESTING §6.4.1 第 4 条）；但源文本是可编辑的，用户点进来想改字，
/// 那一刻必须能收键盘。所以焦点是"用户点了才给"，不是"弹窗一出现就给"。
///
/// **必须在主线程调用。**
pub fn focus_for_typing(app: &AppHandle) {
    if PINNED.load(Ordering::SeqCst) {
        // 钉住不影响打字，这里只是提醒读者两者互不干扰
    }
    set_focusable(app, true);
    if let Some(w) = app.get_webview_window(POPUP_LABEL) {
        let _ = w.emit("popup-focus", ());
    }
}

/// 屏幕中部偏上。输入翻译用的固定位置。
///
/// 划词模式跟着光标走；输入模式用户在打字，窗口位置必须稳定，
/// 否则每翻译一次就跳一次，眼睛和手都跟不上。
fn center_pos(app: &AppHandle) -> (i32, i32) {
    if let Ok(Some(m)) = app.primary_monitor() {
        let size = m.size();
        let pos = m.position();
        let nx = pos.x + (size.width as i32 - POPUP_W as i32) / 2;
        // 偏上 1/6 屏高：下方留给译文展开，上方留出桌面看背景
        let ny = pos.y + (size.height as i32 / 6).max(40);
        return (nx, ny.max(pos.y + 8));
    }
    (200, 160)
}

fn create_popup(app: &AppHandle) -> tauri::Result<tauri::WebviewWindow> {
    let (cx, cy) = cursor();
    let (x, y) = clamp_pos(app, cx + 14, cy + 18, POPUP_H_MIN as i32);
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

/// 光标是否落在**卡片**矩形内（不是窗口矩形）。
///
/// 窗口比卡片大一圈（给阴影留的画布），那一圈是透明的。若按窗口矩形判定，
/// 用户点在卡片外 5~12 px 的透明区就既不会关闭弹窗、也点不到下面的程序，
/// 变成"看不见却吃鼠标"的死区。所以这里要把边距扣掉，只认可见卡片。
fn point_in_window(hwnd: isize, x: i32, y: i32) -> bool {
    let mut r = RECT::default();
    let ok = unsafe { GetWindowRect(HWND(hwnd as *mut core::ffi::c_void), &mut r) }.is_ok();
    if !ok {
        return false;
    }
    x >= r.left + CARD_PAD_X
        && x < r.right - CARD_PAD_X
        && y >= r.top + CARD_PAD_TOP
        && y < r.bottom - CARD_PAD_BOTTOM
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

        // 图钉钉住时用户说了"别自己消失"，点击外部不算数
        if PINNED.load(Ordering::SeqCst) {
            prev_down = left_button_down();
            continue;
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
    let mode = payload.get("mode").and_then(|s| s.as_str()).unwrap_or("select");
    let input = mode == "input";

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

    // 模式决定窗口能不能收键盘。**每次 show 都设一遍**：
    // 窗口可能是上一次划词时建的，现在要复用它做输入翻译。
    // 反过来从输入模式回到划词模式也必须设回 false，
    // 否则下次划词弹窗会抢焦点、把用户选中的文字清掉（06-TESTING §6.4.1 第 4 条）。
    set_focusable(app, input);

    // 换了一次新的翻译 = 上一次的图钉作废。图钉的语义是"钉住这次结果"，
    // 不是"永远钉住弹窗"。用户按热键翻新内容时显然想要新的那个。
    if is_loading && PINNED.load(Ordering::SeqCst) {
        PINNED.store(false, Ordering::SeqCst);
        crate::dlog::dlog("popup: 新一次翻译，图钉自动解除");
    }

    if is_loading {
        // 新一轮翻译：先把高度收回去，再挪到当前光标附近（用户可能已换了位置）。
        // 输入模式下位置固定在屏幕中部偏上，跟着光标跑会边打字边漂。
        let (x, y) = if input {
            center_pos(app)
        } else {
            let (cx, cy) = cursor();
            clamp_pos(app, cx + 14, cy + 18, POPUP_H_MIN as i32)
        };
        let _ = win.set_size(tauri::PhysicalSize::new(POPUP_W as u32, POPUP_H_MIN as u32));
        let _ = win.set_position(tauri::PhysicalPosition::new(x, y));
    }

    crate::dlog::dlog(&format!("popup::show: emit（mode={mode}）"));
    let _ = win.emit("popup-render", payload);

    if just_created {
        // 窗口刚建出来，前端还在加载。现在 show() 会闪一下空白卡片，
        // 所以等 popup_ready 回调时再显示（那时 PENDING 会被补发）。
        return;
    }
    let _ = win.show();
    if input {
        // 复用已有窗口时，set_focus 可能早于 show 生效，再补一次
        let _ = win.set_focus();
    }
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
        // 窗口是"先建好藏着、等前端 ready 了才 show"，所以第一次 show 之前
        // 设的焦点一律无效（对不可见窗口 set_focus 是空操作）。
        // 输入模式必须在 show 之后再要一次焦点，否则键盘进不到输入框。
        if INPUT_MODE.load(Ordering::SeqCst) {
            let _ = w.set_focusable(true);
            let _ = w.set_focus();
            let _ = w.emit("popup-focus", ());
        }
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
    // 图钉与输入模式是「这一次弹窗」的状态，窗口销毁就该复位。
    // 不复位的话下次划词弹窗会带着图钉/可聚焦状态出现：图钉让点击外部关不掉，
    // 可聚焦会把用户选中的文字清掉（06-TESTING §6.4.1 第 4 条）。
    PINNED.store(false, Ordering::SeqCst);
    INPUT_MODE.store(false, Ordering::SeqCst);
    if let Some(w) = app.get_webview_window(POPUP_LABEL) {
        crate::dlog::dlog("popup::close: destroy");
        let _ = w.destroy();
        // 等 WebView2 进程组收尾后再把主进程的工作集换出去：
        // 立刻调只会把还没回收的页又换出去一次，等 1.2s 更稳。
        crate::mem::trim_soon(1200);
    }
}

/// 由前端报告卡片尺寸。原生窗口尺寸必须后端设置（ADR-011），
/// 前端算出来的 CSS 尺寸后端不知道。
///
/// `height` / `width` 是**卡片**尺寸（前端已把阴影边距加上，所以其实是窗口尺寸）。
/// 变高后必须重新夹取位置：否则从 110 撑到 640 时会冲出屏幕底部。
pub fn resize(win: &tauri::WebviewWindow, height: f64, width: f64) {
    let w = width.clamp(POPUP_W - 200.0, POPUP_W + 200.0);
    let h = height.clamp(POPUP_H_MIN * 0.5, POPUP_H_MAX);
    let _ = win.set_size(tauri::PhysicalSize::new(w as u32, h as u32));

    // 变高/变宽后重新夹到屏幕内（用新高度判断，不是最小高度）
    let app = win.app_handle();
    if let Ok(pos) = win.outer_position() {
        let (nx, ny) = clamp_pos(app, pos.x, pos.y, h as i32);
        if nx != pos.x || ny != pos.y {
            let _ = win.set_position(tauri::PhysicalPosition::new(nx, ny));
        }
    }
}

// ---------- 设置窗口 ----------

/// 「下次打开设置页时要停在哪个工具」。托盘点工具项、或从翻译弹窗跳过来时置位，
/// 前端加载完调 `take_pending_tool` 取走并跳转。
///
/// 为什么用「取走」而不是「读取」：跳转只该发生一次，用户之后自己切到别的工具
/// 不该被拉回去。
static PENDING_TOOL: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// 打开设置窗口并跳到指定工具。**必须在主线程调用。**
pub fn open_settings_at(app: &AppHandle, tool_id: &str) {
    if let Ok(mut g) = PENDING_TOOL.lock() {
        *g = Some(tool_id.to_string());
    }
    let existed = app.get_webview_window(SETTINGS_LABEL).is_some();
    open_settings(app);
    if existed {
        // 窗口已经在：它不会重新加载页面，所以直接推个事件过去
        if let Some(w) = app.get_webview_window(SETTINGS_LABEL) {
            let _ = w.emit("settings-goto", tool_id);
        }
    }
}

/// 前端取走待跳转的工具（可能为 `None`）。
pub fn take_pending_tool() -> Option<String> {
    PENDING_TOOL.lock().ok().and_then(|mut g| g.take())
}

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
        // 760 = 原来的 560 + 200（用户 m02639 第 2 条要求）。
        // 左侧栏 160 + 右侧字段区 600，长路径 / BaseURL 这类输入框才不至于挤成一条。
        .inner_size(760.0, 620.0)
        .min_inner_size(660.0, 480.0)
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
