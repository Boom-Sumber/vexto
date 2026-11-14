//! 实验分支 exp/ocr-paddle：PaddleOCR（PP-OCRv4 ONNX）后端。
//!
//! **只存在于实验分支，不合并 main。** 模型文件在 `src-tauri/models/ocr/`
//! （`VEXTO_OCR_MODEL_DIR` 可覆盖）；引擎在第一次调用时初始化并常驻。
//!
//! 走 `paddleocr_rs_onnx`（MIT）+ ONNX Runtime；输入与 Windows OCR 一样是
//! `CapturedImage`（BGRA），输出按行拼接的纯文本，便于两条链路直接对比。

use crate::ocr::CapturedImage;
use image::{DynamicImage, RgbaImage};
use pure_onnx_ocr::{OcrEngine, OcrEngineBuilder};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

struct Loaded {
    engine: Mutex<OcrEngine>,
    init_ms: u128,
}

static ENGINE: OnceLock<Result<Loaded, String>> = OnceLock::new();

fn model_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("VEXTO_OCR_MODEL_DIR") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir);
        }
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
    {
        let p = dir.join("models").join("ocr");
        if p.join("ch_PP-OCRv4_det_infer.onnx").exists() {
            return p;
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("models")
        .join("ocr")
}

fn engine() -> Result<&'static Loaded, String> {
    ENGINE
        .get_or_init(|| {
            let dir = model_dir();
            let started = Instant::now();
            let engine = OcrEngineBuilder::new()
                .det_model_path(dir.join("ch_PP-OCRv4_det_infer.onnx"))
                .rec_model_path(dir.join("ch_PP-OCRv4_rec_infer.onnx"))
                .dictionary_path(dir.join("ppocr_keys_v1.txt"))
                .det_limit_side_len(960)
                .det_unclip_ratio(1.5)
                .build()
                .map_err(|e| format!("初始化 PaddleOCR 失败（{}）：{e}", dir.display()))?;
            Ok(Loaded {
                engine: Mutex::new(engine),
                init_ms: started.elapsed().as_millis(),
            })
        })
        .as_ref()
        .map_err(|e| e.clone())
}

/// BGRA 截图 → PaddleOCR 文本（行之间 `\n`，与 Windows OCR 的 `Text()` 对齐）。
pub fn ocr_bitmap(img: &CapturedImage) -> Result<String, String> {
    let dynimg = to_dynamic(img)?;
    let loaded = engine()?;
    let guard = loaded
        .engine
        .lock()
        .map_err(|_| "PaddleOCR 引擎锁失败".to_string())?;
    let blocks = guard
        .run_from_image(&dynimg)
        .map_err(|e| format!("PaddleOCR 识别失败：{e}"))?;
    let lines: Vec<String> = blocks
        .into_iter()
        .map(|b| b.text)
        .filter(|t| !t.trim().is_empty())
        .collect();
    Ok(lines.join("\n"))
}

/// 上一次（或本次）引擎初始化的耗时；未初始化返回 0。
pub fn init_ms() -> u128 {
    engine().map(|l| l.init_ms).unwrap_or(0)
}

pub fn to_dynamic(img: &CapturedImage) -> Result<DynamicImage, String> {
    let w = img.width;
    let h = img.height;
    if w <= 0 || h <= 0 {
        return Err("截图数据不合法".into());
    }
    let (w, h) = (w as u32, h as u32);
    let need = (w as usize) * (h as usize) * 4;
    if img.pixels.len() < need {
        return Err("截图数据不合法".into());
    }
    let mut rgba = Vec::with_capacity(need);
    for px in img.pixels[..need].as_chunks::<4>().0 {
        rgba.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
    }
    let buf = RgbaImage::from_raw(w, h, rgba).ok_or_else(|| "截图数据不合法".to_string())?;
    Ok(DynamicImage::ImageRgba8(buf))
}

/// 读一张 PNG 成 `CapturedImage`（对比命令用）。
pub fn load_png_as_captured(path: &str) -> Result<CapturedImage, String> {
    let dynimg = image::open(path).map_err(|e| format!("打开图片失败（{path}）：{e}"))?;
    let rgba = dynimg.to_rgba8();
    let (w, h) = rgba.dimensions();
    let mut pixels = Vec::with_capacity((w as usize) * (h as usize) * 4);
    for px in rgba.as_raw().as_chunks::<4>().0 {
        pixels.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
    }
    Ok(CapturedImage {
        width: w as i32,
        height: h as i32,
        pixels,
    })
}
