# ocr-lab · PaddleOCR 实验（分支 `exp/ocr-paddle`）

对比 `Windows.Media.Ocr` 与 PaddleOCR（PP-OCRv4 ONNX）在中文 / 混合字符截图上的
准确率、耗时、内存和体积。**只存在于实验分支，不合并 main。**

## 模型（不入库，约 15.6 MB）

放在 `src-tauri/models/ocr/`：

| 文件 | 大小 | 来源 |
|---|---|---|
| `ch_PP-OCRv4_det_infer.onnx` | 4,745,517 B | `https://hf-mirror.com/SWHL/RapidOCR/resolve/main/PP-OCRv4/ch_PP-OCRv4_det_infer.onnx` |
| `ch_PP-OCRv4_rec_infer.onnx` | 10,857,958 B | 同上 `..._rec_infer.onnx` |
| `ppocr_keys_v1.txt` | 26,250 B | `https://raw.githubusercontent.com/PaddlePaddle/PaddleOCR/main/ppocr/utils/ppocr_keys_v1.txt` |

许可：PaddleOCR 模型 Apache-2.0；`pure-onnx-ocr` Apache-2.0；推理用纯 Rust 的 tract，
**不需要 ONNX Runtime / C/C++ 依赖**。

## 运行

```bash
cargo run --release --manifest-path ocr-lab/Cargo.toml -- src-tauri/models/ocr <图片路径>
```

## 结果（两张受控中文图，CER 忽略空白）

| 引擎 | 20px CER | 12px CER | 单图耗时 | 进程峰值内存 | 体积 |
|---|---|---|---|---|---|
| Windows.Media.Ocr | **15.7%** | **23.3%** | 60 ms | 系统组件 | 0 |
| PaddleOCR（PP-OCRv4） | **5.8%** | **6.9%** | 528–613 ms | 139.3 MB | 模型 15.6 MB + 静态链接 ~30 MB |

- 中文错字明显减少：Windows 的「截图→截、放大镜→大憤、清→渭、别→引」等问题消失。
- 仍存在孤立 `I` / `|` / `1` 的混淆（Paddle 读成 `1` 或 `！`）；12px 下字形本身接近。
  比 Windows 的 `I → ] / C` 好，但不是完美。
- 其它小差异：`PaddleOCR` → `PaddleOcR`（大小写）、个别全/半角标点。
- tract 初始化约 22 ms（文件缓存热），单图推理 0.5–0.6 s；引擎常驻约 140 MB。
  若每次 OCR 后释放引擎可保持空闲内存，但每次多 0.5–1 s 初始化。

## 应用内接入（实验脚手架）

- `src-tauri/src/ocr_paddle.rs`：`pure-onnx-ocr` 引擎单例、BGRA → DynamicImage、按行拼接。
- `src-tauri/src/ocr.rs`：`ocr_bitmap` 按配置 `ocr_engine` 分派到 `windows_ocr_bitmap` / Paddle。
- `src-tauri/src/config.rs`：`ocr_engine`（默认 `windows`）。
- `src-tauri/src/tools/translate_tool.rs`：截图翻译分区加「识别引擎」下拉。
- `src-tauri/src/main.rs`：`ocr_compare` 命令（同一张 PNG 上对比两个引擎）。

## 合并前要定的两件事

1. **安装包体积**：模型 15.6 MB + 静态链接增长 ~30 MB，远超 12 MB 验收线。
   可选：首次使用时下载模型（联网一次），或接受更大的安装包并改验收线。
2. **干净构建**：本机全新 target 下 `ring 0.17.14`（`lanflare-core` 的 rustls 依赖）
   在 VS 2026 / MSVC 14.51 上编译失败，与 Paddle 无关；主仓库因为 target 里有旧缓存
   才没暴露。S6 打包前必须单独处理（升级 ring/rustls 或固定工具链）。

另外：本分支基于会话 66（`0cfab88`），main 已经有会话 67 的修正；合并时需要
先同步 main，`config.rs` / `tools/translate_tool.rs` / `ocr.rs` 可能有小冲突。
