//! 配置读写：`%APPDATA%\lightbox\config.json`。
//!
//! **只增字段**（ADR-013）：新字段一律 `#[serde(default)]` 或 `#[serde(default = "...")]`，
//! 这样老配置文件永远能被新版本读出来。反过来（同版本写的配置被老版本读）不做保证。
//!
//! S2 一次性把 S3–S5 要用的字段全定好，避免每个阶段都动一次这个文件。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Serialize, Deserialize)]
pub struct Config {
    // ---- 翻译引擎 ----
    /// 引擎标识：`baidu` | `openai`。对应 `translate::engine_by_id`
    #[serde(default = "d_engine")]
    pub engine: String,
    /// 百度翻译 APPID
    #[serde(default)]
    pub appid: String,
    /// 百度翻译密钥
    #[serde(default)]
    pub secret: String,
    /// 源语言。`auto` = 自动检测
    #[serde(default = "d_from")]
    pub from: String,
    /// 目标语言。**非中文**的源文本译到这里
    #[serde(default = "d_to")]
    pub to: String,
    /// 第二目标语言。**中文**的源文本译到这里（中文→英/日/…，用户的本意通常是
    /// 「我看不懂的翻成中文，我写得出的翻成外语」）
    #[serde(default = "d_to2")]
    pub to2: String,
    /// 语种检测方式：`local`（whatlang + 脚本启发式，默认）| `baidu`（调检测接口）
    #[serde(default = "d_lang_detect")]
    pub lang_detect: String,

    // ---- OpenAI 兼容（DeepSeek / 硅基流动 / Ollama）----
    /// 接口根地址，不含 `/v1/chat/completions`
    #[serde(default = "d_openai_base")]
    pub openai_base: String,
    #[serde(default = "d_openai_model")]
    pub openai_model: String,
    #[serde(default)]
    pub openai_key: String,
    /// 自定义 system prompt，空则用内置的翻译指令
    #[serde(default)]
    pub openai_prompt: String,

    // ---- 热键 ----
    /// 划词翻译热键，**永远存归一化形式**（`Ctrl+Alt+T`），
    /// 不存 `global-hotkey` 的 `control+alt+KeyT`。见 hotkey::validate_hotkey。
    #[serde(default = "d_hotkey")]
    pub hotkey: String,
    /// 截图翻译热键（S5 才真正注册，S2 只存值）
    #[serde(default = "d_hotkey_ocr")]
    pub hotkey_ocr: String,
    /// 下载热键（S3 才真正注册，S2 只存值）
    #[serde(default = "d_hotkey_download")]
    pub hotkey_download: String,
    /// 输入翻译热键。**本轮只存值、不注册也不进设置页**——弹窗重做（输入框 +
    /// 翻译按钮 + 可聚焦模式）是下一个会话的事，现在注册了按下去没反应，
    /// 等于给用户造一个死配置。
    #[serde(default = "d_hotkey_input")]
    pub hotkey_input: String,

    // ---- 模块开关 ----
    #[serde(default = "d_true")]
    pub enable_translate: bool,
    /// 下载默认关：S3 做完再改默认值
    #[serde(default)]
    pub enable_download: bool,
    #[serde(default)]
    pub enable_transfer: bool,
    #[serde(default)]
    pub enable_ocr: bool,

    // ---- 行为 ----
    #[serde(default = "d_popup_timeout")]
    pub popup_timeout_ms: u64,
    /// 译完是否自动把译文塞进剪贴板
    #[serde(default)]
    pub auto_copy: bool,
    /// 翻译缓存开关。默认开——同一段文字反复划是最高频场景，省一次就是省一次配额
    #[serde(default = "d_true")]
    pub cache_enabled: bool,

    // ---- 路径 ----
    #[serde(default)]
    pub download_dir: String,
    #[serde(default)]
    pub receive_dir: String,

    // ---- 下载（S3a）----
    /// 全局限速，单位 KB/s，0 = 不限。作为 aria2 的启动参数读取，
    /// 改设置后要等 aria2c 下次启动才生效（运行时改限速属 S3b 的调度范围）。
    #[serde(default)]
    pub max_speed_kb: u32,
    /// 同时下载数，界面限 1–16，后端再 clamp 一次防止手改配置越界。
    #[serde(default = "d_max_concurrent")]
    pub max_concurrent: u32,
    /// 单文件 HTTP 连接数：新建任务页「连接数」的默认值，落成 aria2 的
    /// `split` / `max-connection-per-server`。默认 16 与参考界面一致；
    /// aria2 自己是 1，对大文件太保守。
    #[serde(default = "d_connections")]
    pub max_connection_per_server: u32,
    /// 默认 User-Agent。**空串 = 不干涉**，用 aria2 自带 UA，不冒充浏览器。
    ///
    /// 新建页的 UA 框默认取这个值；设置页另给 Chrome / Edge / Safari / Firefox
    /// 四个预设，用户点一下就填进来（会话 25 用户要求）。
    #[serde(default)]
    pub default_user_agent: String,
    /// 下载完成 / 失败时弹系统通知。
    #[serde(default = "d_true")]
    pub notify_download: bool,
    /// 开始下载时也发一条。**默认关**：批量下载会刷屏。
    #[serde(default)]
    pub notify_on_start: bool,
    /// 下载完成 / 失败时通知。`notify_download` 是总开关，这一项是它的细分。
    /// 拆成两个是为了对齐参考界面（「下载开始时 / 下载完成时」两个勾选）。
    #[serde(default = "d_true")]
    pub notify_on_done: bool,
    /// 自定义 DNS 服务器，逗号分隔，落成 aria2 的 `--dns-server`。
    ///
    /// **这不是 DoH**：aria2 只支持明文 DNS，要做 DoH 得自建 DNS 代理（见 ADR-024）。
    #[serde(default)]
    pub dns_servers: String,
    /// 是否校验 HTTPS 证书（aria2 `--check-certificate`）。默认开。
    #[serde(default = "d_true")]
    pub check_certificate: bool,
    /// aria2 RPC 端口。只绑 127.0.0.1；被占用时引擎自动换一个可用端口。
    #[serde(default = "d_rpc_port")]
    pub rpc_port: u16,

    // ---- 完成后与清理（会话 27）----
    /// 全部任务完成后做什么：`none` | `shutdown` | `sleep` | `run`。
    ///
    /// 触发条件是「队列从有活动变成零活动」，且启动 3 分钟内不算——
    /// 否则「开机自启 + 上次全完成」会在启动时就执行。
    #[serde(default = "d_after_none")]
    pub after_all_done: String,
    /// `after_all_done = run` 时启动什么（完整路径或 PATH 里的名字）。
    #[serde(default)]
    pub after_all_done_cmd: String,
    /// 自动限速：按实测速度把上限收敛，别让 aria2 用满连接空转。
    #[serde(default)]
    pub auto_limit: bool,
    /// 删任务时连文件一起删。默认关——删文件是不可逆的，必须显式打开。
    #[serde(default)]
    pub delete_file_with_task: bool,
    /// 文件删除方式：`recycle`（移入回收站，默认）| `permanent`（彻底删除）。
    #[serde(default = "d_delete_mode")]
    pub delete_mode: String,
    /// 删除任务前二次确认（默认开；关掉 = 点一下就删）。
    #[serde(default = "d_true")]
    pub confirm_remove: bool,
    /// 文件已丢失时，顺手把这条历史记录也删掉。
    #[serde(default)]
    pub remove_missing_records: bool,
    /// 退出程序时清空已完成记录。
    #[serde(default)]
    pub clear_finished_on_exit: bool,
    /// 已完成记录保留条数：`100` | `300` | `forever`。
    #[serde(default = "d_history_keep")]
    pub history_keep: String,
    /// 记住上次保存位置：新建任务默认用上次实际用的目录。
    #[serde(default)]
    pub remember_last_dir: bool,
    /// 上次实际用的下载目录。**程序自己写，不进设置界面**。
    #[serde(default)]
    pub last_save_dir: String,
    /// 点击通知时：`folder`（在文件夹中显示）| `list`（打开下载列表）。
    #[serde(default = "d_click_folder")]
    pub notify_click_action: String,
    /// 代理模式：`direct` 直连 / `system` 跟随系统 / `manual` 手动。
    ///
    /// aria2 自己不读 Windows 的 WinINET 设置，所以「跟随系统」必须由我们读
    /// 注册表再翻译成 `all-proxy`（见 `download/proxy.rs`）。
    #[serde(default = "d_proxy_mode")]
    pub proxy_mode: String,
    /// `manual` 模式下用的地址，例如 `http://127.0.0.1:7890`、`socks5://127.0.0.1:1080`。
    /// 允许带 `user:pass@`，写进 conf 时会拆成 aria2 的独立选项。
    #[serde(default)]
    pub proxy_url: String,

    // ---- 浏览器插件接管（兼容 Rayburst-Connect v3）----
    /// 是否在本机启动 HTTP 接口，让浏览器扩展把下载交过来。
    /// 只绑 `127.0.0.1`，端口与密钥必须和扩展 Options 页一致。
    #[serde(default = "d_true")]
    pub extension_bridge_enabled: bool,
    /// 扩展 Options 页里的端口。默认 29110（Rayburst 的默认值）。
    #[serde(default = "d_bridge_port")]
    pub extension_bridge_port: u16,
    /// 扩展 Options 页里的密钥。空串 = 不校验（与 Rayburst 语义一致）。
    #[serde(default)]
    pub extension_bridge_secret: String,

    // ---- 系统集成 ----
    /// 开机自启（写 `HKCU\...\CurrentVersion\Run`）。
    /// 默认关：没经用户同意不往系统里加东西。
    #[serde(default)]
    pub autostart: bool,

    // ---- 版本 ----
    /// 配置格式版本，从 1 开始。老配置没有这个字段时按 1 处理
    #[serde(default = "d_version")]
    pub version: u32,
}

fn d_engine() -> String {
    "baidu".to_string()
}
fn d_from() -> String {
    "auto".to_string()
}
fn d_to() -> String {
    "zh".to_string()
}
/// 默认第二目标语言 = 英语。中文文本最常见的去向。
fn d_to2() -> String {
    "en".to_string()
}
fn d_lang_detect() -> String {
    "local".to_string()
}
fn d_openai_base() -> String {
    "https://api.deepseek.com".to_string()
}
fn d_openai_model() -> String {
    "deepseek-chat".to_string()
}
fn d_hotkey() -> String {
    "Ctrl+Alt+T".to_string()
}
fn d_hotkey_ocr() -> String {
    "Ctrl+Alt+S".to_string()
}
fn d_hotkey_download() -> String {
    "Ctrl+Alt+D".to_string()
}
fn d_hotkey_input() -> String {
    "Ctrl+Alt+I".to_string()
}
fn d_true() -> bool {
    true
}
fn d_popup_timeout() -> u64 {
    15000
}
/// aria2 自己的默认值就是 5，跟它保持一致，别给用户一个"升级后突然变快"的惊喜。
fn d_max_concurrent() -> u32 {
    5
}
/// 单文件连接数默认 16（参考界面同值）。aria2 默认 1，大文件会被拉成一条连接。
fn d_connections() -> u32 {
    16
}
/// 代理默认直连：没有代理时设成别的会让所有下载都连不上。
fn d_proxy_mode() -> String {
    "direct".to_string()
}
/// Rayburst-Connect 的默认端口；用户在扩展 Options 页改过的话这里也要改。
fn d_bridge_port() -> u16 {
    29110
}
/// aria2 RPC 端口。避开 6800（aria2 默认值，本机可能已有别的 aria2 占着）。
fn d_rpc_port() -> u16 {
    16800
}
/// 默认「什么都不做」：关机这种事必须是用户明确选的。
fn d_after_none() -> String {
    "none".to_string()
}
/// 默认走回收站：删错了还能捞回来。
fn d_delete_mode() -> String {
    "recycle".to_string()
}
/// 与历史模块原来的硬编码上限一致，升级后行为不变。
fn d_history_keep() -> String {
    "300".to_string()
}
/// 点通知先开文件夹——那是用户最可能想做的事。
fn d_click_folder() -> String {
    "folder".to_string()
}
fn d_version() -> u32 {
    1
}

impl Default for Config {
    fn default() -> Self {
        Self {
            engine: d_engine(),
            appid: String::new(),
            secret: String::new(),
            from: d_from(),
            to: d_to(),
            to2: d_to2(),
            lang_detect: d_lang_detect(),
            openai_base: d_openai_base(),
            openai_model: d_openai_model(),
            openai_key: String::new(),
            openai_prompt: String::new(),
            hotkey: d_hotkey(),
            hotkey_ocr: d_hotkey_ocr(),
            hotkey_download: d_hotkey_download(),
            hotkey_input: d_hotkey_input(),
            enable_translate: true,
            enable_download: false,
            enable_transfer: false,
            enable_ocr: false,
            popup_timeout_ms: d_popup_timeout(),
            auto_copy: false,
            cache_enabled: true,
            download_dir: String::new(),
            receive_dir: String::new(),
            max_speed_kb: 0,
            max_concurrent: d_max_concurrent(),
            max_connection_per_server: d_connections(),
            default_user_agent: String::new(),
            notify_download: true,
            notify_on_start: false,
            notify_on_done: true,
            dns_servers: String::new(),
            check_certificate: true,
            rpc_port: d_rpc_port(),
            after_all_done: d_after_none(),
            after_all_done_cmd: String::new(),
            auto_limit: false,
            delete_file_with_task: false,
            delete_mode: d_delete_mode(),
            confirm_remove: true,
            remove_missing_records: false,
            clear_finished_on_exit: false,
            history_keep: d_history_keep(),
            remember_last_dir: false,
            last_save_dir: String::new(),
            notify_click_action: d_click_folder(),
            proxy_mode: d_proxy_mode(),
            proxy_url: String::new(),
            extension_bridge_enabled: true,
            extension_bridge_port: d_bridge_port(),
            extension_bridge_secret: String::new(),
            autostart: false,
            version: d_version(),
        }
    }
}

pub fn config_path() -> PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(base).join("lightbox").join("config.json")
}

/// 日志目录（设置页「打开日志目录」用）。
pub fn log_dir() -> PathBuf {
    match config_path().parent() {
        Some(p) => p.to_path_buf(),
        None => PathBuf::from("."),
    }
}

/// 读配置。文件不存在、读不动、JSON 坏掉，一律回落到默认值——
/// 配置坏了不该让程序起不来，用户重填一次即可。
pub fn load() -> Config {
    match std::fs::read_to_string(config_path()) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => Config::default(),
    }
}

pub fn save(cfg: &Config) -> Result<(), String> {
    let p = config_path();
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("无法创建配置目录：{e}"))?;
    }
    let s = serde_json::to_string_pretty(cfg).map_err(|e| format!("序列化配置失败：{e}"))?;
    std::fs::write(&p, s).map_err(|e| format!("写入配置失败：{e}"))
}

#[cfg(test)]
mod tests {
    use super::Config;

    /// 老版本只有 appid，新版本加了字段也必须能读出来（ADR-013）。
    #[test]
    fn config_deserializes_with_missing_fields() {
        let json = r#"{"appid":"x"}"#;
        let c: Config = serde_json::from_str(json).unwrap();
        assert_eq!(c.appid, "x");
        assert_eq!(c.from, "auto");
        assert_eq!(c.to, "zh");
        assert_eq!(c.hotkey, "Ctrl+Alt+T");
        assert_eq!(c.secret, "");
    }

    /// S1 时代的完整配置（只有 5 个字段）必须能被 S2 读出来，且新字段取默认值。
    #[test]
    fn config_missing_new_fields_still_loads() {
        let json = r#"{"appid":"a","secret":"s","from":"auto","to":"zh","hotkey":"Ctrl+Alt+T"}"#;
        let c: Config = serde_json::from_str(json).unwrap();
        assert_eq!(c.engine, "baidu");
        assert_eq!(c.popup_timeout_ms, 15000);
        assert_eq!(c.hotkey_ocr, "Ctrl+Alt+S");
        assert_eq!(c.hotkey_download, "Ctrl+Alt+D");
        assert_eq!(c.to2, "en");
        assert_eq!(c.lang_detect, "local");
        assert_eq!(c.hotkey_input, "Ctrl+Alt+I");
        assert!(c.enable_translate);
        assert!(!c.enable_download);
        assert!(c.cache_enabled);
        assert_eq!(c.version, 1);
        assert_eq!(c.openai_model, "deepseek-chat");
    }

    #[test]
    fn config_roundtrip() {
        let c = Config::default();
        let s = serde_json::to_string(&c).unwrap();
        let d: Config = serde_json::from_str(&s).unwrap();
        assert_eq!(d.hotkey, c.hotkey);
        assert_eq!(d.from, c.from);
        assert_eq!(d.to, c.to);
        assert_eq!(d.engine, c.engine);
        assert_eq!(d.version, c.version);
    }

    /// 空对象 + 空文件内容都必须回落默认值，不能 panic。
    #[test]
    fn config_empty_object_falls_back_to_defaults() {
        let c: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(c.hotkey, "Ctrl+Alt+T");
    }

    /// 乱码 JSON 走 `unwrap_or_default()` 这条路：不能 panic。
    #[test]
    fn config_corrupt_json_falls_back_to_default() {
        let bad = "{ this is not json";
        let c: Config = serde_json::from_str(bad).unwrap_or_default();
        assert_eq!(c.engine, "baidu");
    }

    /// 关掉缓存时 `cache_enabled=false` 要能读出来（布尔字段最容易被漏 default）。
    #[test]
    fn config_respects_explicit_false() {
        let json = r#"{"enable_translate":false,"cache_enabled":false,"enable_download":true}"#;
        let c: Config = serde_json::from_str(json).unwrap();
        assert!(!c.enable_translate);
        assert!(!c.cache_enabled);
        assert!(c.enable_download);
    }

    /// 第二目标语言和语种检测方式都是 S2.5 新加的，老配置没有它们时
    /// 必须各自落到「英语」和「本地检测」——这两个默认值决定了划词翻译的实际行为：
    /// 默认配置下应该是「非中文译成中文、中文译成英语」。
    #[test]
    fn lang_direction_defaults_are_sane() {
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.to, "zh", "非中文的默认去向");
        assert_eq!(cfg.to2, "en", "中文的默认去向");
        assert_eq!(cfg.lang_detect, "local", "默认本地检测，不偷偷打网络");
        assert_eq!(cfg.hotkey_input, "Ctrl+Alt+I");
    }

    /// S3a 新增的下载参数：老配置没有这两个字段时必须落到「不限速 / 5 并发」，
    /// 不能因为缺字段就把程序读崩（ADR-013 只增字段）。
    #[test]
    fn download_params_default_for_old_config() {
        let old = r#"{"appid":"a","download_dir":"D:\\Downloads"}"#;
        let c: Config = serde_json::from_str(old).unwrap();
        assert_eq!(c.download_dir, "D:\\Downloads");
        assert_eq!(c.max_speed_kb, 0);
        assert_eq!(c.max_concurrent, 5);
    }

    /// 会话 25 新增：单文件连接数与默认 User-Agent。
    /// 老配置缺这两个字段时必须落到 16 / 空串——空串表示「不冒充浏览器」，
    /// 而不是某个写死的 UA 字符串。
    #[test]
    fn task_defaults_for_old_config() {
        let c: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(c.max_connection_per_server, 16);
        assert_eq!(c.default_user_agent, "");
    }

    /// 会话 25 第二轮新增的系统集成 / 网络字段：老配置缺它们时的默认值必须是
    /// 「不擅自替用户做主」——不自启、不指定 DNS、校验证书、通知细分为开。
    #[test]
    fn integration_defaults_for_old_config() {
        let c: Config = serde_json::from_str("{}").unwrap();
        assert!(!c.autostart, "没经同意不该写注册表");
        assert_eq!(c.dns_servers, "", "默认用系统 DNS");
        assert!(c.check_certificate, "默认必须校验证书");
        assert!(c.notify_on_done);
        assert!(!c.notify_on_start, "批量下载时开始通知会刷屏");
        assert_eq!(c.rpc_port, 16800);
    }

    /// 会话 27 的清理 / 删除 / 完成动作：默认值必须偏保守——
    /// 不关机、不删文件、删任务要确认、记录保留 300 条。
    #[test]
    fn cleanup_defaults_are_conservative() {
        let c: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(c.after_all_done, "none", "默认不许关机");
        assert!(!c.delete_file_with_task, "默认不许连文件一起删");
        assert_eq!(c.delete_mode, "recycle", "默认走回收站");
        assert!(c.confirm_remove, "默认要确认");
        assert!(!c.remove_missing_records);
        assert!(!c.clear_finished_on_exit);
        assert_eq!(c.history_keep, "300");
        assert!(!c.remember_last_dir);
        assert!(!c.auto_limit);
        assert_eq!(c.notify_click_action, "folder");
    }

    /// 新值要能原样存回去，且不影响其它字段。
    #[test]
    fn download_params_roundtrip() {
        let c = Config {
            max_speed_kb: 2048,
            max_concurrent: 8,
            ..Config::default()
        };
        let s = serde_json::to_string(&c).unwrap();
        let d: Config = serde_json::from_str(&s).unwrap();
        assert_eq!(d.max_speed_kb, 2048);
        assert_eq!(d.max_concurrent, 8);
        assert_eq!(d.engine, "baidu");
    }
}
