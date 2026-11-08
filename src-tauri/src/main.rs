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
mod hotkey;
mod mem;
mod popup;
mod tools;
mod tray;
mod translate;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use serde::Serialize;
use serde_json::{json, Map, Value};
use tauri::{AppHandle, Manager};

/// 托盘「退出」置位。没有它，`ExitRequested` 里的 `prevent_exit()` 会让进程永远关不掉。
pub static QUITTING: AtomicBool = AtomicBool::new(false);

pub struct AppState {
    /// 上一次取到的原文。弹窗「重试」用它，否则重试时又得再取一次词
    /// （用户那时可能已经把选中区取消了）。
    pub last_src: Mutex<Option<String>>,
}

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

// ---------- 核心流程 ----------

/// 取词 → 翻译 → 弹窗，一次完整的翻译流程。
///
/// 调用方可能是热键回调（事件循环线程）或托盘菜单，所以这里第一步就先
/// `std::thread::spawn`，把后面所有阻塞操作（UIA、SendInput、HTTP）都挪出事件循环。
fn run_translate(app: AppHandle, reuse_last_src: bool) {
    std::thread::spawn(move || {
        dlog::dlog(&format!(
            "translate_selection: 开始（reuse={reuse_last_src}）"
        ));

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
        let cached_src = if reuse_last_src {
            app.state::<AppState>()
                .last_src
                .lock()
                .map(|g| g.clone())
                .ok()
                .flatten()
                .unwrap_or_default()
        } else {
            String::new()
        };

        let src = if cached_src.trim().is_empty() {
            match capture::capture_selection() {
                Ok(t) => {
                    if let Ok(mut g) = app.state::<AppState>().last_src.lock() {
                        *g = Some(t.clone());
                    }
                    t
                }
                Err(e) => {
                    dlog::dlog(&format!("translate_selection: 取词失败：{e}"));
                    show_popup(&app, json!({ "state": "error", "src": "", "message": e }));
                    return;
                }
            }
        } else {
            cached_src
        };

        // 3) 先弹「翻译中」，用户按下热键后 1 秒内必须有反应，
        //    等引擎返回（可能几秒）再弹窗会让人以为热键没生效。
        show_popup(&app, json!({ "state": "loading", "src": src }));

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
                show_popup(
                    &app,
                    json!({
                        "state": "done",
                        "src": src,
                        "dst": r.dst,
                        "from": r.from,
                        "to": r.to,
                    }),
                );
            }
            Err(e) => {
                dlog::dlog(&format!("translate_selection: 翻译失败：{e}"));
                show_popup(&app, json!({ "state": "error", "src": src, "message": e }));
            }
        }
    });
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
    run_translate(app, false);
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

/// 哪些 key 是热键字段，需要格式校验 + （部分）重新注册。
/// `hotkey_ocr` 与 `hotkey_download` 只存值不注册：功能分别在 S5 / S3。
fn is_hotkey_key(k: &str) -> bool {
    matches!(k, "hotkey" | "hotkey_ocr" | "hotkey_download")
}

// ---------- 命令：设置页 ----------

// ── 临时兼容层：给 S1a 的旧设置页用 ─────────────────────────────────────
//
// S2a 把设置页的读写换成了按工具分组的 list_tools/get_tool_config/
// save_tool_config，但 `ui/settings.*` 的重写属于 S2b。这两条命令让旧页面
// 在 S2a 期间仍然可用，避免"后端先升级、前端瞬间变砖"。
//
// **S2b 重写设置页时必须删掉这两个函数和 ConfigOut**（03-STANDARDS §3.8
// 禁止为具体页面留 get_xxx_config/save_xxx_config 这类成对命令）。

/// 旧设置页要的整体配置视图。字段是旧页面的三个输入框 + 两个提示。
#[derive(Serialize)]
struct ConfigOut {
    appid: String,
    secret: String,
    hotkey: String,
    config_path: String,
    /// 热键注册失败原因，空串表示正常
    hotkey_error: String,
}

/// 兼容旧设置页：一次拿全部（新页面用 `get_tool_config`）。
#[tauri::command]
fn get_config() -> ConfigOut {
    let cfg = config::load();
    ConfigOut {
        appid: cfg.appid,
        secret: cfg.secret,
        hotkey: cfg.hotkey,
        config_path: config::config_path().display().to_string(),
        hotkey_error: hotkey::last_error(),
    }
}

/// 兼容旧设置页：整体保存。
///
/// 走的是和新路径同一套校验与生效逻辑——热键先验格式、再落盘、再注册、
/// 最后更新托盘文案，所以旧页面存下来的热键和新页面没有行为差异。
///
/// **只在当前配置上覆盖旧页面认识的 5 个字段**，不是整份替换。
/// 旧页面发来的 JSON 里没有 engine / openai_* / enable_* 等 S2 新字段，
/// 若直接 `config::save(&config)`，那些字段会被 serde 默认值覆盖掉——
/// 用户在新页面配好的 OpenAI 引擎会被旧页面悄悄改回百度。
#[tauri::command]
fn save_config(config: config::Config, app: AppHandle) -> SimpleOut {
    let normalized = match hotkey::validate_hotkey(&config.hotkey) {
        Ok(h) => h,
        Err(e) => {
            return SimpleOut {
                ok: false,
                message: e,
            }
        }
    };

    let mut cfg = config::load();
    cfg.appid = config.appid;
    cfg.secret = config.secret;
    cfg.from = config.from;
    cfg.to = config.to;
    cfg.hotkey = normalized.clone();
    if let Err(e) = config::save(&cfg) {
        return SimpleOut {
            ok: false,
            message: e,
        };
    }

    tray::update_hotkey_label(&app, &normalized);
    match hotkey::register_hotkey(&app, &normalized, |a| translate_selection(a.clone())) {
        Ok(()) => SimpleOut {
            ok: true,
            message: format!("已保存，热键 {normalized} 已生效"),
        },
        Err(e) => SimpleOut {
            ok: true,
            message: format!("已保存；但{e}"),
        },
    }
}

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

    // 4) 副作用：热键重新注册
    if let Some(h) = normalized.get("hotkey") {
        match hotkey::register_hotkey(&app, h, |a| translate_selection(a.clone())) {
            Ok(()) => tray::update_hotkey_label(&app, h),
            // 注册失败不回滚保存：用户填的组合确实是他想要的，只是撞了别的软件。
            // 回滚会让他以为"保存失败"，下次还得重填一遍。
            Err(e) => warnings.push(e),
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
    if tool_id == "translate" {
        tray::set_translate_enabled(&app, enabled);
    }
    SaveOut {
        ok: true,
        message: if enabled {
            format!("已启用「{}」", tool.name())
        } else {
            format!("已停用「{}」", tool.name())
        },
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

/// 「重试」：复用上一次的原文重新翻译，不再取词
/// （用户点按钮时选中区多半已经没了，重新取词必然失败）。
#[tauri::command]
fn retry_translate(app: AppHandle) {
    run_translate(app, true);
}

#[tauri::command]
fn open_settings_cmd(app: AppHandle) {
    open_settings(&app);
}

// ---------- 入口 ----------

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(AppState {
            last_src: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![
            // 设置页（schema 驱动，加工具不用动这里）
            list_tools,
            get_tool_config,
            save_tool_config,
            set_tool_enabled,
            test_translate,
            open_config_dir,
            take_pending_tool,
            config_path,
            hotkey_error,
            // 旧设置页兼容，S2b 重写设置页时删掉
            get_config,
            save_config,
            // 翻译弹窗
            popup_ready,
            resize_popup,
            close_popup,
            copy_text,
            retry_translate,
            open_settings_cmd
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            let cfg = config::load();

            tray::build(&handle, &cfg.hotkey)?;

            // 热键注册失败不阻断启动：托盘菜单仍然可用，设置页会显示红字原因。
            match hotkey::register_hotkey(&handle, &cfg.hotkey, |a| translate_selection(a.clone()))
            {
                Ok(()) => dlog::dlog(&format!("setup: 热键 {} 注册成功", cfg.hotkey)),
                Err(e) => dlog::dlog(&format!("setup: {e}")),
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
            tauri::RunEvent::ExitRequested { api, .. }
                if !QUITTING.load(Ordering::SeqCst) =>
            {
                api.prevent_exit();
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
