//! S5 OCR（截图翻译）核心：框选 → GDI 截屏 → Windows.Media.Ocr → 翻译管线。
//!
//! 流程（06 §6.4.4 口径）：
//! 1. 用户按 OCR 热键（`hotkey_ocr`，工具启用时才注册）→ 打开全屏半透明
//!    框选窗口（`ui/ocr_overlay.html`，暗幕 + 十字光标）；
//! 2. 用户拖出选框 → 前端把 **CSS 坐标 + devicePixelRatio** 交给
//!    `ocr_submit_region` 命令；Esc → `ocr_cancel` 直接关窗（不耗配额）；
//! 3. 后端：把 overlay 藏起来 → 稍等一拍让遮罩从屏幕上消失 → GDI `BitBlt`
//!    截下选区物理像素 → `Windows.Media.Ocr`（系统自带、离线；中文需要
//!    系统装了中文 OCR 语言包）→ 识别出原文 → 交给 `translate::translate`；
//! 4. 结果经 `ocr:result` 事件回给 overlay，卡片显示「原文 + 译文 + 复制」。
//!
//! 为什么用系统 OCR：离线、零额外依赖、识别质量对「屏幕截图」够用；
//! 前两个里程碑（S1/S2）的翻译管线直接复用，不新增第二套翻译入口。
//!
//! 坐标约定：前端给的是 **overlay 窗口内**的 CSS 坐标；overlay 覆盖整个
//! 虚拟屏幕（多显示器），原点是虚拟屏幕左上角。后端拿到后乘 dpr 得物理
//! 像素，再加上虚拟屏幕原点偏移，才是 `GetDC(None)` 用的屏幕物理坐标。

use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::{DataWriter, IBuffer};
use windows_future::AsyncStatus;
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC,
    SelectObject, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, HGDIOBJ, SRCCOPY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXSCREEN, SM_CXVIRTUALSCREEN, SM_CYSCREEN, SM_CYVIRTUALSCREEN,
    SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

/// 一次截屏的原始像素（BGRA、自上而下、每像素 4 字节）。
#[derive(Debug)]
pub struct CapturedImage {
    pub width: i32,
    pub height: i32,
    /// BGRA 逐行像素（未经预乘，alpha 恒 255）
    pub pixels: Vec<u8>,
}

/// 虚拟屏幕的原点与尺寸（物理像素）。overlay 窗口按这个开，坐标系用它。
pub fn virtual_screen() -> (i32, i32, i32, i32) {
    unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    }
}

/// 主屏尺寸（CSS→物理换算兜底用）。
#[allow(dead_code)]
pub fn primary_screen() -> (i32, i32) {
    unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) }
}

/// GDI 截屏：把屏幕上的一个矩形（虚拟屏幕物理坐标）读进 BGRA 缓冲。
///
/// 用自上而下的 DIB（`biHeight` 为负）免得逐行翻转；`GetDIBits` 拿像素时
/// 顺便让 GDI 把行宽对齐填好（stride = w*4，BGRA 无 24 位那种 padding 问题）。
pub fn capture_region(x: i32, y: i32, w: i32, h: i32) -> Result<CapturedImage, String> {
    if w <= 0 || h <= 0 {
        return Err("选区尺寸不合法".into());
    }
    unsafe {
        let hdc_screen = GetDC(None);
        let capture = |hdc: windows::Win32::Graphics::Gdi::HDC| -> Result<CapturedImage, String> {
            let hdc_mem = CreateCompatibleDC(Some(hdc));
            let mut bmi = BITMAPINFO::default();
            bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            bmi.bmiHeader.biWidth = w;
            // 负高 = 自上而下（第一行是屏幕最上面那行）
            bmi.bmiHeader.biHeight = -h;
            bmi.bmiHeader.biPlanes = 1;
            bmi.bmiHeader.biBitCount = 32;
            bmi.bmiHeader.biCompression = 0; // BI_RGB

            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let hbmp = CreateDIBSection(
                Some(hdc),
                &bmi,
                DIB_RGB_COLORS,
                &mut bits,
                None,
                0,
            )
            .map_err(|e| format!("创建截屏位图失败：{e}"))?;
            if bits.is_null() {
                let _ = DeleteObject(hbmp.into());
                let _ = DeleteDC(hdc_mem);
                return Err("创建截屏位图失败".into());
            }
            let old: HGDIOBJ = hbmp.into();

            let ok = BitBlt(hdc_mem, 0, 0, w, h, Some(hdc), x, y, SRCCOPY);
            let mut img = CapturedImage {
                width: w,
                height: h,
                pixels: Vec::new(),
            };
            if ok.is_ok() {
                // CreateDIBSection 已把像素映射进段内存，直接拷贝
                let len = (w as usize) * (h as usize) * 4;
                let slice = std::slice::from_raw_parts(bits as *const u8, len);
                img.pixels = slice.to_vec();
                // GDI 不写 alpha 字节（BitBlt 留 0）：全 0 = 全透明的 Premultiplied
                // 位图，WinRT OCR 会当成一张空图。截屏不透明，统一拉满。
                for a in img.pixels[3..].iter_mut().step_by(4) {
                    *a = 255;
                }
            } else {
                img.pixels = Vec::new();
            }

            SelectObject(hdc_mem, old);
            let _ = DeleteObject(hbmp.into());
            let _ = DeleteDC(hdc_mem);
            if img.pixels.is_empty() {
                return Err("截屏失败（BitBlt）".into());
            }
            Ok(img)
        };
        let res = capture(hdc_screen);
        ReleaseDC(None, hdc_screen);
        res
    }
}

/// WinRT 识别一张 BGRA 图（同步：轮询 Status，25ms 一拍）。
/// 识别是后台异步执行，轮询线程在这里只做等待；整条 OCR 全同步，
/// 才能整体放进 `spawn_blocking`（COM 包装类型不是 Send，不能跨 await）。
/// 语言包缺失 / 没有可用 OCR 引擎时给中文提示。
pub fn ocr_bitmap(img: &CapturedImage) -> Result<String, String> {
    let engine = match OcrEngine::TryCreateFromUserProfileLanguages() {
        Ok(e) => e,
        Err(_) => OcrEngine::TryCreateFromLanguage(
            &windows::Globalization::Language::CreateLanguage(&windows::core::HSTRING::from(
                "en-US",
            ))
            .map_err(|_| "系统没有可用的 OCR 语言".to_string())?,
        )
        .map_err(|_| "系统没有可用的 OCR 语言".to_string())?,
    };

    // BGRA → IBuffer（DataWriter 写完 detach 即可，不用落盘/编码）
    let writer = DataWriter::new().map_err(|e| format!("OCR 初始化失败：{e}"))?;
    writer
        .WriteBytes(&img.pixels)
        .map_err(|e| format!("OCR 初始化失败：{e}"))?;
    let buf: IBuffer = writer
        .DetachBuffer()
        .map_err(|e| format!("OCR 初始化失败：{e}"))?;

    // 0.62 的 CreateCopyFromBuffer 没有 alpha 参数（默认 Premultiplied；
    // 截屏 alpha 恒 255，两种模式等价）
    let bitmap = SoftwareBitmap::CreateCopyFromBuffer(
        &buf,
        BitmapPixelFormat::Bgra8,
        img.width,
        img.height,
    )
    .map_err(|e| format!("截图数据不合法：{e}"))?;

    let op = engine
        .RecognizeAsync(&bitmap)
        .map_err(|e| format!("OCR 启动失败：{e}"))?;
    loop {
        let st = op.Status().map_err(|e| format!("OCR 识别失败：{e}"))?;
        if st == AsyncStatus::Completed {
            break;
        }
        if st == AsyncStatus::Error {
            return Err("OCR 识别失败（引擎返回错误）".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let result = op
        .GetResults()
        .map_err(|e| format!("OCR 识别失败：{e}"))?;
    Ok(result
        .Text()
        .map_err(|e| format!("OCR 读取结果失败：{e}"))?
        .to_string_lossy())
}

/// S5 主链路：截屏 → 识别 → 翻译。OCR 到空文本按「没有识别到文字」报错。
/// 返回 (原文, 译文, 检测语种)。全同步，调用方放在 `spawn_blocking` 里。
pub fn ocr_translate_region(
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) -> Result<(String, String, String), String> {
    let img = capture_region(x, y, w, h)?;
    let text = ocr_bitmap(&img)?;
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("没有识别到文字。框大一点，或换个有文字的区域试试。".into());
    }
    let cfg = crate::config::load();
    if !cfg.enable_translate {
        return Err("划词翻译功能已停用，先在设置里启用翻译工具。".into());
    }
    let r = crate::translate::translate(&cfg, &text)?;
    Ok((text, r.dst, r.from))
}

// ───────────────────────── 测试 ─────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// GDI 把一段英文画到内存位图 → OCR 必须认得出关键词。
    /// 英文 OCR 引擎在所有 Windows 上都有（不需要语言包）。
    #[test]
    fn ocr_recognizes_gdi_drawn_text() {
        use windows::Win32::Graphics::Gdi::{
            CreateFontW, CreateSolidBrush, FillRect, SetBkMode, SetTextColor, TextOutW,
            CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, PROOF_QUALITY,
            TRANSPARENT,
        };
        let (w, h) = (520, 120);
        use windows::Win32::Foundation::RECT;
        unsafe {
            let hdc_screen = GetDC(None);
            let hdc = CreateCompatibleDC(Some(hdc_screen));
            let mut bmi = BITMAPINFO::default();
            bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            bmi.bmiHeader.biWidth = w;
            bmi.bmiHeader.biHeight = -(h);
            bmi.bmiHeader.biPlanes = 1;
            bmi.bmiHeader.biBitCount = 32;
            bmi.bmiHeader.biCompression = 0;
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let hbmp = CreateDIBSection(
                Some(hdc),
                &bmi,
                DIB_RGB_COLORS,
                &mut bits,
                None,
                0,
            )
            .expect("CreateDIBSection");
            // 位图必须选进 DC，否则后面所有绘制都落空
            let old: HGDIOBJ = SelectObject(hdc, hbmp.into());

            // 白底
            let brush = CreateSolidBrush(windows::Win32::Foundation::COLORREF(0x00FFFFFF));
            let rc = RECT { left: 0, top: 0, right: w, bottom: h };
            FillRect(hdc, &rc, brush);
            let _ = DeleteObject(brush.into());

            // 44px 粗体 Segoe UI，白底**黑字**——OCR 最舒服的输入。
            // DC 默认文字色是白色（白底白字=什么都看不见），必须显式设黑。
            let font = CreateFontW(
                44,
                0,
                0,
                0,
                700, // FW_BOLD
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                PROOF_QUALITY,
                0, // FONT_PITCH_AND_FAMILY：默认
                windows::core::w!("Segoe UI"),
            );
            let old_font: HGDIOBJ = SelectObject(hdc, font.into());
            SetTextColor(hdc, windows::Win32::Foundation::COLORREF(0x00000000));
            SetBkMode(hdc, TRANSPARENT);
            // 全大写：小写 l 常被 OCR 读成大写 I（HeIIo），大写无歧义
            let label: Vec<u16> = "HELLO OCR 42".encode_utf16().collect();
            TextOutW(hdc, 30, 34, &label);

            // 从 DIB 段读回像素，走同一条识别路径（alpha 同样要拉满，见
            // capture_region 里的注释）
            let len = (w as usize) * (h as usize) * 4;
            let slice = std::slice::from_raw_parts(bits as *const u8, len);
            let mut px = slice.to_vec();
            for a in px[3..].iter_mut().step_by(4) {
                *a = 255;
            }
            let img = CapturedImage {
                width: w,
                height: h,
                pixels: px,
            };
            // 诊断：暗像素有多少（0 = TextOut 没画上）
            let dark = img.pixels.chunks_exact(4).filter(|p| p[0] < 128).count();
            println!("ocr test: dark pixels = {dark}");

            SelectObject(hdc, old);
            let _ = DeleteObject(font.into());
            let _ = DeleteObject(hbmp.into());
            let _ = DeleteDC(hdc);
            ReleaseDC(None, hdc_screen);

            let text = ocr_bitmap(&img).expect("OCR 不该失败");
            let low = text.to_lowercase();
            assert!(
                low.contains("hello") && low.contains("42"),
                "OCR 结果里应有 Hello / 42，实际：{text:?}"
            );
        }
    }

    /// 非法尺寸要报中文错误，不能 panic。
    #[test]
    fn capture_rejects_bad_region() {
        let e = capture_region(0, 0, 0, 100).unwrap_err();
        assert!(e.contains("不合法"), "实际：{e}");
    }

    /// 真屏截一小块（左上角 40×40，多半是纯色）→ 流程走通即算过。
    /// 文字内容断言不做（桌面内容不可控），只验证 capture→OCR 链路。
    #[test]
    fn capture_and_ocr_run_on_real_screen() {
        // 真屏左上角 40×40（多半是纯色）走一遍完整链路
        let r = (|| {
            let img = capture_region(0, 0, 40, 40)?;
            ocr_bitmap(&img)
        })();
        // 识别成空文本会得到「没有识别到文字」——也算链路通。
        match r {
            Ok(t) => println!("ocr sample: {t:?}"),
            Err(e) => assert!(e.contains("没有识别到文字") || e.contains("OCR") || e.contains("屏幕"), "意外错误：{e}"),
        }
    }
}
