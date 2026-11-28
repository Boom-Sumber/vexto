#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
// rust-i18n 生成的查表代码会触发 clippy::unnecessary_to_owned；这是宏展开的
// 假阳性，无法在调用点局部 allow（attribute 会被宏忽略）。
#![allow(clippy::unnecessary_to_owned)]

//! Vexto 入口。
//!
//! S2：多引擎 + 工具注册表 + schema 驱动的设置 IPC。
//!
//! 线程模型（三条边界，改动前先看 docs/03-ENGINEERING-STANDARDS.md §3.5）：
//! - 热键回调跑在**事件循环线程**上 → 只能 `std::thread::spawn` 出去，绝不能回调里做 UIA / 网络。
//! - 工作线程做完取词和翻译 → `app.run_on_main_thread()` 回主线程再碰窗口。
//! - `setup()` 里不能建窗（实测 os error 5），要等 `RunEvent::Ready`。

// 界面翻译：编译期从 src-tauri/locales/*.json 载入；缺失时先英文、再中文。
rust_i18n::i18n!("locales", fallback = ["en", "zh-CN"]);

mod autostart;
mod capture;
mod config;
mod dlog;
mod download;
mod extension_bridge;
mod fileops;
mod hotkey;
mod i18n;
mod lang;
mod mem;
mod ocr;
mod ocr_paddle;
mod popup;
mod region_select;
mod screenshot;
mod tools;
mod transfer;
mod tray;
mod translate;
mod update;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

use serde::Serialize;
use serde_json::{json, Map, Value};
use rust_i18n::t;
use tauri::{AppHandle, Emitter, Manager};

/// 托盘「退出」置位。没有它，`ExitRequested` 里的 `prevent_exit()` 会让进程永远关不掉。
pub static QUITTING: AtomicBool = AtomicBool::new(false);

/// 全局 AppHandle：翻译记录发生在没有窗口参数的调用链里（OCR / 后台线程），
/// 也要能广播 `translate:recorded` 通知工具箱刷新历史。
static APP: OnceLock<AppHandle> = OnceLock::new();

pub fn app_handle() -> Option<&'static AppHandle> {
    APP.get()
}

// ---------- 输出结构 ----------

#[derive(Serialize)]
struct SimpleOut {
    ok: bool,
    message: String,
}

/// 接收确认失败：前端按 `code` 判断，不匹配文案。
#[derive(Serialize)]
struct TransferErrorOut {
    code: String,
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
    /// 是否隐藏右侧通用页头（关于页用品牌区代替）
    hide_header: bool,
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
/// `popup_timeout_ms` 在「常规」里可以改，但 S1 起前端一直是硬编码 15000，
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
                t!("status.line.translateEngineMissing").into_owned()
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
        match translate::translate_recorded(&cfg, &src) {
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
        return Err(t!("error.translate.emptySelection").into_owned());
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
    let r = tauri::async_runtime::spawn_blocking(move || translate::translate_recorded(&cfg1, &t1))
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

/// 只翻译、不弹窗：截图预览里的 OCR 结果卡用。
#[tauri::command]
async fn translate_text_only(text: String) -> Result<Value, String> {
    let t = text.trim().to_string();
    if t.is_empty() {
        return Err(t!("error.translate.emptySelection").into_owned());
    }
    let cfg = config::load();
    let r = tauri::async_runtime::spawn_blocking(move || translate::translate_recorded(&cfg, &t))
        .await
        .map_err(|e| format!("翻译任务未能启动：{e}"))??;
    Ok(json!({ "dst": r.dst, "from": r.from, "to": r.to }))
}

/// 翻译历史：全部记录（最新在前）。筛选与分页在前端做（最多 500 条）。
#[tauri::command]
fn translate_history_list() -> Vec<translate::history::HistoryEntry> {
    translate::history::list()
}

#[tauri::command]
fn translate_history_remove(id: String) -> bool {
    translate::history::remove(&id)
}

#[tauri::command]
fn translate_history_clear() {
    translate::history::clear();
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
                    | tools::FieldKind::ButtonRow { .. }
                    | tools::FieldKind::Group { .. }
                    | tools::FieldKind::About { .. }
                    | tools::FieldKind::ToolList
            ) {
                continue;
            }
            keys.push(f.key);
        }
    }
    keys
}

/// 哪些 key 是热键字段，需要格式校验 + 整体重新注册。
/// `hotkey_ocr` / `hotkey_screenshot` 只在工具启用时注册。
const HOTKEY_KEYS: [&str; 7] = [
    "hotkey",
    "hotkey_input",
    "hotkey_download",
    "hotkey_ocr",
    "hotkey_screenshot",
    "hotkey_fullscreen",
    "hotkey_toolbox",
];

fn is_hotkey_key(k: &str) -> bool {
    HOTKEY_KEYS.contains(&k)
}

/// 工具启停字段：值变了要同步托盘菜单项的可用状态。
const ENABLE_KEYS: [&str; 4] = [
    "enable_translate",
    "enable_screenshot",
    "enable_download",
    "enable_transfer",
];

fn hotkey_label(key: &str) -> String {
    match key {
        "hotkey" => t!("schema.field.general.hotkey.label").into_owned(),
        "hotkey_input" => t!("schema.field.general.hotkey_input.label").into_owned(),
        "hotkey_screenshot" => t!("schema.field.general.hotkey_screenshot.label").into_owned(),
        "hotkey_fullscreen" => t!("schema.field.general.hotkey_fullscreen.label").into_owned(),
        "hotkey_toolbox" => t!("schema.field.general.hotkey_toolbox.label").into_owned(),
        "hotkey_download" => t!("schema.field.general.hotkey_download.label").into_owned(),
        other => other.to_string(),
    }
}

/// 前端可能提交新对象，也可能提交旧字符串（旧页面/手改配置）。
fn parse_hotkey_value(value: &Value) -> Result<hotkey::HotkeyBinding, String> {
    let binding = match value.as_str() {
        Some(text) => hotkey::HotkeyBinding::from_legacy(text),
        None => serde_json::from_value(value.clone())
            .map_err(|_| t!("error.hotkey.invalidValue").into_owned())?,
    };
    binding.validate()
}

/// 保存期查重：只看已启用工具的热键（停用工具的绑定保留但不注册、不显示）。
fn duplicate_hotkey_message(patch: &Map<String, Value>) -> Option<String> {
    let cfg = config::load();
    let mut bindings: Vec<(&'static str, hotkey::HotkeyBinding)> = Vec::new();
    let mut push = |key: &'static str, fallback: &hotkey::HotkeyBinding| {
        let value = patch
            .get(key)
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_else(|| fallback.clone());
        bindings.push((key, value));
    };
    // 工具箱热键不属于任何工具，始终参与查重
    push("hotkey_toolbox", &cfg.hotkey_toolbox);
    if cfg.enable_translate {
        push("hotkey", &cfg.hotkey);
        push("hotkey_input", &cfg.hotkey_input);
    }
    if cfg.enable_screenshot {
        push("hotkey_screenshot", &cfg.hotkey_screenshot);
        push("hotkey_fullscreen", &cfg.hotkey_fullscreen);
    }
    if cfg.enable_download {
        push("hotkey_download", &cfg.hotkey_download);
    }
    let items: Vec<(&'static str, &hotkey::HotkeyBinding)> =
        bindings.iter().map(|(key, value)| (*key, value)).collect();
    hotkey::find_duplicate(&items).map(|(first, second, combo)| {
        t!(
            "error.hotkey.duplicateFields",
            first = hotkey_label(first),
            second = hotkey_label(second),
            combo = combo
        )
        .into_owned()
    })
}

/// 一次性注册所有全局热键（划词 / 输入翻译 / 下载）。
///
/// **必须走 `hotkey::register_all`**，不能连着调两次 `register_hotkey`——
/// 后者开头会 `unregister_all()`，第二次调用会把第一次注册的划词热键注销掉。
/// 下载热键只在工具启用时注册：停用后按了不该有反应（任务书 §S3.8b 验收 9.3.2）。
fn register_hotkeys(app: &AppHandle, cfg: &config::Config) -> Vec<String> {
    let mut list: Vec<hotkey::Binding> = Vec::new();
    // 工具箱不挂在任何工具下，始终注册
    list.push((
        "工具箱热键",
        cfg.hotkey_toolbox.clone(),
        Box::new(|a: &AppHandle| crate::open_download_window(a)),
    ));
    // 翻译工具停用后，划词 / 输入翻译热键不应继续占用系统组合（工具管理统一控制）。
    if cfg.enable_translate {
        list.push((
            "划词热键",
            cfg.hotkey.clone(),
            Box::new(|a: &AppHandle| translate_selection(a.clone())),
        ));
        list.push((
            "输入翻译热键",
            cfg.hotkey_input.clone(),
            Box::new(|a: &AppHandle| open_input_translate(a.clone())),
        ));
    }
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
    // S7 截图：区域截图热键，工具启用时才注册（ADR-040）。
    // 滚动截图进行中时，这个热键变成「停止」——半自动模式下用户双手在滚动，
    // 不一定要去点小窗上的按钮。
    if cfg.enable_screenshot {
        list.push((
            "截图热键",
            cfg.hotkey_screenshot.clone(),
            Box::new(|a: &AppHandle| {
                if crate::screenshot::scroll::is_running() {
                    crate::screenshot::scroll::request_stop();
                } else {
                    crate::popup::open_shot_overlay(a);
                }
            }),
        ));
        // S7 会话 72：PrintScreen 全屏热键。与设置页/托盘全屏共用
        // capture_full_feedback（抓图 → 自动交付 → 右下角缩略图反馈）。
        list.push((
            "全屏截图热键",
            cfg.hotkey_fullscreen.clone(),
            Box::new(|a: &AppHandle| {
                let app = a.clone();
                std::thread::spawn(move || {
                    if let Err(e) = crate::screenshot::capture_full_feedback(&app) {
                        crate::dlog::dlog(&format!("screenshot: 全屏热键失败：{e}"));
                        crate::screenshot::notify_error(&app, &e);
                    }
                });
            }),
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

// ---------- 命令：S7 截图 ----------

/// 框选浮层初始化：快照 URL / 物理尺寸 / 虚拟屏原点 / 边框宽度。
/// 快照由 `region_select::open` 在创建窗口前抓取（见 overlay.rs）。
#[tauri::command]
fn shot_overlay_state() -> Value {
    crate::screenshot::overlay::state_value()
}

/// 选区动作：save / path / clipboard / default / pin / preview / ocr。
///
/// 坐标是浮层内的 CSS 像素；后端按快照物理像素裁切，用户看到什么就裁什么。
/// 钉屏 / 预览成功后由这里排主线程建窗（会话 62 坑 11）。
/// 光标下的窗口高亮（区域截图浮层用）。Win32 枚举/命中放阻塞线程，
/// 前端节流调用；返回屏幕物理像素矩形，由页面换算成 CSS。
#[tauri::command]
async fn shot_window_at() -> Value {
    match tauri::async_runtime::spawn_blocking(crate::screenshot::window_pick::pick_at_cursor).await
    {
        Ok(value) => value,
        Err(e) => json!({ "ok": false, "message": format!("窗口检测失败：{e}") }),
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SelectionArgs {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    dpr: Option<f64>,
    scale_x: Option<f64>,
    scale_y: Option<f64>,
}

/// 对当前选区执行动作（保存 / 复制 / 钉屏 / 预览）。
#[tauri::command]
async fn shot_action(action: String, sel: SelectionArgs, app: AppHandle) -> Value {
    let act = action.clone();
    let selection = crate::screenshot::overlay::Selection {
        x: sel.x,
        y: sel.y,
        w: sel.w,
        h: sel.h,
        dpr: sel.dpr,
        scale: sel.scale_x.zip(sel.scale_y),
    };
    let result = tauri::async_runtime::spawn_blocking(move || {
        crate::screenshot::overlay::apply(&act, selection)
    })
    .await;
    match result {
        Ok(Ok(out)) => {
            if let Some(label) = out.pin_label.clone() {
                schedule_pin_window(&app, label);
            } else if action == "preview" {
                schedule_window(&app, crate::popup::open_preview_window);
            } else if matches!(action.as_str(), "clipboard" | "save_copy") {
                schedule_window(&app, crate::popup::open_feedback_window);
            }
            json!({
                "ok": true,
                "message": out.message,
                "path": out.path.as_ref().map(|p| p.to_string_lossy().to_string()),
                "text": out.text,
            })
        }
        Ok(Err(e)) => json!({ "ok": false, "message": e }),
        Err(e) => json!({ "ok": false, "message": format!("选区动作失败：{e}") }),
    }
}

/// 结果框动作（滚动长图 / 全屏结果）：保存 / 复制 / 路径 / 钉屏 / 预览 / 完成。
#[tauri::command]
async fn result_action(action: String, app: AppHandle) -> Value {
    let act = action.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        crate::screenshot::overlay::apply_result(&act, &config::load())
    })
    .await;
    match result {
        Ok(Ok(out)) => {
            if let Some(label) = out.pin_label.clone() {
                schedule_pin_window(&app, label);
            } else if action == "preview" {
                schedule_window(&app, crate::popup::open_preview_window);
            }
            json!({
                "ok": true,
                "message": out.message,
                "path": out.path.as_ref().map(|p| p.to_string_lossy().to_string()),
                "text": out.text,
            })
        }
        Ok(Err(e)) => json!({ "ok": false, "message": e }),
        Err(e) => json!({ "ok": false, "message": format!("结果动作失败：{e}") }),
    }
}

/// 全屏截图右下角缩略图的状态。
#[tauri::command]
fn feedback_state() -> Value {
    crate::screenshot::feedback::state_value()
}

/// 缩略图反馈窗的复制图片 / 钉住 / 预览。
#[tauri::command]
async fn feedback_action(action: String, app: AppHandle) -> Value {
    let act = action.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        crate::screenshot::feedback::apply(&act, &config::load())
    })
    .await;
    match result {
        Ok(Ok(out)) => {
            if let Some(label) = out.pin_label.clone() {
                schedule_pin_window(&app, label);
            } else if action == "preview" {
                schedule_window(&app, crate::popup::open_preview_window);
            }
            json!({ "ok": true, "message": out.message })
        }
        Ok(Err(e)) => json!({ "ok": false, "message": e }),
        Err(e) => json!({ "ok": false, "message": format!("反馈动作失败：{e}") }),
    }
}

/// 缩略图从全屏收缩到右下角。前端在图片加载完后调用一次。
#[tauri::command]
fn feedback_start_animation(app: AppHandle) {
    let Some(win) = app.get_webview_window(crate::popup::FEEDBACK_LABEL) else {
        return;
    };
    let (vx, vy, vw, vh) = crate::ocr::virtual_screen();
    let dpr = win.scale_factor().unwrap_or(1.0);
    let (iw, ih) = crate::screenshot::feedback::image_size().unwrap_or((16, 9));
    // 缩略图窗口按图片比例算，避免 16:9 的图放进 4:3 窗口出现白边。
    let scale = (360.0 / iw as f64)
        .min(260.0 / ih as f64)
        .min(1.0);
    let final_w = ((iw as f64 * scale).max(120.0) * dpr).round() as i32;
    let final_h = ((ih as f64 * scale).max(80.0) * dpr).round() as i32;
    let margin = (16.0 * dpr).round() as i32;
    // 用工作区（排除任务栏）的右下角，避免缩略图被任务栏挡住。
    let (wx, wy, ww, wh) = work_area(app.primary_monitor().ok().flatten())
        .unwrap_or((vx, vy, vw, vh));
    let fx = wx + ww - final_w - margin;
    let fy = wy + wh - final_h - margin;
    let steps = 18i32;
    for i in 1..=steps {
        let t = i as f64 / steps as f64;
        // ease-out：先快后慢
        let e = 1.0 - (1.0 - t) * (1.0 - t);
        let w = (vw as f64 + (final_w - vw) as f64 * e).round() as i32;
        let h = (vh as f64 + (final_h - vh) as f64 * e).round() as i32;
        let x = (vx as f64 + (fx - vx) as f64 * e).round() as i32;
        let y = (vy as f64 + (fy - vy) as f64 * e).round() as i32;
        let _ = win.set_size(tauri::PhysicalSize::new(w.max(1) as u32, h.max(1) as u32));
        let _ = win.set_position(tauri::PhysicalPosition::new(x, y));
        std::thread::sleep(std::time::Duration::from_millis(13));
    }
    let _ = win.set_ignore_cursor_events(false);
}

/// Tauri 的 Monitor 工作区（物理像素，已排除任务栏）。
fn work_area(monitor: Option<tauri::Monitor>) -> Option<(i32, i32, i32, i32)> {
    let m = monitor?;
    let wa = m.work_area();
    Some((
        wa.position.x,
        wa.position.y,
        wa.size.width as i32,
        wa.size.height as i32,
    ))
}

/// 关闭缩略图反馈窗并清理暂存图。
#[tauri::command]
fn feedback_close(app: AppHandle) {
    crate::popup::close_feedback_window(&app);
    crate::screenshot::feedback::clear();
}

/// 关掉截图 overlay（Esc / 成功后的自动关闭）。
#[tauri::command]
fn shot_cancel(app: AppHandle) {
    crate::popup::close_shot_overlay(&app);
}

/// 全屏截图（设置页按钮）。与 PrintScreen 热键、托盘全屏共用同一套逻辑：
/// 抓图 → 按「截图后」配置自动交付 → 右下角缩略图反馈。
#[tauri::command]
async fn shot_full(app: AppHandle) -> SaveOut {
    let r = tauri::async_runtime::spawn_blocking(move || {
        crate::screenshot::capture_full_feedback(&app)
    })
    .await;
    match r {
        Ok(Ok(message)) => SaveOut {
            ok: true,
            message,
            warnings: vec![],
        },
        Ok(Err(e)) => SaveOut {
            ok: false,
            message: e,
            warnings: vec![],
        },
        Err(e) => SaveOut {
            ok: false,
            message: format!("全屏截图任务失败：{e}"),
            warnings: vec![],
        },
    }
}

/// 活动窗口截图（托盘 / 设置页按钮）：同上，结果框里确认后再保存。
#[tauri::command]
async fn shot_window(app: AppHandle) -> SaveOut {
    shot_capture_preview(crate::screenshot::Mode::ActiveWindow, app).await
}

async fn shot_capture_preview(mode: crate::screenshot::Mode, app: AppHandle) -> SaveOut {
    let r = tauri::async_runtime::spawn_blocking(move || {
        let img = crate::screenshot::grab(mode)?;
        let (w, h) = (img.width as u32, img.height as u32);
        let path = crate::screenshot::preview::stage_image(&img)?;
        Ok::<_, String>((path, w, h))
    })
    .await;
    match r {
        Ok(Ok((path, w, h))) => {
            crate::screenshot::overlay::set_result(
                path,
                w,
                h,
                crate::screenshot::overlay::ResultKind::Full,
            );
            schedule_window(&app, |a| {
                crate::region_select::open_result(a, crate::region_select::Purpose::Shot)
            });
            SaveOut {
                ok: true,
                message: t!("status.screenshotPreviewOpened").into_owned(),
                warnings: vec![],
            }
        }
        Ok(Err(e)) => SaveOut {
            ok: false,
            message: e,
            warnings: vec![],
        },
        Err(e) => SaveOut {
            ok: false,
            message: format!("截图任务失败：{e}"),
            warnings: vec![],
        },
    }
}

/// 滚动截图：浮层选完区域后调用。
///
/// 浮层**不再销毁**：保留边框作为捕获范围提示，窗口进入点击穿透 +
/// `WDA_EXCLUDEFROMCAPTURE`，边框不会进入长图。结束后同一浮层显示结果缩略图
/// 与动作栏，用户确认后才复制到输出目录。
#[tauri::command]
fn scroll_start(
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    dpr: Option<f64>,
    overlay: Option<String>,
    app: AppHandle,
) {
    let (px, py, pw, ph) = crate::region_select::physical_rect(x, y, w, h, dpr);
    let purpose = if overlay.as_deref() == Some("shot") {
        crate::region_select::Purpose::Shot
    } else {
        crate::region_select::Purpose::Scroll
    };
    crate::region_select::set_capture_mode(&app, purpose, true);
    let dpr = dpr.filter(|d| d.is_finite() && *d > 0.0).unwrap_or(1.0);

    // 与 `open_settings_cmd` 同一个坑：在 IPC 回调里**同步**建第二个 WebView，
    // controller 会卡在 0x0 —— 窗口有了、页面却是 about:blank（会话 16/18
    // 各踩一次，会话 62 自测又踩一次）。排到工作线程睡 300ms 跨过当前事件
    // 循环 tick，再回主线程建窗。
    {
        let app_win = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            let a = app_win.clone();
            let _ = app_win.run_on_main_thread(move || {
                crate::popup::open_scroll_control(&a, (px, py, pw, ph), dpr);
            });
        });
    }

    let app2 = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let cfg = config::load();
        let mut on_progress = |p: crate::screenshot::scroll::Progress| {
            let stage = match p.stage {
                crate::screenshot::scroll::Stage::Capturing => "capturing",
                crate::screenshot::scroll::Stage::Writing => "writing",
            };
            let _ = app2.emit_to(
                crate::popup::SCROLL_CONTROL_LABEL,
                "scroll:progress",
                json!({
                    "stage": stage,
                    "frames": p.frames,
                    "height": p.height,
                    "added": p.added,
                }),
            );
        };
        let done = crate::screenshot::scroll::run(
            crate::screenshot::scroll::Rect {
                x: px,
                y: py,
                w: pw,
                h: ph,
            },
            &cfg,
            &mut on_progress,
        );
        crate::dlog::warn(&format!("scroll: {}", done.message));
        if let Some(path) = done.path.clone() {
            // 失败但只有第一帧：这是「滚动失败回退第一帧」，不是长图，
            // 允许复制图片；真正拼出多帧的长图仍禁止复制（ADR-043）。
            let kind = if done.status == crate::screenshot::scroll::Status::Failed
                && done.frames <= 1
            {
                crate::screenshot::overlay::ResultKind::Full
            } else {
                crate::screenshot::overlay::ResultKind::Scroll
            };
            crate::screenshot::overlay::set_result(
                path,
                done.width.max(1) as u32,
                done.height.max(1) as u32,
                kind,
            );
        }
        let url = done
            .path
            .as_ref()
            .and_then(|p| crate::screenshot::preview::display_url(p));
        let payload = json!({
            "ok": done.path.is_some(),
            "status": done.status.as_str(),
            "message": done.message,
            "url": url,
            "width": done.width,
            "height": done.height,
            "frames": done.frames,
        });
        // 先恢复浮层交互，再把结果发给浮层；否则结果框还会点击穿透。
        crate::region_select::set_capture_mode(&app2, purpose, false);
        let label = purpose.label();
        let _ = app2.emit_to(label, "scroll:done", payload.clone());
        let _ = app2.emit_to(crate::popup::SCROLL_CONTROL_LABEL, "scroll:done", payload);
        std::thread::sleep(std::time::Duration::from_millis(250));
        let a = app2.clone();
        let _ = app2.run_on_main_thread(move || crate::popup::close_scroll_control(&a));
    });
}

/// 停止滚动截图（小窗按钮 / 截图热键）。
#[tauri::command]
fn scroll_stop() {
    crate::screenshot::scroll::request_stop();
}

/// 关掉滚动截图框选浮层（Esc / 太小的误触框）。
#[tauri::command]
fn scroll_cancel(app: AppHandle) {
    crate::region_select::set_capture_mode(&app, crate::region_select::Purpose::Scroll, false);
    crate::popup::close_scroll_overlay(&app);
}

/// 关掉滚动截图进度窗。
#[tauri::command]
fn scroll_close(app: AppHandle) {
    crate::popup::close_scroll_control(&app);
}

// ---------- 命令：S7 钉屏 + 预览（会话 63） ----------

/// 建第二个 WebView（钉屏 / 预览）必须排到主线程：IPC 回调里同步
/// `WebviewWindowBuilder::build()` 会得到 `about:blank`（会话 62 坑 11）。
fn schedule_window<F>(app: &AppHandle, f: F)
where
    F: FnOnce(&AppHandle) + Send + 'static,
{
    let a = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        let b = a.clone();
        let _ = a.run_on_main_thread(move || f(&b));
    });
}

/// 打开某个 label 的钉屏窗口（多窗口模式每个新图一个 label）。
fn schedule_pin_window(app: &AppHandle, label: String) {
    schedule_window(app, move |a| {
        crate::popup::open_pin_window_for(a, &label);
    });
}

/// 钉屏：单窗多图时加入并切换；关掉「单窗口多图」时每张图开一个新窗口。
fn pin_path(app: &AppHandle, path: &std::path::Path) -> SimpleOut {
    let single = config::load().pinscreen_single_window;
    match crate::screenshot::pin::add(path, single) {
        Ok((label, _, _)) => {
            if let Some(win) = app.get_webview_window(&label) {
                let _ = win.show();
                let _ = win.set_focus();
                crate::screenshot::pin::fit_window(&win);
                crate::screenshot::pin::emit_state(&win);
            } else {
                schedule_pin_window(app, label);
            }
            SimpleOut {
                ok: true,
                message: t!("status.pinned").into_owned(),
            }
        }
        Err(e) => SimpleOut {
            ok: false,
            message: e,
        },
    }
}

#[tauri::command]
fn pin_add(path: String, app: AppHandle) -> SimpleOut {
    pin_path(&app, std::path::Path::new(&path))
}

/// 钉屏窗口内切换上一张 / 下一张（多窗口模式每窗只有一张，等于无操作）。
#[tauri::command]
fn pin_step(window: tauri::WebviewWindow, delta: i32) -> SimpleOut {
    if crate::screenshot::pin::step(window.label(), delta).is_none() {
        return SimpleOut {
            ok: false,
            message: t!("error.pin.noImage").into_owned(),
        };
    }
    crate::screenshot::pin::fit_window(&window);
    crate::screenshot::pin::emit_state(&window);
    SimpleOut {
        ok: true,
        message: String::new(),
    }
}

/// 钉屏滚轮缩放：`delta > 0` 放大、`< 0` 缩小；`reset = true` 回到 100%。
#[tauri::command]
fn pin_zoom(window: tauri::WebviewWindow, delta: i32, reset: Option<bool>) -> SimpleOut {
    match crate::screenshot::pin::zoom_window(&window, delta, reset.unwrap_or(false)) {
        Some(percent) => SimpleOut {
            ok: true,
            message: format!("{percent}%"),
        },
        None => SimpleOut {
            ok: false,
            message: t!("error.pin.noImage").into_owned(),
        },
    }
}

/// 钉屏控制点缩放：按百分比绝对设置。
#[tauri::command]
fn pin_set_zoom(window: tauri::WebviewWindow, zoom: f64) -> SimpleOut {
    match crate::screenshot::pin::set_zoom(&window, zoom) {
        Some(percent) => SimpleOut {
            ok: true,
            message: format!("{percent}%"),
        },
        None => SimpleOut {
            ok: false,
            message: t!("error.pin.noImage").into_owned(),
        },
    }
}

/// 钉屏操作条：复制当前窗口的图片到剪贴板。
#[tauri::command]
fn pin_copy(window: tauri::WebviewWindow) -> SimpleOut {
    match crate::screenshot::pin::copy_current(window.label()) {
        Ok(message) => SimpleOut { ok: true, message },
        Err(e) => SimpleOut {
            ok: false,
            message: e,
        },
    }
}

/// 钉屏操作条：另存为（当前图片复制到用户选择的位置）。
#[tauri::command]
async fn pin_save_as(window: tauri::WebviewWindow) -> SimpleOut {
    let label = window.label().to_string();
    let r = tauri::async_runtime::spawn_blocking(move || {
        crate::screenshot::pin::save_as_current(&label)
    })
    .await;
    match r {
        Ok(Ok(message)) => SimpleOut { ok: true, message },
        Ok(Err(e)) => SimpleOut {
            ok: false,
            message: e,
        },
        Err(e) => SimpleOut {
            ok: false,
            message: t!("error.saveTaskFailed", detail = e).into_owned(),
        },
    }
}

/// 钉屏操作条：用当前图片打开预览窗。
#[tauri::command]
fn pin_preview(window: tauri::WebviewWindow, app: AppHandle) -> SimpleOut {
    let Some((path, _, _)) = crate::screenshot::pin::current(window.label()) else {
        return SimpleOut {
            ok: false,
            message: t!("error.pin.noImage").into_owned(),
        };
    };
    crate::screenshot::preview::set_source(path);
    schedule_window(&app, crate::popup::open_preview_window);
    SimpleOut {
        ok: true,
        message: t!("status.previewOpened").into_owned(),
    }
}

#[tauri::command]
fn pin_state(window: tauri::WebviewWindow) -> Value {
    crate::screenshot::pin::state_value(window.label())
}

#[tauri::command]
fn pin_close(window: tauri::WebviewWindow) {
    let _ = window.destroy();
}

/// 打开截图预览（暂存 + asset protocol 放行，不发图片字节）。
fn preview_path(app: &AppHandle, path: &std::path::Path) -> SimpleOut {
    match crate::screenshot::preview::stage_file(path) {
        Ok(staged) => {
            crate::screenshot::preview::set_source(staged);
            if let Some(win) = app.get_webview_window(crate::popup::PREVIEW_LABEL) {
                let _ = win.show();
                let _ = win.set_focus();
                crate::screenshot::preview::emit_state(&win);
            } else {
                schedule_window(app, crate::popup::open_preview_window);
            }
            SimpleOut {
                ok: true,
                message: t!("status.previewOpened").into_owned(),
            }
        }
        Err(e) => SimpleOut {
            ok: false,
            message: e,
        },
    }
}

#[tauri::command]
fn preview_open(path: String, app: AppHandle) -> SimpleOut {
    preview_path(&app, std::path::Path::new(&path))
}

#[tauri::command]
fn preview_state() -> Value {
    crate::screenshot::preview::state_value()
}

/// 预览页显示的截图保存目录（与「保存」动作一致）。
#[tauri::command]
fn preview_save_dir() -> Value {
    let dir = crate::screenshot::output::resolve_dir(&config::load());
    json!(dir.to_string_lossy().to_string())
}

/// 截图图库：列出截图目录里的图片。缩略图由 `vextogallery` 协议按需生成。
#[tauri::command]
fn screenshot_gallery_list() -> Vec<crate::screenshot::gallery::GalleryItem> {
    crate::screenshot::gallery::list()
}

#[tauri::command]
fn screenshot_gallery_remove(path: String) -> SimpleOut {
    match crate::screenshot::gallery::remove(&path) {
        Ok(()) => SimpleOut {
            ok: true,
            message: String::new(),
        },
        Err(e) => SimpleOut {
            ok: false,
            message: e,
        },
    }
}

#[tauri::command]
fn screenshot_gallery_copy(path: String) -> SimpleOut {
    match crate::screenshot::gallery::copy(&path) {
        Ok(()) => SimpleOut {
            ok: true,
            message: t!("status.imageCopied").into_owned(),
        },
        Err(e) => SimpleOut {
            ok: false,
            message: e,
        },
    }
}

#[tauri::command]
fn screenshot_gallery_preview(path: String, app: AppHandle) -> SimpleOut {
    match crate::screenshot::gallery::png_target(&path) {
        Ok(png) => preview_path(&app, &png),
        Err(e) => SimpleOut {
            ok: false,
            message: e,
        },
    }
}

#[tauri::command]
fn screenshot_gallery_pin(path: String, app: AppHandle) -> SimpleOut {
    match crate::screenshot::gallery::png_target(&path) {
        Ok(png) => pin_path(&app, &png),
        Err(e) => SimpleOut {
            ok: false,
            message: e,
        },
    }
}

/// 打开预览页显示的截图保存目录。
#[tauri::command]
fn preview_open_save_dir() -> SimpleOut {
    let dir = crate::screenshot::output::resolve_dir(&config::load());
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return SimpleOut {
            ok: false,
            message: t!(
                "error.createScreenshotDir",
                path = dir.display(),
                detail = e
            )
            .into_owned(),
        };
    }
    match std::process::Command::new("explorer").arg(&dir).spawn() {
        Ok(_) => SimpleOut {
            ok: true,
            message: t!("status.openedPath", path = dir.display()).into_owned(),
        },
        Err(e) => SimpleOut {
            ok: false,
            message: t!("error.openDir", detail = e).into_owned(),
        },
    }
}

/// 接收预览页画布合成的 PNG（原始字节，不走 base64），替换当前预览源。
/// 之后的存盘 / 复制 / 钉屏 / OCR 继续按文件路径走既有链路。
#[tauri::command]
fn preview_commit_edited(request: tauri::ipc::Request<'_>) -> Value {
    let bytes = match request.body() {
        tauri::ipc::InvokeBody::Raw(bytes) => bytes,
        _ => {
            return json!({
                "ok": false,
                "message": "编辑结果必须以二进制 PNG 提交",
            })
        }
    };
    match crate::screenshot::preview::commit_edited_png(bytes) {
        Ok(path) => {
            let (w, h) = crate::screenshot::preview::image_size(&path).unwrap_or((0, 0));
            json!({
                "ok": true,
                "message": format!("已应用标注（{w}×{h}）"),
                "state": crate::screenshot::preview::state_value(),
            })
        }
        Err(e) => json!({ "ok": false, "message": e }),
    }
}

/// 选区工具条提交编辑结果：先落暂存，再由 `shot_edited_action` 执行动作。
#[tauri::command]
fn shot_edited_commit(request: tauri::ipc::Request<'_>) -> Value {
    let bytes = match request.body() {
        tauri::ipc::InvokeBody::Raw(bytes) => bytes,
        _ => {
            return json!({
                "ok": false,
                "message": "编辑结果必须以二进制 PNG 提交",
            })
        }
    };
    match crate::screenshot::preview::stage_edited_png(bytes) {
        Ok(path) => {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            json!({ "ok": true, "name": name })
        }
        Err(e) => json!({ "ok": false, "message": e }),
    }
}

#[tauri::command]
async fn shot_edited_action(action: String, name: String, app: AppHandle) -> Value {
    let act = action.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        crate::screenshot::preview::apply_staged_action(&act, &name)
    })
    .await;
    match result {
        Ok(Ok(out)) => {
            if let Some(label) = out.pin_label.clone() {
                schedule_pin_window(&app, label);
            } else if action == "preview" {
                schedule_window(&app, crate::popup::open_preview_window);
            } else if matches!(action.as_str(), "copy" | "save") {
                schedule_window(&app, crate::popup::open_feedback_window);
            }
            json!({
                "ok": true,
                "message": out.message,
                "path": out.path.as_ref().map(|p| p.to_string_lossy().to_string()),
            })
        }
        Ok(Err(e)) => json!({ "ok": false, "message": e }),
        Err(e) => json!({ "ok": false, "message": format!("编辑动作失败：{e}") }),
    }
}

/// 对预览图执行存盘 / 复制 / 钉屏 / OCR。裁切与解码都放到阻塞线程。
#[tauri::command]
async fn preview_action(
    head: u32,
    tail: u32,
    left: Option<u32>,
    right: Option<u32>,
    action: String,
    app: AppHandle,
) -> Value {
    let l = left.unwrap_or(0);
    let r = right.unwrap_or(0);
    let result = tauri::async_runtime::spawn_blocking(move || {
        crate::screenshot::preview::apply(&action, head, tail, l, r)
    })
    .await;
    match result {
        Ok(Ok(out)) => {
            // 钉屏动作带回窗口 label；窗口仍要按坑 11 排到主线程建。
            if let Some(label) = out.pin_label.clone() {
                schedule_pin_window(&app, label);
            }
            json!({
                "ok": true,
                "message": out.message,
                "path": out.path.as_ref().map(|p| p.to_string_lossy().to_string()),
                "text": out.text,
            })
        }
        Ok(Err(e)) => json!({ "ok": false, "message": e }),
        Err(e) => json!({ "ok": false, "message": format!("预览动作失败：{e}") }),
    }
}

#[tauri::command]
fn preview_close(app: AppHandle) {
    crate::popup::close_preview_window(&app);
}

// ---------- 命令：设置页 ----------

/// 设置页左列表的唯一数据源。
#[tauri::command]
fn list_tools() -> Vec<Value> {
    tools::registry::all()
        .into_iter()
        .map(|t| {
            let mut out = serde_json::to_value(ToolOut {
                id: t.id(),
                name: t.name(),
                desc: t.desc(),
                icon: t.icon(),
                enabled: t.enabled(),
                switchable: t.switchable(),
                status_line: t.status_line(),
                status_kind: t.status_kind(),
                hide_header: t.hide_header(),
                sections: t.sections(),
            })
            .unwrap_or(Value::Null);
            crate::i18n::localize_tool(&mut out);
            out
        })
        .collect()
}

/// 前端挂载前问一次：把配置里的 `system` 解析成实际语言。
#[tauri::command]
fn get_ui_locale() -> String {
    crate::i18n::current().to_string()
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
            // 截图保存目录：与下载 / 互传同一口径，留空时显示系统「图片\Vexto」。
            if k == "screenshot_dir" {
                if let Some(s) = v.as_str() {
                    if s.trim().is_empty() {
                        v = Value::String(
                            crate::screenshot::output::default_dir()
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
    let mut normalized: std::collections::HashMap<String, Value> =
        std::collections::HashMap::new();
    for (k, v) in &values {
        if !is_hotkey_key(k) {
            continue;
        }
        let binding = match parse_hotkey_value(v) {
            Ok(binding) => binding,
            Err(e) => {
                return SaveOut {
                    ok: false,
                    message: e,
                    warnings,
                };
            }
        };
        let value = match serde_json::to_value(&binding) {
            Ok(value) => value,
            Err(e) => {
                return SaveOut {
                    ok: false,
                    message: e.to_string(),
                    warnings,
                };
            }
        };
        normalized.insert(k.clone(), value);
    }

    // 2) 只保留该工具声明的 key，并写入归一化后的热键
    let allowed = tool_value_keys(&tool_id);
    if allowed.is_empty() && tools::registry::by_id(&tool_id).is_none() {
        return SaveOut {
            ok: false,
            message: t!("error.unknownTool", id = tool_id).into_owned(),
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
                patch.insert(k.clone(), h.clone());
            }
            None => {
                patch.insert(k.clone(), v.clone());
            }
        }
    }

    // 2.1) 跨模块重复组合：只检查本次保存涉及热键时，避免无关字段被历史重复值阻塞。
    if HOTKEY_KEYS.iter().any(|k| patch.contains_key(*k)) {
        if let Some(message) = duplicate_hotkey_message(&patch) {
            return SaveOut {
                ok: false,
                message,
                warnings,
            };
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

    // 截图保存目录同理：清空后保存也回写成「图片\Vexto」。
    if patch
        .get("screenshot_dir")
        .and_then(|v| v.as_str())
        .is_some_and(|s| s.trim().is_empty())
    {
        patch.insert(
            "screenshot_dir".to_string(),
            Value::String(
                crate::screenshot::output::default_dir()
                    .to_string_lossy()
                    .to_string(),
            ),
        );
    }

    // 截图命名规则先校验：非法规则一个字节都不写进配置，
    // 否则用户会以为保存成功、截图却换了名字或直接失败（会话 81）。
    if let Some(Value::String(rule)) = patch.get("screenshot_name_rule") {
        if let Err(e) = crate::screenshot::output::render_name(rule) {
            return SaveOut {
                ok: false,
                message: e,
                warnings,
            };
        }
    }

    // 3) 目录字段：不存在就提示（不阻塞保存，用户可能马上就要去建）
    for k in ["download_dir", "receive_dir", "screenshot_dir"] {
        if let Some(Value::String(s)) = patch.get(k) {
            let s = s.trim();
            if !s.is_empty() && !std::path::Path::new(s).is_dir() {
                warnings.push(
                    t!("warn.configDirMissing", path = s).into_owned(),
                );
            }
        }
    }

    // 3.5) 多播地址是服务启动参数，非法值别写进配置（否则下次启动直接失败）。
    if let Some(Value::String(s)) = patch.get("transfer_multicast") {
        let s = s.trim();
        if !s.is_empty() && s.parse::<std::net::Ipv4Addr>().is_err() {
            return SaveOut {
                ok: false,
                message: t!("warn.multicastInvalid", value = s).into_owned(),
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
    // 日志开关 / 等级立即生效，不必重启。
    dlog::configure(cfg.log_enabled, &cfg.log_level);

    // 4) 副作用：热键重新注册。
    //    任一热键（或 OCR 启用开关）变了都要**整体重注册**：register_all
    //    开头会 unregister_all，只重注册单个的话，另一个会被静默注销掉
    //    （S1 起就埋着的雷）。`enable_ocr` 决定 OCR 热键要不要注册。
    let hotkey_reregister = patch.contains_key("enable_ocr")
        || patch.contains_key("enable_screenshot")
        || HOTKEY_KEYS.iter().any(|k| patch.contains_key(*k));
    if hotkey_reregister {
        let errs = register_hotkeys(&app, &cfg);
        // 注册失败不回滚保存：用户填的组合确实是他想要的，只是撞了别的软件。
        // 回滚会让他以为"保存失败"，下次还得重填一遍。
        warnings.extend(errs);
    }

    // 5) 副作用：工具启停字段变了要同步托盘菜单项的可用状态
    if ENABLE_KEYS.iter().any(|k| patch.contains_key(*k)) {
        tray::refresh_tool_states();
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
            warnings.push(t!("warn.autostartFailed", detail = e).into_owned());
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

    // 10b) 界面语言变了：更新 Rust 全局 locale、重建托盘、更新窗口标题，
    //      再通知前端。前端收到后即时重渲染；controller 页面自行 reload。
    if patch.contains_key("ui_language") {
        crate::i18n::set_locale(&cfg.ui_language);
        crate::tray::apply_locale(&app);
        crate::i18n::apply_window_titles(&app);
        let _ = app.emit(
            "i18n:changed",
            serde_json::json!({ "locale": crate::i18n::current() }),
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
            t!("warn.sidecarRestartNeeded").into_owned(),
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
            warnings.push(
                t!("warn.transferRestartFailed", detail = e).into_owned(),
            );
        }
    }

    SaveOut {
        ok: true,
        message: t!("status.saved").into_owned(),
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
                message: t!("error.unknownTool", id = tool_id).into_owned(),
                warnings: vec![],
            }
        }
    };
    if !enabled {
        // 四个功能工具至少留一个：全停用后工具箱没有任何可用入口。
        // 必须在 set_enabled 之前判断——它会直接落盘。
        let remaining = tools::registry::all()
            .iter()
            .filter(|t| t.switchable() && t.id() != tool_id.as_str() && t.enabled())
            .count();
        if remaining == 0 {
            return SaveOut {
                ok: false,
                message: t!("error.tool.lastOne").into_owned(),
                warnings: vec![],
            };
        }
    }
    if let Err(e) = tool.set_enabled(enabled) {
        return SaveOut {
            ok: false,
            message: e,
            warnings: vec![],
        };
    }
    let mut warnings: Vec<String> = Vec::new();
    if tool_id == "translate" {
        // 翻译启停改变「要不要注册划词 / 输入翻译热键」，必须整体重注册。
        warnings.extend(register_hotkeys(&app, &config::load()));
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
    if tool_id == "screenshot" {
        // 截图启停同样改变热键集合；停用时把可能开着的框选浮层一起收掉。
        warnings.extend(register_hotkeys(&app, &config::load()));
        if !enabled {
            crate::popup::close_shot_overlay(&app);
        }
    }
    // 托盘右键菜单里这个工具的入口跟着置灰 / 恢复。
    tray::refresh_tool_states();
    SaveOut {
        ok: true,
        message: if enabled {
            t!("status.toolEnabled", name = tool.name()).into_owned()
        } else {
            t!("status.toolDisabled", name = tool.name()).into_owned()
        },
        warnings,
    }
}

/// 取一组热键字段的出厂值。`None` = 有 key 不在 Config 里（写错名字）。
fn default_hotkey_patch(keys: &[&str]) -> Option<Map<String, Value>> {
    let defaults = config_to_map(&config::Config::default());
    let mut patch = Map::new();
    for key in keys {
        patch.insert((*key).to_string(), defaults.get(*key)?.clone());
    }
    Some(patch)
}

/// 恢复某一组快捷键的出厂默认值（ADR-092）。
///
/// 复用 `save_tool_config`：校验、跨模块查重、落盘、重新注册都在那里，
/// 默认值也不会绕过「组合已被占用」的提示。
fn reset_hotkeys(app: &AppHandle, keys: &[&str]) -> SaveOut {
    let Some(patch) = default_hotkey_patch(keys) else {
        return SaveOut {
            ok: false,
            message: t!("error.hotkey.invalidValue").into_owned(),
            warnings: vec![],
        };
    };
    save_tool_config("general".to_string(), patch, app.clone())
}

#[tauri::command]
fn reset_hotkeys_translate(app: AppHandle) -> SaveOut {
    reset_hotkeys(&app, &["hotkey", "hotkey_input"])
}

#[tauri::command]
fn reset_hotkeys_screenshot(app: AppHandle) -> SaveOut {
    reset_hotkeys(&app, &["hotkey_screenshot", "hotkey_fullscreen"])
}

#[tauri::command]
fn reset_hotkeys_download(app: AppHandle) -> SaveOut {
    reset_hotkeys(&app, &["hotkey_download"])
}

#[tauri::command]
fn reset_hotkeys_toolbox(app: AppHandle) -> SaveOut {
    reset_hotkeys(&app, &["hotkey_toolbox"])
}

/// 恢复全部出厂配置并重启（ADR-092）。
///
/// 只重写 config.json：下载历史、截图目录里的文件都不动。重启让下载引擎、
/// 互传服务、语言、主题、托盘全部按初始配置重新初始化，避免留下半套旧状态。
#[tauri::command]
fn reset_all_config(app: AppHandle) -> SaveOut {
    if let Err(e) = config::save(&config::Config::default()) {
        return SaveOut {
            ok: false,
            message: e,
            warnings: vec![],
        };
    }
    dlog::dlog("config: 已恢复默认配置，准备重启");
    // 先让 IPC 回包送到前端，再退出重启；用户已经在弹框里确认过这一步。
    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        crate::tray::restart_app(&handle);
    });
    SaveOut {
        ok: true,
        message: t!("status.configReset").into_owned(),
        warnings: vec![],
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
            message: t!("error.createConfigDir", detail = e).into_owned(),
        };
    }
    // 用 explorer 打开比 std::fs 直接列目录更符合用户预期。
    // 失败时把真实路径告诉用户，让他手动去。
    match std::process::Command::new("explorer").arg(&dir).spawn() {
        Ok(_) => SimpleOut {
            ok: true,
            message: t!("status.configDirOpened").into_owned(),
        },
        Err(e) => SimpleOut {
            ok: false,
            message: t!("error.openDirWithPath", detail = e, path = dir.display())
                .into_owned(),
        },
    }
}

/// 关于页内容：品牌、功能、链接、许可证、第三方组件。
#[tauri::command]
fn about_info() -> tools::about::AboutInfo {
    tools::about::info()
}

/// 关于页外链：只接受固定 id，不接受任意 URL。
#[tauri::command]
fn about_open_link(kind: String) -> Result<(), String> {
    tools::about::open_link(&kind)
}

/// Vexto Connect 的安装入口（ADR-094）。
const EXTENSION_EDGE_URL: &str =
    "https://microsoftedge.microsoft.com/addons/detail/vexto-connect/cigeilefcndlknedmmadpefnhknjicgb";
const EXTENSION_FIREFOX_URL: &str = "https://addons.mozilla.org/firefox/addon/vexto-connect/";
const EXTENSION_CHROME_ZIP_URL: &str =
    "https://github.com/Ruxsmiles/vexto-connect/releases/latest/download/vexto-connect-chrome.zip";

fn open_extension(url: &str, ok_message: String) -> SimpleOut {
    match crate::update::open_url(url) {
        Ok(()) => SimpleOut {
            ok: true,
            message: ok_message,
        },
        Err(e) => SimpleOut {
            ok: false,
            message: e,
        },
    }
}

#[tauri::command]
fn open_extension_edge() -> SimpleOut {
    open_extension(
        EXTENSION_EDGE_URL,
        t!("status.extensionPageOpened").into_owned(),
    )
}

#[tauri::command]
fn open_extension_firefox() -> SimpleOut {
    open_extension(
        EXTENSION_FIREFOX_URL,
        t!("status.extensionPageOpened").into_owned(),
    )
}

#[tauri::command]
fn download_extension_chrome() -> SimpleOut {
    open_extension(
        EXTENSION_CHROME_ZIP_URL,
        t!("status.extensionZipStarted").into_owned(),
    )
}

/// 复制诊断信息：版本 / 系统 / 配置与日志路径 / 更新状态，不含凭据。
#[tauri::command]
fn about_diagnostics() -> String {
    tools::about::diagnostics()
}

/// 设置页启动时调用：取走「应该跳到哪个工具」。
#[tauri::command]
fn take_pending_tool() -> Option<String> {
    popup::take_pending_tool()
}

#[tauri::command]
fn take_pending_section() -> Option<String> {
    popup::take_pending_section()
}

/// 打开 `config.json` 本身（用系统默认程序，通常是记事本 / VS Code）。
///
/// 用户 m02639 要求把侧栏那行配置路径移进「常规」，并加一个按钮直接打开文件。
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
                message: t!(
                    "error.configFileMissing",
                    detail = e,
                    path = p.display()
                )
                .into_owned(),
            };
        }
    }
    match std::process::Command::new("explorer").arg(&p).spawn() {
        Ok(_) => SimpleOut {
            ok: true,
            message: t!("status.configFileOpened").into_owned(),
        },
        Err(e) => SimpleOut {
            ok: false,
            message: t!(
                "error.openFileWithPath",
                detail = e,
                path = p.display()
            )
            .into_owned(),
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

/// 从设置窗口打开 Vexto 工具箱。建窗同样要跨过当前 IPC tick。
#[tauri::command]
fn open_toolbox(app: AppHandle) {
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        let a = app.clone();
        let _ = app.run_on_main_thread(move || crate::open_download_window(&a));
    });
}

/// 工具箱空态里的「去开启」：打开设置并落在常规 → 工具管理。
#[tauri::command]
fn open_tool_settings(app: AppHandle) {
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        let a = app.clone();
        let _ = app.run_on_main_thread(move || {
            popup::open_settings_at_section(&a, "general", "tools")
        });
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
        return Err(t!("error.downloadNoDir").into_owned());
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

/// 下载页「GitHub 加速」：查询一个公开仓库的 Releases。
///
/// 网络请求放 `spawn_blocking`，不占 Tauri 的异步执行线程；配置在进线程前
/// 读一次，避免请求过程中用户改设置导致前后不一致。
#[tauri::command]
async fn github_releases(
    repo: String,
    all: bool,
    page: u32,
    refresh: bool,
) -> Result<crate::download::github::ReleasesOut, String> {
    let cfg = config::load();
    match tauri::async_runtime::spawn_blocking(move || {
        crate::download::github::fetch(&cfg, &repo, all, page, refresh)
    })
    .await
    {
        Ok(r) => r,
        Err(e) => Err(format!("GitHub 解析失败：{e}")),
    }
}

/// GitHub 资源的「下载」：直接入队，不再打开新建任务页。
///
/// 目录 / 限速 / UA 使用下载设置里的默认值；主地址与备用镜像一起交给
/// aria2。前端收到成功后自己切到「全部」分类。
#[tauri::command]
fn github_download(url: String, filename: String) -> Result<(), String> {
    crate::download::github::validate_asset_url(&url)?;
    let cfg = config::load();
    if !cfg.enable_download {
        return Err(t!("error.downloadNotEnabled").into_owned());
    }
    let name = crate::download::naming::sanitize(&filename);
    if name.is_empty() {
        return Err(t!("error.githubBadFileName").into_owned());
    }
    let mut candidates = crate::download::github::download_candidates(&url, &cfg)?;
    let uri = candidates.remove(0);
    let task = crate::download::NewTask {
        uri,
        name,
        mirrors: candidates,
        ..Default::default()
    };
    crate::download::add_task(&cfg, &task)?;
    Ok(())
}

/// GitHub「clone 仓库」：选父目录，后台 git clone，完成后返回路径。
///
/// `None` 表示用户取消了文件夹选择。clone 可能跑几十秒到几分钟，
/// 前端用常驻 toast 展示进度，命令本身不阻塞 UI。
#[tauri::command]
async fn github_clone(
    repo: String,
) -> Result<Option<crate::download::github::CloneResult>, String> {
    let cfg = config::load();
    let repo = crate::download::github::parse_repo(&repo)?;
    let parent = tauri::async_runtime::spawn_blocking(crate::fileops::pick_folder)
        .await
        .map_err(|e| format!("打开文件夹选择框失败：{e}"))??;
    let Some(parent) = parent else {
        return Ok(None);
    };
    let result = tauri::async_runtime::spawn_blocking(move || {
        crate::download::github::clone_repo(&repo, &parent, &cfg)
    })
    .await
    .map_err(|e| format!("clone 线程失败：{e}"))??;
    Ok(Some(result))
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
fn notify_test(app: AppHandle) -> Result<SimpleOut, String> {
    crate::download::notify::test(&app)?;
    Ok(SimpleOut {
        ok: true,
        message: t!("status.testNotificationSent").into_owned(),
    })
}

/// 「打开下载目录」按钮。目录不存在就先建，否则资源管理器会弹「找不到」。
#[tauri::command]
fn open_download_dir() -> Result<SimpleOut, String> {
    let cfg = config::load();
    let dir = crate::download::engine::configured_download_dir(&cfg);
    std::fs::create_dir_all(&dir)
        .map_err(|e| t!("error.createDownloadDir", detail = e).into_owned())?;
    std::process::Command::new("explorer.exe")
        .arg(&dir)
        .spawn()
        .map_err(|e| t!("error.openDownloadDir", detail = e).into_owned())?;
    Ok(SimpleOut {
        ok: true,
        message: t!("status.downloadDirOpened").into_owned(),
    })
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
        // 极轻模式开：销毁 WebView 立刻释放内存（ADR-007）。
        // 关掉后只隐藏：窗口还在，`open_*()` 直接 show() 复用现成的 WebView，
        // 下次打开不再重新加载页面，代价是内存常驻（会话 90 起默认关）。
        "close" => {
            // 安装包已下载：关闭设置窗口等同于「立即更新」，由更新模块接管。
            if window.label() == popup::SETTINGS_LABEL
                && crate::update::install_if_ready(window.app_handle())
            {
                return Ok(());
            }
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
fn transfer_respond(session_id: String, accept: bool) -> Result<(), TransferErrorOut> {
    transfer::backend()
        .respond(&session_id, accept)
        .map_err(|e| TransferErrorOut {
            code: e.code().to_string(),
            message: e.message(),
        })
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

/// 预览截图命名规则生成的示例文件名（设置页「预览」按钮）。
#[tauri::command]
fn preview_screenshot_name(rule: String) -> Result<String, String> {
    crate::screenshot::output::render_name(&rule)
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
        transfer::server::ServiceState::Listening { port } => {
            t!("status.line.transferListeningPort", port = port).into_owned()
        }
        transfer::server::ServiceState::Failed(e) => {
            t!("status.line.transferStartFailed", message = e).into_owned()
        }
        transfer::server::ServiceState::Stopped => {
            t!("status.line.transferNotRunning").into_owned()
        }
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
        return Err(t!("error.transferNotEnabled").into_owned());
    }
    let tc = transfer::TransferConfig::from_app(&cfg);
    transfer::backend().stop()?;
    transfer::backend().start(&tc)?;
    Ok(SimpleOut {
        ok: true,
        message: t!("status.transferRestarted").into_owned(),
    })
}

/// 设置页「打开接收目录」：和下载页的「打开下载目录」同义。
#[tauri::command]
fn transfer_open_dir() -> Result<SimpleOut, String> {
    let cfg = config::load();
    let dir = transfer::server::resolve_receive_dir(&cfg);
    std::fs::create_dir_all(&dir)
        .map_err(|e| t!("error.createReceiveDir", detail = e).into_owned())?;
    std::process::Command::new("explorer.exe")
        .arg(&dir)
        .spawn()
        .map_err(|e| t!("error.openReceiveDir", detail = e).into_owned())?;
    Ok(SimpleOut {
        ok: true,
        message: t!("status.receiveDirOpened").into_owned(),
    })
}

// ---------- 入口 ----------

fn main() {
    // 必须在任何 HTTP 客户端构建之前：局域网传输目标豁免代理（风险 55/62 同类坑）。
    transfer::send::ensure_lan_no_proxy();
    tauri::Builder::default()
        // 单实例（S6）：第二个实例唤起已有窗口；没有窗口就打开设置页。
        // 必须在其它插件之前注册（Tauri 官方要求）。
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window(crate::popup::MAIN_LABEL) {
                let _ = w.show();
                let _ = w.set_focus();
            } else {
                crate::popup::open_settings(app);
            }
        }))
        // 钉屏 / 预览的图片读取：只读暂存目录的自定义协议（ADR-044）。
        // 不用 tauri 的 asset protocol——它会引入 `http-range`，与「零新 crate」冲突。
        .register_uri_scheme_protocol("vextoimg", |_ctx, request| {
            crate::screenshot::preview::serve_staged(request.uri().path())
        })
        // 截图图库的缩略图：只服务缓存目录里的 <32 hex>.png，不接受任意路径。
        .register_uri_scheme_protocol("vextogallery", |_ctx, request| {
            crate::screenshot::gallery::serve_thumb(request.uri().path())
        })
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![
            // 设置页（schema 驱动，加工具不用动这里）
            list_tools,
            get_tool_config,
            save_tool_config,
            set_tool_enabled,
            reset_hotkeys_translate,
            reset_hotkeys_screenshot,
            reset_hotkeys_download,
            reset_hotkeys_toolbox,
            reset_all_config,
            test_translate,
            test_engine_profile,
            open_config_dir,
            open_config_file,
            take_pending_tool,
            take_pending_section,
            about_info,
            about_open_link,
            open_extension_edge,
            open_extension_firefox,
            download_extension_chrome,
            about_diagnostics,
            config_path,
            get_ui_locale,
            hotkey_error,
            // 翻译弹窗 / 窗口首帧
            popup_ready,
            window_ready,
            resize_popup,
            close_popup,
            copy_text,
            translate_text,
            translate_text_only,
            translate_history_list,
            translate_history_remove,
            translate_history_clear,
            switch_engine,
            set_pinned,
            start_dragging,
            focus_for_typing,
            open_settings_from_popup,
            open_settings_cmd,
            open_toolbox,
            open_tool_settings,
            ocr_submit_region,
            ocr_cancel,
            // 截图（S7）
            shot_overlay_state,
            shot_action,
            shot_window_at,
            shot_cancel,
            result_action,
            feedback_state,
            feedback_action,
            feedback_start_animation,
            feedback_close,
            shot_full,
            shot_window,
            preview_screenshot_name,
            // 滚动截图（S7 会话 62）
            scroll_start,
            scroll_stop,
            scroll_cancel,
            scroll_close,
            // 钉屏 + 预览（S7 会话 63）
            pin_add,
            pin_step,
            pin_zoom,
            pin_set_zoom,
            pin_copy,
            pin_save_as,
            pin_preview,
            pin_state,
            pin_close,
            preview_open,
            preview_state,
            screenshot_gallery_list,
            screenshot_gallery_remove,
            screenshot_gallery_copy,
            screenshot_gallery_preview,
            screenshot_gallery_pin,
            preview_save_dir,
            preview_open_save_dir,
            preview_commit_edited,
            shot_edited_commit,
            shot_edited_action,
            preview_action,
            preview_close,
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
            github_releases,
            github_download,
            github_clone,
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
            // 应用内更新（ADR-061）
            update::update_state,
            update::update_check,
            update::update_download,
            update::update_cancel,
            update::update_install,
            update::update_open_release,
            // 自绘标题栏通用操作（所有无边框窗口共用）
            window_action
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            let _ = APP.set(handle.clone());
            let cfg = config::load();
            crate::i18n::init(&cfg);
            dlog::configure(cfg.log_enabled, &cfg.log_level);

            // 开机自启自愈：配置是唯一真相，注册表只是它的投影。
            // 换过安装目录、被安全软件清掉、或用户手工删过，都在这里补齐 / 抹掉。
            // 失败只记日志——自启坏掉不该让程序起不来。
            crate::autostart::sync(cfg.autostart);

            // 点击通知的处理器：**只注册一次**，重复注册会让一次点击触发多次。
            // Windows 只在 toast 还停在屏幕上时上报点击，收进操作中心后就收不到了。
            crate::download::notify::register_click(&handle);

            tray::build(&handle)?;

            // 静默日检：延迟 15 秒再联网，不拖慢启动；失败只记日志。
            update::spawn_startup(handle.clone());

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

            // 浏览器扩展接管（Vexto Connect，protocol v3）：只绑 127.0.0.1。
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
            // 收到传输请求时打开互传窗口，确认对话框由前端渲染成
            // 带遮罩的子对话框（会话 80，用户要求）。
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
    use super::{
        apply_values, config_to_map, default_hotkey_patch, is_hotkey_key, probe_text,
        tool_value_keys, EXTENSION_CHROME_ZIP_URL, EXTENSION_EDGE_URL, EXTENSION_FIREFOX_URL,
    };
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
        // 快捷键已集中到「常规 → 快捷键」
        assert!(!keys.contains(&"hotkey"));
        assert!(!keys.contains(&"hotkey_input"));
        // 会话 31 起引擎页签是「翻译模式 + 两张列表」
        assert!(keys.contains(&"active_engine"));
        assert!(keys.contains(&"to"));
        // 会话 31：厂商与模型都是列表字段，翻译模式单独存 active_engine
        assert!(keys.contains(&"vendor_profiles"));
        assert!(keys.contains(&"openai_profiles"));
        // 通用工具的字段不该出现在翻译里
        assert!(!keys.contains(&"popup_timeout_ms"));
        // 工具管理不是配置字段
        assert!(!tool_value_keys("general").contains(&"tool_list"));
    }

    #[test]
    fn tool_value_keys_unknown_tool_is_empty() {
        assert!(tool_value_keys("nope").is_empty());
    }

    #[test]
    fn hotkey_keys_are_recognized() {
        assert!(is_hotkey_key("hotkey"));
        assert!(is_hotkey_key("hotkey_ocr"));
        assert!(is_hotkey_key("hotkey_screenshot"));
        assert!(is_hotkey_key("hotkey_download"));
        assert!(!is_hotkey_key("appid"));
    }

    /// 扩展安装入口必须指向 HTTPS，且 Firefox 链接不带语言前缀（跟随浏览器语言）。
    #[test]
    fn extension_links_point_to_marketplaces() {
        assert!(EXTENSION_EDGE_URL
            .starts_with("https://microsoftedge.microsoft.com/addons/detail/vexto-connect/"));
        assert!(EXTENSION_FIREFOX_URL.starts_with("https://addons.mozilla.org/"));
        assert!(!EXTENSION_FIREFOX_URL.contains("/zh-CN/"));
        assert!(EXTENSION_FIREFOX_URL.contains("/firefox/addon/vexto-connect/"));
        assert!(EXTENSION_CHROME_ZIP_URL
            .ends_with("/releases/latest/download/vexto-connect-chrome.zip"));
    }

    /// 分组「恢复默认」提交的必须是出厂热键对象，未知 key 直接失败（ADR-092）。
    #[test]
    fn default_hotkey_patch_uses_factory_values() {
        let patch = default_hotkey_patch(&["hotkey", "hotkey_input"]).unwrap();
        assert_eq!(patch.len(), 2);
        assert_eq!(patch["hotkey"]["modifiers"], json!(["Ctrl", "Alt"]));
        assert_eq!(patch["hotkey"]["key"], "T");
        assert_eq!(patch["hotkey"]["match"], "exact");
        assert_eq!(patch["hotkey_input"]["key"], "I");
        assert!(default_hotkey_patch(&["not_a_hotkey"]).is_none());
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
                            | crate::tools::FieldKind::ButtonRow { .. }
                            | crate::tools::FieldKind::Group { .. }
                            | crate::tools::FieldKind::About { .. }
                            | crate::tools::FieldKind::ToolList
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
