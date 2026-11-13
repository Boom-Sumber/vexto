#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Vexto 入口。
//!
//! S2：多引擎 + 工具注册表 + schema 驱动的设置 IPC。
//!
//! 线程模型（三条边界，改动前先看 docs/03-ENGINEERING-STANDARDS.md §3.5）：
//! - 热键回调跑在**事件循环线程**上 → 只能 `std::thread::spawn` 出去，绝不能回调里做 UIA / 网络。
//! - 工作线程做完取词和翻译 → `app.run_on_main_thread()` 回主线程再碰窗口。
//! - `setup()` 里不能建窗（实测 os error 5），要等 `RunEvent::Ready`。

mod autostart;
mod capture;
mod config;
mod dlog;
mod download;
mod extension_bridge;
mod fileops;
mod hotkey;
mod lang;
mod mem;
mod ocr;
mod popup;
mod tools;
mod transfer;
mod tray;
mod translate;

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use serde::Serialize;
use serde_json::{json, Map, Value};
use tauri::{AppHandle, Emitter, Manager};

/// 托盘「退出」置位。没有它，`ExitRequested` 里的 `prevent_exit()` 会让进程永远关不掉。
pub static QUITTING: AtomicBool = AtomicBool::new(false);

// ---------- 输出结构 ----------

#[derive(Serialize)]
struct SimpleOut {
    ok: bool,
    message: String,
}

#[derive(Serialize)]
struct SaveOut {
    ok: bool,
    message: String,
    /// 保存成功但用户该知道的事（目录不存在、热键被占用…）
    warnings: Vec<String>,
}

#[derive(Serialize)]
struct ToolOut {
    id: &'static str,
    name: &'static str,
    desc: &'static str,
    /// 设置页侧栏图标（ui/icons.js 注册名；空 = 前端退回圆点）。纯展示。
    icon: &'static str,
    enabled: bool,
    /// 设置页右上角要不要画「启用」开关（通用 / 关于是 false）
    switchable: bool,
    status_line: String,
    /// 状态行颜色语义：ok 绿 / warn 黄 / error 红（会话 52）
    status_kind: &'static str,
    sections: Vec<tools::Section>,
}

/// `get_tool_config` 的返回：只含该工具自己的字段值。
#[derive(Serialize)]
struct ToolConfigOut {
    values: Value,
}

#[derive(Serialize)]
struct TestOut {
    ok: bool,
    /// 成功时是译文
    message: String,
    /// 引擎实际用的语向
    from: String,
    to: String,
    /// 耗时（毫秒）
    ms: u64,
}

// ---------- 窗口操作统一走主线程 ----------

pub fn open_settings(app: &AppHandle) {
    let a = app.clone();
    // 窗口必须在事件循环线程创建（实测：在 setup() 里建窗直接 os error 5）
    let _ = app.run_on_main_thread(move || popup::open_settings(&a));
}

/// 打开设置页并停在指定工具。
pub fn open_settings_at(app: &AppHandle, tool_id: &str) {
    let a = app.clone();
    let id = tool_id.to_string();
    let _ = app.run_on_main_thread(move || popup::open_settings_at(&a, &id));
}

/// 打开下载主窗口。窗口必须在事件循环线程创建（setup() 里建窗会 os error 5），
/// 所以热键的工作线程与托盘菜单都统一走这里。
pub fn open_download_window(app: &AppHandle) {
    let a = app.clone();
    let _ = app.run_on_main_thread(move || popup::open_main_window(&a));
}

/// 打开主窗口并停在互传视图（托盘「局域网互传…」）。
pub fn open_transfer_window(app: &AppHandle) {
    let a = app.clone();
    let _ = app.run_on_main_thread(move || popup::open_main_window_with_view(&a, "transfer"));
}

fn show_popup(app: &AppHandle, payload: Value) {
    let a = app.clone();
    let _ = app.run_on_main_thread(move || popup::show(&a, payload));
}

/// 给弹窗 payload 补上「多久自动关闭」。
///
/// `popup_timeout_ms` 在「通用」里可以改，但 S1 起前端一直是硬编码 15000，
/// 这个配置从来没生效过。现在由后端统一塞进去，前端只负责用——
/// 配置在后端、行为也在后端驱动，前端不该自己猜一个数。
fn with_timeout(mut payload: Value, cfg: &config::Config) -> Value {
    if let Value::Object(m) = &mut payload {
        m.insert("timeout".to_string(), json!(cfg.popup_timeout_ms));
    }
    payload
}

/// 给弹窗 payload 补上引擎信息：当前引擎、可选引擎列表、图钉状态。
///
/// 引擎行要能点开选引擎（用户 m02735 的要求），所以列表必须随每次渲染一起给，
/// 不能等前端点开了再问一次 —— 那会让弹窗闪一下。
fn with_engine(mut payload: Value, cfg: &config::Config) -> Value {
    if let Value::Object(m) = &mut payload {
        // 弹窗的引擎菜单直接列「用户配好的实例」：值就是 `active_engine` 的写法
        // （`vendor:<id>` / `openai:<id>`），点哪个就写哪个，不需要再换算。
        let label = cfg.active_engine_label();
        m.insert("engine".to_string(), json!(cfg.active_engine));
        m.insert(
            "engineName".to_string(),
            json!(if label.is_empty() {
                "未配置引擎".to_string()
            } else {
                label
            }),
        );
        let list: Vec<Value> = cfg
            .engine_choices()
            .into_iter()
            .map(|(id, name)| json!({ "id": id, "name": name }))
            .collect();
        m.insert("engines".to_string(), Value::Array(list));
        m.insert("pinned".to_string(), json!(popup::is_pinned()));
    }
    payload
}

/// 组装一个完整的弹窗 payload：超时 + 引擎 + 模式。
fn popup_payload(state: &str, mode: &str, cfg: &config::Config, mut body: Value) -> Value {
    if let Value::Object(m) = &mut body {
        m.insert("state".to_string(), json!(state));
        m.insert("mode".to_string(), json!(mode));
    }
    with_engine(with_timeout(body, cfg), cfg)
}

// ---------- 核心流程 ----------

/// 取词 → 翻译 → 弹窗，一次完整的划词翻译流程。
///
/// 调用方可能是热键回调（事件循环线程）或托盘菜单，所以这里第一步就先
/// `std::thread::spawn`，把后面所有阻塞操作（UIA、SendInput、HTTP）都挪出事件循环。
///
/// **S3 起没有「复用上次原文」这条路了**：弹窗上的「重试」按钮已按用户要求去掉，
/// 输入翻译走 `open_input_translate`。所以这里也不再需要 `reuse` 参数。
fn run_translate(app: AppHandle) {
    std::thread::spawn(move || {
        dlog::dlog("translate_selection: 开始");

        let cfg = config::load();
        if !cfg.enable_translate {
            dlog::dlog("translate_selection: 翻译工具已停用，忽略");
            return;
        }

        // 1) 缺凭据就直接引到设置页：这时弹窗只会显示一句"去设置"，不如直接开设置
        if !translate::engine_ready(&cfg) {
            dlog::dlog("translate_selection: 引擎未配置，打开设置页");
            let a = app.clone();
            let _ = app.run_on_main_thread(move || {
                popup::close(&a);
                popup::open_settings_at(&a, "translate");
            });
            return;
        }

        // 2) 取词
        let src = match capture::capture_selection() {
            Ok(t) => t,
            Err(e) => {
                dlog::dlog(&format!("translate_selection: 取词失败：{e}"));
                let p = popup_payload("error", "select", &cfg, json!({ "src": "", "message": e }));
                show_popup(&app, p);
                return;
            }
        };

        // 3) 先弹「翻译中」，用户按下热键后 1 秒内必须有反应，
        //    等引擎返回（可能几秒）再弹窗会让人以为热键没生效。
        let p = popup_payload("loading", "select", &cfg, json!({ "src": src }));
        show_popup(&app, p);

        // 4) 翻译
        match translate::translate(&cfg, &src) {
            Ok(r) => {
                dlog::dlog(&format!(
                    "translate_selection: ok {} -> {} chars",
                    r.from,
                    r.dst.chars().count()
                ));
                // 自动复制（S2 新增的可选项）
                if cfg.auto_copy {
                    capture::write_clipboard_text(&r.dst);
                }
                let p = popup_payload(
                    "done",
                    "select",
                    &cfg,
                    json!({
                        "src": src,
                        "dst": r.dst,
                        "from": r.from,
                        "to": r.to,
                        "dir": {
                            "from": crate::lang::name_of(&r.from),
                            "to": crate::lang::name_of(&r.to),
                        },
                    }),
                );
                show_popup(&app, p);
            }
            Err(e) => {
                dlog::dlog(&format!("translate_selection: 翻译失败：{e}"));
                let p =
                    popup_payload("error", "select", &cfg, json!({ "src": src, "message": e }));
                show_popup(&app, p);
            }
        }
    });
}

/// 「输入翻译」：唤出可编辑的弹窗，用户自己打字，回车或点「翻译」出结果。
///
/// 和 `run_translate` 是两条路，不共用。理由：划词那条是
/// 「取词 → 翻译 → 弹」三段式，取词会动剪贴板、会发 Ctrl+C；
/// 输入模式压根没有取词阶段，硬塞进去只会让那个函数长出
/// `if mode == input` 的分支，两边都难读。
///
/// 这里只做一件简单事：开窗（或复用）+ 告诉前端「这是输入模式，抢焦点」。
fn open_input_translate(app: AppHandle) {
    dlog::dlog("input_translate: 唤起");
    let a = app.clone();
    let _ = app.run_on_main_thread(move || {
        let cfg = config::load();
        if !cfg.enable_translate {
            dlog::dlog("input_translate: 翻译工具已停用，忽略");
            return;
        }
        // 缺凭据时先去配置，不然弹个输入框让用户白打一段字
        if !translate::engine_ready(&cfg) {
            dlog::dlog("input_translate: 引擎未配置，打开设置页");
            popup::close(&a);
            popup::open_settings_at(&a, "translate");
            return;
        }
        // fresh = 这是刚按热键唤出的空框，前端要把上次输入清掉。
        // 只有第一次开窗那一次带 fresh，后续 loading/done 都不带，
        // 否则每翻译一次就把用户刚打的字清空。
        let p = popup_payload("input", "input", &cfg, json!({ "src": "", "fresh": true }));
        popup::show(&a, p);
        // 窗口建出来之后才允许聚焦，否则 set_focus 打在还没显示的窗口上
        popup::set_focusable(&a, true);
        // 已经存在的窗口收到这个事件后把焦点放进源文本框
        if let Some(w) = a.get_webview_window(popup::POPUP_LABEL) {
            let _ = w.emit("popup-focus", ());
        }
    });
}

/// 弹窗里点了「翻译」按钮或按了回车。
///
/// 是个 `async` 命令：翻译要发 HTTP，阻塞式 `reqwest` 若跑在命令线程上
/// 会把整个 UI 卡住几秒。`spawn_blocking` 把它挪到阻塞线程池，
/// 命令线程立刻返回、前端不冻结。
///
/// `mode` 由前端给：`input`（默认）或 `select`。同一段文字在两种模式下
/// 都要能翻——划词弹窗里用户改了源文本、或切了引擎之后，需要能重翻一次。
/// 模式只影响回给前端的 payload，翻译本身完全一样。
#[tauri::command]
async fn translate_text(
    text: String,
    mode: Option<String>,
    app: AppHandle,
) -> Result<Value, String> {
    let t = text.trim().to_string();
    if t.is_empty() {
        return Err("没有要翻译的内容".into());
    }
    let m: &'static str = match mode.as_deref() {
        Some("select") => "select",
        _ => "input",
    };
    let cfg = config::load();

    // 先让弹窗进入「翻译中」。
    // 源文本给空串：前端在 input 模式下不会覆盖用户正在输入的文本
    // （见 ui/popup.js 的 render() 注释），所以这里不会把框里内容清掉。
    let a0 = app.clone();
    let cfg0 = cfg.clone();
    let _ = app.run_on_main_thread(move || {
        let p = popup_payload("loading", m, &cfg0, json!({ "src": "" }));
        popup::show(&a0, p);
    });

    let cfg1 = cfg.clone();
    let t1 = t.clone();
    let r = tauri::async_runtime::spawn_blocking(move || translate::translate(&cfg1, &t1))
        .await
        .map_err(|e| format!("翻译任务未能启动：{e}"))?;

    let a = app.clone();
    let p = match &r {
        Ok(v) => {
            dlog::dlog(&format!(
                "translate_text: ok {} -> {} chars（mode={m}）",
                v.from,
                v.dst.chars().count()
            ));
            popup_payload(
                "done",
                m,
                &cfg,
                json!({
                    "src": t,
                    "dst": v.dst,
                    "from": v.from,
                    "to": v.to,
                    "dir": {
                        "from": crate::lang::name_of(&v.from),
                        "to": crate::lang::name_of(&v.to),
                    },
                }),
            )
        }
        Err(e) => {
            dlog::dlog(&format!("translate_text: 翻译失败：{e}（mode={m}）"));
            popup_payload("error", m, &cfg, json!({ "src": t, "message": e }))
        }
    };
    let _ = app.run_on_main_thread(move || popup::show(&a, p));

    match r {
        Ok(v) => Ok(json!({ "ok": true, "dst": v.dst })),
        Err(e) => Ok(json!({ "ok": false, "message": e })),
    }
}

// 就绪判断统一放在 `translate::engine_ready()`：它先按 `active_engine` 找到实例，
// 再把实例里的凭据投影到扁平字段，最后才问引擎。这里曾经直接读废弃的 `cfg.engine`
// （迁移后恒为空）→ 划词热键永远被引去设置页，会话 31 实机发现并修掉。

/// 热键 / 托盘「翻译选中文字」的入口。
pub fn translate_selection(app: AppHandle) {
    run_translate(app);
}

// ---------- 配置值 <-> JSON ----------

/// `Config` → JSON 对象。所有工具字段都是它的子集。
fn config_to_map(cfg: &config::Config) -> Map<String, Value> {
    match serde_json::to_value(cfg) {
        Ok(Value::Object(m)) => m,
        _ => Map::new(),
    }
}

/// 从 JSON 对象写回 `Config`。只覆盖传入的键，其余保持原值（部分保存）。
fn apply_values(cfg: &mut config::Config, values: &Map<String, Value>) -> Result<(), String> {
    let mut base = config_to_map(cfg);
    for (k, v) in values {
        // 未知键直接忽略：前端可能来自旧版本，或者用户拖了个新版前端过来。
        // 报错会更糟——用户会看到一堆他无法处理的错误。
        if base.contains_key(k) {
            base.insert(k.clone(), v.clone());
        }
    }
    let merged = serde_json::to_string(&Value::Object(base))
        .map_err(|e| format!("序列化配置失败：{e}"))?;
    *cfg = serde_json::from_str(&merged)
        .map_err(|e| format!("配置值不合法（类型对不上）：{e}"))?;
    Ok(())
}

/// 一个工具声明的所有「要存进 Config 的 key」。
fn tool_value_keys(tool_id: &str) -> Vec<&'static str> {
    let Some(t) = tools::registry::by_id(tool_id) else {
        return Vec::new();
    };
    let mut keys: Vec<&'static str> = Vec::new();
    for s in t.sections() {
        for f in &s.fields {
            if matches!(
                f.kind,
                tools::FieldKind::Note { .. }
                    | tools::FieldKind::Button
                    | tools::FieldKind::Group { .. }
            ) {
                continue;
            }
            keys.push(f.key);
        }
    }
    keys
}

/// 哪些 key 是热键字段，需要格式校验 + 整体重新注册。
/// `hotkey_ocr` 自 S5 起真注册（工具启用时）；`hotkey_download` 已在 S3a 注册。
const HOTKEY_KEYS: [&str; 4] = ["hotkey", "hotkey_input", "hotkey_download", "hotkey_ocr"];

fn is_hotkey_key(k: &str) -> bool {
    HOTKEY_KEYS.contains(&k)
}

/// 一次性注册所有全局热键（划词 / 输入翻译 / 下载）。
///
/// **必须走 `hotkey::register_all`**，不能连着调两次 `register_hotkey`——
/// 后者开头会 `unregister_all()`，第二次调用会把第一次注册的划词热键注销掉。
/// 下载热键只在工具启用时注册：停用后按了不该有反应（任务书 §S3.8b 验收 9.3.2）。
fn register_hotkeys(app: &AppHandle, cfg: &config::Config) -> Vec<String> {
    let mut list: Vec<hotkey::Binding> = vec![
        (
            "划词热键",
            cfg.hotkey.clone(),
            Box::new(|a: &AppHandle| translate_selection(a.clone())),
        ),
        (
            "输入翻译热键",
            cfg.hotkey_input.clone(),
            Box::new(|a: &AppHandle| open_input_translate(a.clone())),
        ),
    ];
    if cfg.enable_download {
        list.push((
            "下载热键",
            cfg.hotkey_download.clone(),
            // 热键回调跑在事件循环线程：读剪贴板 / spawn / HTTP 一律挪到工作线程
            // （03 §3.5.2）。这里不捕获 AppHandle，闭包天然是 'static。
            //
            // 会话 16 起热键**不再直接入队**：它只负责打开「新建任务」窗口，
            // 剪贴板里是链接就预填进去。直接入队的话用户没有任何改目录 / 改名的
            // 机会，「已经有 a.zip 时再下 a.zip」就只能靠自动改名兜。
            Box::new(|a: &AppHandle| {
                let app = a.clone();
                std::thread::spawn(move || {
                    let prefill = download::clipboard_url();
                    dlog::dlog(&format!(
                        "download: 热键打开新建任务窗口 prefill={}",
                        prefill.as_deref().unwrap_or("(无链接)")
                    ));
                    let a2 = app.clone();
                    let _ = app.run_on_main_thread(move || {
                        crate::popup::open_new_task(
                            &a2,
                            prefill.map(crate::popup::NewTaskPrefill::url_only),
                        );
                    });
                });
            }),
        ));
    }
    // S5 OCR：截图翻译热键，工具启用时才注册（停用后按了不该有反应）。
    if cfg.enable_ocr {
        list.push((
            "OCR 热键",
            cfg.hotkey_ocr.clone(),
            Box::new(|a: &AppHandle| crate::popup::open_ocr_overlay(a)),
        ));
    }
    hotkey::register_all(app, list)
}

// ---------- 命令：S5 OCR（截图翻译） ----------

/// 前端框选完成。流程：overlay 藏暗幕（前端已做）→ 等 350ms 让遮罩从屏幕
/// 上消失 → 截屏选区（物理像素）→ WinRT 识别 → 翻译 → `ocr:result` 事件回
/// overlay 显示卡片。任何一步失败都以 `ok:false` 回，overlay 显示中文原因。
#[tauri::command]
fn ocr_submit_region(
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    dpr: Option<f64>,
    app: AppHandle,
) {
    let win = match app.get_webview_window(crate::popup::OCR_LABEL) {
        Some(w) => w,
        None => return,
    };
    tauri::async_runtime::spawn_blocking(move || {
        // 遮罩是前端画的：收到命令时它可能还没从屏幕上消失，等一拍再截
        std::thread::sleep(std::time::Duration::from_millis(350));

        let dpr = dpr.filter(|d| d.is_finite() && *d > 0.0).unwrap_or(1.0);
        let (vsx, vsy, vsw, vsh) = crate::ocr::virtual_screen();
        // CSS → 物理 → 加虚拟屏幕原点偏移
        let px = vsx + (x * dpr).round() as i32;
        let py = vsy + (y * dpr).round() as i32;
        let pw = ((w * dpr).round() as i32).min(vsw);
        let ph = ((h * dpr).round() as i32).min(vsh);

        let result = crate::ocr::ocr_translate_region(px, py, pw, ph);
        let payload = match result {
            Ok((original, translated, from)) => serde_json::json!({
                "ok": true,
                "original": original,
                "translated": translated,
                "from": from,
            }),
            Err(e) => serde_json::json!({ "ok": false, "error": e }),
        };
        let _ = win.emit("ocr:result", payload);
    });
}

/// 关掉 OCR overlay（Esc / 结果卡上的关闭）。窗口 destroy，下次热键重建。
#[tauri::command]
fn ocr_cancel(app: AppHandle) {
    if let Some(w) = app.get_webview_window(crate::popup::OCR_LABEL) {
        let _ = w.destroy();
    }
}

// ---------- 命令：设置页 ----------

/// 设置页左列表的唯一数据源。
#[tauri::command]
fn list_tools() -> Vec<ToolOut> {
    tools::registry::all()
        .into_iter()
        .map(|t| ToolOut {
            id: t.id(),
            name: t.name(),
            desc: t.desc(),
            icon: t.icon(),
            enabled: t.enabled(),
            switchable: t.switchable(),
            status_line: t.status_line(),
            status_kind: t.status_kind(),
            sections: t.sections(),
        })
        .collect()
}

/// 取一个工具自己的字段值（只返回它声明过的 key）。
#[tauri::command]
fn get_tool_config(tool_id: String) -> ToolConfigOut {
    let cfg = config::load();
    let all = config_to_map(&cfg);
    let mut out = Map::new();
    for k in tool_value_keys(&tool_id) {
        if let Some(v) = all.get(k) {
            let mut v = v.clone();
            // 下载目录留空时**直接把默认路径填给前端**：用户要求输入框里看到
            // 具体路径，而不是一个占位符提示（会话 17）。
            if k == "download_dir" {
                if let Some(s) = v.as_str() {
                    if s.trim().is_empty() {
                        v = Value::String(crate::download::engine::default_download_dir());
                    }
                }
            }
            // 互传接收目录同理：输入框里直接显示完整路径，不留空。
            if k == "receive_dir" {
                if let Some(s) = v.as_str() {
                    if s.trim().is_empty() {
                        v = Value::String(
                            crate::transfer::server::resolve_receive_dir(&cfg)
                                .to_string_lossy()
                                .to_string(),
                        );
                    }
                }
            }
            out.insert(k.to_string(), v);
        }
    }
    ToolConfigOut {
        values: Value::Object(out),
    }
}

/// 保存一个工具的字段值。
///
/// 顺序很重要：**先校验热键 → 再落盘 → 再注册 → 再更新托盘**。
/// 校验放在最前面，是为了让非法热键一个字节都不写进配置文件。
#[tauri::command]
fn save_tool_config(
    tool_id: String,
    values: Map<String, Value>,
    app: AppHandle,
) -> SaveOut {
    let mut warnings: Vec<String> = Vec::new();

    // 1) 热键格式校验（后端不信任前端，06-TESTING §6.4.1 第 11j 条）
    let mut normalized: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for k in values.keys() {
        if !is_hotkey_key(k) {
            continue;
        }
        let raw = match values.get(k).and_then(|v| v.as_str()) {
            Some(s) => s,
            None => continue,
        };
        match hotkey::validate_hotkey(raw) {
            Ok(h) => {
                normalized.insert(k.clone(), h);
            }
            Err(e) => {
                return SaveOut {
                    ok: false,
                    message: e,
                    warnings,
                };
            }
        }
    }

    // 2) 只保留该工具声明的 key，并写入归一化后的热键
    let allowed = tool_value_keys(&tool_id);
    if allowed.is_empty() && tools::registry::by_id(&tool_id).is_none() {
        return SaveOut {
            ok: false,
            message: format!("未知的工具「{tool_id}」"),
            warnings,
        };
    }
    let mut patch = Map::new();
    for (k, v) in &values {
        if !allowed.contains(&k.as_str()) {
            continue;
        }
        match normalized.get(k) {
            Some(h) => {
                patch.insert(k.clone(), Value::String(h.clone()));
            }
            None => {
                patch.insert(k.clone(), v.clone());
            }
        }
    }

    // 2.5) 下载目录被清空时回写成默认路径。
    //      用户要求：输入框里直接是具体路径；清空后失焦保存，也要变回那个默认地址，
    //      而不是留一个空框。所以配置里不再保留「空串 = 跟随系统」这个中间态。
    if patch
        .get("download_dir")
        .and_then(|v| v.as_str())
        .is_some_and(|s| s.trim().is_empty())
    {
        patch.insert(
            "download_dir".to_string(),
            Value::String(crate::download::engine::default_download_dir()),
        );
    }

    // 互传接收目录清空时同样回写成默认完整路径（用户要求框里始终是具体路径）。
    if patch
        .get("receive_dir")
        .and_then(|v| v.as_str())
        .is_some_and(|s| s.trim().is_empty())
    {
        let mut base = config::load();
        base.receive_dir.clear();
        let default = crate::transfer::server::resolve_receive_dir(&base)
            .to_string_lossy()
            .to_string();
        patch.insert("receive_dir".to_string(), Value::String(default));
    }

    // 3) 目录字段：不存在就提示（不阻塞保存，用户可能马上就要去建）
    for k in ["download_dir", "receive_dir"] {
        if let Some(Value::String(s)) = patch.get(k) {
            let s = s.trim();
            if !s.is_empty() && !std::path::Path::new(s).is_dir() {
                warnings.push(format!("目录不存在，保存后仍然生效：{s}"));
            }
        }
    }

    // 3.5) 多播地址是服务启动参数，非法值别写进配置（否则下次启动直接失败）。
    if let Some(Value::String(s)) = patch.get("transfer_multicast") {
        let s = s.trim();
        if !s.is_empty() && s.parse::<std::net::Ipv4Addr>().is_err() {
            return SaveOut {
                ok: false,
                message: format!("多播地址不合法：{s}"),
                warnings,
            };
        }
    }

    let mut cfg = config::load();
    if let Err(e) = apply_values(&mut cfg, &patch) {
        return SaveOut {
            ok: false,
            message: e,
            warnings,
        };
    }
    if let Err(e) = config::save(&cfg) {
        return SaveOut {
            ok: false,
            message: e,
            warnings,
        };
    }

    // 4) 副作用：热键重新注册。
    //    任一热键（或 OCR 启用开关）变了都要**整体重注册**：register_all
    //    开头会 unregister_all，只重注册单个的话，另一个会被静默注销掉
    //    （S1 起就埋着的雷）。`enable_ocr` 决定 OCR 热键要不要注册。
    let hotkey_reregister = patch.contains_key("enable_ocr")
        || HOTKEY_KEYS.iter().any(|k| patch.contains_key(*k));
    if hotkey_reregister {
        let errs = register_hotkeys(&app, &cfg);
        // 注册失败不回滚保存：用户填的组合确实是他想要的，只是撞了别的软件。
        // 回滚会让他以为"保存失败"，下次还得重填一遍。
        warnings.extend(errs);
        if let Some(h) = normalized.get("hotkey") {
            tray::update_hotkey_label(&app, h);
        }
    }

    // 5) 副作用：翻译工具启停要同步托盘项
    if tool_id == "translate" {
        tray::set_translate_enabled(&app, cfg.enable_translate);
    }

    // 6) 缓存开关变了要清空缓存，否则用户关了缓存还在命中旧结果
    if patch.contains_key("cache_enabled") && !cfg.cache_enabled {
        translate::cache().clear();
    }

    // 7) 扩展接管的开关 / 端口 / 密钥变了：重启本地监听。
    //    端口是 bind 时占用的，改完不重启等于没改；密钥也只在新连接上生效。
    if ["extension_bridge_enabled", "extension_bridge_port", "extension_bridge_secret"]
        .iter()
        .any(|k| patch.contains_key(*k))
    {
        extension_bridge::restart(app.clone());
    }

    // 8) 代理变了：`all-proxy` 是 aria2 支持热改的选项，直接推给运行中的 engine。
    //    不用重启 sidecar —— 重启会打断正在下载的任务（conf 里没有 input-file）。
    if ["proxy_mode", "proxy_url"].iter().any(|k| patch.contains_key(*k)) {
        crate::download::proxy::apply_runtime(&cfg);
    }

    // 9) 开机自启：写 / 删注册表 Run 键。
    //    写失败（组策略锁了注册表、或 exe 在受保护目录）必须告诉用户，
    //    否则他下次开机发现没起来，只会以为功能坏了。
    if patch.contains_key("autostart") {
        if let Err(e) = autostart::apply(cfg.autostart) {
            warnings.push(format!("开机自启没设置成功：{e}"));
        }
    }

    // 10) 外观变了（主题 / 主色调）：广播给所有窗口，theme.js 收到后即时换肤。
    //     纯前端生效，无服务 / 引擎依赖；没开着的窗口下次 init 时自然读到新值。
    if ["appearance_theme", "accent_color"]
        .iter()
        .any(|k| patch.contains_key(*k))
    {
        let _ = app.emit(
            "appearance:changed",
            serde_json::json!({
                "theme": cfg.appearance_theme,
                "accent": cfg.accent_color,
            }),
        );
    }

    // 11) DNS / 证书校验 / RPC 端口都是 aria2 的**启动参数**：运行中的 sidecar
    //     读不到新值。提示一句，用户才知道要不要手动重启引擎。
    if ["dns_servers", "check_certificate", "rpc_port"]
        .iter()
        .any(|k| patch.contains_key(*k))
        && crate::download::engine::rpc_handle().is_some()
    {
        warnings.push(
            "DNS / 证书校验 / RPC 端口要等下载引擎下次启动才生效（可以先停用再启用下载）"
                .to_string(),
        );
    }

    // 11) 极轻模式重新打开：把之前挂起的窗口立刻回收。
    //     关掉极轻模式期间窗口关闭只 hide，光改配置不会让它们自己消失；
    //     不主动清一次，用户会看到「开回极轻模式了内存却还是没降」。
    //     只在值真的翻成 true 时跑（同值保存 commit() 已经不进 pending）。
    if patch.contains_key("lite_mode") && cfg.lite_mode {
        crate::popup::prune_hidden_windows(&app);
    }

    // 12) 传输服务的对外身份 / 落盘参数都在启动时烘焙进 announce 与 /info，
    //     改完必须重启才生效——否则用户改完设备名，对方看到的还是旧名字。
    //     进行中的传输会被打断，这是改这些字段的代价。
    if [
        "device_alias",
        "receive_dir",
        "require_pin",
        "pin",
        "transfer_auto_accept",
        "transfer_port",
        "transfer_use_https",
        "transfer_multicast",
        "transfer_device_type",
        "transfer_device_model",
        "transfer_peer_timeout_secs",
        "transfer_interface_whitelist",
    ]
    .iter()
    .any(|k| patch.contains_key(*k))
        && cfg.enable_transfer
        && !matches!(
            transfer::server::state(),
            transfer::server::ServiceState::Stopped
        )
    {
        let tc = transfer::TransferConfig::from_app(&cfg);
        if let Err(e) = transfer::backend()
            .stop()
            .and_then(|_| transfer::backend().start(&tc))
        {
            warnings.push(format!("传输服务重启失败，新设置要等下次启动才生效：{e}"));
        }
    }

    SaveOut {
        ok: true,
        message: "已保存".to_string(),
        warnings,
    }
}

/// 勾/取消左侧工具的启用状态。和 `save_tool_config` 分开：
/// 这是一个开关，不是一个字段值，混进 values 里会让前端难处理。
#[tauri::command]
fn set_tool_enabled(tool_id: String, enabled: bool, app: AppHandle) -> SaveOut {
    let tool = match tools::registry::by_id(&tool_id) {
        Some(t) => t,
        None => {
            return SaveOut {
                ok: false,
                message: format!("未知的工具「{tool_id}」"),
                warnings: vec![],
            }
        }
    };
    if let Err(e) = tool.set_enabled(enabled) {
        return SaveOut {
            ok: false,
            message: e,
            warnings: vec![],
        };
    }
    let mut warnings: Vec<String> = Vec::new();
    if tool_id == "translate" {
        tray::set_translate_enabled(&app, enabled);
    }
    if tool_id == "download" {
        // 下载工具启停会改变「要不要注册下载热键」。必须整体重注册：
        // register_all 开头 unregister_all，单独注册一个会把划词热键静默注销。
        warnings.extend(register_hotkeys(&app, &config::load()));
        // 停用下载工具就不要再对外提供扩展接口，避免「接管了却不干活」。
        if enabled {
            extension_bridge::restart(app.clone());
        } else {
            extension_bridge::stop();
        }
    }
    SaveOut {
        ok: true,
        message: if enabled {
            format!("已启用「{}」", tool.name())
        } else {
            format!("已停用「{}」", tool.name())
        },
        warnings,
    }
}

/// 「测试一下」：用固定文本真跑一次翻译，返回译文与耗时。
///
/// 用固定文本而不是让用户自己输入，是为了让"能用/不能用"这件事有个确定的判据；
/// 也避免用户在设置页里粘贴一大段东西。
///
/// **探针文案要按目标语言选**（与 S2 任务书 §2.6.4 第 ⑤ 条给的那句略有出入，
/// 理由是实测：任务书那句是「Hello, world! 这是一段测试文本。」中英混排，
/// 百度会自动判定成 zh，用户默认又是 to=zh，于是「翻译」变成 zh→zh 原样返回——
/// 看起来像成功，实际上什么都没验证到。改成"目标语言是中文就用英文探针、
/// 否则用中文探针"，保证语向一定不同。）
fn probe_text(to: &str) -> &'static str {
    let head = to.split(['-', '_']).next().unwrap_or("zh");
    if head == "zh" {
        "Hello, world! This is a translation test."
    } else {
        "你好，世界！这是一段翻译测试。"
    }
}

#[tauri::command]
fn test_translate() -> TestOut {
    let cfg = config::load();
    let probe = probe_text(&cfg.to);
    let t0 = Instant::now();
    match translate::translate(&cfg, probe) {
        Ok(r) => TestOut {
            ok: true,
            message: r.dst,
            from: r.from,
            to: r.to,
            ms: t0.elapsed().as_millis() as u64,
        },
        Err(e) => TestOut {
            ok: false,
            message: e,
            from: String::new(),
            to: String::new(),
            ms: t0.elapsed().as_millis() as u64,
        },
    }
}

/// 测试某一套引擎配置（会话 32：卡片头 / 新建弹窗里的「测试」按钮）。
///
/// 传进来的可能是一份刚改过、甚至还没进列表的草稿，所以**不能**拿
/// `config::load()` 里那个 active_engine 去测。做法是 clone 当前配置，把两张
/// 列表换成「只有这一套」、把 active 指过去，再走正常的 translate()。
/// 全程不落盘，也不影响用户当前正在用的引擎。
///
/// 缓存必须临时关掉：cache_key 只含 engine.id() / 语向 / 原文，不含实例，
/// 否则连测两套凭据不同的百度实例会命中上一次的结果，测试就成假的了。
#[tauri::command]
fn test_engine_profile(profile: Value) -> TestOut {
    let cfg = config::load();
    let probe = probe_text(&cfg.to);
    let mut tmp = cfg.clone();
    tmp.cache_enabled = false;
    const ID: &str = "__probe__";
    if profile.get("vendor").is_some() {
        let mut p: config::VendorProfile = serde_json::from_value(profile).unwrap_or_default();
        p.id = ID.into();
        tmp.vendor_profiles = vec![p];
        tmp.openai_profiles.clear();
        tmp.active_engine = format!("vendor:{ID}");
    } else {
        let mut p: config::ModelProfile = serde_json::from_value(profile).unwrap_or_default();
        p.id = ID.into();
        tmp.openai_profiles = vec![p];
        tmp.vendor_profiles.clear();
        tmp.active_engine = format!("openai:{ID}");
    }
    let t0 = Instant::now();
    match translate::translate(&tmp, probe) {
        Ok(r) => TestOut {
            ok: true,
            message: r.dst,
            from: r.from,
            to: r.to,
            ms: t0.elapsed().as_millis() as u64,
        },
        Err(e) => TestOut {
            ok: false,
            message: e,
            from: String::new(),
            to: String::new(),
            ms: t0.elapsed().as_millis() as u64,
        },
    }
}

/// 前端首屏渲染完才把窗口显示出来（会话 32 修白屏）。
/// 普通窗口都是 `visible(false)` 建的：WebView2 首帧是白底，早 show 就会闪一下。
/// 划词弹窗走自己的 `popup_ready`，不走这里。
#[tauri::command]
fn window_ready(window: tauri::WebviewWindow) -> Result<(), String> {
    window.show().map_err(|e| format!("显示窗口失败：{e}"))?;
    // visible(false) 时 build 后那次 set_focus 是空操作，show 之后要补一次
    let _ = window.set_focus();
    Ok(())
}

/// 打开配置目录（文件资源管理器）。
#[tauri::command]
fn open_config_dir() -> SimpleOut {
    let dir = config::log_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return SimpleOut {
            ok: false,
            message: format!("无法创建配置目录：{e}"),
        };
    }
    // 用 explorer 打开比 std::fs 直接列目录更符合用户预期。
    // 失败时把真实路径告诉用户，让他手动去。
    match std::process::Command::new("explorer").arg(&dir).spawn() {
        Ok(_) => SimpleOut {
            ok: true,
            message: dir.display().to_string(),
        },
        Err(e) => SimpleOut {
            ok: false,
            message: format!("打开目录失败：{e}\n路径：{}", dir.display()),
        },
    }
}

/// 设置页启动时调用：取走「应该跳到哪个工具」。
#[tauri::command]
fn take_pending_tool() -> Option<String> {
    popup::take_pending_tool()
}

/// 打开 `config.json` 本身（用系统默认程序，通常是记事本 / VS Code）。
///
/// 用户 m02639 要求把侧栏那行配置路径移进「通用」，并加一个按钮直接打开文件。
/// 文件不存在时先写一份默认配置出来，否则点按钮只会弹一个「找不到文件」。
#[tauri::command]
fn open_config_file() -> SimpleOut {
    let p = config::config_path();
    if !p.exists() {
        // 先落一份默认配置。用户点这个按钮的意图是「看看配置长什么样」，
        // 给他一个空目录不如给他一份可读的示例。
        let cfg = config::load();
        if let Err(e) = config::save(&cfg) {
            return SimpleOut {
                ok: false,
                message: format!("配置文件不存在，尝试创建也失败了：{e}\n路径：{}", p.display()),
            };
        }
    }
    match std::process::Command::new("explorer").arg(&p).spawn() {
        Ok(_) => SimpleOut {
            ok: true,
            message: p.display().to_string(),
        },
        Err(e) => SimpleOut {
            ok: false,
            message: format!("打开配置文件失败：{e}\n路径：{}", p.display()),
        },
    }
}

/// 配置文件的真实路径，设置页底部显示用。
#[tauri::command]
fn config_path() -> String {
    config::config_path().display().to_string()
}

/// 热键注册失败的原因（空串表示正常），设置页红字用。
#[tauri::command]
fn hotkey_error() -> String {
    hotkey::last_error()
}

// ---------- 命令：翻译弹窗 ----------

/// 前端通知「我准备好了，可以发内容了」。
///
/// 必须等前端 await 完 listen 后再调，否则补发的 popup-render 会丢
/// （现象：弹窗永远停在"翻译中"，见 docs/stages/S1 §S1.6 坑 1）。
#[tauri::command]
fn popup_ready(app: AppHandle) {
    popup::ready(&app);
}

#[tauri::command]
fn resize_popup(win: tauri::WebviewWindow, height: f64, width: f64) {
    popup::resize(&win, height, width);
}

#[tauri::command]
fn close_popup(app: AppHandle) {
    popup::close(&app);
}

/// 「复制译文」的兜底路线：不可聚焦的窗口里 navigator.clipboard 可能被拒。
#[tauri::command]
fn copy_text(text: String) {
    capture::write_clipboard_text(&text);
}

/// 弹窗里点了图钉。**必须在主线程调用**（要改窗口置顶标志）。
#[tauri::command]
fn set_pinned(pinned: bool, app: AppHandle) {
    popup::set_pinned(&app, pinned);
}

/// 弹窗里在标题栏按下了鼠标左键 → 拖动窗口。
///
/// 底层是 `tao` 的 `handle_os_dragging`：`ReleaseCapture()` +
/// `PostMessage(WM_NCLBUTTONDOWN, HTCAPTION)`，交给系统进入窗口移动循环。
///
/// **不检查窗口是否 `focusable`**：这一步在 `WS_EX_NOACTIVATE` 窗口上也能工作
/// （系统移动循环不要求窗口可激活），实测划词模式的弹窗照样能拖。
#[tauri::command]
fn start_dragging(win: tauri::WebviewWindow) -> Result<(), String> {
    dlog::dlog("popup: start_dragging");
    win.start_dragging().map_err(|e| {
        let msg = format!("拖动失败：{e}");
        dlog::dlog(&format!("popup: {msg}"));
        msg
    })
}

/// 弹窗里点了引擎行里的某一项 → 切引擎并立刻重翻当前源文本。
///
/// 存进配置（用户切了引擎就是一次配置变更，下次热键还用这个），
/// 然后复用 `translate_text` 的逻辑重翻 —— 但那条命令要 AppHandle + 源文本，
/// 前端自己已经有源文本，所以这里只负责切引擎，重翻由前端再调一次 `translate_text`。
#[tauri::command]
fn switch_engine(engine_id: String) -> SimpleOut {
    let mut cfg = config::load();
    // 只在「用户自己配好的实例」里选：传一个不存在的 id 会被拒，
    // 不会把 active_engine 写成一个谁都解析不了的值。
    let Some((_, label)) = cfg
        .engine_choices()
        .into_iter()
        .find(|(id, _)| *id == engine_id)
    else {
        return SimpleOut {
            ok: false,
            message: format!("未知的翻译引擎「{engine_id}」"),
        };
    };
    cfg.active_engine = engine_id;
    if let Err(e) = config::save(&cfg) {
        return SimpleOut {
            ok: false,
            message: e,
        };
    }
    SimpleOut {
        ok: true,
        message: format!("已切换到{label}"),
    }
}

/// 弹窗里点了源文本框（划词模式）。门 3 选项 A：用户点了才允许抢焦点。
///
/// 不这么做的话，划词弹窗一出现就抢焦点，用户选中的文字会被清掉，
/// 而用户按热键的本意是"翻译我刚选的东西"，选中消失后连源文本都没了。
#[tauri::command]
fn focus_for_typing(app: AppHandle) {
    popup::focus_for_typing(&app);
}

/// 弹窗里的「设置」按钮：打开设置页并停在翻译这一栏。
#[tauri::command]
fn open_settings_from_popup(app: AppHandle) {
    crate::dlog::dlog("open_settings_from_popup: 命令到达");
    // 先关弹窗，再开设置页。
    //
    // 这不是"顺手关一下"：实测从弹窗自己的命令里创建第二个 WebView 窗口，
    // `WebviewWindowBuilder::build()` 会卡死（日志停在"主线程开始建窗"，
    // 设置窗口白页、进程无响应）。同一时刻用托盘菜单开设置页却完全正常，
    // 差别只在"命令来自弹窗自身"。产品语义上也不该两个窗口叠着——
    // 用户点设置就是要配置，弹窗留着没用。
    popup::close(&app);
    let a = app.clone();
    // 关窗是异步的（销毁 + WebView2 子进程退出），等它收尾再建新窗口。
    let _ = app.run_on_main_thread(move || {
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(250));
            let a2 = a.clone();
            let _ = a.run_on_main_thread(move || popup::open_settings_at(&a2, "translate"));
        });
    });
}

#[tauri::command]
fn open_settings_cmd(app: AppHandle) {
    // 与 `new_task_done` 同一个坑：从某条 IPC 回调里**同步**创建第二个 WebView
    // 窗口，controller 会卡在 0x0 —— 窗口标题有了、页面却是 `about:blank`
    // （会话 16 / 18 各踩一次）。排到工作线程睡 300ms 跨过当前事件循环 tick，
    // 再回主线程建窗。
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        let a = app.clone();
        let _ = app.run_on_main_thread(move || open_settings(&a));
    });
}

// ---------- 命令：下载主窗口（S3b 后端） ----------

/// 前端初始化时主动拉一次快照；之后靠 pump 的 `tasks:snapshot` 事件增量更新。
/// 没有 aria2c 时返回**空快照**而不是报错：「当前没任务」是正常状态。
#[tauri::command]
fn download_snapshot() -> Result<crate::download::rpc::Snapshot, String> {
    let mut snap = match crate::download::engine::rpc_handle() {
        Some(r) => r.snapshot()?,
        None => crate::download::rpc::Snapshot::default(),
    };
    // 把本地历史补进来：引擎空闲退出后，刚打开的窗口也要能看到下过的任务。
    crate::download::history::merge(&mut snap);
    Ok(snap)
}

/// 写操作统一入口：拿只读句柄 → 执行 → 唤醒 pump 立刻刷新一次。
fn with_download_rpc<T>(
    f: impl FnOnce(&crate::download::rpc::Rpc) -> Result<T, String>,
) -> Result<T, String> {
    let r = crate::download::engine::rpc_handle()
        .ok_or_else(|| "下载引擎还没启动（当前没有任务）".to_string())?;
    let out = f(&r)?;
    crate::download::pump::notify();
    Ok(out)
}

#[tauri::command]
fn download_pause(gid: String) -> Result<(), String> {
    with_download_rpc(|r| r.pause(&gid))
}

#[tauri::command]
fn download_unpause(gid: String) -> Result<(), String> {
    with_download_rpc(|r| r.unpause(&gid))
}

/// `06` §6.4.2 第 5 条要「删掉所有任务」，含已完成 / 出错的记录。
/// aria2 对这类记录调 `remove` 会报 GID not found，必须分流。
///
/// 会话 27 起：如果设置里打开了「删任务同时删文件」，这里连文件一起删
/// （默认进回收站，见 `fileops::delete`）。**前端已经做过二次确认**，
/// 后端不做弹窗——这是个 Tauri 命令，没有窗口可以弹。
#[tauri::command]
fn download_remove(gid: String) -> Result<(), String> {
    let cfg = config::load();
    // 删之前先把文件路径拿到手：历史与实时任务各查一次，删完就查不到了。
    let target = if cfg.delete_file_with_task {
        crate::download::engine::rpc_handle()
            .and_then(|r| r.snapshot().ok())
            .and_then(|s| s.tasks.into_iter().find(|t| t.gid == gid))
            .or_else(|| crate::download::history::find(&gid))
            .map(|t| t.file_path)
            .filter(|p| !p.trim().is_empty())
    } else {
        None
    };

    // 历史记录先删：它是本地数据，删了就不会再被 merge 补回来。
    let from_history = crate::download::history::remove(&gid);
    let live = with_download_rpc(|r| {
        let status = r
            .snapshot()?
            .tasks
            .into_iter()
            .find(|t| t.gid == gid)
            .map(|t| t.status)
            .unwrap_or_default();
        match crate::download::rpc::removal_method(&status) {
            "aria2.remove" => r.remove(&gid),
            _ => r.remove_result(&gid),
        }
    });
    let removed = match live {
        Ok(()) => Ok(()),
        // 只剩历史记录（引擎已退出或 gid 已不在 aria2）时，删历史就算成功。
        Err(_) if from_history => Ok(()),
        Err(e) => Err(e),
    };

    // 任务删掉了但文件删失败，不能整体报错——那是两件事，任务确实没了。
    if removed.is_ok() {
        if let Some(path) = target {
            let recycle = cfg.delete_mode != "permanent";
            if let Err(e) = crate::fileops::delete(&path, recycle) {
                crate::dlog::dlog(&format!("remove: 删文件失败（任务已删）：{e}"));
                return Err(format!("任务已删除，但文件没删掉：{e}"));
            }
        }
    }
    removed
}

#[tauri::command]
fn download_clear_finished() -> Result<(), String> {
    // 历史先清：即使 aria2 已经退出，按钮也应该能把列表清空。
    crate::download::history::clear_finished();
    match with_download_rpc(|r| r.purge()) {
        Ok(()) => Ok(()),
        // 引擎没在跑（只剩历史）时，清历史就是全部工作。
        Err(_) if crate::download::engine::rpc_handle().is_none() => {
            crate::download::pump::notify();
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// `06` §6.4.2 第 8 条：打开所在目录**并选中文件**。
/// `explorer /select,<绝对路径>` 才会在资源管理器里高亮文件；文件还没落盘
/// （下载中 / BT 取 metadata）时退回只打开目录，不硬造一个不存在的路径。
#[tauri::command]
fn download_open_dir(gid: String) -> Result<(), String> {
    let task = crate::download::engine::rpc_handle()
        .and_then(|r| r.snapshot().ok())
        .and_then(|s| s.tasks.into_iter().find(|t| t.gid == gid))
        // 历史记录也能「打开目录」：引擎退出后这是唯一能定位到文件的操作。
        .or_else(|| crate::download::history::find(&gid))
        .ok_or_else(|| "找不到这个任务".to_string())?;
    // aria2 的 files[0].path 用正斜杠（C:/Users/...）。explorer.exe 的 /select,
    // 参数遇到正斜杠会把路径当开关解析，会话 15 本机 A/B 实测：正斜杠打开的是
    // 「桌面」并选中 Documents，反斜杠才打开目标目录。这里统一转成 Windows 分隔符。
    let arg = if !task.file_path.is_empty() && std::path::Path::new(&task.file_path).is_file() {
        format!("/select,{}", task.file_path.replace('/', "\\"))
    } else {
        task.dir.replace('/', "\\")
    };
    if arg.trim().is_empty() {
        return Err("这个任务没有下载目录".to_string());
    }
    std::process::Command::new("explorer.exe")
        .arg(&arg)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("打开目录失败：{e}"))
}

// ---------- 命令：新建下载任务（会话 16） ----------

#[derive(serde::Serialize)]
struct DownloadAddFail {
    uri: String,
    error: String,
}

#[derive(serde::Serialize)]
struct DownloadAddOut {
    ok: usize,
    failed: Vec<DownloadAddFail>,
}

/// 新建页提交：逐条入队，失败的带原因返回，成功的只计数。
///
/// 不用 `?` 提前返回：用户一次贴了 5 个链接，第 2 个坏掉不该让另外 4 个也不下。
#[tauri::command]
fn download_add(tasks: Vec<crate::download::NewTask>) -> DownloadAddOut {
    let mut cfg = config::load();
    let mut ok = 0usize;
    let mut failed = Vec::new();
    for t in &tasks {
        match crate::download::add_task(&cfg, t) {
            Ok(_) => ok += 1,
            Err(error) => failed.push(DownloadAddFail {
                uri: t.uri.clone(),
                error,
            }),
        }
    }
    // 「记住上次保存位置」：只在真的入队成功时写，且只认用户填过的目录——
    // 留空 = 跟随默认目录，那种情况不该把默认路径固化成「上次用过」。
    if ok > 0 && cfg.remember_last_dir {
        if let Some(dir) = tasks
            .iter()
            .map(|t| t.dir.trim())
            .find(|d| !d.is_empty())
            .map(|d| d.replace('/', "\\"))
        {
            if dir != cfg.last_save_dir {
                cfg.last_save_dir = dir;
                let _ = config::save(&cfg);
            }
        }
    }
    crate::download::pump::notify();
    DownloadAddOut { ok, failed }
}

/// 种子任务页提交：一次一个 `.torrent`。返回 gid，失败时把原因交给前端显示。
///
/// 不做「批量种子」：种子文件是用户从磁盘逐个挑的，没有「一次贴多行」的场景；
/// 失败也不该像 `download_add` 那样把链接回填，直接报错让用户重选就行。
#[tauri::command]
fn download_add_torrent(task: crate::download::NewTorrent) -> Result<String, String> {
    let cfg = config::load();
    let st = crate::download::add_torrent(&cfg, &task)?;
    Ok(st.gid)
}

/// 新建任务页的默认值：目录 / 限速 / 连接数 / User-Agent。
///
/// 一次取回四个值而不是四次 invoke：新建页每次开窗都要这四项，串四个 IPC 会让
/// 窗口先画出空框再一个个跳。`ua` 为空表示「不干涉」——用 aria2 自带 UA，
/// 不冒充浏览器（会话 25 用户确认）。
#[derive(Serialize)]
struct TaskDefaultsOut {
    dir: String,
    speed: u32,
    connections: u32,
    ua: String,
}

#[tauri::command]
fn download_task_defaults() -> TaskDefaultsOut {
    let cfg = config::load();
    // 「记住上次保存位置」：上次那个目录还在就用它，目录被删了退回默认——
    // 否则新建页会预填一个不存在的路径，用户点新建才发现目录建不出来。
    let remembered = if cfg.remember_last_dir {
        let d = cfg.last_save_dir.trim();
        if !d.is_empty() && std::path::Path::new(d).is_dir() {
            Some(d.to_string())
        } else {
            None
        }
    } else {
        None
    };
    TaskDefaultsOut {
        dir: remembered.unwrap_or_else(crate::download::engine::default_download_dir),
        speed: cfg.max_speed_kb,
        // 界面上限是 16；手改配置越界时在这里夹一次，别把一个非法值送进 aria2。
        connections: cfg.max_connection_per_server.clamp(1, 16),
        ua: cfg.default_user_agent.clone(),
    }
}

/// 新建页取热键 / 扩展预填的条目（取走后清空）。
#[tauri::command]
fn take_new_task_prefill() -> Vec<crate::popup::NewTaskPrefill> {
    crate::popup::take_new_task_prefill()
}

/// 新建页点「新建」成功后：关掉自己，打开 / 聚焦列表窗口。
///
/// **不能在本条 IPC 的回调栈里同步建第二个窗口**：实测 new_task 的 WebView2
/// controller 还没完全初始化时，main 的 controller 会卡在 `0x0`——窗口标题
/// 有了、页面全白（`about:blank`，body 为空）。排到工作线程睡 300ms、跨过
/// 当前事件循环 tick 再回主线程建窗，就能正常加载。
#[tauri::command]
fn new_task_done(app: AppHandle) {
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        let a = app.clone();
        let _ = app.run_on_main_thread(move || crate::popup::close_new_task_and_show_list(&a));
    });
}

/// 新建页「取消」：只关自己，不打开列表。
#[tauri::command]
fn close_new_task(app: AppHandle) {
    let a = app.clone();
    let _ = app.run_on_main_thread(move || crate::popup::close_new_task(&a));
}

/// 新建页「重新获取」用的探测结果。
#[derive(serde::Serialize)]
struct ProbeOut {
    name: Option<String>,
    size: Option<u64>,
}

/// 从 `Content-Disposition` 取文件名。
///
/// 优先 `filename*=UTF-8''%E4%B8%AD.zip`（RFC 5987），其次 `filename="a.zip"`。
fn content_disposition_name(headers: &reqwest::header::HeaderMap) -> Option<String> {
    let raw = headers.get(reqwest::header::CONTENT_DISPOSITION)?;
    // 头里带未转义的非 ASCII 字节时 `to_str()` 会整条失败（少数老站点会这样），
    // 宽松解码兜住，再交给 naming 统一解析 + 清洗。
    let text = match raw.to_str() {
        Ok(s) => s.to_string(),
        Err(_) => String::from_utf8_lossy(raw.as_bytes()).to_string(),
    };
    crate::download::naming::from_content_disposition(&text)
}

/// `bytes 0-0/12345` → `12345`（`*` 表示总长未知）。
fn content_range_total(v: &str) -> Option<u64> {
    let total = v.rsplit('/').next()?.trim();
    if total == "*" {
        None
    } else {
        total.parse().ok()
    }
}

/// 读响应头里的 `Content-Length`。
///
/// **不能用 `resp.content_length()`**：reqwest 0.13 那个方法读的是 body 的
/// `size_hint().exact()`，HEAD 响应 body 为空，永远返回 `Some(0)` —— 实测踩过。
fn header_content_length(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|n| *n > 0)
}

/// 新建页探测：文件名 + 文件大小。只读取响应头，不下载正文。
///
/// **这个 client 不能 `.no_proxy()`**：探测的是外网 URL，要走用户的系统代理；
/// 与 `download::rpc::Rpc::new()` 那个回环 RPC 客户端正好相反（那个必须禁用代理）。
///
/// HEAD 拿不到（简单静态服务常返回 405/501）就退回 `GET` + `Range: bytes=0-0`，
/// 从 `Content-Range` 里读总长。
#[tauri::command]
fn download_probe(url: String) -> ProbeOut {
    let u = url.trim();
    let lower = u.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return ProbeOut {
            name: None,
            size: None,
        };
    }
    let is_loopback = lower.contains("://127.0.0.1") || lower.contains("://localhost");
    let builder = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(8));
    // 回环地址不能走系统代理（本机 HTTP_PROXY 会把 127.0.0.1 也发给代理），
    // 外网地址则必须走代理 —— 两种情况的 client 配置正好相反。
    let builder = if is_loopback { builder.no_proxy() } else { builder };
    let Ok(client) = builder.build() else {
        return ProbeOut {
            name: None,
            size: None,
        };
    };

    let mut name = None;
    let mut size = None;

    if let Ok(resp) = client.head(u).send() {
        if resp.status().is_success() {
            size = header_content_length(resp.headers());
            name = content_disposition_name(resp.headers());
        }
    }
    if size.is_none() {
        if let Ok(resp) = client.get(u).header("Range", "bytes=0-0").send() {
            // 服务器支持 Range 时返回 206：`Content-Length` 只是这 1 字节的长度，
            // 总长在 `Content-Range: bytes 0-0/12345` 里；不支持时返回 200 + 完整长度。
            size = resp
                .headers()
                .get("content-range")
                .and_then(|v| v.to_str().ok())
                .and_then(content_range_total)
                .or_else(|| header_content_length(resp.headers()));
            if name.is_none() {
                name = content_disposition_name(resp.headers());
            }
        }
    }
    if name.is_none() {
        name = crate::download::naming::file_name_from_uri(u);
    }
    ProbeOut { name, size }
}

/// 目录所在卷的可用字节数。新建页「可用 153.3 GB」用。
#[tauri::command]
fn disk_free(dir: String) -> Option<u64> {
    #[cfg(windows)]
    {
        use windows::core::HSTRING;
        use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
        let d = if dir.trim().is_empty() {
            crate::download::engine::default_download_dir()
        } else {
            dir.trim().to_string()
        };
        let mut avail = 0u64;
        let ok = unsafe {
            GetDiskFreeSpaceExW(
                &HSTRING::from(d.as_str()),
                Some(&mut avail as *mut u64),
                None,
                None,
            )
        };
        if ok.is_ok() {
            return Some(avail);
        }
    }
    #[cfg(not(windows))]
    let _ = dir;
    None
}

/// 新建页的「文件夹」按钮：在资源管理器里打开指定目录。
#[tauri::command]
fn open_path(path: String) {
    let p = path.trim();
    if p.is_empty() {
        return;
    }
    let _ = std::process::Command::new("explorer.exe").arg(p).spawn();
}

/// 「重启下载引擎」按钮：按当前设置重新拉起 ariac2。
///
/// 设置页的 Button 字段都是无参命令，所以单独包一层；返回 `SimpleOut`
/// 是为了复用设置页现成的结果框渲染（`renderResult`）。
#[tauri::command]
fn download_engine_restart() -> Result<SimpleOut, String> {
    let cfg = config::load();
    let message = crate::download::engine::restart(&cfg)?;
    Ok(SimpleOut { ok: true, message })
}

/// 任务列表要的两条界面开关：要不要二次确认、删任务会不会连文件一起删。
///
/// 单独一个命令而不是塞进 `snapshot`：后者是 aria2 的任务数据，
/// 混进配置项会让「历史合并」那条路径多一个不相干的字段。
#[derive(Serialize)]
struct UiFlagsOut {
    confirm: bool,
    delete_file: bool,
    delete_mode: String,
}

#[tauri::command]
fn download_ui_flags() -> UiFlagsOut {
    let cfg = config::load();
    UiFlagsOut {
        confirm: cfg.confirm_remove,
        delete_file: cfg.delete_file_with_task,
        delete_mode: cfg.delete_mode,
    }
}

/// 「发送测试通知」按钮。设置页的 Button 字段都是**无参命令**，所以单独包一层。
///
/// 不看「启用系统通知」开关：用户点测试就是要当场确认通道通不通，
/// 总开关关着也该发——否则点了没反应，看起来像坏了。
#[tauri::command]
fn notify_test(app: AppHandle) -> Result<(), String> {
    crate::download::notify::test(&app)
}

/// 「打开下载目录」按钮。目录不存在就先建，否则资源管理器会弹「找不到」。
#[tauri::command]
fn open_download_dir() -> Result<(), String> {
    let cfg = config::load();
    let dir = crate::download::engine::configured_download_dir(&cfg);
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建下载目录失败：{e}"))?;
    std::process::Command::new("explorer.exe")
        .arg(&dir)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("打开下载目录失败：{e}"))
}

/// 列表页「+」：打开新建任务窗口，**不关列表**（两窗并存，用户明确要求）。
///
/// 与 `new_task_done` 同理：第二个 WebviewWindow 要在当前 IPC tick 之后建，
/// 否则新窗口的 WebView2 controller 卡在 `0x0`。
#[tauri::command]
fn open_new_task(app: AppHandle) {
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        let a = app.clone();
        let _ = app.run_on_main_thread(move || crate::popup::open_new_task(&a, None));
    });
}

/// 自绘标题栏的统一窗口操作：min / max / close。
///
/// 用 `WebviewWindow` 参数拿「当前窗口」，前端不需要传 label，也不会误操作别的窗口。
/// 关闭走 `destroy()`（ADR-007）：`hide()` 会留下 6 个 msedgewebview2 子进程。
#[tauri::command]
fn window_action(window: tauri::WebviewWindow, action: String) -> Result<(), String> {
    match action.as_str() {
        "min" => window.minimize().map_err(|e| format!("最小化失败：{e}")),
        "max" => {
            let maxed = window.is_maximized().unwrap_or(false);
            if maxed {
                window.unmaximize().map_err(|e| format!("还原失败：{e}"))
            } else {
                window.maximize().map_err(|e| format!("最大化失败：{e}"))
            }
        }
        // 极轻模式开（默认）：销毁 WebView 立刻释放内存（ADR-007）。
        // 关掉后只隐藏：窗口还在，`open_*()` 直接 show() 复用现成的 WebView，
        // 下次打开不再重新加载页面，代价是内存常驻。
        "close" => {
            if config::load().lite_mode {
                window.destroy().map_err(|e| format!("关闭窗口失败：{e}"))
            } else {
                window.hide().map_err(|e| format!("隐藏窗口失败：{e}"))
            }
        }
        other => Err(format!("未知窗口操作：{other}")),
    }
}

// ---------- 命令：局域网互传（S4a/S4b）----------

/// 设备列表数据接口：S4a 的「设备列表有数据可查」。
/// 正式列表 UI 在 S4b，这里先把数据面放出来（设置页状态行也读同一份快照）。
#[tauri::command]
fn transfer_peers() -> Vec<transfer::Peer> {
    transfer::backend().peers()
}

/// 待确认的传入请求（最旧在前）。确认窗与互传视图都从这里拉。
#[tauri::command]
fn transfer_pending() -> Vec<transfer::IncomingRequest> {
    transfer::backend().pending_requests()
}

/// 主动刷新设备列表：重发 announce + 探测在线状态 + 剔除已离线设备。
/// 立即返回，结果通过 `transfer:event` 的 `peers` 事件推给前端。
#[tauri::command]
fn transfer_refresh() -> Result<(), String> {
    transfer::backend().refresh()
}

/// 当前 / 刚结束的传输快照。传输页开场拉一次，之后靠 `transfer:event` 增量更新。
#[tauri::command]
fn transfer_active() -> Vec<transfer::ActiveTransfer> {
    transfer::backend().active_transfers()
}

/// 取消一次**接收中**的会话（发送侧用 `transfer_cancel_send`，两条路最后都归到同一套事件）。
#[tauri::command]
fn transfer_cancel_receive(session_id: String) -> Result<(), String> {
    transfer::backend().cancel_receive(&session_id)
}

/// 选文件卡片要显示「文件 N 个 / 大小 X MB」，前端只有路径，大小在这里补。
/// 读不到（文件被删/无权限）就给 0，别让整张卡片报错。
#[tauri::command]
fn transfer_path_sizes(paths: Vec<String>) -> Vec<u64> {
    paths
        .iter()
        .map(|p| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0))
        .collect()
}

/// 一页传输历史（`page` 从 1 开始；越界会自动回到最后一页）。
#[derive(serde::Serialize)]
struct HistoryPage {
    items: Vec<transfer::history::HistoryEntry>,
    total: usize,
    page: usize,
    page_size: usize,
}

#[tauri::command]
fn transfer_history_page(page: usize, page_size: usize) -> HistoryPage {
    let (items, total) = transfer::history::page(page, page_size);
    let size = page_size.clamp(1, 200);
    // 后端可能把越界的页码收到最后一页，这里把「实际用的页码」算回给前端
    let want = page.max(1);
    let start = (want - 1) * size;
    let page_out = if start >= total && total > 0 {
        (total - 1) / size + 1
    } else {
        want
    };
    HistoryPage {
        items,
        total,
        page: page_out,
        page_size: size,
    }
}

/// 删一条历史。返回是否删到了（前端据此决定要不要重取当前页）。
#[tauri::command]
fn transfer_history_remove(id: String) -> bool {
    transfer::history::remove(&id)
}

/// 清空历史。
#[tauri::command]
fn transfer_history_clear() {
    transfer::history::clear();
}

/// 接受 / 拒绝一条传入请求（§S4.7.2 确认框的两个按钮）。
#[tauri::command]
fn transfer_respond(session_id: String, accept: bool) -> Result<(), String> {
    transfer::backend().respond(&session_id, accept)
}

/// 发送文件给一台设备。`target` = 设备指纹或 `ip:port`；异步跑，进度走事件。
#[tauri::command]
fn transfer_send(target: String, paths: Vec<String>, pin: Option<String>) -> Result<String, String> {
    let files: Vec<std::path::PathBuf> = paths.into_iter().map(Into::into).collect();
    transfer::backend()
        .send(&target, &files, pin)
        .map(|h| h.session_id)
}

/// 取消一次发送（接收侧取消是对方在自己界面上点）。
#[tauri::command]
fn transfer_cancel_send(session_id: String) -> Result<(), String> {
    transfer::send::cancel_send(&session_id)
}

/// 系统文件选择对话框（多选）。发送入口。
#[tauri::command]
fn transfer_pick_files() -> Result<Vec<String>, String> {
    fileops::pick_files()
}

/// 确认窗拉数据：取最早一条待确认请求。
#[tauri::command]
fn transfer_take_confirm() -> Option<transfer::IncomingRequest> {
    transfer::server::pending_list().into_iter().next()
}

/// 主窗口加载时取预选视图（托盘「局域网互传…」用）。
#[tauri::command]
fn transfer_take_view() -> Option<String> {
    popup::take_pending_main_view()
}

/// 取走拖放到主窗口的文件路径（事件只是叫醒，数据用「取走」语义）。
#[tauri::command]
fn transfer_take_drop() -> Vec<String> {
    popup::take_transfer_drop()
}

/// 互传「接收」页要展示的本机信息：对外显示名、接收目录、服务状态。
#[derive(Serialize)]
struct TransferLocalOut {
    alias: String,
    receive_dir: String,
    enabled: bool,
    status: String,
    /// 本机局域网地址（接收页显示「主机地址」用）。取第一个非回环 IPv4；
    /// 多网卡时按接口名排序取第一个，界面上只显示一个够用。
    #[serde(default)]
    ips: Vec<String>,
}

#[tauri::command]
fn transfer_local_info() -> TransferLocalOut {
    let cfg = config::load();
    let tc = transfer::TransferConfig::from_app(&cfg);
    let status = match transfer::server::state() {
        transfer::server::ServiceState::Listening { port } => format!("正在监听 · 端口 {port}"),
        transfer::server::ServiceState::Failed(e) => format!("启动失败：{e}"),
        transfer::server::ServiceState::Stopped => "服务未运行".to_string(),
    };
    // 只挑非回环、IPv4、非 link-local（169.254.x.x）的地址
    let mut ips: Vec<String> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| !i.is_loopback())
        .filter_map(|i| match i.ip() {
            std::net::IpAddr::V4(v4) if !v4.is_link_local() => Some(v4.to_string()),
            _ => None,
        })
        .collect();
    ips.sort();
    ips.dedup();

    TransferLocalOut {
        alias: tc.alias,
        receive_dir: tc.receive_dir.to_string_lossy().to_string(),
        enabled: cfg.enable_transfer,
        status,
        ips,
    }
}

/// 设置页「重启传输服务」：端口被占 / 服务异常时手动恢复。
#[tauri::command]
fn transfer_restart() -> Result<SimpleOut, String> {
    let cfg = config::load();
    if !cfg.enable_transfer {
        return Err("互传未启用，先打开上方的启用开关".to_string());
    }
    let tc = transfer::TransferConfig::from_app(&cfg);
    transfer::backend().stop()?;
    transfer::backend().start(&tc)?;
    Ok(SimpleOut {
        ok: true,
        message: "传输服务已重启".to_string(),
    })
}

/// 设置页「打开接收目录」：和下载页的「打开下载目录」同义。
#[tauri::command]
fn transfer_open_dir() -> Result<SimpleOut, String> {
    let cfg = config::load();
    let dir = transfer::server::resolve_receive_dir(&cfg);
    std::fs::create_dir_all(&dir).map_err(|e| format!("无法创建接收目录：{e}"))?;
    std::process::Command::new("explorer.exe")
        .arg(&dir)
        .spawn()
        .map_err(|e| format!("打开接收目录失败：{e}"))?;
    Ok(SimpleOut {
        ok: true,
        message: dir.display().to_string(),
    })
}

// ---------- 入口 ----------

fn main() {
    // 必须在任何 HTTP 客户端构建之前：局域网传输目标豁免代理（风险 55/62 同类坑）。
    transfer::send::ensure_lan_no_proxy();
    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![
            // 设置页（schema 驱动，加工具不用动这里）
            list_tools,
            get_tool_config,
            save_tool_config,
            set_tool_enabled,
            test_translate,
            test_engine_profile,
            open_config_dir,
            open_config_file,
            take_pending_tool,
            config_path,
            hotkey_error,
            // 翻译弹窗 / 窗口首帧
            popup_ready,
            window_ready,
            resize_popup,
            close_popup,
            copy_text,
            translate_text,
            switch_engine,
            set_pinned,
            start_dragging,
            focus_for_typing,
            open_settings_from_popup,
            open_settings_cmd,
            ocr_submit_region,
            ocr_cancel,
            // 下载主窗口（S3b 后端）
            download_snapshot,
            download_pause,
            download_unpause,
            download_remove,
            download_clear_finished,
            download_open_dir,
            // 新建下载任务（会话 16 / 会话 24 种子与高级选项）
            download_add,
            download_add_torrent,
            download_task_defaults,
            notify_test,
            download_ui_flags,
            download_engine_restart,
            open_download_dir,
            download_probe,
            disk_free,
            // 局域网互传（S4a 设备列表 + S4b 收发）
            transfer_peers,
            transfer_pending,
            transfer_refresh,
            transfer_active,
            transfer_cancel_receive,
            transfer_path_sizes,
            transfer_history_page,
            transfer_history_remove,
            transfer_history_clear,
            transfer_respond,
            transfer_send,
            transfer_cancel_send,
            transfer_pick_files,
            transfer_take_confirm,
            transfer_take_view,
            transfer_take_drop,
            transfer_local_info,
            transfer_restart,
            transfer_open_dir,
            open_path,
            take_new_task_prefill,
            new_task_done,
            close_new_task,
            open_new_task,
            // 自绘标题栏通用操作（所有无边框窗口共用）
            window_action
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            let cfg = config::load();

            // 开机自启自愈：配置是唯一真相，注册表只是它的投影。
            // 换过安装目录、被安全软件清掉、或用户手工删过，都在这里补齐 / 抹掉。
            // 失败只记日志——自启坏掉不该让程序起不来。
            crate::autostart::sync(cfg.autostart);

            // 点击通知的处理器：**只注册一次**，重复注册会让一次点击触发多次。
            // Windows 只在 toast 还停在屏幕上时上报点击，收进操作中心后就收不到了。
            crate::download::notify::register_click(&handle);

            tray::build(&handle, &cfg.hotkey)?;

            // 热键注册失败不阻断启动：托盘菜单仍然可用，设置页会显示红字原因。
            // 必须一次注册全部（划词 + 输入翻译）：register_hotkey 开头会
            // unregister_all，连着调两次会把前一个注销掉。
            let errs = register_hotkeys(&handle, &cfg);
            if errs.is_empty() {
                let extra = if cfg.enable_download {
                    format!(" / {}（下载）", cfg.hotkey_download)
                } else {
                    String::new()
                };
                dlog::dlog(&format!(
                    "setup: 热键 {} / {}{extra} 注册成功",
                    cfg.hotkey, cfg.hotkey_input
                ));
            } else {
                for e in &errs {
                    dlog::dlog(&format!("setup: {e}"));
                }
            }

            // S3b：轮询泵只发事件、不建窗。主窗口可见时 500ms、否则 2s；
            // 窗口 label 与 popup::open_main_window() 共用常量。
            {
                let for_emit = handle.clone();
                let for_visible = handle.clone();
                let for_rows = handle.clone();
                download::pump::start(
                    move |snap| {
                        // 不能 `let _ =`：事件发不出去时主窗口会永远停在第一帧，
                        // 用户看到的是「进度卡住」。把失败写进日志以便定位。
                        if let Err(e) = for_emit.emit("tasks:snapshot", snap) {
                            dlog::dlog(&format!("pump: 推送 tasks:snapshot 失败：{e}"));
                        }
                    },
                    move || {
                        for_visible
                            .get_webview_window(popup::MAIN_LABEL)
                            .and_then(|w| w.is_visible().ok())
                            .unwrap_or(false)
                    },
                    move |rows| {
                        // 历史落盘 + 系统通知都只在「终态变化」时真正动作，
                        // 500 ms 一轮不会变成 500 ms 一次磁盘写。
                        download::history::observe(rows);
                        download::notify::observe(&for_rows, rows);
                    },
                );
            }

            // 浏览器扩展接管（兼容 Rayburst-Connect v3）：只绑 127.0.0.1。
            // 端口占用等失败只写状态行与日志，不阻断启动。
            extension_bridge::start(handle.clone());

            // S4：局域网互传自启。发现类服务必须随进程常驻才有人找得到
            // （对方随时可能来连）；失败只记日志与状态行，不阻断启动。
            if cfg.enable_transfer {
                let tc = transfer::TransferConfig::from_app(&cfg);
                match transfer::backend().start(&tc) {
                    Ok(()) => dlog::dlog("setup: 传输服务已启动"),
                    Err(e) => dlog::dlog(&format!("setup: 传输服务启动失败：{e}")),
                }
            }

            // 传输事件出口：一律转发给前端（transfer:event）；
            // 收到传输请求时另开确认小窗（S4b，§S4.7.2）。
            {
                let for_emit = handle.clone();
                transfer::server::set_event_sink(move |evt| {
                    if matches!(
                        evt,
                        transfer::events::TransferEvent::Incoming { .. }
                    ) {
                        // 建窗必须在主线程，且避开 IPC 回调同 tick（风险 58）：
                        // 这里本来就在传输服务线程，跨个短 sleep 再丢回主线程。
                        let h = for_emit.clone();
                        std::thread::spawn(move || {
                            std::thread::sleep(std::time::Duration::from_millis(300));
                            let h2 = h.clone();
                            let _ = h.run_on_main_thread(move || {
                                popup::open_transfer_confirm(&h2);
                            });
                        });
                    }
                    if let Err(e) = for_emit.emit("transfer:event", &evt) {
                        dlog::dlog(&format!("transfer: 事件推送失败：{e}"));
                    }
                });
            }

            dlog::dlog("setup: 托盘已创建");
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("构建 Tauri 应用失败")
        .run(|app, event| match event {
            // 窗口必须等事件循环跑起来才能创建：在 setup() 里建窗会 os error 5。
            tauri::RunEvent::Ready => {
                let cfg = config::load();
                // 第一次运行没配引擎：直接把设置页打开，否则用户不知道该干什么。
                if !translate::engine_ready(&cfg) {
                    open_settings(app);
                }
            }
            // 常驻程序要挡住「最后一个窗口关掉就退出」的默认行为。
            // 真正退出时主动关掉 sidecar：--stop-with-process 是兜底，但主动
            // shutdown 才能让 aria2 把 session 写完，也不依赖父进程"死得干净"。
            tauri::RunEvent::ExitRequested { api, .. } => {
                if !QUITTING.load(Ordering::SeqCst) {
                    api.prevent_exit();
                } else {
                    // 先停泵，避免它拿着已经 shutdown 的 RPC 继续发快照。
                    download::pump::stop();
                    if let Err(e) = download::engine::shutdown_all() {
                        dlog::dlog(&format!("exit: 关闭 aria2c 失败：{e}"));
                    }
                    // 传输服务同理：主动停才能关掉 53317 的 TCP/UDP 监听。
                    if let Err(e) = transfer::backend().stop() {
                        dlog::dlog(&format!("exit: 关闭传输服务失败：{e}"));
                    }
                    // 「退出时清空已完成记录」：只清终态，出错的任务留着
                    // （用户可能正想排查它为什么失败）。
                    if config::load().clear_finished_on_exit {
                        crate::download::history::clear_finished();
                        dlog::dlog("exit: 已按设置清空已完成记录");
                    }
                }
            }
            _ => {}
        });
}

#[cfg(test)]
mod tests {
    use super::{apply_values, config_to_map, is_hotkey_key, probe_text, tool_value_keys};
    use crate::config::Config;
    use serde_json::json;

    /// 探针文案必须和目标语言不同语种，否则「测试一下」会 zh→zh 原样返回，
    /// 用户以为通了其实什么都没验证。
    #[test]
    fn probe_text_always_uses_a_different_language() {
        assert!(probe_text("zh").is_ascii(), "目标是中文时探针要用英文");
        assert!(!probe_text("en").is_ascii(), "目标是英文时探针要用中文");
        assert!(
            probe_text("zh-CN").is_ascii(),
            "带地区后缀的中文也要认出来，探针同样用英文"
        );
        assert!(probe_text("ja").starts_with('你'));
    }

    /// 部分保存：只改传进来的键，其余字段必须原样保留。
    #[test]
    fn apply_values_only_touches_given_keys() {
        let mut cfg = Config {
            appid: "old".into(),
            to: "zh".into(),
            ..Config::default()
        };
        let patch = match json!({ "appid": "new" }) {
            serde_json::Value::Object(m) => m,
            _ => unreachable!(),
        };
        apply_values(&mut cfg, &patch).unwrap();
        assert_eq!(cfg.appid, "new");
        assert_eq!(cfg.to, "zh", "没传的字段不能被清空");
    }

    /// 未知键被忽略而不是报错（旧/新前端混用的兼容性）。
    #[test]
    fn apply_values_ignores_unknown_keys() {
        let mut cfg = Config::default();
        let patch = match json!({ "not_a_real_field": 1 }) {
            serde_json::Value::Object(m) => m,
            _ => unreachable!(),
        };
        assert!(apply_values(&mut cfg, &patch).is_ok());
    }

    /// 类型对不上要报错，不能悄悄吞掉（否则用户保存了但没生效）。
    #[test]
    fn apply_values_rejects_wrong_type() {
        let mut cfg = Config::default();
        let patch = match json!({ "popup_timeout_ms": "not a number" }) {
            serde_json::Value::Object(m) => m,
            _ => unreachable!(),
        };
        assert!(apply_values(&mut cfg, &patch).is_err());
    }

    /// 每个工具都能报出自己那批 key，且翻译里包含热键字段。
    /// 会话 33：`lite_mode` 必须在保存白名单里，否则 `save_tool_config` 的
    /// 第 11 步（开回极轻模式时回收挂起窗口）永远拿不到这个 key。
    #[test]
    fn tool_value_keys_include_lite_mode() {
        assert!(tool_value_keys("general").contains(&"lite_mode"));
    }

    #[test]
    fn tool_value_keys_come_from_sections() {
        let keys = tool_value_keys("translate");
        assert!(keys.contains(&"hotkey"));
        // 会话 31 起引擎页签是「翻译模式 + 两张列表」
        assert!(keys.contains(&"active_engine"));
        assert!(keys.contains(&"to"));
        // 会话 31：厂商与模型都是列表字段，翻译模式单独存 active_engine
        assert!(keys.contains(&"vendor_profiles"));
        assert!(keys.contains(&"openai_profiles"));
        assert!(keys.contains(&"active_engine"));
        // 通用工具的字段不该出现在翻译里
        assert!(!keys.contains(&"popup_timeout_ms"));
    }

    #[test]
    fn tool_value_keys_unknown_tool_is_empty() {
        assert!(tool_value_keys("nope").is_empty());
    }

    #[test]
    fn hotkey_keys_are_recognized() {
        assert!(is_hotkey_key("hotkey"));
        assert!(is_hotkey_key("hotkey_ocr"));
        assert!(is_hotkey_key("hotkey_download"));
        assert!(!is_hotkey_key("appid"));
    }

    /// `config_to_map` 必须包含所有工具引用到的 key，否则前端读不到值。
    #[test]
    fn config_map_covers_all_tool_keys() {
        let cfg = Config::default();
        let map = config_to_map(&cfg);
        for tool in crate::tools::registry::all() {
            for s in tool.sections() {
                for f in &s.fields {
                    if matches!(
                        f.kind,
                        crate::tools::FieldKind::Note { .. }
                            | crate::tools::FieldKind::Button
                            | crate::tools::FieldKind::Group { .. }
                    ) {
                        continue;
                    }
                    assert!(
                        map.contains_key(f.key),
                        "工具 {} 的字段 {} 在 Config 里不存在",
                        tool.id(),
                        f.key
                    );

                }
            }
        }
    }
}
