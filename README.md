# Vexto

一个 Windows 常驻托盘的轻量工具箱，把划词翻译、截图、下载、局域网传文件合进一个进程。

**卖点：轻。** 托盘常驻不拉 WebView2；只有真正打开窗口时才创建。

## 功能

| 能力 | 说明 |
|---|---|
| 划词翻译 | 选中文字按热键，光标旁出译文卡片；支持百度 / 有道 / OpenAI 兼容多套引擎实例 |
| 截图翻译 | 框选屏幕区域 → PaddleOCR（默认，中文更准）或 Windows 自带 OCR → 走翻译引擎 |
| 截图工具 | 区域 / 全屏 / 活动窗口；滚动长图；钉屏；预览裁头尾；全屏截图右下角缩略图反馈 |
| 下载 | HTTP / BT / magnet，aria2c sidecar，空闲自动退出 |
| 局域网传文件 | 与官方 LocalSend 互通，接收 / 发送 / 历史 / 校验 |
| 外观 | 明亮 / 暗色 / 跟随系统，六种主色调 + 取色器 |

## 安装

运行 `Vexto_0.1.0_x64-setup.exe`。安装器按当前用户安装（`%LOCALAPPDATA%`），**可以选择自定义安装路径**，不需要管理员权限。

当前安装包 **21.12 MiB**（含 PaddleOCR 模型，LZMA 压缩后）；安装后程序目录约 42 MB。

## 首次使用

1. 托盘图标右键 → 设置。
2. 「翻译」里选择或新增翻译引擎，填 APPID / 密钥（百度 / 有道 / OpenAI 兼容）。
3. 划词翻译：选中文字按 `Ctrl+Alt+T`；截图翻译：按 `Ctrl+Alt+S` 框选。
4. 截图工具默认关闭，设置 → 截图 → 启用后热键才注册。

## 热键

| 功能 | 默认热键 |
|---|---|
| 划词翻译 | `Ctrl+Alt+T` |
| 输入翻译 | `Ctrl+Alt+I` |
| 截图翻译（OCR） | `Ctrl+Alt+S` |
| 区域截图 | `Ctrl+Alt+A` |
| 全屏截图 | `Ctrl+PrintScreen` |
| 下载 | `Ctrl+Alt+D` |

全部可在设置页改；PrintScreen 组合可能被系统或其他软件占用，保存时会提示注册失败。

截图完成态还可以按 `L` 用当前选区直接进入滚动截图；滚动截图失败时会保留第一帧，仍可保存 / 复制 / 钉住 / 预览。

## 资源占用（开发态实测，2026-11-15）

| 状态 | 实测 |
|---|---|
| 托盘空闲 | **17.2 MB / WebView2=0** |
| 设置页打开 | 约 **346.8 MB / 5 个 WebView2** |
| 全部关闭后 | 约 **3.2 MB / 0** |
| 安装包 | **21.12 MiB** |

空闲数值线按 ADR-037 作为「尽量优化」的监控指引；结构性红线是**无窗口时 WebView2=0**、关窗即销毁。

## 已知限制

- Windows 10 1809+ / Windows 11。
- 中文 OCR 需要 Paddle 模型（随包）或系统中文 OCR 语言包；竖排、艺术字、低分辨率识别差是能力边界。
- 滚动截图要求选区内的画面整体滚动；只有一半区域滚动时会失败并回退到第一帧。
- 不做自动更新：升级时托盘右键退出，运行新的 setup 覆盖安装，`%APPDATA%\vexto\config.json` 会保留。
- 安装器不自动配置防火墙；首次 BT 下载若弹 Windows 安全中心，点「取消」即可，HTTP 下载不受影响。
- 截图按虚拟屏幕处理，多显示器时是一整张。

## 升级

1. 托盘右键 → 退出。
2. 运行新的 `Vexto_<版本>_x64-setup.exe` 覆盖安装。
3. 配置保留在 `%APPDATA%\vexto\config.json`；卸载默认也保留该文件。

## 开发与构建

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"

cargo build --release                        # 产物: target\release\lightbox.exe
cargo clippy --release -- -D warnings        # 必须零警告
cargo test
cargo tauri build                            # NSIS: target\release\bundle\nsis\
powershell -File scripts\measure-mem.ps1     # 内存实测（进程需在跑）
```

技术栈：Rust + Tauri 2，前端纯静态 HTML/CSS/JS —— 没有 npm、Vite、TypeScript 或前端框架。

## 目录

```
AGENTS.md          Agent 工具进入本仓库的第一个文件（六条铁律）
docs/              全套开发文档，索引 docs/00-INDEX.md；进度台账 docs/11-DEV-PROGRESS.md
scripts/           build.ps1 / measure-mem.ps1 / gen-icons.ps1
src-tauri/         Rust 源码、模型、aria2c、第三方许可
ui/                纯静态前端
```

开发约定与完整规则见 `AGENTS.md`、`docs/03-ENGINEERING-STANDARDS.md`。

## 许可

第三方组件及其许可原文在 `src-tauri/licenses/`：

- aria2c：GPLv2（`aria2-COPYING` / `aria2-VERSION.txt`）
- PaddleOCR PP-OCRv4 模型：Apache-2.0（`paddleocr-models-NOTICE.txt`）
- pure-onnx-ocr：Apache-2.0（`pure-onnx-ocr-Apache-2.0.txt`）

本程序以 **GNU GPL-3.0** 协议开源，全文见仓库根 `LICENSE`。第三方组件仍按各自许可分发；详见 `docs/07-LICENSING.md`。
