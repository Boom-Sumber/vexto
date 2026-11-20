//! 区域截图时的窗口检测：枚举顶层窗口、命中当前光标下的窗口、区分客户区/子控件。
//!
//! 浮层自己盖住整个屏幕，`WindowFromPoint` 最先拿到的是浮层；所以命中检测
//! 以缓存的 `EnumWindows` 矩形列表为主，`WindowFromPoint + GetAncestor(GA_ROOT)`
//! 只在光标下的窗口不属于本进程时作为更精确的补充。
//!
//! 所有坐标都是屏幕物理像素；前端按快照尺寸换算成 CSS 坐标。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, TRUE};
use windows::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS,
};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, EnumWindows, GetAncestor, GetClassNameW, GetClientRect, GetCursorPos,
    GetWindowLongPtrW, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsIconic,
    IsWindowVisible, WindowFromPoint, GA_ROOT, GWL_EXSTYLE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};

const CACHE_TTL: Duration = Duration::from_millis(650);
const CHILD_CACHE_TTL: Duration = Duration::from_millis(500);
/// 1×1 辅助窗口、输入法候选框之类不是用户想选的目标。
const MIN_WINDOW_SIDE: i32 = 8;

#[derive(Clone)]
struct Entry {
    hwnd: isize,
    rect: RECT,
    client: RECT,
    title: String,
    class: String,
}

struct Cache {
    at: Instant,
    entries: Arc<Vec<Entry>>,
}

static CACHE: Mutex<Option<Cache>> = Mutex::new(None);

#[derive(Clone)]
struct ChildEntry {
    rect: RECT,
    title: String,
    class: String,
}

struct ChildCache {
    root: isize,
    at: Instant,
    children: Arc<Vec<ChildEntry>>,
}

static CHILD_CACHE: Mutex<Option<ChildCache>> = Mutex::new(None);

/// 系统覆盖层 / 桌面 / 按钮类窗口不作为可选目标。
fn ignored_class(class: &str) -> bool {
    matches!(
        class,
        "Progman"
            | "WorkerW"
            | "CEF-OSC-WIDGET"
            | "Button"
            | "Shell_TrayWnd"
            | "Shell_SecondaryTrayWnd"
            | "Windows.UI.Core.CoreWindow"
    )
}

fn window_text(hwnd: HWND) -> String {
    let mut buf = [0u16; 512];
    let n = unsafe { GetWindowTextW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

fn rect_contains(rect: RECT, x: i32, y: i32) -> bool {
    x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom
}

fn rect_area(rect: RECT) -> i64 {
    let w = (rect.right - rect.left).max(0) as i64;
    let h = (rect.bottom - rect.top).max(0) as i64;
    w * h
}

fn intersect(a: RECT, b: RECT) -> RECT {
    RECT {
        left: a.left.max(b.left),
        top: a.top.max(b.top),
        right: a.right.min(b.right),
        bottom: a.bottom.min(b.bottom),
    }
}

unsafe fn window_rect(hwnd: HWND) -> Option<RECT> {
    let mut rect = RECT::default();
    let dwm = DwmGetWindowAttribute(
        hwnd,
        DWMWA_EXTENDED_FRAME_BOUNDS,
        &mut rect as *mut _ as *mut core::ffi::c_void,
        std::mem::size_of::<RECT>() as u32,
    );
    if dwm.is_err() || rect.right <= rect.left || rect.bottom <= rect.top {
        let _ = GetWindowRect(hwnd, &mut rect);
    }
    (rect.right > rect.left && rect.bottom > rect.top).then_some(rect)
}

unsafe fn client_rect(hwnd: HWND) -> Option<RECT> {
    let mut client = RECT::default();
    if GetClientRect(hwnd, &mut client).is_err()
        || client.right <= 0
        || client.bottom <= 0
    {
        return None;
    }
    let mut tl = POINT { x: 0, y: 0 };
    let mut br = POINT {
        x: client.right,
        y: client.bottom,
    };
    if !ClientToScreen(hwnd, &mut tl).as_bool() || !ClientToScreen(hwnd, &mut br).as_bool() {
        return None;
    }
    Some(RECT {
        left: tl.x,
        top: tl.y,
        right: br.x,
        bottom: br.y,
    })
}

unsafe fn inspect(hwnd: HWND) -> Option<Entry> {
    if hwnd.0.is_null() || !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
        return None;
    }
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if pid == GetCurrentProcessId() {
        return None;
    }
    let class = class_name(hwnd);
    if ignored_class(&class) {
        return None;
    }
    let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
    if ex & WS_EX_TOOLWINDOW.0 != 0 || ex & WS_EX_NOACTIVATE.0 != 0 {
        return None;
    }
    let mut cloaked = 0u32;
    if DwmGetWindowAttribute(
        hwnd,
        DWMWA_CLOAKED,
        &mut cloaked as *mut _ as *mut core::ffi::c_void,
        std::mem::size_of::<u32>() as u32,
    )
    .is_ok()
        && cloaked != 0
    {
        return None;
    }
    let rect = window_rect(hwnd)?;
    if rect.right - rect.left < MIN_WINDOW_SIDE
        || rect.bottom - rect.top < MIN_WINDOW_SIDE
    {
        return None;
    }
    let client = client_rect(hwnd).unwrap_or(rect);
    Some(Entry {
        hwnd: hwnd.0 as isize,
        rect,
        client,
        title: window_text(hwnd),
        class,
    })
}

unsafe extern "system" fn enum_top_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let out = &mut *(lparam.0 as *mut Vec<Entry>);
    if let Some(entry) = inspect(hwnd) {
        out.push(entry);
    }
    TRUE
}

fn enumerate_top() -> Vec<Entry> {
    let mut out = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(enum_top_cb), LPARAM(&mut out as *mut _ as isize));
    }
    out
}

fn snapshot() -> Arc<Vec<Entry>> {
    let now = Instant::now();
    if let Some(cache) = CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .filter(|c| now.duration_since(c.at) < CACHE_TTL)
    {
        return cache.entries.clone();
    }
    let entries = Arc::new(enumerate_top());
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Cache {
        at: now,
        entries: entries.clone(),
    });
    entries
}

struct ChildCtx {
    parent: RECT,
    children: Vec<ChildEntry>,
}

unsafe extern "system" fn enum_child_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let ctx = &mut *(lparam.0 as *mut ChildCtx);
    if hwnd.0.is_null() || !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
        return TRUE;
    }
    let class = class_name(hwnd);
    if ignored_class(&class) {
        return TRUE;
    }
    let mut rect = RECT::default();
    if GetWindowRect(hwnd, &mut rect).is_err() {
        return TRUE;
    }
    let clipped = intersect(rect, ctx.parent);
    if clipped.right - clipped.left < 4 || clipped.bottom - clipped.top < 4 {
        return TRUE;
    }
    ctx.children.push(ChildEntry {
        rect: clipped,
        title: window_text(hwnd),
        class,
    });
    TRUE
}

fn children_of(entry: &Entry) -> Arc<Vec<ChildEntry>> {
    let now = Instant::now();
    if let Some(cache) = CHILD_CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .filter(|c| c.root == entry.hwnd && now.duration_since(c.at) < CHILD_CACHE_TTL)
    {
        return cache.children.clone();
    }
    let mut ctx = ChildCtx {
        parent: entry.rect,
        children: Vec::new(),
    };
    unsafe {
        let _ = EnumChildWindows(
            Some(HWND(entry.hwnd as *mut core::ffi::c_void)),
            Some(enum_child_cb),
            LPARAM(&mut ctx as *mut _ as isize),
        );
    }
    let children = Arc::new(ctx.children);
    *CHILD_CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some(ChildCache {
        root: entry.hwnd,
        at: now,
        children: children.clone(),
    });
    children
}

fn child_at(entry: &Entry, x: i32, y: i32) -> Option<ChildEntry> {
    let children = children_of(entry);
    children
        .iter()
        .filter(|child| rect_contains(child.rect, x, y))
        .min_by_key(|child| rect_area(child.rect))
        .cloned()
}

fn rect_json(rect: RECT) -> Value {
    json!({
        "x": rect.left,
        "y": rect.top,
        "w": rect.right - rect.left,
        "h": rect.bottom - rect.top,
    })
}

fn hit_json(entry: &Entry, kind: &str, child: Option<&ChildEntry>) -> Value {
    let mut value = json!({
        "ok": true,
        "kind": kind,
        "title": entry.title,
        "class": entry.class,
        "rect": rect_json(entry.rect),
        "client": rect_json(entry.client),
    });
    if let Some(child) = child {
        value["child"] = json!({
            "kind": "child",
            "title": child.title,
            "class": child.class,
            "rect": rect_json(child.rect),
        });
    }
    value
}

fn pick_at(x: i32, y: i32) -> Option<Value> {
    let entries = snapshot();
    let mut hit: Option<Entry> = None;

    // 浮层通常在最上面，WindowFromPoint 会先拿到本进程窗口；只有拿到外部窗口时
    // 才用它（能反映实时的 Z 序）。否则按 EnumWindows 的 Z 序缓存找第一个包含点。
    unsafe {
        let under = WindowFromPoint(POINT { x, y });
        if !under.0.is_null() {
            let mut pid = 0u32;
            GetWindowThreadProcessId(under, Some(&mut pid));
            if pid != GetCurrentProcessId() {
                let root = GetAncestor(under, GA_ROOT);
                if !root.0.is_null() {
                    hit = entries
                        .iter()
                        .find(|e| e.hwnd == root.0 as isize)
                        .cloned()
                        .or_else(|| inspect(root));
                }
            }
        }
    }

    let entry = hit.or_else(|| {
        entries
            .iter()
            .find(|e| rect_contains(e.rect, x, y))
            .cloned()
    })?;
    if !rect_contains(entry.rect, x, y) {
        return None;
    }

    let child = child_at(&entry, x, y);
    let kind = if child.is_some() {
        "child"
    } else if rect_contains(entry.client, x, y) {
        "client"
    } else {
        "window"
    };
    Some(hit_json(&entry, kind, child.as_ref()))
}

/// 读取当前鼠标位置并返回窗口高亮信息；没有合适窗口时返回 `{"ok":false}`。
pub fn pick_at_cursor() -> Value {
    let mut point = POINT::default();
    unsafe {
        if GetCursorPos(&mut point).is_err() {
            return json!({ "ok": false });
        }
    }
    pick_at(point.x, point.y).unwrap_or_else(|| json!({ "ok": false }))
}

/// 浮层销毁时清掉缓存，避免下一次截图拿到过期窗口。
pub fn clear_cache() {
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
    *CHILD_CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignored_classes_cover_desktop_and_system_overlays() {
        assert!(ignored_class("Progman"));
        assert!(ignored_class("WorkerW"));
        assert!(ignored_class("CEF-OSC-WIDGET"));
        assert!(ignored_class("Button"));
        assert!(ignored_class("Shell_TrayWnd"));
        assert!(!ignored_class("Notepad"));
        assert!(!ignored_class("Chrome_WidgetWin_1"));
    }

    #[test]
    fn rect_helpers() {
        let r = RECT {
            left: 10,
            top: 20,
            right: 110,
            bottom: 220,
        };
        assert!(rect_contains(r, 10, 20));
        assert!(rect_contains(r, 109, 219));
        assert!(!rect_contains(r, 110, 220));
        assert_eq!(rect_area(r), 20_000);
        let cut = intersect(
            r,
            RECT {
                left: 50,
                top: 0,
                right: 200,
                bottom: 100,
            },
        );
        assert_eq!(
            (cut.left, cut.top, cut.right, cut.bottom),
            (50, 20, 110, 100)
        );
    }
}
