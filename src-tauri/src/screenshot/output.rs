//! S7 截图输出：PNG 编码、存盘、剪贴板、复制路径。
//!
//! **长图不在这里**：滚动截图（会话 62）用 `png::Writer::stream_writer()`
//! 直接逐行写盘，不经过 `encode_png`。本文件只处理单张截图。
//!
//! 图片**不走 IPC base64**（ADR-040）：这里只产出文件路径，前端要预览时用
//! `convertFileSrc` 读文件。剪贴板走 `arboard`，失败必须把原因交给用户。

use crate::config::Config;
use crate::ocr::CapturedImage;
use std::io::Write;
use std::path::{Path, PathBuf};

/// 一次截图交付的结果。`message` 是给用户看的一句话。
#[derive(Debug, Clone)]
pub struct Delivered {
    pub path: Option<PathBuf>,
    pub copied_image: bool,
    pub copied_path: bool,
    pub message: String,
}

/// 系统「图片」文件夹下的 Vexto 子目录。
///
/// 与下载的 `下载\Vexto` 同构。不能直接拼 `%USERPROFILE%\Pictures`：
/// 用户可能把图片文件夹重定向到别的盘（`SHGetKnownFolderPath` 读的是真实路径）。
pub fn default_dir() -> PathBuf {
    #[cfg(windows)]
    {
        use windows::Win32::System::Com::CoTaskMemFree;
        use windows::Win32::UI::Shell::{FOLDERID_Pictures, KF_FLAG_DEFAULT, SHGetKnownFolderPath};

        let got = unsafe { SHGetKnownFolderPath(&FOLDERID_Pictures, KF_FLAG_DEFAULT, None) };
        if let Ok(ptr) = got {
            if !ptr.is_null() {
                let s = unsafe { ptr.to_string() }.unwrap_or_default();
                unsafe { CoTaskMemFree(Some(ptr.0 as *const core::ffi::c_void)) };
                if !s.trim().is_empty() {
                    return PathBuf::from(s).join("Vexto");
                }
            }
        }
    }
    let base = std::env::var("USERPROFILE").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(base).join("Pictures").join("Vexto")
}

/// 这次截图该落到哪个目录：设置 > 系统默认。
pub fn resolve_dir(cfg: &Config) -> PathBuf {
    let s = cfg.screenshot_dir.trim();
    if s.is_empty() {
        default_dir()
    } else {
        PathBuf::from(s)
    }
}

/// BGRA → RGBA。`CapturedImage` 的 alpha 恒 255，这里只交换 B/R。
fn bgra_to_rgba(pixels: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(pixels.len());
    let (chunks, _) = pixels.as_chunks::<4>();
    for px in chunks {
        out.extend_from_slice(&[px[2], px[1], px[0], 255]);
    }
    out
}

/// 逐行写 PNG，避免再分配一整张 RGBA 缓冲。
fn write_png<W: Write>(w: W, img: &CapturedImage) -> Result<(), String> {
    let width = img.width as usize;
    let height = img.height as usize;
    if width == 0 || height == 0 {
        return Err("截图尺寸不合法".into());
    }
    let mut enc = png::Encoder::new(w, width as u32, height as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| format!("写 PNG 头失败：{e}"))?;
    let mut stream = writer
        .stream_writer()
        .map_err(|e| format!("写 PNG 失败：{e}"))?;
    for row in img.pixels.chunks_exact(width * 4) {
        stream
            .write_all(&bgra_to_rgba(row))
            .map_err(|e| format!("写 PNG 失败：{e}"))?;
    }
    stream.finish().map_err(|e| format!("写 PNG 失败：{e}"))?;
    Ok(())
}

/// 存盘并返回最终路径。目录不存在会创建；重名递增 ` (1)`。
pub fn save_png(img: &CapturedImage, dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("创建截图目录失败（{}）：{e}", dir.display()))?;
    let path = unique_path(dir, &file_stem());
    let file = std::fs::File::create(&path)
        .map_err(|e| format!("创建截图文件失败（{}）：{e}", path.display()))?;
    let mut w = std::io::BufWriter::new(file);
    write_png(&mut w, img)?;
    w.flush().map_err(|e| format!("写截图文件失败：{e}"))?;
    Ok(path)
}

fn file_stem() -> String {
    let t = local_time();
    format!(
        "截图_{:04}{:02}{:02}_{:02}{:02}{:02}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
    )
}

fn unique_path(dir: &Path, stem: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.png"));
    if !first.exists() {
        return first;
    }
    for n in 1..10_000 {
        let p = dir.join(format!("{stem} ({n}).png"));
        if !p.exists() {
            return p;
        }
    }
    dir.join(format!("{stem} ({}).png", std::process::id()))
}

/// 长图 / 预览共用的下一个可用 PNG 路径（不覆盖已有文件）。
pub(crate) fn next_png_path(dir: &Path) -> PathBuf {
    unique_path(dir, &file_stem())
}

#[cfg(windows)]
fn local_time() -> windows::Win32::Foundation::SYSTEMTIME {
    unsafe { windows::Win32::System::SystemInformation::GetLocalTime() }
}

/// 写图片到剪贴板（RGBA）。
pub fn copy_image(img: &CapturedImage) -> Result<(), String> {
    let mut cb = arboard::Clipboard::new().map_err(|e| format!("打开剪贴板失败：{e}"))?;
    cb.set_image(arboard::ImageData {
        width: img.width.max(0) as usize,
        height: img.height.max(0) as usize,
        bytes: std::borrow::Cow::Owned(bgra_to_rgba(&img.pixels)),
    })
    .map_err(|e| format!("写剪贴板失败：{e}"))
}

/// 写文本到剪贴板（复制文件路径）。
pub fn copy_path(path: &Path) -> Result<(), String> {
    let mut cb = arboard::Clipboard::new().map_err(|e| format!("打开剪贴板失败：{e}"))?;
    cb.set_text(path.to_string_lossy().to_string())
        .map_err(|e| format!("写剪贴板失败：{e}"))
}

/// 按 `screenshot_after_action` 交付一张截图。
///
/// `both` 先复制图片再存盘：存盘失败时用户至少还有剪贴板里的图，
/// 错误消息里也会说清楚哪一步失败。
pub fn deliver(img: &CapturedImage, cfg: &Config) -> Result<Delivered, String> {
    let action = cfg.screenshot_after_action.trim();
    match action {
        "clipboard" => {
            copy_image(img)?;
            Ok(Delivered {
                path: None,
                copied_image: true,
                copied_path: false,
                message: "已复制到剪贴板".into(),
            })
        }
        "path" => {
            let path = save_png(img, &resolve_dir(cfg))?;
            if let Err(e) = copy_path(&path) {
                return Err(format!("已保存到 {}，但复制路径失败：{e}", path.display()));
            }
            Ok(Delivered {
                message: format!("已保存并复制路径：{}", path.display()),
                path: Some(path),
                copied_image: false,
                copied_path: true,
            })
        }
        "save" => {
            let path = save_png(img, &resolve_dir(cfg))?;
            Ok(Delivered {
                message: format!("已保存：{}", path.display()),
                path: Some(path),
                copied_image: false,
                copied_path: false,
            })
        }
        // 默认 both（也兜住用户手改坏的配置）
        _ => {
            copy_image(img)?;
            match save_png(img, &resolve_dir(cfg)) {
                Ok(path) => Ok(Delivered {
                    message: format!("已复制并保存：{}", path.display()),
                    path: Some(path),
                    copied_image: true,
                    copied_path: false,
                }),
                Err(e) => Err(format!("图片已复制到剪贴板，但保存失败：{e}")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> CapturedImage {
        // 2×2：红、绿、蓝、白（BGRA 顺序）
        CapturedImage {
            width: 2,
            height: 2,
            pixels: vec![
                0, 0, 255, 255, // 红
                0, 255, 0, 255, // 绿
                255, 0, 0, 255, // 蓝
                255, 255, 255, 255, // 白
            ],
        }
    }

    #[test]
    fn png_roundtrip_keeps_size_and_pixels() {
        let img = sample();
        let mut bytes = Vec::new();
        write_png(&mut bytes, &img).unwrap();
        let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        let mut reader = decoder.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!(info.width, 2);
        assert_eq!(info.height, 2);
        // 第一个像素应是红：R=255,G=0,B=0
        assert_eq!(&buf[0..3], &[255, 0, 0]);
    }

    #[test]
    fn unique_path_increments() {
        let dir = std::env::temp_dir().join(format!("vexto-shot-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = unique_path(&dir, "x");
        std::fs::write(&a, b"1").unwrap();
        let b = unique_path(&dir, "x");
        assert_ne!(a, b);
        assert!(b.to_string_lossy().contains("(1)"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 存盘路径 + PNG 可解码。用临时目录，不碰用户真实的「图片」文件夹。
    #[test]
    fn save_png_writes_decodable_file() {
        let dir = std::env::temp_dir().join(format!("vexto-save-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let img = sample();
        let path = save_png(&img, &dir).unwrap();
        assert!(path.exists(), "文件不存在：{}", path.display());
        let bytes = std::fs::read(&path).unwrap();
        let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        let mut reader = decoder.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!((info.width, info.height), (2, 2));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_image_is_rejected() {
        let img = CapturedImage {
            width: 0,
            height: 0,
            pixels: Vec::new(),
        };
        let mut out = Vec::new();
        assert!(write_png(&mut out, &img).is_err());
    }
}
