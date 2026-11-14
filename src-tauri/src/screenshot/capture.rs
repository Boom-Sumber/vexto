//! S7 抓图：区域 / 全屏 / 活动窗口。
//!
//! 区域与全屏都复用 `ocr::capture_region`（S5 已验证的 GDI 路径，含
//! `SelectObject` 修复）；活动窗口优先 `PrintWindow(PW_RENDERFULLCONTENT)`
//! ——被别的窗口挡住也能取到内容，失败或取到全黑时回退到屏幕 BitBlt。
//!
//! 所有函数返回 `CapturedImage`（BGRA、自上而下、alpha=255），
//! 与 S5 OCR 共用同一种像素结构，编码 / 剪贴板 / OCR 都只认这一种。

use crate::ocr::{capture_region, virtual_screen, CapturedImage};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, SelectObject, BITMAPINFO,
    BITMAPINFOHEADER, DIB_RGB_COLORS, HGDIOBJ,
};
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetForegroundWindow, GetTopWindow, GetWindow, GetWindowRect,
    GetWindowThreadProcessId, IsIconic, IsWindowVisible, GW_HWNDNEXT, PW_RENDERFULLCONTENT,
};

/// 全屏：整个虚拟屏幕（多显示器时是一整张，不是只主屏）。
pub fn capture_full_screen() -> Result<CapturedImage, String> {
    let (x, y, w, h) = virtual_screen();
    capture_region(x, y, w, h)
}

/// 活动窗口的最小边长。Z 序里有一堆 1×1 / 几十像素的辅助窗口（输入法、
/// 托盘提示、硬件加速 helper），它们 `IsWindowVisible` 也是 true；不按尺寸
/// 过滤就会截到「一个黑点」（会话 61 真机复测实际踩到，1×1 PNG）。
const MIN_WINDOW_SIDE: i32 = 32;

fn size_ok(w: i32, h: i32) -> bool {
    w >= MIN_WINDOW_SIDE && h >= MIN_WINDOW_SIDE
}

/// 活动窗口：前台窗口的可见边界。
///
/// 最小化窗口没有可见像素，直接给中文原因，不产出黑图（06 §6.4.5 A9）。
pub fn capture_active_window() -> Result<CapturedImage, String> {
    unsafe {
        let hwnd = target_window().ok_or_else(|| "没有可截图的活动窗口".to_string())?;
        let rect = window_bounds(hwnd)?;
        let w = rect.right - rect.left;
        let h = rect.bottom - rect.top;
        if !size_ok(w, h) {
            return Err("活动窗口太小（可能是系统辅助窗口），请换一个窗口再试".into());
        }

        // 先试 PrintWindow：被遮挡的窗口也能取到内容。
        // 有些窗口（部分游戏 / 硬件加速表面）会返回 true 但全黑，此时回退 BitBlt。
        if let Ok(img) = capture_via_print_window(hwnd, w, h) {
            if !looks_blank(&img) {
                return Ok(img);
            }
        }
        capture_region(rect.left, rect.top, w, h)
    }
}

/// 选目标窗口：前台窗口优先；如果前台是我们的设置页 / 托盘 / 桌面，
/// 从 Z 序顶端往下找第一个可见、未最小化、非本进程的顶层窗口。
///
/// 从托盘菜单触发时，前台常常是 `Shell_TrayWnd` 而不是用户想截的应用，
/// 只认 `GetForegroundWindow` 会截到任务栏。
unsafe fn target_window() -> Option<HWND> {
    let fg = GetForegroundWindow();
    if is_capturable(fg) {
        return Some(fg);
    }
    let mut cur = GetTopWindow(None).ok();
    while let Some(h) = cur {
        if is_capturable(h) {
            return Some(h);
        }
        cur = GetWindow(h, GW_HWNDNEXT).ok();
    }
    None
}

unsafe fn is_capturable(hwnd: HWND) -> bool {
    if hwnd.0.is_null() || !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
        return false;
    }
    // 1×1 / 小尺寸辅助窗口不算「活动窗口」：它们可见但截出来就是一个黑点。
    let mut rect = RECT::default();
    if GetWindowRect(hwnd, &mut rect).is_err()
        || !size_ok(rect.right - rect.left, rect.bottom - rect.top)
    {
        return false;
    }
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if pid == GetCurrentProcessId() {
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

/// DWM 扩展边框（去掉不可见阴影）；失败时退回 `GetWindowRect`。
unsafe fn window_bounds(hwnd: HWND) -> Result<RECT, String> {
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
    if rect.right <= rect.left || rect.bottom <= rect.top {
        return Err("活动窗口没有可见区域".into());
    }
    Ok(rect)
}

/// 把窗口内容画进一张自上而下的 32 位 DIB。
unsafe fn capture_via_print_window(hwnd: HWND, w: i32, h: i32) -> Result<CapturedImage, String> {
    if w <= 0 || h <= 0 {
        return Err("窗口尺寸不合法".into());
    }
    let hdc_mem = CreateCompatibleDC(None);
    let mut bmi = BITMAPINFO::default();
    bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = w;
    bmi.bmiHeader.biHeight = -h; // 负高 = 自上而下
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = 0; // BI_RGB

    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    let hbmp = CreateDIBSection(Some(hdc_mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0)
        .map_err(|e| format!("创建窗口截图位图失败：{e}"))?;
    if bits.is_null() {
        let _ = DeleteObject(hbmp.into());
        let _ = DeleteDC(hdc_mem);
        return Err("创建窗口截图位图失败".into());
    }
    let old: HGDIOBJ = SelectObject(hdc_mem, hbmp.into());

    let ok = PrintWindow(hwnd, hdc_mem, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT));
    let mut img = CapturedImage {
        width: w,
        height: h,
        pixels: Vec::new(),
    };
    if ok.as_bool() {
        let len = (w as usize) * (h as usize) * 4;
        let slice = std::slice::from_raw_parts(bits as *const u8, len);
        img.pixels = slice.to_vec();
        // GDI / PrintWindow 不写 alpha：全 0 会被当成全透明，统一拉满。
        for a in img.pixels[3..].iter_mut().step_by(4) {
            *a = 255;
        }
    }

    SelectObject(hdc_mem, old);
    let _ = DeleteObject(hbmp.into());
    let _ = DeleteDC(hdc_mem);

    if img.pixels.is_empty() {
        return Err("PrintWindow 失败".into());
    }
    Ok(img)
}

/// 抽样看是否全黑。PrintWindow 对少数硬件加速表面会「成功但全 0」，
/// 这时要回退屏幕 BitBlt。只看 RGB，alpha 恒 255。
fn looks_blank(img: &CapturedImage) -> bool {
    if img.pixels.is_empty() {
        return true;
    }
    let px = img.pixels.len() / 4;
    let step = (px / 4096).max(1);
    (0..px).step_by(step).all(|i| {
        let o = i * 4;
        img.pixels[o] == 0 && img.pixels[o + 1] == 0 && img.pixels[o + 2] == 0
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bad_rect_is_rejected() {
        assert!(capture_region(0, 0, 0, 10).is_err());
        assert!(capture_region(0, 0, 10, 0).is_err());
        assert!(capture_region(0, 0, -5, 10).is_err());
    }

    /// 全黑判定：全 0 是空白；有一个非 0 像素就不是。
    #[test]
    fn blank_detection() {
        let mut img = CapturedImage {
            width: 2,
            height: 2,
            pixels: vec![0; 16],
        };
        assert!(looks_blank(&img));
        img.pixels[0] = 1;
        assert!(!looks_blank(&img));
    }

    /// 1×1 辅助窗口必须被尺寸过滤掉（真机复测踩过：截出来是一个黑点）。
    #[test]
    fn tiny_windows_are_rejected() {
        assert!(!size_ok(1, 1));
        assert!(!size_ok(31, 200));
        assert!(size_ok(32, 32));
        assert!(size_ok(1920, 1080));
    }
}
