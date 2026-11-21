//! 弹窗与设置窗口的生命周期。
//!
//! **窗口一律运行时创建**：`tauri.conf.json` 里 `app.windows` 是 `[]`，启动时不建任何
//! WebView —— 这是"轻"的核心（启动即建窗会立刻拉起 6 个 msedgewebview2.exe，约 269 MB）。
//!
//! 关闭语义由「极轻模式」开关决定（ADR-007 / 会话 32）：默认开 = **destroy**，
//! 立刻释放 WebView2 的 6 个子进程（实测约 300 MB）；关掉 = **hide**，窗口与
//! WebView 挂起常驻，下次打开不用重新加载页面，但内存不释放。

use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::Duration;

use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetWindowRect, IsWindow};

pub const POPUP_LABEL: &str = "popup";
pub const SETTINGS_LABEL: &str = "settings";
/// 下载主窗口（任务列表）。pump 用同一个 label 判断「窗口是否可见」。
pub const MAIN_LABEL: &str = "main";
/// 新建下载任务窗口。列表页点「+」时与主窗口**同时存在**。
pub const NEW_TASK_LABEL: &str = "new-task";

/// S5 OCR 框选 overlay：覆盖整个虚拟屏幕的透明窗口（暗幕 + 十字光标）。
pub const OCR_LABEL: &str = "ocr_overlay";
/// S7 截图框选 overlay。与 OCR 共用 `region_select`，只是 label / 页面不同。
pub const SHOT_LABEL: &str = "shot_overlay";
/// S7 滚动截图的框选 overlay（选区确定后立刻销毁，换成进度小窗）。
pub const SCROLL_LABEL: &str = "scroll_overlay";
/// 滚动截图的进度 / 停止小窗。放在选区之外，捕获期间不会被截进图。
pub const SCROLL_CONTROL_LABEL: &str = "scroll_control";
/// 钉屏窗口（单窗多图，会话 63）。
pub const PIN_LABEL: &str = "pin";/// 截图预览窗口：裁头尾 + 送存盘 / 剪贴板 / 钉屏 / OCR（会话 63）。
pub const PREVIEW_LABEL: &str = "preview";
/// 全屏截图右下角缩略图反馈（S7 会话 72）。
pub const FEEDBACK_LABEL: &str = "shot_feedback";

/// 新建窗口要预填的一条任务。热键只给 `url`；浏览器扩展还会带文件名、
/// 大小、Referer、UA 与 Cookie（Cookie **只走数据通道，不进界面**）。
#[derive(Clone, Debug, serde::Serialize)]
pub struct NewTaskPrefill {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub referer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cookie: Option<String>,
    /// 同一文件的其他下载地址（镜像 / 直连兜底）。新建页不展示，提交时
    /// 一起交给 aria2；为空时序列化里不出现。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub mirrors: Vec<String>,
}

impl NewTaskPrefill {
    /// 热键 / 剪贴板只有链接时的构造器。
    pub fn url_only(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            filename: None,
            size: None,
            referer: None,
            user_agent: None,
            cookie: None,
            mirrors: Vec::new(),
        }
    }
}

/// 待预填队列。用「取走」而不是「读取」：预填只该发生一次，用户改完内容后
/// 窗口再被聚焦不该覆盖回原链接。用 `Vec` 而不是单个值，是因为浏览器里连续
/// 点两个下载时，两条都要进同一个新建页（用户选「合并进同一窗口」）。
static NEW_TASK_PREFILL: std::sync::Mutex<Vec<NewTaskPrefill>> = std::sync::Mutex::new(Vec::new());

/// 主窗口打开后要停在哪个视图（`"transfer"` = 互传页）。托盘「局域网互传…」
/// 用。同样用「取走」语义：只影响第一次加载，之后用户切走不拉回。
static PENDING_MAIN_VIEW: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// 前端取走待显示的主窗口视图（可能为 `None`）。
pub fn take_pending_main_view() -> Option<String> {
    PENDING_MAIN_VIEW
        .lock()
        .ok()
        .and_then(|mut g| g.take())
}

/// 拖放到主窗口的文件路径（S4b 发送入口之一，§PRD F4.4）。事件只是通知，
/// 路径用「取走」语义交给前端（风险 71 同款）。
static TRANSFER_DROP: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// 前端取走拖放进来的文件路径。
pub fn take_transfer_drop() -> Vec<String> {
    TRANSFER_DROP
        .lock()
        .ok()
        .map(|mut g| g.drain(..).collect())
        .unwrap_or_default()
}

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
///
/// **绝不能加 `--renderer-process-limit=1`**（曾经加过，是个 bug）：
/// 这个参数限制的是**整个浏览器进程**的 renderer 数量，而弹窗与设置窗口
/// 共用同一个 WebView2 环境（同一个 data directory）。弹窗先开时占掉唯一的
/// renderer 名额，设置窗口就渲染不出任何东西 —— 表现为纯白空白页；
/// 随后关闭弹窗时 WebView2 环境状态被搅乱，主线程卡死、程序"无响应"。
/// 省下来的那点内存（实测约 40 MB）远不值一个打不开的设置页。
pub(crate) const WEBVIEW_ARGS: &str =
    "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection";

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

/// 当前这一轮弹窗的窗口**是否已经摆过位置**。
///
/// 摆位置只该在「新一次唤出」时做一次。以前是每收到 `state:"loading"` 就重摆，
/// 而「点翻译按钮 / 按回车 / 切引擎」都会触发一次 loading，
/// 于是用户每操作一次弹窗就跳一次位置（输入模式跳回屏幕中部偏上，
/// 划词模式跟着当前光标跑）——用户 m03664 报的第 2 个 bug。
/// `close()` 里复位，下一轮唤出才重新摆。
static PLACED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

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

/// CSS 逻辑像素 → 物理像素。窗口 API 的 `PhysicalSize` 不认逻辑像素。
fn to_physical(logical: f64, scale: f64) -> f64 {
    (logical * scale).round()
}

/// 把弹窗摆到光标右下方，并保证整块不越出屏幕。
///
/// `h_logical` 必须是**即将使用的真实窗口高度**（逻辑像素），不能拿最小高度糊弄：
/// 弹窗内容一变高就会冲出屏幕底部（S1b 实测出现过 bottom=1106 > 屏高 1080，
/// 底部 26 px 跑到屏幕外，用户看不到也点不到）。
fn clamp_pos(app: &AppHandle, x: i32, y: i32, h_logical: f64) -> (i32, i32) {
    let monitor = app
        .monitor_from_point(x as f64, y as f64)
        .ok()
        .flatten()
        .or_else(|| app.primary_monitor().ok().flatten());
    let Some(m) = monitor else {
        return (x, y);
    };
    let scale = m.scale_factor();
    let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let w = to_physical(POPUP_W, scale) as i32;
    let h = to_physical(h_logical, scale) as i32;
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
    (nx, ny)
}

/// 划词模式下用户主动点进源文本框 → 这时才允许窗口抢焦点。
///
/// 门 3 选项 A（用户 m03141 选的）：划词弹窗默认不抢焦点，保住选中高亮
/// （06-TESTING §6.4.1 第 4 条）；但源文本是可编辑的，用户点进来想改字，
/// 那一刻必须能收键盘。所以焦点是"用户点了才给"，不是"弹窗一出现就给"。
///
/// **必须在主线程调用。**
pub fn focus_for_typing(app: &AppHandle) {
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
    // 输入翻译在光标所在显示器居中，避免多屏不同缩放时跑到主屏去。
    let (cx, cy) = cursor();
    let monitor = app
        .monitor_from_point(cx as f64, cy as f64)
        .ok()
        .flatten()
        .or_else(|| app.primary_monitor().ok().flatten());
    if let Some(m) = monitor {
        let size = m.size();
        let pos = m.position();
        let scale = m.scale_factor();
        let w = to_physical(POPUP_W, scale) as i32;
        let nx = pos.x + (size.width as i32 - w) / 2;
        // 偏上 1/6 屏高：下方留给译文展开，上方留出桌面看背景
        let ny = pos.y + (size.height as i32 / 6).max(40);
        return (nx, ny.max(pos.y + 8));
    }
    (200, 160)
}

fn create_popup(app: &AppHandle) -> tauri::Result<tauri::WebviewWindow> {
    let (cx, cy) = cursor();
    let (x, y) = clamp_pos(app, cx + 14, cy + 18, POPUP_H_MIN);
    WebviewWindowBuilder::new(app, POPUP_LABEL, WebviewUrl::App("popup.html".into()))
        .title("Vexto")
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
///
/// `GetWindowRect` 返回物理像素，`CARD_PAD_*` 是 CSS 逻辑像素，必须乘缩放比。
fn point_in_window(hwnd: isize, x: i32, y: i32, scale: f64) -> bool {
    let mut r = RECT::default();
    let ok = unsafe { GetWindowRect(HWND(hwnd as *mut core::ffi::c_void), &mut r) }.is_ok();
    if !ok {
        return false;
    }
    let pad_x = to_physical(CARD_PAD_X as f64, scale) as i32;
    let pad_top = to_physical(CARD_PAD_TOP as f64, scale) as i32;
    let pad_bottom = to_physical(CARD_PAD_BOTTOM as f64, scale) as i32;
    x >= r.left + pad_x
        && x < r.right - pad_x
        && y >= r.top + pad_top
        && y < r.bottom - pad_bottom
}

/// 弹窗是 `focusable(false)`（不抢焦点，才保得住用户选中的文字），
/// 因此它**收不到 `Focused(false)` 事件**，没法用失焦来实现"点外面关掉"。
///
/// 这里改用 30ms 轮询：左键从抬起变按下的那一刻，若光标不在弹窗矩形内就关闭。
/// 选轮询而不是 `WH_MOUSE_LL` 全局钩子的理由：钩子会让全系统每次鼠标移动都进我们的
/// 回调，回调稍慢一点 Windows 就静默把钩子摘掉，出问题时极难排查；轮询最多慢 30ms，
/// 而且**点击照常穿透给下面的程序**，用户点哪就激活哪，体验上跟没有这个窗口一样。
fn watch_outside_click(app: AppHandle, hwnd: isize, scale: f64) {
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
            let inside = point_in_window(hwnd, cx, cy, scale);
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
    let scale = win.scale_factor().unwrap_or(1.0);
    std::thread::spawn(move || watch_outside_click(a, hwnd, scale));
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

    // 图钉**不能**在这里被清掉。用户 m05732 第 3 条报的正是这个：
    // 钉住之后在弹窗里回车/点「翻译」/切引擎，每一轮都会收到一次
    // `state:"loading"`，于是图钉被自动解除，窗口几秒后自己关掉。
    // 钉住的语义是「钉住这个窗口，直到用户自己取消或关掉」——
    // 同一轮里的重译（换引擎、改原文再翻）属于同一轮，不能动图钉。
    // 新一轮的唤出走的是 close() → 那里会把 PINNED 复位。

    // 只在新一次唤出时摆位置。同一轮里的 loading→done→loading（点翻译按钮、
    // 按回车、切引擎都会走这条）不再挪窗口，否则弹窗每操作一次跳一次。
    // `just_created` 覆盖第一次建窗；`fresh` 覆盖"复用窗口但按了新热键"；
    // PLACED 兜住"窗口还在、这一轮还没摆过"的情况。
    let first_of_round = just_created
        || !PLACED.load(Ordering::SeqCst)
        || payload.get("fresh").and_then(|b| b.as_bool()) == Some(true);
    if is_loading && first_of_round {
        // 新一轮翻译：先把高度收回去，再摆到该在的位置。
        // 输入模式下位置固定在屏幕中部偏上，跟着光标跑会边打字边漂。
        let (x, y) = if input {
            center_pos(app)
        } else {
            let (cx, cy) = cursor();
            clamp_pos(app, cx + 14, cy + 18, POPUP_H_MIN)
        };
        // 逻辑尺寸：Tauri 会按显示器 DPI 自己换算成物理像素。
        let _ = win.set_size(tauri::LogicalSize::new(POPUP_W, POPUP_H_MIN));
        let _ = win.set_position(tauri::PhysicalPosition::new(x, y));
    }
    if first_of_round {
        PLACED.store(true, Ordering::SeqCst);
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
/// 极轻模式开（默认）= `destroy`：`hide` 之后 6 个 msedgewebview2 子进程
/// （实测合计约 280 MB）会一直驻留；`destroy` 之后整个 WebView2 进程组退出，
/// 主进程回到纯 Rust 的几 MB。极轻模式关 = `hide`，窗口挂起复用。
/// 从「关」切回「开」时 `save_tool_config` 会调 `prune_hidden_windows()`，
/// 把这一轮挂起的窗口统一回收（会话 33）。
pub fn close(app: &AppHandle) {
    // 先让监视器退场，避免它对着正在销毁的句柄继续轮询
    WATCHED_HWND.store(0, Ordering::SeqCst);
    // 图钉与输入模式是「这一次弹窗」的状态，窗口销毁就该复位。
    // 不复位的话下次划词弹窗会带着图钉/可聚焦状态出现：图钉让点击外部关不掉，
    // 可聚焦会把用户选中的文字清掉（06-TESTING §6.4.1 第 4 条）。
    PINNED.store(false, Ordering::SeqCst);
    INPUT_MODE.store(false, Ordering::SeqCst);
    // 位置记忆也要清：下次唤出是一次新弹窗，得按当时的模式/光标重新摆。
    PLACED.store(false, Ordering::SeqCst);
    if let Some(w) = app.get_webview_window(POPUP_LABEL) {
        if crate::config::load().lite_mode {
            crate::dlog::dlog("popup::close: destroy（极轻模式）");
            let _ = w.destroy();
            // 等 WebView2 进程组收尾后再把主进程的工作集换出去：
            // 立刻调只会把还没回收的页又换出去一次，等 1.2s 更稳。
            crate::mem::trim_soon(1200);
        } else {
            // 极轻模式关了：只隐藏，WebView 留着给下一次划词直接复用。
            // 状态已经在上面复位，下一次 show() 会当成新一轮重新摆位置。
            crate::dlog::dlog("popup::close: hide（极轻模式已关，WebView 挂起）");
            let _ = w.hide();
        }
    }
}

/// 极轻模式重新开启时要回收的窗口 label。
/// **不含设置窗口**：用户此刻正在里面点「保存」，把它销毁了面板会当场消失。
pub fn prunable_labels() -> [&'static str; 3] {
    [POPUP_LABEL, MAIN_LABEL, NEW_TASK_LABEL]
}

/// 开启极轻模式后，把上一次「关掉极轻模式」期间挂起（hide）的窗口一次性回收。
///
/// 会话 33 的 bug：关掉极轻模式时窗口关闭只是 hide，之后即使重新打开极轻模式，
/// 这些 WebView 也没人再去关一次，于是继续占着内存；只有设置窗口在用户关闭它时
/// 走了新的 destroy 分支。所以在「lite_mode 变为 true」那次保存里主动清一遍。
/// 只销毁**不可见**的窗口：正显示着的留给用户，等他关闭时自然按新语义 destroy。
pub fn prune_hidden_windows(app: &AppHandle) {
    let mut n = 0;
    for label in prunable_labels() {
        let Some(w) = app.get_webview_window(label) else {
            continue;
        };
        // 可见性查不出来时按「可见」处理，宁可少收一个也别把正在用的窗口弄没。
        if w.is_visible().unwrap_or(true) {
            continue;
        }
        if w.destroy().is_ok() {
            n += 1;
        }
    }
    if n > 0 {
        crate::dlog::dlog(&format!("popup: 极轻模式开启，回收 {n} 个挂起的窗口"));
        // WebView2 进程组收尾后再换出工作集，和 close() 的 destroy 分支一致
        crate::mem::trim_soon(1200);
    }
}

/// 由前端报告卡片尺寸。原生窗口尺寸必须后端设置（ADR-011），
/// 前端算出来的 CSS 尺寸后端不知道。
///
/// `height` / `width` 是**卡片**尺寸（前端已把阴影边距加上，所以其实是窗口尺寸），
/// 单位是 **CSS 逻辑像素**。缩放比不是 1 时必须先乘再交给 `PhysicalSize`，
/// 否则 150% 缩放下窗口只有应有尺寸的 2/3，卡片内容会被裁掉。
/// 变高后必须重新夹取位置：否则从 110 撑到 640 时会冲出屏幕底部。
pub fn resize(win: &tauri::WebviewWindow, height: f64, width: f64) {
    let scale = win.scale_factor().unwrap_or(1.0);
    let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let w_logical = width.clamp(POPUP_W - 200.0, POPUP_W + 200.0);
    let h_logical = height.clamp(POPUP_H_MIN * 0.5, POPUP_H_MAX);
    let w = to_physical(w_logical, scale);
    let h = to_physical(h_logical, scale);
    let _ = win.set_size(tauri::PhysicalSize::new(w as u32, h as u32));

    // 变高/变宽后重新夹到屏幕内（用新高度判断，不是最小高度）。
    //
    // 注意这里**不能**复用 clamp_pos()：那个函数是给"贴着光标弹出"设计的，
    // 里面有一条"下方放不下就翻到光标上方"的分支 `ny = y - h - 20`。
    // 传给它的是窗口自己的左上角，一旦弹窗本来就在屏幕下半部分，这条分支
    // 会把整块往上顶 (h + 20)，表现就是"点一次翻译，弹窗跳到屏幕中间偏上"。
    // 缩放只该做一件事：把越界的边推回屏幕内，顶边不动。
    let app = win.app_handle();
    if let Ok(pos) = win.outer_position() {
        let (nx, ny) = clamp_keep_topleft(app, pos.x, pos.y, w as i32, h as i32);
        if nx != pos.x || ny != pos.y {
            let _ = win.set_position(tauri::PhysicalPosition::new(nx, ny));
        }
    }
}

/// 缩放时用的夹取：**保持左上角不动**，只把越界的右下边推回屏幕内。
///
/// 与 `clamp_pos` 的区别是它不尝试"改到光标另一侧"——窗口已经在屏幕上了，
/// 挪走只会让用户找不到它。屏幕装不下时顶边优先（宁可盖住底部一点，
/// 也不能让标题栏跑到屏幕外，那样连拖都拖不回来）。
fn clamp_keep_topleft(app: &AppHandle, x: i32, y: i32, w: i32, h: i32) -> (i32, i32) {
    if let Ok(Some(m)) = app.primary_monitor() {
        let size = m.size();
        let pos = m.position();
        let right = pos.x + size.width as i32;
        let bottom = pos.y + size.height as i32;
        let mut nx = x;
        let mut ny = y;
        if nx + w > right {
            nx = right - w;
        }
        if ny + h > bottom {
            ny = bottom - h;
        }
        if nx < pos.x {
            nx = pos.x;
        }
        if ny < pos.y {
            ny = pos.y;
        }
        return (nx, ny);
    }
    (x, y)
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
        .title("Vexto 设置")
        // 无边框 + 自绘标题栏（docs/13-UI-DESIGN-SYSTEM.md）。设置页字段多、路径长，
        // 仍然 resizable(true)，边缘缩放由前端 `titlebar.js` 的 8 向热区接管。
        .decorations(false)
        .shadow(true)
        // 820：会话 25 起设置页改成「标签在左、控件在右」两栏，
        // 标签列 150 + 控件列要放长路径 / BaseURL / UA，760 不够。
        // 最小 700 保证两栏都还站得住（再窄标签列就会挤成两行）。
        .inner_size(820.0, 620.0)
        .min_inner_size(700.0, 480.0)
        .resizable(true)
        .center()
        // 会话 32：WebView2 首帧是白底，窗口一建出来就 show 会闪一下白屏。
        // 先藏着，等前端 load 完调 window_ready 再显示；深色窗口底色是双保险。
        .visible(false)
        .background_color(tauri::window::Color(0x16, 0x18, 0x1d, 255))
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

/// 打开下载主窗口。已存在则聚焦，不重复创建。**必须在主线程调用。**
///
/// 关闭即销毁（ADR-007 / 任务书 §S3.7.3）：主窗口体量大，用 hide 会让
/// 6 个 msedgewebview2.exe 一直驻留；数据全在后端 pump 里，重建窗口后
/// 前端调 `download_snapshot()` 就能恢复列表，进度不会重新开始。
pub fn open_main_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(MAIN_LABEL) {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
        return;
    }

    let r = WebviewWindowBuilder::new(app, MAIN_LABEL, WebviewUrl::App("main.html".into()))
        .title("Vexto 下载")
        // 无边框 + 自绘标题栏（docs/13-UI-DESIGN-SYSTEM.md）。任务列表可按
        // 文件名长短拉宽，所以保留 resizable，边缘缩放由 titlebar.js 接管。
        .decorations(false)
        .shadow(true)
        .inner_size(900.0, 600.0)
        .min_inner_size(720.0, 480.0)
        .resizable(true)
        .center()
        // 会话 32：WebView2 首帧是白底，窗口一建出来就 show 会闪一下白屏。
        // 先藏着，等前端 load 完调 window_ready 再显示；深色窗口底色是双保险。
        .visible(false)
        .background_color(tauri::window::Color(0x16, 0x18, 0x1d, 255))
        .additional_browser_args(WEBVIEW_ARGS)
        .build();

    match r {
        Ok(win) => {
            // 新建的窗口要抢焦点：实测不调 set_focus() 时，热键入队后窗口会
            // 落在其它窗口后面，用户以为「没反应」（会话 14 实机取证）。
            // 只有已存在的分支会走上面的 set_focus，新建分支必须自己补。
            let _ = win.set_focus();
            let emit_win = win.clone();
            win.on_window_event(move |ev| match ev {
                // 拖放文件进来 = 发送到局域网设备（PRD F4.4）。路径存队列、
                // 发通知，前端只在互传视图里取（下载视图拖放静默丢弃）。
                tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) => {
                    let list: Vec<String> = paths
                        .iter()
                        .map(|p| p.to_string_lossy().to_string())
                        .collect();
                    if !list.is_empty() {
                        if let Ok(mut g) = TRANSFER_DROP.lock() {
                            g.extend(list);
                        }
                        let _ = emit_win.emit("transfer:dropped", ());
                    }
                }
                tauri::WindowEvent::Destroyed => {
                    crate::dlog::dlog("popup: 下载主窗口已销毁，1.2s 后收缩工作集");
                    crate::mem::trim_soon(1200);
                }
                _ => {}
            });
        }
        Err(e) => crate::dlog::dlog(&format!("create main window failed: {e}")),
    }
}

/// 打开主窗口并预选视图（`view` 目前只有 `"transfer"`）。
pub fn open_main_window_with_view(app: &AppHandle, view: &str) {
    // 已有窗口时页面已加载，预选不会自动生效——发个通知让前端自己切。
    // 只在「要新建窗口」时登记预选，否则这个值会残留到下一次建窗。
    if let Some(w) = app.get_webview_window(MAIN_LABEL) {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
        let _ = w.emit("main:switch-view", view);
        return;
    }
    if let Ok(mut g) = PENDING_MAIN_VIEW.lock() {
        *g = Some(view.to_string());
    }
    open_main_window(app);
}

/// 打开「收到传输请求」的确认界面。
///
/// 会话 80：确认框不再是独立窗口，而是互传窗口里的遮罩对话框
/// （用户要求「互传窗口的子窗口 + 遮罩层」）。这里只负责把主窗口叫起来并
/// 切到互传视图；对话框由 `ui/main.js` 的 `transfer:event` 渲染。
/// **必须在主线程调用。**
pub fn open_transfer_confirm(app: &AppHandle) {
    open_main_window_with_view(app, "transfer");
}

/// 打开 S5 OCR 框选 overlay。实现已抽到 `region_select`，OCR 的事件名与
/// 行为保持不变（ADR-041）。
/// **必须在主线程调用**（热键回调本身就在事件循环线程，直接调即可）。
pub fn open_ocr_overlay(app: &AppHandle) {
    crate::region_select::open(app, crate::region_select::Purpose::Ocr);
}

/// 打开 S7 截图框选 overlay。与 OCR 共用窗口 / 坐标 / 取消语义。
pub fn open_shot_overlay(app: &AppHandle) {
    crate::region_select::open(app, crate::region_select::Purpose::Shot);
}

/// 关掉截图 overlay（Esc / 成功交付后）。窗口 destroy，下次热键重建。
pub fn close_shot_overlay(app: &AppHandle) {
    crate::region_select::close(app, crate::region_select::Purpose::Shot);
}

/// 关掉滚动截图框选 overlay。
pub fn close_scroll_overlay(app: &AppHandle) {
    crate::region_select::close(app, crate::region_select::Purpose::Scroll);
}

/// 滚动截图的进度 / 停止小窗。
///
/// 位置优先选虚拟屏幕四角中**不与选区相交**的那一个：捕获期间它一直在屏幕上，
/// 落在选区里就会被拼进长图（06 §6.4.5 B11）。
pub fn open_scroll_control(app: &AppHandle, sel: (i32, i32, i32, i32), dpr: f64) {
    if let Some(w) = app.get_webview_window(SCROLL_CONTROL_LABEL) {
        let _ = w.destroy();
    }
    let dpr = if dpr.is_finite() && dpr > 0.0 { dpr } else { 1.0 };
    let logical_w = 300.0f64;
    let logical_h = 176.0f64;
    let pw = (logical_w * dpr).round() as i32;
    let ph = (logical_h * dpr).round() as i32;
    let margin = (16.0 * dpr).round() as i32;
    let (vx, vy, vw, vh) = crate::ocr::virtual_screen();
    let candidates = [
        (vx + vw - pw - margin, vy + vh - ph - margin),
        (vx + vw - pw - margin, vy + margin),
        (vx + margin, vy + vh - ph - margin),
        (vx + margin, vy + margin),
    ];
    let (sx, sy, sw, sh) = sel;
    let (px, py) = candidates
        .into_iter()
        .find(|(x, y)| !overlaps((*x, *y, pw, ph), (sx, sy, sw, sh)))
        .unwrap_or(candidates[0]);

    let r = WebviewWindowBuilder::new(
        app,
        SCROLL_CONTROL_LABEL,
        WebviewUrl::App("scroll_control.html".into()),
    )
    .title("滚动截图")
    .decorations(false)
    .always_on_top(true)
    .resizable(false)
    .maximizable(false)
    .minimizable(false)
    .skip_taskbar(true)
    // 不抢焦点：抢了之后用户没法在目标窗口里继续滚（半自动模式）。
    .focused(false)
    .inner_size(logical_w, logical_h)
    .additional_browser_args(WEBVIEW_ARGS)
    .build();

    match r {
        Ok(win) => {
            let _ = win.set_position(tauri::PhysicalPosition::new(px, py));
        }
        Err(e) => crate::dlog::dlog(&format!("create scroll control failed: {e}")),
    }
}

fn overlaps(a: (i32, i32, i32, i32), b: (i32, i32, i32, i32)) -> bool {
    let (ax, ay, aw, ah) = a;
    let (bx, by, bw, bh) = b;
    ax < bx + bw && ax + aw > bx && ay < by + bh && ay + ah > by
}

/// 关掉滚动截图进度窗。
pub fn close_scroll_control(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(SCROLL_CONTROL_LABEL) {
        let _ = w.destroy();
    }
}

/// 按 label 打开钉屏窗口：`pin` 走单窗多图（复用），其余 label 每图一窗。
/// **必须在主线程调用**（会话 62 坑 11：IPC 回调里同步建第二个 WebView 会 about:blank）。
pub fn open_pin_window_for(app: &AppHandle, label: &str) {
    if label == PIN_LABEL {
        if let Some(w) = app.get_webview_window(PIN_LABEL) {
            let _ = w.show();
            let _ = w.set_focus();
            crate::screenshot::pin::fit_window(&w);
            crate::screenshot::pin::emit_state(&w);
            return;
        }
    }
    let label_owned = label.to_string();
    let r = WebviewWindowBuilder::new(app, label, WebviewUrl::App("pin.html".into()))
        .title("钉屏")
        .decorations(false)
        .shadow(false)
        .transparent(true)
        .always_on_top(true)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .skip_taskbar(true)
        .inner_size(480.0, 360.0)
        // 会话 32 同款防白屏：先藏，前端 window_ready 再显示。
        .visible(false)
        .additional_browser_args(WEBVIEW_ARGS)
        .build();
    match r {
        Ok(win) => {
            let l = label_owned.clone();
            win.on_window_event(move |ev| {
                if matches!(ev, tauri::WindowEvent::Destroyed) {
                    crate::screenshot::pin::on_window_destroyed(&l);
                }
            });
            crate::screenshot::pin::fit_window(&win);
            let _ = win.center();
        }
        Err(e) => crate::dlog::dlog(&format!("create pin window failed: {e}")),
    }
}

/// 打开 / 聚焦截图预览窗口。
/// **必须在主线程调用**（同上，坑 11）。
pub fn open_preview_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(PREVIEW_LABEL) {
        let _ = w.show();
        let _ = w.set_focus();
        crate::screenshot::preview::emit_state(&w);
        return;
    }
    // 无边框 + 自绘标题栏（docs/13-UI-DESIGN-SYSTEM.md，项目内所有窗口统一）。
    let r = WebviewWindowBuilder::new(
        app,
        PREVIEW_LABEL,
        WebviewUrl::App("preview.html".into()),
    )
    .title("截图预览")
    .decorations(false)
    .shadow(true)
    .inner_size(980.0, 780.0)
    .min_inner_size(640.0, 480.0)
    .visible(false)
    .center()
    .background_color(tauri::window::Color(0x16, 0x18, 0x1d, 255))
    .additional_browser_args(WEBVIEW_ARGS)
    .build();
    match r {
        Ok(win) => {
            win.on_window_event(|ev| {
                if matches!(ev, tauri::WindowEvent::Destroyed) {
                    crate::screenshot::preview::on_window_destroyed();
                }
            });
        }
        Err(e) => crate::dlog::dlog(&format!("create preview window failed: {e}")),
    }
}

/// 关掉预览窗口。
pub fn close_preview_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(PREVIEW_LABEL) {
        let _ = w.destroy();
    }
}

/// 打开 / 更新全屏截图右下角缩略图反馈。
/// 已存在时只更新状态与显示，不销毁重建——销毁会触发 `feedback::clear`，
/// 把刚设置的新图删掉。**必须在主线程调用**（坑 11）。
pub fn open_feedback_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(FEEDBACK_LABEL) {
        let _ = w.show();
        let _ = w.emit("feedback:state", crate::screenshot::feedback::state_value());
        return;
    }
    let logical_w = 320.0f64;
    let logical_h = 240.0f64;
    let r = WebviewWindowBuilder::new(
        app,
        FEEDBACK_LABEL,
        WebviewUrl::App("feedback.html".into()),
    )
    .title("截图")
    .decorations(false)
    .transparent(true)
    .always_on_top(true)
    .resizable(false)
    .maximizable(false)
    .minimizable(false)
    .skip_taskbar(true)
    .focusable(false)
    .focused(false)
    .shadow(false)
    .visible(false)
    .inner_size(logical_w, logical_h)
    .additional_browser_args(WEBVIEW_ARGS)
    .build();

    match r {
        Ok(win) => {
            // 先铺满虚拟屏幕：页面显示全屏截图后由 `feedback_start_animation`
            // 把窗口收缩到右下角。动画期间点击穿透，结束再恢复。
            let (vx, vy, vw, vh) = crate::ocr::virtual_screen();
            let _ = win.set_position(tauri::PhysicalPosition::new(vx, vy));
            let _ = win.set_size(tauri::PhysicalSize::new(
                vw.max(1) as u32,
                vh.max(1) as u32,
            ));
            let _ = win.set_ignore_cursor_events(true);
            win.on_window_event(|ev| {
                if matches!(ev, tauri::WindowEvent::Destroyed) {
                    crate::screenshot::feedback::clear();
                }
            });
        }
        Err(e) => crate::dlog::dlog(&format!("create feedback window failed: {e}")),
    }
}

/// 关掉全屏截图缩略图反馈窗。
pub fn close_feedback_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(FEEDBACK_LABEL) {
        let _ = w.destroy();
    }
}

/// 打开「新建任务」窗口。已存在则聚焦，并把新链接补发过去。**必须在主线程调用。**
///
/// 与列表窗口是两个独立的 `WebviewWindow`（用户明确要求两窗同时存在）：
/// 列表页点「+」→ 新建成窗；新建页点「新建」→ 关自己、开/聚焦列表。
pub fn open_new_task(app: &AppHandle, prefill: Option<NewTaskPrefill>) {
    let has_prefill = prefill.is_some();
    if let Some(p) = prefill {
        if let Ok(mut g) = NEW_TASK_PREFILL.lock() {
            g.push(p);
        }
    }

    if let Some(w) = app.get_webview_window(NEW_TASK_LABEL) {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
        // 窗口已存在时页面不会重新加载，只能推事件让前端来取。
        // **事件只是个「有新东西」的通知，不携带也不取走数据**：窗口刚建、
        // 页面还没注册监听时，带数据的 emit 会丢失（浏览器连续点两个下载时
        // 实测两条都丢）。前端收到通知后再调 `take_new_task_prefill` 取，
        // 即使通知丢了，初始化时的那次 take 也能把队列一次取干净。
        if has_prefill {
            let _ = w.emit("new-task-prefill", ());
        }
        return;
    }

    let r = WebviewWindowBuilder::new(app, NEW_TASK_LABEL, WebviewUrl::App("new_task.html".into()))
        .title("新建任务")
        // 无边框 + 自绘标题栏（用户要求）。系统标题栏去掉后，拖动只能靠前端
        // 标题栏上的 `data-tauri-drag-region`；最小化 / 关闭也由自绘按钮接管。
        .decorations(false)
        .shadow(true)
        // 600×540 与参考界面（Rayburst 新建任务）保持同一比例。
        .inner_size(600.0, 540.0)
        .min_inner_size(600.0, 540.0)
        .resizable(false)
        .center()
        // 会话 32：WebView2 首帧是白底，窗口一建出来就 show 会闪一下白屏。
        // 先藏着，等前端 load 完调 window_ready 再显示；深色窗口底色是双保险。
        .visible(false)
        .background_color(tauri::window::Color(0x16, 0x18, 0x1d, 255))
        .additional_browser_args(WEBVIEW_ARGS)
        .build();

    match r {
        Ok(win) => {
            let _ = win.set_focus();
            win.on_window_event(|ev| {
                if matches!(ev, tauri::WindowEvent::Destroyed) {
                    crate::dlog::dlog("popup: 新建任务窗口已销毁，1.2s 后收缩工作集");
                    crate::mem::trim_soon(1200);
                }
            });
        }
        Err(e) => crate::dlog::dlog(&format!("create new task window failed: {e}")),
    }
}

/// 前端取走待预填的条目（可能为空）。
pub fn take_new_task_prefill() -> Vec<NewTaskPrefill> {
    NEW_TASK_PREFILL
        .lock()
        .map(|mut g| std::mem::take(&mut *g))
        .unwrap_or_default()
}

/// 只关新建窗口（「取消」/ 自绘标题栏的关闭按钮）。**必须在主线程调用。**
///
/// 取消不该打开列表窗口 —— 用户可能只是一时误按热键。
pub fn close_new_task(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(NEW_TASK_LABEL) {
        let _ = w.destroy();
    }
}

/// 新建任务完成：打开（或聚焦）列表窗口，再关掉新建窗口。**必须在主线程调用。**
///
/// **顺序是「先开后关」**：两个窗口共用同一个 WebView2 环境，先 `destroy()`
/// 再建新窗口会让新窗口的 WebView 停在 `about:blank`（本机实测：窗口标题是
/// 「Vexto 下载」，页面 body 为空）。先建新窗口、再销毁旧窗口，环境始终
/// 有一个活着的宿主，导航正常。代价是任务栏里有两三帧两个窗口同时存在。
pub fn close_new_task_and_show_list(app: &AppHandle) {
    open_main_window(app);
    if let Some(w) = app.get_webview_window(NEW_TASK_LABEL) {
        let _ = w.destroy();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popup_physical_size_follows_scale_factor() {
        // 420 CSS px 的卡片在 100% / 125% / 150% / 200% 缩放下对应的物理宽度。
        assert_eq!(to_physical(POPUP_W, 1.0), 444.0);
        assert_eq!(to_physical(POPUP_W, 1.25), 555.0);
        assert_eq!(to_physical(POPUP_W, 1.5), 666.0);
        assert_eq!(to_physical(POPUP_W, 2.0), 888.0);
        // 卡片边距也要同比例放大，否则“点外部关闭”会把可见卡片边缘算进去。
        assert_eq!(to_physical(CARD_PAD_X as f64, 1.5), 18.0);
    }

    /// 开回极轻模式时只回收「别人」：设置窗口必须留着，用户正在里面点保存。
    #[test]
    fn prunable_labels_cover_all_but_settings() {
        let labels = prunable_labels();
        assert_eq!(labels.len(), 3);
        assert!(labels.contains(&POPUP_LABEL));
        assert!(labels.contains(&MAIN_LABEL));
        assert!(labels.contains(&NEW_TASK_LABEL));
        assert!(!labels.contains(&SETTINGS_LABEL));
    }
}
