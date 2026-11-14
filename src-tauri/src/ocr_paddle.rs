//! PaddleOCR（PP-OCRv4 ONNX）识别后端。
//!
//! 用 `pure-onnx-ocr`（Apache-2.0，纯 Rust tract 推理）跑 DBNet 检测 + CTC 识别，
//! 不需要 ONNX Runtime / C/C++ 依赖。模型随包放在 `models/ocr/`（见 ADR-049）。
//!
//! 引擎按次创建：一次 OCR 约 0.5–0.6s，结束后释放约 140MB 的模型/张量内存，
//! 保持托盘常驻的轻量口径。Windows.Media.Ocr 仍可在设置里选回。

use crate::ocr::CapturedImage;
use image::{DynamicImage, RgbaImage};
use pure_onnx_ocr::{OcrEngine, OcrEngineBuilder};
use std::path::PathBuf;

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

fn build_engine() -> Result<OcrEngine, String> {
    let dir = model_dir();
    OcrEngineBuilder::new()
        .det_model_path(dir.join("ch_PP-OCRv4_det_infer.onnx"))
        .rec_model_path(dir.join("ch_PP-OCRv4_rec_infer.onnx"))
        .dictionary_path(dir.join("ppocr_keys_v1.txt"))
        .det_limit_side_len(960)
        .det_unclip_ratio(1.5)
        .build()
        .map_err(|e| format!("PaddleOCR 初始化失败（{}）：{e}", dir.display()))
}

/// BGRA 截图 → PaddleOCR 文本（行之间 `\n`，与 Windows OCR 的 `Text()` 对齐）。
pub fn ocr_bitmap(img: &CapturedImage) -> Result<String, String> {
    let dynimg = to_dynamic(img)?;
    let engine = build_engine()?;
    let blocks = engine
        .run_from_image(&dynimg)
        .map_err(|e| format!("PaddleOCR 识别失败：{e}"))?;
    let lines: Vec<String> = blocks
        .into_iter()
        .map(|b| b.text)
        .filter(|t| !t.trim().is_empty())
        .collect();
    Ok(lines.join("\n"))
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
