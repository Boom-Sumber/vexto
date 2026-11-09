#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! LightBox 入口。
//!
//! S2：多引擎 + 工具注册表 + schema 驱动的设置 IPC。
//!
//! 线程模型（三条边界，改动前先看 docs/03-ENGINEERING-STANDARDS.md §3.5）：
//! - 热键回调跑在**事件循环线程**上 → 只能 `std::thread::spawn` 出去，绝不能回调里做 UIA / 网络。
//! - 工作线程做完取词和翻译 → `app.run_on_main_thread()` 回主线程再碰窗口。
//! - `setup()` 里不能建窗（实测 os error 5），要等 `RunEvent::Ready`。

mod capture;
mod config;
mod dlog;
mod download;
mod hotkey;
mod lang;
mod mem;
mod popup;
mod tools;
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
    enabled: bool,
    status_line: String,
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
        m.insert("engine".to_string(), json!(cfg.engine));
        m.insert(
            "engineName".to_string(),
            json!(translate::engine_by_id(&cfg.engine)
                .map(|e| e.name())
                .unwrap_or("未配置引擎")),
        );
        let list: Vec<Value> = translate::ENGINE_OPTIONS
            .iter()
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
        if !engine_ready(&cfg) {
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
        if !engine_ready(&cfg) {
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

/// 当前引擎是否配好了。用于「缺配置就直接开设置页」这条分支——
/// 以前这里是硬编码检查百度的 appid/secret，多引擎之后必须问引擎自己。
fn engine_ready(cfg: &config::Config) -> bool {
    translate::engine_by_id(&cfg.engine)
        .map(|e| e.is_ready(cfg))
        .unwrap_or(false)
}

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
    match tools::registry::by_id(tool_id) {
        Some(t) => t
            .sections()
            .iter()
            .flat_map(|s| s.fields.iter())
            .filter(|f| {
                !matches!(
                    f.kind,
                    tools::FieldKind::Note { .. } | tools::FieldKind::Button
                )
            })
            .map(|f| f.key)
            .collect(),
        None => Vec::new(),
    }
}

/// 哪些 key 是热键字段，需要格式校验 + 整体重新注册。
/// `hotkey_ocr` 仍只存值不注册（功能在 S5）；`hotkey_download` 已在 S3a 注册。
fn is_hotkey_key(k: &str) -> bool {
    matches!(k, "hotkey" | "hotkey_ocr" | "hotkey_download" | "hotkey_input")
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
            Box::new(|_a: &AppHandle| {
                std::thread::spawn(|| match download::add_from_clipboard() {
                    Ok(st) => dlog::dlog(&format!(
                        "download: 入队成功 gid={} status={} {}/{} B",
                        st.gid, st.status, st.completed_length, st.total_length
                    )),
                    Err(e) => dlog::dlog(&format!("download: {e}")),
                });
            }),
        ));
    }
    hotkey::register_all(app, list)
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
            enabled: t.enabled(),
            status_line: t.status_line(),
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
            out.insert(k.to_string(), v.clone());
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

    // 3) 目录字段：不存在就提示（不阻塞保存，用户可能马上就要去建）
    for k in ["download_dir", "receive_dir"] {
        if let Some(Value::String(s)) = patch.get(k) {
            let s = s.trim();
            if !s.is_empty() && !std::path::Path::new(s).is_dir() {
                warnings.push(format!("目录不存在，保存后仍然生效：{s}"));
            }
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
    //    任一热键变了都要**整体重注册**：register_all 开头会 unregister_all，
    //    只重注册单个的话，另一个会被静默注销掉（S1 起就埋着的雷）。
    let hotkey_changed = ["hotkey", "hotkey_input", "hotkey_download"]
        .iter()
        .any(|k| patch.contains_key(*k));
    if hotkey_changed {
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
    if translate::engine_by_id(&engine_id).is_none() {
        return SimpleOut {
            ok: false,
            message: format!("未知的翻译引擎「{engine_id}」"),
        };
    }
    let mut cfg = config::load();
    cfg.engine = engine_id.clone();
    if let Err(e) = config::save(&cfg) {
        return SimpleOut {
            ok: false,
            message: e,
        };
    }
    SimpleOut {
        ok: true,
        message: format!(
            "已切换到{}",
            translate::engine_by_id(&engine_id)
                .map(|e| e.name())
                .unwrap_or(&engine_id)
        ),
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
    open_settings(&app);
}

// ---------- 命令：下载主窗口（S3b 后端） ----------

/// 前端初始化时主动拉一次快照；之后靠 pump 的 `tasks:snapshot` 事件增量更新。
/// 没有 aria2c 时返回**空快照**而不是报错：「当前没任务」是正常状态。
#[tauri::command]
fn download_snapshot() -> Result<crate::download::rpc::Snapshot, String> {
    match crate::download::engine::rpc_handle() {
        Some(r) => r.snapshot(),
        None => Ok(crate::download::rpc::Snapshot::default()),
    }
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

#[tauri::command]
fn download_remove(gid: String) -> Result<(), String> {
    with_download_rpc(|r| r.remove(&gid))
}

#[tauri::command]
fn download_clear_finished() -> Result<(), String> {
    with_download_rpc(|r| r.purge())
}

/// 打开任务的下载目录。`06` §6.4.2 第 8 条还要求「选中文件」，
/// 需要 `TaskRow` 带上 `files[0].path`，会话 14 与主窗口 UI 一起补。
#[tauri::command]
fn download_open_dir(gid: String) -> Result<(), String> {
    let dir = crate::download::engine::rpc_handle()
        .and_then(|r| r.snapshot().ok())
        .and_then(|s| s.tasks.into_iter().find(|t| t.gid == gid).map(|t| t.dir))
        .filter(|d| !d.trim().is_empty())
        .ok_or_else(|| "找不到这个任务的下载目录".to_string())?;
    std::process::Command::new("explorer.exe")
        .arg(&dir)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("打开目录失败：{e}"))
}

// ---------- 入口 ----------

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            // 设置页（schema 驱动，加工具不用动这里）
            list_tools,
            get_tool_config,
            save_tool_config,
            set_tool_enabled,
            test_translate,
            open_config_dir,
            open_config_file,
            take_pending_tool,
            config_path,
            hotkey_error,
            // 翻译弹窗
            popup_ready,
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
            // 下载主窗口（S3b 后端）
            download_snapshot,
            download_pause,
            download_unpause,
            download_remove,
            download_clear_finished,
            download_open_dir
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            let cfg = config::load();

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

            // S3b：轮询泵只发事件、不建窗。主窗口 UI 在会话 14 接上，
            // 在那之前 is_visible() 恒为 false，泵按后台 2s 间隔跑。
            {
                let for_emit = handle.clone();
                let for_visible = handle.clone();
                download::pump::start(
                    move |snap| {
                        let _ = for_emit.emit("tasks:snapshot", snap);
                    },
                    move || {
                        for_visible
                            .get_webview_window("main")
                            .and_then(|w| w.is_visible().ok())
                            .unwrap_or(false)
                    },
                );
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
                if !engine_ready(&cfg) {
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
    #[test]
    fn tool_value_keys_come_from_sections() {
        let keys = tool_value_keys("translate");
        assert!(keys.contains(&"hotkey"));
        assert!(keys.contains(&"engine"));
        assert!(keys.contains(&"to"));
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
                        crate::tools::FieldKind::Note { .. } | crate::tools::FieldKind::Button
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
