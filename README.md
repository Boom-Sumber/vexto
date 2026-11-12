# Vexto

一个 Windows 常驻托盘的轻量工具箱，把几款开源小工具的核心能力合进一个进程：

| 能力 | 模块 | 阶段 |
|---|---|---|
| 划词翻译 | 划词翻译 | S1 |
| 截图 OCR 翻译 | 截图翻译 | S5 |
| HTTP / BT / magnet 下载 | 下载（aria2c sidecar） | S3 |
| 局域网跨设备传文件 | 局域网传文件 | S4 |

**唯一卖点：轻。** 托盘常驻不许到几百 MB。

## 当前状态

**S0（工程脚手架）**：能编译、能起托盘、内存测得出来。**界面和功能都还没有**。

## 技术栈

Rust + Tauri 2，前端是纯静态 HTML/CSS/JS —— 没有 npm、没有 Vite、没有 TypeScript、没有前端框架。

## 构建

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"

cargo build --release                        # 产物: target\release\lightbox.exe
cargo clippy --release -- -D warnings        # 必须零警告
cargo test

powershell -File scripts\build.ps1           # 构建 + 打印产物体积
powershell -File scripts\measure-mem.ps1     # 内存实测（进程需在跑）
```

## 目录

```
AGENTS.md          Agent 工具进入本仓库的第一个文件（六条铁律）
docs/              全套开发文档，索引在 docs/00-INDEX.md
                   docs/11-DEV-PROGRESS.md 是唯一进度台账
scripts/           build.ps1 / measure-mem.ps1 / gen-icons.ps1
src-tauri/         Rust 源码（main / tray / dlog / mem / tools）
ui/                纯静态前端
```

## 开发约定

1. **一次会话只做一个阶段**，只做该阶段任务书写明的「本会话目标」。
2. 开工先读 `docs/11-DEV-PROGRESS.md` §11.4 + 最后一条记录；收尾必写台账并把 commit hash 填进去。
3. 交付必须带证据：编译输出、实测内存数字、日志或截图。没证据的「完成」一律不算。

完整规则见 `AGENTS.md` 与 `docs/03-ENGINEERING-STANDARDS.md`。

## 许可

本项目自身许可见 `LICENSE`（S6 阶段补齐）。用到的第三方组件及其许可原文放在 `src-tauri/licenses/`。
**禁止引入 GPL / AGPL 依赖**，见 `docs/07-LICENSING.md`。
