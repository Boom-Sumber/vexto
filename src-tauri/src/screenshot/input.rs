//! 滚动动作（可插拔）：滚动消息 / 滚轮 / 方向键 / PageDown / 手动。
//!
//! 不同程序对滚动消息的反应差异很大（Chrome 的渲染子窗口只认滚轮，
//! 资源管理器认 `WM_VSCROLL`，Office 两者都认）。主循环只调这里，
//! 失败时的回退策略由 `scroll.rs` 决定。
//!
//! **不改用户焦点**：全部用 `PostMessageW` 投递，不抢前台、不移动鼠标。

use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Input::KeyboardAndMouse::{VK_DOWN, VK_NEXT, VIRTUAL_KEY};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetParent, GetScrollInfo, GetTopWindow, GetWindow, GetWindowRect,
    GetWindowThreadProcessId, IsIconic, IsWindowVisible, PostMessageW, WindowFromPoint,
    GW_HWNDNEXT, SCROLLINFO, SIF_ALL, SB_LINEDOWN, SB_VERT, WM_KEYDOWN, WM_KEYUP, WM_MOUSEWHEEL,
    WM_VSCROLL,
};

/// 滚轮一格。向下的增量是负数（与 Win32 的 `WHEEL_DELTA` 约定一致）。
const WHEEL_DELTA: i32 = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Message,
    Wheel,
    Key,
    PageDown,
    Manual,
}

impl Action {
    /// 配置里的 `scroll_action`；未知值按默认的滚动消息处理。
    pub fn parse(s: &str) -> Action {
        match s.trim().to_ascii_lowercase().as_str() {
            "wheel" => Action::Wheel,
            "key" => Action::Key,
            "pagedown" => Action::PageDown,
            "manual" => Action::Manual,
            _ => Action::Message,
        }
    }

    /// 发完动作后给目标程序留多少时间渲染。
    ///
    /// Chromium 的滚动是**平滑动画**（约 250–350ms）：等太短会抓到中间帧，
    /// 相邻帧只差几像素，拼出来的图会错位（会话 63 真机实测：160ms 时
    /// 100 帧只拼出 400 多像素且接缝错乱）。这里按动画走完来等。
    pub fn settle_ms(self) -> u64 {
        match self {
            Action::Wheel => 450,
            Action::Message => 400,
            Action::Key | Action::PageDown => 400,
            Action::Manual => 0,
        }
    }
}

/// 目标窗口句柄。存成 `isize` 才能跨线程传给抓帧线程。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target(isize);

impl Target {
    fn hwnd(self) -> HWND {
        HWND(self.0 as *mut core::ffi::c_void)
    }
}

/// 取选区中心点下的窗口。**必须在框选浮层销毁之后调用**——
/// 浮层还盖着的话 `WindowFromPoint` 只会拿到浮层自己。
///
/// 先拿最深的子窗口（Chrome 的渲染子窗口才认滚轮），拿不到再按 Z 序
/// 找第一个包含该点、可见、未最小化、非本进程的顶层窗口。
pub fn target_at(x: i32, y: i32) -> Option<Target> {
    unsafe {
        let point = POINT { x, y };
        let hit = WindowFromPoint(point);
        if !hit.0.is_null() && usable(hit, x, y) {
            return Some(Target(hit.0 as isize));
        }
        let mut cur = GetTopWindow(None).ok();
        while let Some(h) = cur {
            if usable(h, x, y) {
                return Some(Target(h.0 as isize));
            }
            cur = GetWindow(h, GW_HWNDNEXT).ok();
        }
        None
    }
}

/// 可见、未最小化、不是本进程、不是任务栏 / 桌面，且确实包含该点。
unsafe fn usable(hwnd: HWND, x: i32, y: i32) -> bool {
    if hwnd.0.is_null() || !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
        return false;
    }
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if pid == GetCurrentProcessId() {
        return false;
    }
    let mut rect = windows::Win32::Foundation::RECT::default();
    if GetWindowRect(hwnd, &mut rect).is_err()
        || x < rect.left
        || x >= rect.right
        || y < rect.top
        || y >= rect.bottom
    {
        return false;
    }
    // 1×1 / 几十像素的辅助窗口不可能是用户要滚的窗口（坑 10 同族）。
    if rect.right - rect.left < 32 || rect.bottom - rect.top < 32 {
        return false;
    }
    let mut buf = [0u16; 64];
    let n = GetClassNameW(hwnd, &mut buf);
    let class = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
    !matches!(
        class.as_str(),
        "Shell_TrayWnd" | "Shell_SecondaryTrayWnd" | "Progman" | "WorkerW"
    )
}

/// 发一次滚动动作。返回是否真的投递出去（手动模式恒为 false）。
pub fn scroll(target: Target, action: Action, x: i32, y: i32) -> bool {
    if action == Action::Manual {
        return false;
    }
    let hwnd = target.hwnd();
    unsafe {
        match action {
            Action::Message => PostMessageW(
                Some(hwnd),
                WM_VSCROLL,
                WPARAM(SB_LINEDOWN.0 as usize),
                LPARAM(0),
            )
            .is_ok(),
            Action::Wheel => PostMessageW(
                Some(hwnd),
                WM_MOUSEWHEEL,
                WPARAM(wheel_wparam()),
                LPARAM(wheel_lparam(x, y) as isize),
            )
            .is_ok(),
            Action::Key => post_key(hwnd, VK_DOWN),
            Action::PageDown => post_key(hwnd, VK_NEXT),
            Action::Manual => false,
        }
    }
}

fn wheel_wparam() -> usize {
    ((-WHEEL_DELTA) << 16) as u32 as usize
}

/// `WM_MOUSEWHEEL` 的 lParam：低 16 位 x、高 16 位 y（屏幕坐标）。
fn wheel_lparam(x: i32, y: i32) -> u32 {
    (x as u16 as u32) | ((y as u16 as u32) << 16)
}

unsafe fn post_key(hwnd: HWND, vk: VIRTUAL_KEY) -> bool {
    let down = PostMessageW(Some(hwnd), WM_KEYDOWN, WPARAM(vk.0 as usize), LPARAM(0)).is_ok();
    let up = PostMessageW(Some(hwnd), WM_KEYUP, WPARAM(vk.0 as usize), LPARAM(0)).is_ok();
    down && up
}

/// 目标窗口（或它最近的父窗口）是否已经滚到底。
///
/// 返回 `None` 表示这个窗口没有暴露滚动信息（Chrome 的合成器滚动就是这样），
/// 调用方只能按「无法确认是否到底」保守处理，不许把它当成成功。
pub fn at_bottom(target: Target) -> Option<bool> {
    let mut hwnd = Some(target.hwnd());
    for _ in 0..8 {
        let h = hwnd?;
        let mut info = SCROLLINFO {
            cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
            fMask: SIF_ALL,
            ..Default::default()
        };
        if unsafe { GetScrollInfo(h, SB_VERT, &mut info) }.is_ok() && info.nMax > info.nMin {
            return Some(is_bottom(info.nPos, info.nPage, info.nMax));
        }
        hwnd = unsafe { GetParent(h) }.ok().filter(|p| !p.0.is_null());
    }
    None
}

/// Win32 滚动条到底的条件：`nPos + nPage >= nMax`。
///
/// 不能写成 `nPos >= nMax - 1`：`nMax` 是内容末端的坐标，`nPage` 是可视页高，
/// 底部时 `nPos` 只到 `nMax - nPage` 附近，按前者判断永远判不出到底
/// （会话 62 自测：Notepad 底部 nPos=45002 / nPage=709 / nMax=45711）。
fn is_bottom(n_pos: i32, n_page: u32, n_max: i32) -> bool {
    n_pos as i64 + n_page.max(1) as i64 >= n_max as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_parse_defaults_to_message() {
        assert_eq!(Action::parse("message"), Action::Message);
        assert_eq!(Action::parse("wheel"), Action::Wheel);
        assert_eq!(Action::parse("Key"), Action::Key);
        assert_eq!(Action::parse("pagedown"), Action::PageDown);
        assert_eq!(Action::parse("manual"), Action::Manual);
        assert_eq!(Action::parse(""), Action::Message);
        assert_eq!(Action::parse("手改坏的值"), Action::Message);
    }

    /// 滚轮向下：wParam 高 16 位是 -120 的补码。
    #[test]
    fn wheel_delta_points_down() {
        assert_eq!(wheel_wparam() >> 16, 0xFF88);
    }

    /// 屏幕坐标必须原样落在 lParam 的低 / 高 16 位。
    #[test]
    fn wheel_lparam_packs_screen_point() {
        assert_eq!(wheel_lparam(100, 200), 100 | (200 << 16));
        // 副屏负坐标按 16 位截断
        assert_eq!(wheel_lparam(-10, -20), 0xFFF6 | (0xFFEC << 16));
    }

    /// 到底判定：必须把可视页高算进去（会话 62 真机实测的数字）。
    #[test]
    fn bottom_needs_page_height() {
        assert!(is_bottom(45_002, 709, 45_711));
        assert!(!is_bottom(9_326, 709, 45_711));
        // 内容比一页还短：一开始就在底部
        assert!(is_bottom(0, 709, 600));
        // 没有页高信息时按 1 行算
        assert!(is_bottom(45_711, 0, 45_711));
        assert!(!is_bottom(45_709, 0, 45_711));
    }
}
