//! 截图预览与钉屏共用的图片暂存 / 裁切 / 交付。
//!
//! **图片不进 IPC**（ADR-040）：钉屏与预览只把「显示 URL」交给前端，页面用
//! 自定义协议 `vextoimg://` 读暂存文件（ADR-044）。Tauri 的 asset protocol
//! 需要额外 crate（`http-range`），与「S7 一期零新 crate 下载」冲突，所以
//! 自己注册一个只读暂存目录的 URI scheme。
//!
//! **裁切流式处理**：长图裁头尾不允许整图进内存。解码器逐行读、编码器
//! `stream_writer()` 逐行写，内存只有一行缓冲。OCR / 剪贴板需要像素缓冲，
//! 这时按 `MAX_DECODE_PIXELS` 设上限并给中文原因。

use crate::ocr::CapturedImage;
use crate::screenshot::output;
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// OCR / 剪贴板这类需要整张像素的功能，最多解这么多像素（约 64 MB BGRA）。
pub const MAX_DECODE_PIXELS: u64 = 16_000_000;

/// 预览窗口当前显示的暂存文件。
static PREVIEW_SOURCE: Mutex<Option<PathBuf>> = Mutex::new(None);

pub fn set_source(path: PathBuf) {
    *PREVIEW_SOURCE.lock().unwrap_or_else(|e| e.into_inner()) = Some(path);
}

pub fn source() -> Option<PathBuf> {
    PREVIEW_SOURCE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

pub fn clear_source() -> Option<PathBuf> {
    PREVIEW_SOURCE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
}

/// 给预览页面拉取的状态。只给显示 URL，不把本机路径交给 WebView。
pub fn state_value() -> serde_json::Value {
    let Some(path) = source() else {
        return serde_json::json!({ "url": null, "width": 0, "height": 0 });
    };
    match image_size(&path) {
        Ok((w, h)) => serde_json::json!({
            "url": display_url(&path),
            "width": w,
            "height": h,
        }),
        Err(_) => serde_json::json!({ "url": null, "width": 0, "height": 0 }),
    }
}

/// 预览页面加载 / 重新打开时推送状态。
pub fn emit_state(win: &tauri::WebviewWindow) {
    use tauri::Emitter;
    let _ = win.emit("preview:state", state_value());
}

/// 暂存目录：与配置同一个数据目录（ADR-038）。
pub fn stage_dir() -> PathBuf {
    crate::config::data_dir().join("stage")
}

fn file_key(src: &Path) -> Result<u64, String> {
    use std::hash::{Hash, Hasher};
    let meta = std::fs::metadata(src)
        .map_err(|e| format!("找不到图片文件（{}）：{e}", src.display()))?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    src.to_string_lossy().hash(&mut h);
    meta.len().hash(&mut h);
    if let Ok(t) = meta.modified() {
        if let Ok(d) = t.duration_since(std::time::UNIX_EPOCH) {
            d.as_nanos().hash(&mut h);
        }
    }
    Ok(h.finish())
}

/// 把源图片放到暂存目录。
///
/// 优先硬链接（同盘不占第二份空间），失败再复制。同名文件已存在就直接复用
/// ——名字由「绝对路径 + 大小 + 修改时间」哈希得来，内容一致。
pub fn stage_file(src: &Path) -> Result<PathBuf, String> {
    let src = src
        .canonicalize()
        .map_err(|e| format!("找不到图片文件（{}）：{e}", src.display()))?;
    let dir = stage_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建暂存目录失败：{e}"))?;
    let dst = dir.join(format!("{:016x}.png", file_key(&src)?));
    if !dst.exists() && std::fs::hard_link(&src, &dst).is_err() {
        std::fs::copy(&src, &dst).map_err(|e| format!("暂存图片失败：{e}"))?;
    }
    Ok(dst)
}

/// 把整张抓图编码进暂存目录（全屏 / 活动窗口结果），返回可被 `vextoimg`
/// 显示的安全文件名。图片在本进程内存里，调用方负责只传一次。
pub fn stage_image(img: &CapturedImage) -> Result<PathBuf, String> {
    let dir = stage_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建暂存目录失败：{e}"))?;
    let tmp = output::save_png(img, &dir)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dst = dir.join(format!("full-{stamp}.png"));
    if let Err(e) = std::fs::rename(&tmp, &dst) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("暂存全屏截图失败：{e}"));
    }
    Ok(dst)
}

/// 前端可用的显示 URL。只暴露暂存文件名，不暴露本机路径。
pub fn display_url(staged: &Path) -> Option<String> {
    let name = staged.file_name()?.to_str()?;
    let safe = name.ends_with(".png")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    safe.then(|| format!("http://vextoimg.localhost/{name}"))
}

/// `vextoimg` 协议的处理函数：只返回暂存目录里的 PNG。
///
/// 这是 ADR-044 的落点：不引 `protocol-asset` 的 `http-range`，也不让前端
/// 传任意路径——请求里的名字必须是一个安全文件名。
pub fn serve_staged(path: &str) -> tauri::http::Response<Vec<u8>> {
    let not_found = || {
        tauri::http::Response::builder()
            .status(404)
            .body(Vec::new())
            .unwrap_or_else(|_| tauri::http::Response::new(Vec::new()))
    };
    let name = path.trim_start_matches('/');
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || !name.ends_with(".png")
    {
        return not_found();
    }
    match std::fs::read(stage_dir().join(name)) {
        Ok(bytes) => tauri::http::Response::builder()
            .header("Content-Type", "image/png")
            .header("Cache-Control", "no-store")
            // 前端 canvas 要取像素（放大镜取色），必须允许跨源读取。
            .header("Access-Control-Allow-Origin", "*")
            .header("Cross-Origin-Resource-Policy", "cross-origin")
            .body(bytes)
            .unwrap_or_else(|_| not_found()),
        Err(_) => not_found(),
    }
}

pub fn image_size(path: &Path) -> Result<(u32, u32), String> {
    let file = std::fs::File::open(path)
        .map_err(|e| format!("读取图片失败（{}）：{e}", path.display()))?;
    let reader = png::Decoder::new(BufReader::new(file))
        .read_info()
        .map_err(|e| format!("解析 PNG 失败：{e}"))?;
    Ok(reader.info().size())
}

/// 裁切后的矩形：`(x, y, w, h)`。头尾各裁掉 `head` / `tail` 像素。
pub fn crop_rect(width: u32, height: u32, head: u32, tail: u32) -> Option<(u32, u32, u32, u32)> {
    if width == 0 || height == 0 {
        return None;
    }
    let out_h = height.checked_sub(head)?.checked_sub(tail)?;
    if out_h == 0 {
        return None;
    }
    Some((0, head, width, out_h))
}

struct Decoded {
    reader: png::Reader<BufReader<std::fs::File>>,
    width: u32,
    height: u32,
    color: png::ColorType,
}

fn open_decoder(src: &Path) -> Result<Decoded, String> {
    let file = std::fs::File::open(src)
        .map_err(|e| format!("读取图片失败（{}）：{e}", src.display()))?;
    let mut decoder = png::Decoder::new(BufReader::new(file));
    decoder.set_transformations(png::Transformations::EXPAND);
    let reader = decoder
        .read_info()
        .map_err(|e| format!("解析 PNG 失败：{e}"))?;
    let info = reader.info();
    if info.interlaced {
        return Err("暂不支持隔行扫描的 PNG".into());
    }
    let (width, height) = info.size();
    let (color, depth) = reader.output_color_type();
    if depth != png::BitDepth::Eight || !matches!(color, png::ColorType::Rgb | png::ColorType::Rgba)
    {
        return Err("只支持 8 位 RGB / RGBA 的 PNG".into());
    }
    Ok(Decoded {
        width,
        height,
        color,
        reader,
    })
}

/// 流式裁切：逐行解码、逐行编码，不把整图放进内存。
pub fn crop_png(src: &Path, dst: &Path, head: u32, tail: u32) -> Result<(u32, u32), String> {
    let mut d = open_decoder(src)?;
    let (_, _, out_w, out_h) = crop_rect(d.width, d.height, head, tail)
        .ok_or_else(|| "裁切范围不合法".to_string())?;

    let out_file = std::fs::File::create(dst)
        .map_err(|e| format!("创建预览文件失败（{}）：{e}", dst.display()))?;
    let mut out_buf = BufWriter::new(out_file);
    let result = (|| -> Result<(), String> {
        let mut enc = png::Encoder::new(&mut out_buf, out_w, out_h);
        enc.set_color(d.color);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().map_err(|e| format!("写 PNG 头失败：{e}"))?;
        let mut stream = writer
            .stream_writer()
            .map_err(|e| format!("写裁切结果失败：{e}"))?;
        let mut y = 0u32;
        while let Some(row) = d
            .reader
            .next_row()
            .map_err(|e| format!("解码 PNG 失败：{e}"))?
        {
            if y >= head && y < head + out_h {
                stream
                    .write_all(row.data())
                    .map_err(|e| format!("写裁切结果失败：{e}"))?;
            }
            y += 1;
            if y >= head + out_h {
                break;
            }
        }
        if y < head + out_h {
            return Err("PNG 行数不足，文件可能不完整".into());
        }
        stream.finish().map_err(|e| format!("写裁切结果失败：{e}"))?;
        Ok(())
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(dst);
        return Err(e);
    }
    out_buf
        .flush()
        .map_err(|e| format!("保存裁切结果失败：{e}"))?;
    Ok((out_w, out_h))
}

/// 解码裁切区域为 BGRA，供 OCR / 剪贴板使用。
pub fn crop_to_image(src: &Path, head: u32, tail: u32) -> Result<CapturedImage, String> {
    let mut d = open_decoder(src)?;
    let (_, _, out_w, out_h) = crop_rect(d.width, d.height, head, tail)
        .ok_or_else(|| "裁切范围不合法".to_string())?;
    let pixels = out_w as u64 * out_h as u64;
    if pixels > MAX_DECODE_PIXELS {
        return Err(format!(
            "裁切区域太大（{out_w}×{out_h}，{pixels} 像素），请改用「存盘」"
        ));
    }
    let mut out = Vec::with_capacity((pixels * 4) as usize);
    let mut y = 0u32;
    while let Some(row) = d
        .reader
        .next_row()
        .map_err(|e| format!("解码 PNG 失败：{e}"))?
    {
        if y >= head && y < head + out_h {
            let data = row.data();
            match d.color {
                png::ColorType::Rgba => {
                    for px in data.as_chunks::<4>().0 {
                        out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
                    }
                }
                _ => {
                    for px in data.as_chunks::<3>().0 {
                        out.extend_from_slice(&[px[2], px[1], px[0], 255]);
                    }
                }
            }
        }
        y += 1;
        if y >= head + out_h {
            break;
        }
    }
    if y < head + out_h {
        return Err("PNG 行数不足，文件可能不完整".into());
    }
    Ok(CapturedImage {
        width: out_w as i32,
        height: out_h as i32,
        pixels: out,
    })
}

fn validate_region(
    img_w: u32,
    img_h: u32,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
) -> Result<(u32, u32), String> {
    if w == 0
        || h == 0
        || x.checked_add(w).is_none_or(|v| v > img_w)
        || y.checked_add(h).is_none_or(|v| v > img_h)
    {
        return Err("裁切区域超出图片范围".into());
    }
    Ok((w, h))
}

/// 矩形裁切（选框用）：流式逐行读、只写中间列，内存只有一行缓冲。
pub fn crop_region_png(
    src: &Path,
    dst: &Path,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
) -> Result<(u32, u32), String> {
    let mut d = open_decoder(src)?;
    let (out_w, out_h) = validate_region(d.width, d.height, x, y, w, h)?;
    let bpp = if d.color == png::ColorType::Rgba { 4 } else { 3 };

    let out_file = std::fs::File::create(dst)
        .map_err(|e| format!("创建裁切文件失败（{}）：{e}", dst.display()))?;
    let mut out_buf = BufWriter::new(out_file);
    let result = (|| -> Result<(), String> {
        let mut enc = png::Encoder::new(&mut out_buf, out_w, out_h);
        enc.set_color(d.color);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().map_err(|e| format!("写 PNG 头失败：{e}"))?;
        let mut stream = writer
            .stream_writer()
            .map_err(|e| format!("写裁切结果失败：{e}"))?;
        let mut row = 0u32;
        while let Some(data) = d
            .reader
            .next_row()
            .map_err(|e| format!("解码 PNG 失败：{e}"))?
        {
            if row >= y && row < y + out_h {
                let bytes = data.data();
                let start = (x * bpp) as usize;
                let end = ((x + out_w) * bpp) as usize;
                stream
                    .write_all(&bytes[start..end])
                    .map_err(|e| format!("写裁切结果失败：{e}"))?;
            }
            row += 1;
            if row >= y + out_h {
                break;
            }
        }
        if row < y + out_h {
            return Err("PNG 行数不足，文件可能不完整".into());
        }
        stream.finish().map_err(|e| format!("写裁切结果失败：{e}"))?;
        Ok(())
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(dst);
        return Err(e);
    }
    out_buf
        .flush()
        .map_err(|e| format!("保存裁切结果失败：{e}"))?;
    Ok((out_w, out_h))
}

/// 矩形裁切并解码为 BGRA（复制 / OCR 用）。受 `MAX_DECODE_PIXELS` 限制。
pub fn crop_region_to_image(
    src: &Path,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
) -> Result<CapturedImage, String> {
    let mut d = open_decoder(src)?;
    let (out_w, out_h) = validate_region(d.width, d.height, x, y, w, h)?;
    let pixels = out_w as u64 * out_h as u64;
    if pixels > MAX_DECODE_PIXELS {
        return Err(format!(
            "选区太大（{out_w}×{out_h}，{pixels} 像素），请改用「保存」"
        ));
    }
    let mut out = Vec::with_capacity((pixels * 4) as usize);
    let mut row = 0u32;
    while let Some(data) = d
        .reader
        .next_row()
        .map_err(|e| format!("解码 PNG 失败：{e}"))?
    {
        if row >= y && row < y + out_h {
            let bytes = data.data();
            match d.color {
                png::ColorType::Rgba => {
                    let start = (x * 4) as usize;
                    let end = ((x + out_w) * 4) as usize;
                    for px in bytes[start..end].as_chunks::<4>().0 {
                        out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
                    }
                }
                _ => {
                    let start = (x * 3) as usize;
                    let end = ((x + out_w) * 3) as usize;
                    for px in bytes[start..end].as_chunks::<3>().0 {
                        out.extend_from_slice(&[px[2], px[1], px[0], 255]);
                    }
                }
            }
        }
        row += 1;
        if row >= y + out_h {
            break;
        }
    }
    if row < y + out_h {
        return Err("PNG 行数不足，文件可能不完整".into());
    }
    Ok(CapturedImage {
        width: out_w as i32,
        height: out_h as i32,
        pixels: out,
    })
}

/// 预览面板上的四个动作。
#[derive(Debug, Clone)]
pub struct ActionOutcome {
    pub message: String,
    pub path: Option<PathBuf>,
    pub text: Option<String>,
    /// 钉屏动作创建/复用的窗口 label（供主线程打开对应窗口）。
    pub pin_label: Option<String>,
}

/// 对当前预览图执行一个动作。`head` / `tail` 是像素行数。
pub fn apply(action: &str, head: u32, tail: u32) -> Result<ActionOutcome, String> {
    let src = source().ok_or_else(|| "预览窗口没有图片".to_string())?;
    match action {
        "save_as" => {
            let cfg = crate::config::load();
            let dir = output::resolve_dir(&cfg);
            let _ = std::fs::create_dir_all(&dir);
            let def = output::next_png_path(&dir, &cfg);
            let Some(dst) = crate::fileops::pick_save_png(&def)? else {
                return Ok(ActionOutcome {
                    pin_label: None,
                    message: "已取消保存".into(),
                    path: None,
                    text: None,
                });
            };
            let (w, h) = crop_png(&src, &dst, head, tail)?;
            Ok(ActionOutcome {
                pin_label: None,
                message: format!("已另存为（{w}×{h}）：{}", dst.display()),
                path: Some(dst),
                text: None,
            })
        }
        "save" => {
            let cfg = crate::config::load();
            let dir = output::resolve_dir(&cfg);
            std::fs::create_dir_all(&dir).map_err(|e| format!("创建截图目录失败：{e}"))?;
            let dst = output::next_png_path(&dir, &cfg);
            let (w, h) = crop_png(&src, &dst, head, tail)?;
            Ok(ActionOutcome {
                pin_label: None,
                message: format!("已保存（{w}×{h}）：{}", dst.display()),
                path: Some(dst),
                text: None,
            })
        }
        "clipboard" => {
            let img = crop_to_image(&src, head, tail)?;
            output::copy_image(&img)?;
            Ok(ActionOutcome {
                pin_label: None,
                message: format!("已复制到剪贴板（{}×{}）", img.width, img.height),
                path: None,
                text: None,
            })
        }
        "pin" => {
            let dir = stage_dir();
            std::fs::create_dir_all(&dir).map_err(|e| format!("创建暂存目录失败：{e}"))?;
            let dst = dir.join(format!(
                "crop-{:016x}-{head}-{tail}.png",
                file_key(&src)?
            ));
            let (w, h) = crop_png(&src, &dst, head, tail)?;
            let single = crate::config::load().pinscreen_single_window;
            let (label, _, _) = crate::screenshot::pin::add_staged(dst, single)?;
            Ok(ActionOutcome {
                pin_label: Some(label),
                message: format!("已钉屏（{w}×{h}）"),
                path: None,
                text: None,
            })
        }
        "ocr" => {
            let img = crop_to_image(&src, head, tail)?;
            let text = crate::ocr::ocr_bitmap(&img)?;
            let empty = text.trim().is_empty();
            Ok(ActionOutcome {
                pin_label: None,
                message: if empty {
                    "没有识别到文字".into()
                } else {
                    "识别完成".into()
                },
                path: None,
                text: Some(text),
            })
        }
        other => Err(format!("未知的预览动作：{other}")),
    }
}

/// 预览窗口关闭：清掉状态；暂存文件若没被钉屏使用就删除。
pub fn on_window_destroyed() {
    let Some(path) = clear_source() else {
        return;
    };
    if !crate::screenshot::pin::owns(&path) {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_png(path: &Path, width: u32, rows: &[u8]) {
        let height = rows.len() as u32;
        let file = std::fs::File::create(path).unwrap();
        let mut buf = BufWriter::new(file);
        {
            let mut enc = png::Encoder::new(&mut buf, width, height);
            enc.set_color(png::ColorType::Rgb);
            enc.set_depth(png::BitDepth::Eight);
            let mut w = enc.write_header().unwrap();
            let mut s = w.stream_writer().unwrap();
            for v in rows {
                let row = vec![*v; (width * 3) as usize];
                s.write_all(&row).unwrap();
            }
            s.finish().unwrap();
        }
        buf.flush().unwrap();
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vexto-preview-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn crop_rect_rejects_empty_and_returns_row_range() {
        assert_eq!(crop_rect(100, 200, 10, 20), Some((0, 10, 100, 170)));
        assert_eq!(crop_rect(100, 200, 190, 10), None);
        assert_eq!(crop_rect(100, 200, 200, 0), None);
        assert_eq!(crop_rect(0, 200, 0, 0), None);
    }

    #[test]
    fn crop_png_keeps_requested_rows() {
        let dir = temp_dir("crop");
        let src = dir.join("src.png");
        let rows: Vec<u8> = (0..20).collect();
        write_png(&src, 4, &rows);
        let dst = dir.join("out.png");
        let (w, h) = crop_png(&src, &dst, 5, 3).unwrap();
        assert_eq!((w, h), (4, 12));

        let file = std::fs::File::open(&dst).unwrap();
        let mut reader = png::Decoder::new(BufReader::new(file)).read_info().unwrap();
        let info = reader.info().size();
        assert_eq!(info, (4, 12));
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let frame = reader.next_frame(&mut buf).unwrap();
        assert_eq!(frame.height, 12);
        // 第一行应是源图第 5 行（值 5）
        assert_eq!(&buf[0..3], &[5, 5, 5]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn crop_to_image_converts_rgb_to_bgra() {
        let dir = temp_dir("image");
        let src = dir.join("src.png");
        write_png(&src, 3, &[1, 2, 3, 4]);
        let img = crop_to_image(&src, 1, 1).unwrap();
        assert_eq!((img.width, img.height), (3, 2));
        assert_eq!(&img.pixels[0..4], &[2, 2, 2, 255]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 矩形裁切：只保留 x..x+w 列与 y..y+h 行。
    #[test]
    fn region_crop_keeps_columns_and_rows() {
        let dir = temp_dir("region");
        let src = dir.join("src.png");
        // 4 行、每行灰度就是行号；两列像素值相同
        write_png(&src, 4, &[10, 20, 30, 40]);
        let dst = dir.join("out.png");
        let (w, h) = crop_region_png(&src, &dst, 1, 1, 2, 2).unwrap();
        assert_eq!((w, h), (2, 2));
        let file = std::fs::File::open(&dst).unwrap();
        let mut reader = png::Decoder::new(BufReader::new(file)).read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        reader.next_frame(&mut buf).unwrap();
        // 第一行应来自源图第 1 行（值 20），且只有 2 列
        assert_eq!(&buf[0..3], &[20, 20, 20]);
        assert_eq!(&buf[6..9], &[30, 30, 30]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn region_out_of_bounds_is_rejected() {
        let dir = temp_dir("region-bad");
        let src = dir.join("src.png");
        write_png(&src, 4, &[1, 2, 3]);
        let dst = dir.join("out.png");
        assert!(crop_region_png(&src, &dst, 3, 0, 2, 1).is_err());
        assert!(crop_region_png(&src, &dst, 0, 2, 1, 2).is_err());
        assert!(crop_region_to_image(&src, 0, 0, 0, 1).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
