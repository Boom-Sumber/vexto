// 实验分支：PaddleOCR（PP-OCRv4 ONNX）离线对比工具（pure-onnx-ocr / tract）。
// 用法：ocr-lab [模型目录] [图片路径]
use pure_onnx_ocr::OcrEngineBuilder;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "src-tauri/models/ocr".to_string());
    let image_path = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| "test.png".to_string());

    let t = Instant::now();
    let engine = OcrEngineBuilder::new()
        .det_model_path(format!("{dir}/ch_PP-OCRv4_det_infer.onnx"))
        .rec_model_path(format!("{dir}/ch_PP-OCRv4_rec_infer.onnx"))
        .dictionary_path(format!("{dir}/ppocr_keys_v1.txt"))
        .det_limit_side_len(960)
        .det_unclip_ratio(1.5)
        .build()
        .expect("初始化 pure-onnx-ocr 失败");
    println!("init_ms={}", t.elapsed().as_millis());

    let t = Instant::now();
    let results = engine.run_from_path(&image_path).expect("OCR 失败");
    println!("ocr_ms={}", t.elapsed().as_millis());
    println!("lines={}", results.len());
    for r in &results {
        println!("{:.3}\t{}", r.confidence, r.text);
    }
    let text: Vec<&str> = results.iter().map(|r| r.text.as_str()).collect();
    println!("---");
    println!("{}", text.join("\n"));
}
