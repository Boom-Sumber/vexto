//! 配置读写：`%APPDATA%\vexto\config.json`（ADR-038；更名前是 `%APPDATA%\lightbox`）。
//!
//! **只增字段**（ADR-013）：新字段一律 `#[serde(default)]` 或 `#[serde(default = "...")]`，
//! 这样老配置文件永远能被新版本读出来。反过来（同版本写的配置被老版本读）不做保证。
//!
//! S2 一次性把 S3–S5 要用的字段全定好，避免每个阶段都动一次这个文件。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Once;

use crate::hotkey::HotkeyBinding;

/// 翻译模式取值。`engine_mode` 只认这三个，代码里一律用常量，避免拼错。
pub const MODE_VENDOR_MT: &str = "vendor_mt";
pub const MODE_VENDOR_LLM: &str = "vendor_llm";
pub const MODE_OPENAI: &str = "openai";

/// 厂商取值。目前两家：百度、有道。
pub const VENDOR_BAIDU: &str = "baidu";
pub const VENDOR_YOUDAO: &str = "youdao";

/// 内置免配置引擎的 `active_engine` 值。不是 `vendor_profiles` 里的一项：
/// 微软 Edge 端点不需要任何凭据，永远可用（ADR-058）。
pub const MICROSOFT_ENGINE: &str = "microsoft";

/// 一套 OpenAI 兼容接口的配置。用户可以在设置里建多套、各起名字，
/// 用哪套由 `openai_active` 指向它的 `id`。
///
/// 为什么不做成「一个 BaseURL + 一个模型名」：同一个人经常要在
/// DeepSeek / 硅基流动 / 本地 Ollama 之间切，各家的 BaseURL、模型名、Key
/// 都不一样，只留一套就得每次改三个字段。起个名字存下来，切的时候点一下就行。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelProfile {
    /// 稳定标识（前端生成 uuid）。**用户改不了**，改名不影响它，
    /// 这样「使用中」的指向不会因为改名而失效。
    #[serde(default)]
    pub id: String,
    /// 用户自定义的名字，显示在设置页和翻译弹窗里
    #[serde(default)]
    pub name: String,
    /// 接口根地址，不含 `/v1/chat/completions`
    #[serde(default)]
    pub base: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub key: String,
    /// 自定义 system prompt，空则用内置的翻译指令
    #[serde(default)]
    pub prompt: String,
    /// 是否启用（卡片头的小开关）。false 时不出现在「翻译模式」下拉里
    /// （会话 53 用户裁定）；老配置缺省视为启用。
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// 老配置没有 `enabled` 字段，缺省视为启用（会话 53 卡片开关）。
fn default_true() -> bool {
    true
}

/// 一套「厂商翻译」配置。
///
/// 可以对同一家厂商建多套（例如两个百度应用、一个有额度一个没有），各起名字，
/// 用哪套由 `active_engine` 指向。`use_llm` 决定要不要给这家开放大模型选项，
/// 当前实际走机翻还是大模型由 `active_engine` 的 `:llm` 后缀表达（会话 52）。
///
/// 四组凭据都放在一个结构里而不是 `enum`：用户在弹窗里切厂商时，已经填过的另一家
/// 字段不会丢（切回来还在），也不用为「切厂商」写一套字段搬迁逻辑。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct VendorProfile {
    /// 稳定标识（前端生成），用户改不了；改名不影响「当前使用」的指向
    #[serde(default)]
    pub id: String,
    /// 用户自定义的名字，显示在设置页与翻译弹窗里
    #[serde(default)]
    pub name: String,
    /// 厂商：`baidu` | `youdao`。以后接第三家只扩这个字段的取值
    #[serde(default)]
    pub vendor: String,
    /// 这家**允许**用大模型翻译（true）还是只有机器翻译（false）。
    /// 打开后「翻译模式」下拉会列出这家的大模型选项；当前实际用哪个
    /// 由 `active_engine` 的 `:llm` 后缀决定（会话 52 语义）。
    #[serde(default)]
    pub use_llm: bool,
    /// 是否启用（卡片头的小开关）。false 时整个实例不出现在
    /// 「翻译模式」下拉里（会话 53 用户裁定）；老配置缺省视为启用。
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 百度：APPID / 密钥
    #[serde(default)]
    pub appid: String,
    #[serde(default)]
    pub secret: String,
    /// 有道：应用 ID / 应用密钥
    #[serde(default)]
    pub youdao_app_key: String,
    #[serde(default)]
    pub youdao_app_secret: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Config {
    // ---- 翻译引擎 ----
    /// **老字段，只用于读旧配置**（会话 31 起不再由界面写入）。
    ///
    /// 演变史：`engine`（baidu/openai）→ `engine_mode` + `vendor`（三模式 + 厂商）
    /// → 现在 `vendor_profiles` + `active_engine`（可多套实例）。前两代都只在
    /// `migrate_*()` 里读一次，迁完清空、不再序列化。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub engine: String,
    /// 老字段：`vendor_mt` | `vendor_llm` | `openai`
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub engine_mode: String,
    /// 老字段：`baidu` | `youdao`
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub vendor: String,
    /// **迁移后仍用作「投影目标」**：翻译前会把当前厂商实例的凭据写进这几个
    /// 字段，引擎实现（`baidu.rs` / `youdao.rs`）就不用知道「实例」这回事。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub appid: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub secret: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub youdao_app_key: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub youdao_app_secret: String,
    /// 厂商实例列表。界面上是「厂商管理」的卡片列表。
    #[serde(default)]
    pub vendor_profiles: Vec<VendorProfile>,
    /// 当前用哪一个实例：`vendor:<id>` 或 `openai:<id>`。
    ///
    /// 一个字符串同时表达「哪一类」和「哪一个」，比「模式 + 各自 active」两个
    /// 字段少一半改坏的可能（两个字段可能互相矛盾）。
    #[serde(default)]
    pub active_engine: String,

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
    /// **老字段，只用于读旧配置**（会话 30 起不再由界面写入）。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub openai_base: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub openai_model: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub openai_key: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub openai_prompt: String,
    /// OpenAI 兼容的多套模型配置。空列表 = 还没添加过。
    #[serde(default)]
    pub openai_profiles: Vec<ModelProfile>,
    /// **老字段**：会话 30 用它指向当前模型，31 起统一走 `active_engine`
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub openai_active: String,

    // ---- 热键 ----
    /// 划词翻译热键。旧配置是 `"Ctrl+Alt+T"` 字符串，读取时自动转成
    /// `{ modifiers, key, match }`（ADR-090）；新配置一律写对象。
    #[serde(default = "d_hotkey")]
    pub hotkey: HotkeyBinding,
    /// 截图翻译热键（S5 才真正注册，S2 只存值）
    #[serde(default = "d_hotkey_ocr")]
    pub hotkey_ocr: HotkeyBinding,
    /// 下载热键（S3 才真正注册，S2 只存值）
    #[serde(default = "d_hotkey_download")]
    pub hotkey_download: HotkeyBinding,
    /// 输入翻译热键。
    #[serde(default = "d_hotkey_input")]
    pub hotkey_input: HotkeyBinding,

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
    /// OCR 引擎：`paddle`（PaddleOCR，默认）/ `windows`（系统自带）
    #[serde(default = "d_ocr_engine")]
    pub ocr_engine: String,

    // ---- 截图（S7）----
    /// 区域截图热键（S7 才真正注册）
    #[serde(default = "d_hotkey_screenshot")]
    pub hotkey_screenshot: HotkeyBinding,
    /// 全屏截图热键（S7 会话 72；与设置页/托盘全屏共用反馈）
    #[serde(default = "d_hotkey_fullscreen")]
    pub hotkey_fullscreen: HotkeyBinding,
    /// 截图工具总开关。默认关（ADR-040：未启用时零进程零线程）。
    #[serde(default)]
    pub enable_screenshot: bool,
    /// 截图保存目录。留空 = 系统「图片」下的 Vexto 子目录。
    #[serde(default)]
    pub screenshot_dir: String,
    /// 截图文件名规则。`$...$` 里是格式占位（`yyyyMMdd_HHmmss` / `uuid` / `unix`），
    /// 其余部分是用户自定义的前缀 / 后缀。默认 `Vexto_$yyyyMMdd_HHmmss$.png`。
    #[serde(default = "d_screenshot_name_rule")]
    pub screenshot_name_rule: String,
    /// 截图后的动作：`both`（复制图片并保存）/ `clipboard` / `path`（保存并复制路径）/ `save`
    #[serde(default = "d_shot_after")]
    pub screenshot_after_action: String,
    /// 滚动截图默认模式：`auto`（全自动）/ `manual`（半自动）
    #[serde(default = "d_scroll_mode")]
    pub scroll_mode: String,
    /// 滚动方式：`message` / `wheel` / `key` / `pagedown` / `manual`
    #[serde(default = "d_scroll_action")]
    pub scroll_action: String,
    /// 框选边框宽度（3–5 px，会话 65 选框改版）
    #[serde(default = "d_shot_border")]
    pub screenshot_border_width: u16,
    /// 放大镜倍数（2–10×，会话 66）
    #[serde(default = "d_shot_magnifier")]
    pub screenshot_magnifier_zoom: u16,
    /// 全屏截图右下角缩略图显示时长（3–5 秒）
    #[serde(default = "d_shot_feedback_seconds")]
    pub screenshot_feedback_seconds: u16,
    /// 缩略图淡出动画时长（200–1500 ms）
    #[serde(default = "d_shot_feedback_fade_ms")]
    pub screenshot_feedback_fade_ms: u32,
    /// 截图是否包含鼠标指针（滚动长图不包含）
    #[serde(default)]
    pub screenshot_include_cursor: bool,
    /// 钉屏单窗多图（false = 每图一窗，内存占用更高）
    #[serde(default = "d_true")]
    pub pinscreen_single_window: bool,

    // ---- 行为 ----
    #[serde(default = "d_popup_timeout")]
    pub popup_timeout_ms: u64,
    /// 译完是否自动把译文塞进剪贴板
    #[serde(default)]
    pub auto_copy: bool,
    /// 翻译缓存开关。默认开——同一段文字反复划是最高频场景，省一次就是省一次配额
    #[serde(default = "d_true")]
    pub cache_enabled: bool,
    /// 日志总开关。**默认开**（会话 90 用户要求，ADR-067）：出问题时用户不必
    /// 先去设置里打开开关才能留下现场。
    #[serde(default = "d_true")]
    pub log_enabled: bool,
    /// 日志等级：`trace` / `debug` / `info` / `warn` / `error`。默认 `info`。
    #[serde(default = "d_log_level")]
    pub log_level: String,

    // ---- 路径 ----
    #[serde(default)]
    pub download_dir: String,
    /// 局域网互传的接收目录。**留空 = 系统「下载」下的 Vexto 子目录**
    /// （解析在 transfer/server.rs::resolve_receive_dir，不在保存时回填）。
    #[serde(default)]
    pub receive_dir: String,

    // ---- 局域网互传（S4）----
    /// 本机对其他设备显示的名字。空 = 计算机名。
    #[serde(default)]
    pub device_alias: String,
    /// 接收传输是否要求对方输入 PIN。默认关（门 1 决定：确认弹窗才是主门，
    /// 强制 PIN 在家庭局域网是纯摩擦）。校验链路在 S4b 接。
    #[serde(default)]
    pub require_pin: bool,
    /// PIN（4–6 位数字）。仅 require_pin 开启时有意义。
    #[serde(default)]
    pub pin: String,
    /// 收到传输请求时是否自动接受（跳过确认框）。默认关：确认弹窗才是主门。
    #[serde(default)]
    pub transfer_auto_accept: bool,
    /// 传输完成后是否自动关闭传输页。默认关：让用户自己看到「已完成」再退出。
    /// 与 `transfer_auto_accept` 是两件事：那个管「要不要弹确认框」，这个管「页面留不留」。
    #[serde(default)]
    pub transfer_auto_complete: bool,
    /// 传输服务端口（UDP 组播 + TCP 同号）。默认 53317。
    #[serde(default = "d_transfer_port")]
    pub transfer_port: u16,
    /// 接收传输是否使用 HTTPS。默认开（ADR-035）。
    #[serde(default = "d_true")]
    pub transfer_use_https: bool,
    /// 组播地址。默认 224.0.0.167。
    #[serde(default = "d_transfer_multicast")]
    pub transfer_multicast: String,
    /// 设备类型：mobile / desktop / web / headless / server。
    #[serde(default = "d_transfer_device_type")]
    pub transfer_device_type: String,
    /// 设备型号（组播公告里的 deviceModel）。
    #[serde(default = "d_transfer_device_model")]
    pub transfer_device_model: String,
    /// 设备列表保留时长（秒）：超过这个时间没再收到公告就移除。
    #[serde(default = "d_transfer_peer_timeout")]
    pub transfer_peer_timeout_secs: u64,
    /// 网络接口白名单（逗号分隔的 IP 前缀；空 = 所有网卡）。
    #[serde(default)]
    pub transfer_interface_whitelist: String,

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
    /// 自定义 DNS 服务器，逗号分隔，落成 aria2 的 `dns-server=`。
    ///
    /// 随包的 aria2 1.37 没有 DoH 选项（`-h#all` 只有 `--async-dns` /
    /// `--async-dns-server=IP`），所以设置页按「系统默认 / 阿里 / 腾讯 / 360」
    /// 给明文 DNS 选项，不假装支持 DoH。
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
    /// GitHub 访问令牌（可选）。只用于下载页「GitHub 加速」查询 Releases：
    /// 匿名接口 60 次/小时，填入后 5000 次/小时。**不写日志、不进下载任务**。
    #[serde(default)]
    pub github_token: String,

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
    /// **默认开**（会话 86 用户要求）：Vexto 是常驻托盘工具，默认开机就在；
    /// 老配置里显式写了 `false` 的保持关闭，只给缺字段的配置补默认值。
    #[serde(default = "d_true")]
    pub autostart: bool,

    /// 托盘图标左键功能：`settings` / `tasks` / `input` / `screenshot`。
    /// 默认打开设置页。右键菜单不受影响。
    #[serde(default = "d_tray_left_click")]
    pub tray_left_click: String,

    /// 更新下载线路：`auto` / `direct` / `custom`。
    /// `auto` 依次尝试 v4.gh-proxy.org → gh-proxy.org → 直连（国内直连发布资产
    /// 经常失败，见 ADR-061）。
    #[serde(default = "d_update_download_source")]
    pub update_download_source: String,
    /// 自定义代理前缀，只在 `update_download_source = custom` 时使用。
    /// 必须以 `http://` 或 `https://` 开头；程序会自动补结尾 `/`。
    #[serde(default)]
    pub update_custom_proxy: String,

    /// 极轻模式：关闭窗口时销毁 WebView，把内存还给系统。
    /// **默认关**（会话 90 用户要求，ADR-067；老配置缺字段时同样按关处理）。
    /// 关掉后窗口只 `hide()`、WebView 常驻，下次打开更快但会一直占内存。
    /// 这一个开关同时管设置 / 下载列表 / 新建任务 / 划词弹窗四条关闭路径。
    #[serde(default)]
    pub lite_mode: bool,

    // ---- 外观 ----
    /// 界面语言：`system` | `zh-CN` | `en` | `ja` | `ko` | `es`。
    /// **默认跟随系统**；Windows 语言不在支持列表时回落英文。
    /// 只影响界面文案，不影响翻译功能的目标语言。
    #[serde(default = "d_ui_language")]
    pub ui_language: String,
    /// 主题：`dark` | `light` | `system`（跟随 Windows 应用模式）。
    /// **默认跟随系统**（会话 90 用户要求，ADR-067）。只影响前端 `body[data-theme]`，
    /// 不参与任何后端行为（外观功能会话）。
    #[serde(default = "d_appearance_theme", skip_serializing_if = "String::is_empty")]
    pub appearance_theme: String,
    /// 主色调 `#rrggbb`。默认 #11a780（翡翠绿）；空串 = 跟随主题 CSS 的同一默认。
    /// 前端把它覆盖到 `--theme`，按钮渐变 / 光晕 / 选中态全部联动。
    /// **不 skip 空串**：main.rs 有个守卫测试要求每个 schema 字段都能在
    /// 序列化后的 Config 里找到（config_map_covers_all_tool_keys）。
    #[serde(default = "d_accent_color")]
    pub accent_color: String,

    // ---- 版本 ----
    /// 配置格式版本，从 1 开始。老配置没有这个字段时按 1 处理
    #[serde(default = "d_version")]
    pub version: u32,
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
fn d_hotkey() -> HotkeyBinding {
    HotkeyBinding::from_legacy("Ctrl+Alt+T")
}
fn d_hotkey_ocr() -> HotkeyBinding {
    HotkeyBinding::from_legacy("Ctrl+Alt+S")
}
fn d_hotkey_download() -> HotkeyBinding {
    HotkeyBinding::from_legacy("Ctrl+Alt+D")
}
fn d_hotkey_input() -> HotkeyBinding {
    HotkeyBinding::from_legacy("Ctrl+Alt+I")
}
fn d_hotkey_screenshot() -> HotkeyBinding {
    HotkeyBinding::from_legacy("Ctrl+Alt+A")
}
fn d_hotkey_fullscreen() -> HotkeyBinding {
    HotkeyBinding::from_legacy("Ctrl+PrintScreen")
}
/// 默认截图文件名规则（会话 81）。`$...$` 是格式占位，其余部分是前缀 / 后缀。
pub const DEFAULT_SCREENSHOT_NAME_RULE: &str = "Vexto_$yyyyMMdd_HHmmss$.png";

fn d_shot_after() -> String {
    "both".to_string()
}
fn d_screenshot_name_rule() -> String {
    DEFAULT_SCREENSHOT_NAME_RULE.to_string()
}
fn d_tray_left_click() -> String {
    "settings".to_string()
}

fn d_update_download_source() -> String {
    "auto".to_string()
}
fn d_scroll_mode() -> String {
    "auto".to_string()
}
fn d_scroll_action() -> String {
    "message".to_string()
}
fn d_shot_border() -> u16 {
    3
}
fn d_shot_magnifier() -> u16 {
    8
}
fn d_shot_feedback_seconds() -> u16 {
    4
}
fn d_shot_feedback_fade_ms() -> u32 {
    400
}
fn d_ocr_engine() -> String {
    "paddle".to_string()
}
fn d_true() -> bool {
    true
}
fn d_popup_timeout() -> u64 {
    15000
}
fn d_transfer_port() -> u16 {
    53317
}
fn d_transfer_multicast() -> String {
    "224.0.0.167".to_string()
}
fn d_transfer_device_type() -> String {
    "desktop".to_string()
}
fn d_transfer_device_model() -> String {
    "Windows".to_string()
}
fn d_transfer_peer_timeout() -> u64 {
    600
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
fn d_ui_language() -> String {
    "system".to_string()
}
fn d_appearance_theme() -> String {
    "system".to_string()
}
fn d_accent_color() -> String {
    "#11a780".to_string()
}
fn d_log_level() -> String {
    "info".to_string()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            engine: String::new(),
            engine_mode: String::new(),
            vendor: String::new(),
            appid: String::new(),
            secret: String::new(),
            vendor_profiles: Vec::new(),
            active_engine: String::new(),
            from: d_from(),
            to: d_to(),
            to2: d_to2(),
            lang_detect: d_lang_detect(),
            openai_base: String::new(),
            openai_model: String::new(),
            openai_key: String::new(),
            openai_prompt: String::new(),
            openai_profiles: Vec::new(),
            openai_active: String::new(),
            youdao_app_key: String::new(),
            youdao_app_secret: String::new(),
            hotkey: d_hotkey(),
            hotkey_ocr: d_hotkey_ocr(),
            hotkey_download: d_hotkey_download(),
            hotkey_input: d_hotkey_input(),
            hotkey_screenshot: d_hotkey_screenshot(),
            hotkey_fullscreen: d_hotkey_fullscreen(),
            enable_translate: true,
            enable_download: false,
            enable_transfer: false,
            enable_ocr: false,
            ocr_engine: d_ocr_engine(),
            enable_screenshot: false,
            screenshot_dir: String::new(),
            screenshot_name_rule: d_screenshot_name_rule(),
            screenshot_after_action: d_shot_after(),
            scroll_mode: d_scroll_mode(),
            scroll_action: d_scroll_action(),
            screenshot_border_width: d_shot_border(),
            screenshot_magnifier_zoom: d_shot_magnifier(),
            screenshot_feedback_seconds: d_shot_feedback_seconds(),
            screenshot_feedback_fade_ms: d_shot_feedback_fade_ms(),
            screenshot_include_cursor: false,
            pinscreen_single_window: true,
            popup_timeout_ms: d_popup_timeout(),
            auto_copy: false,
            cache_enabled: true,
            log_enabled: true,
            log_level: d_log_level(),
            download_dir: String::new(),
            receive_dir: String::new(),
            device_alias: String::new(),
            require_pin: false,
            pin: String::new(),
            transfer_auto_accept: false,
            transfer_auto_complete: false,
            transfer_port: d_transfer_port(),
            transfer_use_https: true,
            transfer_multicast: d_transfer_multicast(),
            transfer_device_type: d_transfer_device_type(),
            transfer_device_model: d_transfer_device_model(),
            transfer_peer_timeout_secs: d_transfer_peer_timeout(),
            transfer_interface_whitelist: String::new(),
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
            github_token: String::new(),
            extension_bridge_enabled: true,
            extension_bridge_port: d_bridge_port(),
            extension_bridge_secret: String::new(),
            autostart: true,
            tray_left_click: d_tray_left_click(),
            update_download_source: d_update_download_source(),
            update_custom_proxy: String::new(),
            lite_mode: false,
            ui_language: d_ui_language(),
            appearance_theme: d_appearance_theme(),
            accent_color: d_accent_color(),
            version: d_version(),
        }
    }
}

/// 数据目录名。ADR-038：更名 Vexto 后统一为 `vexto`（更名前是 `lightbox`）。
pub const DATA_DIR_NAME: &str = "vexto";
/// 更名前的老目录名。只在一次性迁移时读，之后不再写入（ADR-038）。
pub const LEGACY_DATA_DIR_NAME: &str = "lightbox";

fn appdata() -> PathBuf {
    std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

/// `%APPDATA%\vexto`
pub fn data_dir() -> PathBuf {
    appdata().join(DATA_DIR_NAME)
}

/// `%APPDATA%\lightbox`（更名前的目录，只读；迁移后原样保留，不删）
pub fn legacy_data_dir() -> PathBuf {
    appdata().join(LEGACY_DATA_DIR_NAME)
}

pub fn config_path() -> PathBuf {
    data_dir().join("config.json")
}

/// 日志目录（设置页「打开日志目录」用）。与配置文件同目录。
pub fn log_dir() -> PathBuf {
    data_dir()
}

static MIGRATION: Once = Once::new();

/// 一次性把老目录的 `config.json` 复制到新目录（ADR-038）。
///
/// **只搬配置**：互传身份（证书 / 私钥 / 指纹）不搬，新目录里自然重新生成；
/// 传输历史、下载历史、aria2 会话、调试日志一律不带（清空重新开始）。
/// 旧目录原样留着当只读兜底，本函数不删任何东西。幂等：新目录已有配置就跳过。
pub fn migrate_legacy_data() {
    MIGRATION.call_once(|| {
        if let Err(e) = migrate_config_file(&config_path(), &legacy_data_dir().join("config.json"))
        {
            // 迁移失败不致命：load() 会回落到默认值，用户重填一次即可（与读配置失败同口径）。
            crate::dlog::dlog(&format!("config: 迁移旧配置失败：{e}"));
        }
    });
}

/// 迁移动作本身，抽成纯函数便于单测（不读环境变量）。
///
/// 返回 `true` = 确实搬了；`false` = 新目录已有配置 / 老目录没有配置，无需搬。
fn migrate_config_file(new: &Path, legacy: &Path) -> Result<bool, String> {
    if new.exists() || !legacy.exists() {
        return Ok(false);
    }
    if let Some(dir) = new.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("无法创建数据目录：{e}"))?;
    }
    std::fs::copy(legacy, new).map_err(|e| format!("复制旧配置失败：{e}"))?;
    Ok(true)
}

/// 读配置。文件不存在、读不动、JSON 坏掉，一律回落到默认值——
/// 配置坏了不该让程序起不来，用户重填一次即可。
/// 把老的 `engine`（`baidu` / `openai`）换算成 `engine_mode` + `vendor`。
///
/// 只在 `engine_mode` 为空时读 `engine`——迁移过一次之后就不再回头看它，
/// 否则用户把模式改成「厂商大模型」，下次启动又被旧字段扳回机器翻译。
///
/// 顺手做一次合法性兜底：`config.json` 是纯文本，用户手改成 `"vendor_x"` 或
/// 空串时不该让整个翻译功能不可用，落回「百度机器翻译」。
pub fn migrate_engine(cfg: &mut Config) {
    if cfg.engine_mode.trim().is_empty() {
        let legacy = cfg.engine.trim().to_ascii_lowercase();
        cfg.engine_mode = if legacy == MODE_OPENAI {
            MODE_OPENAI.to_string()
        } else {
            MODE_VENDOR_MT.to_string()
        };
    }
    if !matches!(
        cfg.engine_mode.as_str(),
        MODE_VENDOR_MT | MODE_VENDOR_LLM | MODE_OPENAI
    ) {
        cfg.engine_mode = MODE_VENDOR_MT.to_string();
    }
    if !matches!(cfg.vendor.as_str(), VENDOR_BAIDU | VENDOR_YOUDAO) {
        cfg.vendor = VENDOR_BAIDU.to_string();
    }
    // 迁移完成，旧字段清空 → 不再序列化（`skip_serializing_if`），配置文件里不
    // 会留一个永远不更新的死字段。
    cfg.engine.clear();
}

/// 把老的「单套 OpenAI 配置」搬进 `openai_profiles`。
///
/// 判断依据是 `openai_key` 非空：`openai_base` / `openai_model` 以前有默认值，
/// 光看它们分不出「用户填过」还是「出厂默认」，而没填 Key 的 OpenAI 配置本来也用不了。
pub fn migrate_openai(cfg: &mut Config) {
    if cfg.openai_profiles.is_empty() && !cfg.openai_key.trim().is_empty() {
        cfg.openai_profiles.push(ModelProfile {
            id: "default".to_string(),
            name: "默认模型".to_string(),
            base: if cfg.openai_base.trim().is_empty() {
                "https://api.deepseek.com".to_string()
            } else {
                cfg.openai_base.clone()
            },
            model: if cfg.openai_model.trim().is_empty() {
                "deepseek-chat".to_string()
            } else {
                cfg.openai_model.clone()
            },
            key: cfg.openai_key.clone(),
            prompt: cfg.openai_prompt.clone(),
            enabled: true,
        });
    }
    // 手改配置时可能漏字段（对象少一个键也能反序列化成功）：补上 id 与名字，
    // 否则「使用中」指不到它、列表卡片也没标题。
    for (i, p) in cfg.openai_profiles.iter_mut().enumerate() {
        if p.id.trim().is_empty() {
            p.id = format!("profile-{}", i + 1);
        }
        if p.name.trim().is_empty() {
            p.name = format!("模型 {}", i + 1);
        }
    }
    // active 指向不存在的 id → 退回第一条（删掉「使用中」那一项时会走到这）
    if !cfg.openai_profiles.iter().any(|p| p.id == cfg.openai_active) {
        cfg.openai_active = cfg
            .openai_profiles
            .first()
            .map(|p| p.id.clone())
            .unwrap_or_default();
    }
    // 旧字段清空，之后不再序列化
    cfg.openai_base.clear();
    cfg.openai_model.clear();
    cfg.openai_key.clear();
    cfg.openai_prompt.clear();
}

/// 把老的单套凭据搬成「厂商实例」，并算出 `active_engine`。
///
/// 迁移是**幂等**的：只要 `vendor_profiles` 已经有内容就不再根据老字段造新的，
/// 老字段本身也会被清空（不再序列化），所以第二次跑什么都不会发生。
/// `active_engine` 里「用这套厂商实例的大模型翻译」的后缀。
///
/// 会话 52 起 `use_llm` 开关的语义改为**允许**这家出两个模式：开关打开后，
/// 「翻译模式」下拉同时列出 `vendor:<id>`（机翻）和 `vendor:<id>:llm`（大模型）
/// 两项，选哪个由 `active_engine` 自己说了算——不再由实例的 `use_llm` 单独决定。
pub const LLM_ENGINE_SUFFIX: &str = ":llm";

pub fn migrate_vendors(cfg: &mut Config) {
    // 1) 老的单套凭据 → 实例（只在列表为空时）
    if cfg.vendor_profiles.is_empty() {
        // 老配置只有「当前这家用不用大模型」一个标志，两家都迁时给 engine_mode
        // 指向的那家，另一家保持机器翻译。
        let legacy_llm = cfg.engine_mode.trim() == MODE_VENDOR_LLM;
        let legacy_vendor = if cfg.vendor.trim() == VENDOR_YOUDAO {
            VENDOR_YOUDAO
        } else {
            VENDOR_BAIDU
        };
        if !cfg.appid.trim().is_empty() || !cfg.secret.trim().is_empty() {
            cfg.vendor_profiles.push(VendorProfile {
                id: "baidu-default".to_string(),
                name: "百度".to_string(),
                vendor: VENDOR_BAIDU.to_string(),
                use_llm: legacy_llm && legacy_vendor == VENDOR_BAIDU,
                appid: cfg.appid.clone(),
                secret: cfg.secret.clone(),
                ..VendorProfile::default()
            });
        }
        if !cfg.youdao_app_key.trim().is_empty() || !cfg.youdao_app_secret.trim().is_empty() {
            cfg.vendor_profiles.push(VendorProfile {
                id: "youdao-default".to_string(),
                name: "有道".to_string(),
                vendor: VENDOR_YOUDAO.to_string(),
                use_llm: legacy_llm && legacy_vendor == VENDOR_YOUDAO,
                youdao_app_key: cfg.youdao_app_key.clone(),
                youdao_app_secret: cfg.youdao_app_secret.clone(),
                ..VendorProfile::default()
            });
        }
    }

    // 2) 手改配置可能漏字段：补 id / 名字 / 厂商，否则卡片没标题、指向也会断
    for (i, p) in cfg.vendor_profiles.iter_mut().enumerate() {
        if p.vendor.trim().is_empty() {
            p.vendor = VENDOR_BAIDU.to_string();
        }
        if p.id.trim().is_empty() {
            p.id = format!("vendor-{}", i + 1);
        }
        if p.name.trim().is_empty() {
            p.name = format!("{} {}", vendor_label(&p.vendor), i + 1);
        }
    }

    // 3) 老模式 → active_engine（只在没设过时）
    if cfg.active_engine.trim().is_empty() {
        if cfg.engine_mode.trim() == MODE_OPENAI {
            let id = if cfg.openai_profiles.iter().any(|p| p.id == cfg.openai_active) {
                cfg.openai_active.clone()
            } else {
                cfg.openai_profiles
                    .first()
                    .map(|p| p.id.clone())
                    .unwrap_or_default()
            };
            if !id.is_empty() {
                cfg.active_engine = format!("openai:{id}");
            }
        } else {
            let want = if cfg.vendor.trim() == VENDOR_YOUDAO {
                VENDOR_YOUDAO
            } else {
                VENDOR_BAIDU
            };
            if let Some(p) = cfg
                .vendor_profiles
                .iter()
                .find(|p| p.vendor == want)
                .or_else(|| cfg.vendor_profiles.first())
            {
                // 老配置 engine_mode=厂商大模型 时，实例在步骤 1 里已带上 use_llm；
                // 新语义下「正在用大模型」由后缀表达，这里照搬到 active_engine。
                let suffix = if p.use_llm { LLM_ENGINE_SUFFIX } else { "" };
                cfg.active_engine = format!("vendor:{}{suffix}", p.id);
            }
        }
    }

    // 4) active 失效（删了当前那套 / 指向不存在的 id / **被关掉了开关**）→
    //    顺位到第一个**启用**的实例（会话 53：停用的实例不能装作还能用）。
    //    顺序：厂商在前、OpenAI 在后，和界面上的分组顺序一致。
    let vendor_active_ok = cfg.active_vendor_profile().is_some_and(|p| p.enabled);
    let openai_active_ok = cfg.active_openai_profile().is_some_and(|p| p.enabled);
    let microsoft_active = cfg
        .active_engine
        .trim()
        .eq_ignore_ascii_case(MICROSOFT_ENGINE);
    if !microsoft_active && !vendor_active_ok && !openai_active_ok {
        cfg.active_engine = cfg
            .vendor_profiles
            .iter()
            .find(|p| p.enabled)
            .map(|p| {
                // 顺位落到机翻；该实例开了 use_llm 的话用户在下拉里再点大模型
                format!("vendor:{}", p.id)
            })
            .or_else(|| {
                cfg.openai_profiles
                    .iter()
                    .find(|p| p.enabled)
                    .map(|p| format!("openai:{}", p.id))
            })
            .unwrap_or_else(|| MICROSOFT_ENGINE.to_string());
    }

    // 5) 老字段清空 → 不再序列化
    cfg.engine.clear();
    cfg.engine_mode.clear();
    cfg.vendor.clear();
    cfg.appid.clear();
    cfg.secret.clear();
    cfg.youdao_app_key.clear();
    cfg.youdao_app_secret.clear();
    cfg.openai_active.clear();
}

/// 厂商代码 → 中文名。未知值原样返回，别给卡片一个空标题。
pub fn vendor_label(vendor: &str) -> &str {
    match vendor.trim() {
        VENDOR_YOUDAO => "有道",
        VENDOR_BAIDU => "百度",
        other => other,
    }
}

impl Config {
    /// 当前选中的厂商实例。`active_engine` 不是 `vendor:` 前缀或指向不存在的 id
    /// 时返回 `None`（由 `migrate_vendors` 负责顺位，运行期不会长期为空）。
    pub fn active_vendor_profile(&self) -> Option<&VendorProfile> {
        let raw = self.active_engine.trim().strip_prefix("vendor:")?;
        // 「vendor:<id>:llm」也指向同一个实例：后缀只决定用机翻还是大模型
        let id = raw.strip_suffix(LLM_ENGINE_SUFFIX).unwrap_or(raw);
        self.vendor_profiles.iter().find(|p| p.id == id)
    }

    /// 当前选中的 OpenAI 模型。
    pub fn active_openai_profile(&self) -> Option<&ModelProfile> {
        let id = self.active_engine.trim().strip_prefix("openai:")?;
        self.openai_profiles.iter().find(|p| p.id == id)
    }

    /// 「翻译模式」下拉的全部候选项：`(active_engine 的值, 显示名)`。
    ///
    /// 厂商实例在前、OpenAI 模型在后，和设置页的分组顺序一致。
    /// 开了 `use_llm` 的厂商出**两个**候选项（机翻 + 大模型，会话 52）；
    /// 没开就只有机翻一项。`enabled=false` 的实例整条不出现（会话 53）。
    /// 没配置任何实例时返回空——设置页据此显示「先去下面添加」。
    pub fn engine_choices(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = vec![(
            MICROSOFT_ENGINE.to_string(),
            "微软翻译（免配置）".to_string(),
        )];
        for p in self.vendor_profiles.iter().filter(|p| p.enabled) {
            let name = p.name.trim().to_string();
            out.push((
                format!("vendor:{}", p.id),
                if p.use_llm {
                    format!("{name} · 机翻")
                } else {
                    name.clone()
                },
            ));
            if p.use_llm {
                out.push((
                    format!("vendor:{}{LLM_ENGINE_SUFFIX}", p.id),
                    format!("{name} · 大模型"),
                ));
            }
        }
        out.extend(
            self.openai_profiles
                .iter()
                .filter(|p| p.enabled)
                .map(|p| {
                    (
                        format!("openai:{}", p.id),
                        format!("OpenAI 兼容 · {}", p.name.trim()),
                    )
                }),
        );
        out
    }

    /// 当前实例的显示名（状态行、翻译弹窗用）。没有可用实例时给空串。
    pub fn active_engine_label(&self) -> String {
        if self
            .active_engine
            .trim()
            .eq_ignore_ascii_case(MICROSOFT_ENGINE)
        {
            return "微软翻译".to_string();
        }
        if let Some(p) = self.active_vendor_profile() {
            // 用机翻还是大模型由 `active_engine` 的 `:llm` 后缀说了算，
            // `use_llm` 只决定下拉里要不要展示大模型选项。
            return if self.active_engine.trim().ends_with(LLM_ENGINE_SUFFIX) {
                format!("{} · 大模型", p.name.trim())
            } else if p.use_llm {
                format!("{} · 机翻", p.name.trim())
            } else {
                p.name.trim().to_string()
            };
        }
        if let Some(p) = self.active_openai_profile() {
            return format!("OpenAI 兼容 · {}", p.name.trim());
        }
        String::new()
    }
}

pub fn load() -> Config {
    // 更名后第一次读配置时，把老目录的配置搬过来（幂等，ADR-038）。
    migrate_legacy_data();
    let mut cfg = match std::fs::read_to_string(config_path()) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => Config::default(),
    };
    migrate_engine(&mut cfg);
    migrate_openai(&mut cfg);
    migrate_vendors(&mut cfg);
    cfg
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
    use super::{
        migrate_config_file, migrate_engine, migrate_openai, migrate_vendors, Config,
        ModelProfile, VendorProfile, DATA_DIR_NAME, LEGACY_DATA_DIR_NAME, LLM_ENGINE_SUFFIX,
        MODE_OPENAI, MODE_VENDOR_LLM, MODE_VENDOR_MT, MICROSOFT_ENGINE, VENDOR_BAIDU,
        VENDOR_YOUDAO,
    };

    /// ADR-038：数据目录叫 `vexto`，老目录 `lightbox` 只作为迁移来源存在。
    #[test]
    fn data_dir_is_vexto_and_legacy_is_lightbox() {
        assert_eq!(DATA_DIR_NAME, "vexto");
        assert_eq!(LEGACY_DATA_DIR_NAME, "lightbox");
        assert!(super::config_path().ends_with(std::path::Path::new("vexto").join("config.json")));
        assert!(super::legacy_data_dir().ends_with("lightbox"));
    }

    /// 老目录有配置、新目录没有 → 逐字节复制一份，老文件原样留着。
    #[test]
    fn migrates_legacy_config_byte_identical() {
        let dir = std::env::temp_dir().join(format!("vexto-migrate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let legacy = dir.join("lightbox").join("config.json");
        let new = dir.join("vexto").join("config.json");
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        // 带 BOM 无关的普通字节：迁移必须是纯拷贝，不做任何解析/重排。
        std::fs::write(&legacy, b"{\"appid\":\"x\"}\r\n").unwrap();

        assert!(migrate_config_file(&new, &legacy).unwrap());
        assert_eq!(std::fs::read(&new).unwrap(), b"{\"appid\":\"x\"}\r\n");
        assert!(legacy.exists(), "老配置必须保留，不能搬走");

        // 第二次调用不覆盖新配置（用户以后改过的设置不会被老文件扳回去）。
        std::fs::write(&new, b"{\"appid\":\"new\"}").unwrap();
        assert!(!migrate_config_file(&new, &legacy).unwrap());
        assert_eq!(std::fs::read(&new).unwrap(), b"{\"appid\":\"new\"}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 老目录没有配置 → 什么都不做，也不该凭空建出新目录。
    #[test]
    fn no_legacy_config_creates_nothing() {
        let dir = std::env::temp_dir().join(format!("vexto-migrate-none-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let legacy = dir.join("lightbox").join("config.json");
        let new = dir.join("vexto").join("config.json");

        assert!(!migrate_config_file(&new, &legacy).unwrap());
        assert!(!dir.join("vexto").exists(), "没有可迁移的配置就不该建目录");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 老版本只有 appid，新版本加了字段也必须能读出来（ADR-013）。
    #[test]
    fn config_deserializes_with_missing_fields() {
        let json = r#"{"appid":"x"}"#;
        let c: Config = serde_json::from_str(json).unwrap();
        assert_eq!(c.appid, "x");
        assert_eq!(c.from, "auto");
        assert_eq!(c.to, "zh");
        assert_eq!(c.hotkey.compact(), "Ctrl+Alt+T");
        assert_eq!(c.secret, "");
    }

    /// ADR-090：旧配置的字符串热键要自动转成结构体，新配置写回对象。
    #[test]
    fn legacy_hotkey_string_migrates_to_struct() {
        let c: Config = serde_json::from_str(r#"{"hotkey":"Ctrl+Alt+T"}"#).unwrap();
        assert_eq!(c.hotkey.modifiers, vec!["Ctrl", "Alt"]);
        assert_eq!(c.hotkey.key, "T");
        assert_eq!(c.hotkey.match_mode, "exact");
        let value = serde_json::to_value(&c).unwrap();
        assert_eq!(value["hotkey"]["key"], "T");
        assert_eq!(value["hotkey"]["match"], "exact");
        assert!(value["hotkey"].is_object(), "新配置必须存对象");
    }

    /// 新结构体读取、缺 match 兜底、坏对象不拖垮整份配置。
    #[test]
    fn hotkey_object_is_read_leniently() {
        let c: Config = serde_json::from_str(
            r#"{"hotkey":{"modifiers":["alt","ctrl"],"key":"t"}}"#,
        )
        .unwrap();
        assert_eq!(c.hotkey.compact(), "Ctrl+Alt+T");

        let bad: Config = serde_json::from_str(r#"{"hotkey":{"key":"NoSuchKey"}}"#).unwrap();
        assert_eq!(bad.from, "auto", "坏热键不能拖垮其它字段");
    }

    /// S1 时代的完整配置（只有 5 个字段）必须能被 S2 读出来，且新字段取默认值。
    #[test]
    fn config_missing_new_fields_still_loads() {
        let json = r#"{"appid":"a","secret":"s","from":"auto","to":"zh","hotkey":"Ctrl+Alt+T"}"#;
        let mut c: Config = serde_json::from_str(json).unwrap();
        // 会话 29 起老配置走迁移：没有 engine 字段 → 百度机器翻译
        migrate_engine(&mut c);
        assert_eq!(c.engine_mode, MODE_VENDOR_MT);
        assert_eq!(c.vendor, VENDOR_BAIDU);
        assert_eq!(c.popup_timeout_ms, 15000);
        assert_eq!(c.hotkey_ocr.compact(), "Ctrl+Alt+S");
        assert_eq!(c.hotkey_download.compact(), "Ctrl+Alt+D");
        assert_eq!(c.to2, "en");
        assert_eq!(c.lang_detect, "local");
        assert_eq!(c.hotkey_input.compact(), "Ctrl+Alt+I");
        assert_eq!(c.hotkey_screenshot.compact(), "Ctrl+Alt+A");
        assert_eq!(c.hotkey_fullscreen.compact(), "Ctrl+PrintScreen");
        assert_eq!(c.screenshot_feedback_seconds, 4);
        assert_eq!(c.screenshot_feedback_fade_ms, 400);
        assert!(!c.enable_screenshot);
        assert_eq!(c.screenshot_after_action, "both");
        assert_eq!(c.scroll_mode, "auto");
        assert_eq!(c.scroll_action, "message");
        assert_eq!(c.screenshot_border_width, 3);
        assert_eq!(c.screenshot_magnifier_zoom, 8);
        assert!(!c.screenshot_include_cursor);
        assert_eq!(c.ocr_engine, "paddle");
        assert!(c.pinscreen_single_window);
        assert!(c.log_enabled, "会话 90 起默认开启日志");
        assert_eq!(c.log_level, "info");
        assert_eq!(c.transfer_peer_timeout_secs, 600);
        assert!(c.enable_translate);
        assert!(!c.enable_download);
        assert!(c.cache_enabled);
        assert_eq!(c.version, 1);
        // 会话 30 起 OpenAI 配置改成列表，旧的四个字段只在迁移时读
        assert_eq!(c.openai_model, "");
        assert!(c.openai_profiles.is_empty());
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
        assert_eq!(c.hotkey.compact(), "Ctrl+Alt+T");
    }

    /// 乱码 JSON 走 `unwrap_or_default()` 这条路：不能 panic。
    #[test]
    fn config_corrupt_json_falls_back_to_default() {
        let bad = "{ this is not json";
        let c: Config = serde_json::from_str(bad).unwrap_or_default();
        // 默认配置一个实例都没有，翻译模式留空，由界面提示「先去添加」
        assert!(c.engine_mode.is_empty());
        assert!(c.vendor_profiles.is_empty());
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
        assert_eq!(cfg.hotkey_input.compact(), "Ctrl+Alt+I");
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
        assert!(c.autostart, "会话 86 起默认开机自启");
        // 老配置里显式关掉的仍然保持关闭
        let c: Config = serde_json::from_str(r#"{"autostart":false}"#).unwrap();
        assert!(!c.autostart);
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
        assert!(d.engine_mode.is_empty());
    }

    // ── 会话 30：OpenAI 多模型列表 ────────────────────────────

    /// 老配置（只有单套 base/model/key）要搬进列表，并且旧字段清空不再落盘。
    #[test]
    fn migrate_openai_moves_legacy_into_list() {
        let mut c: Config = serde_json::from_str(
            r#"{"openai_base":"https://api.deepseek.com","openai_model":"deepseek-chat","openai_key":"sk-1","openai_prompt":"只输出译文"}"#,
        )
        .unwrap();
        migrate_openai(&mut c);
        assert_eq!(c.openai_profiles.len(), 1);
        let p = &c.openai_profiles[0];
        assert_eq!(p.base, "https://api.deepseek.com");
        assert_eq!(p.model, "deepseek-chat");
        assert_eq!(p.key, "sk-1");
        assert_eq!(p.prompt, "只输出译文");
        assert_eq!(p.name, "默认模型", "迁移出来的那条要有名字，否则列表卡片没标题");
        assert_eq!(c.openai_active, p.id, "迁移后它就是当前使用的那套");
        // 旧字段清空 + 不序列化
        assert!(c.openai_key.is_empty());
        let s = serde_json::to_string(&c).unwrap();
        assert!(!s.contains("openai_key"), "旧字段不该再写进配置文件：{s}");
        assert!(s.contains("openai_profiles"));
    }

    /// 没填过 Key 的老配置不该凭空长出一条空模型。
    #[test]
    fn migrate_openai_does_not_invent_profile_without_key() {
        let mut c: Config = serde_json::from_str(
            r#"{"openai_base":"https://api.deepseek.com","openai_model":"deepseek-chat"}"#,
        )
        .unwrap();
        migrate_openai(&mut c);
        assert!(c.openai_profiles.is_empty());
        assert!(c.openai_active.is_empty());
    }

    /// 手改配置漏了 id / name 时要补上：缺 id 会让「使用中」指不到它。
    #[test]
    fn migrate_openai_fills_missing_id_and_name() {
        let mut c: Config = serde_json::from_str(
            r#"{"openai_profiles":[{"base":"https://a"},{"base":"https://b"}]}"#,
        )
        .unwrap();
        migrate_openai(&mut c);
        assert_eq!(c.openai_profiles[0].id, "profile-1");
        assert_eq!(c.openai_profiles[1].id, "profile-2");
        assert_eq!(c.openai_profiles[0].name, "模型 1");
        assert_eq!(c.openai_profiles[1].name, "模型 2");
        assert_eq!(c.openai_active, "profile-1", "没有 active 时默认第一条");
    }

    /// 删掉「使用中」那一项后，active 指向不存在的 id —— 要退回第一条，
    /// 否则引擎会报「还没添加模型」但其实还有两套。
    #[test]
    fn migrate_openai_resets_dangling_active() {
        let mut c: Config = serde_json::from_str(
            r#"{"openai_profiles":[{"id":"a","name":"A","key":"k"},{"id":"b","name":"B","key":"k"}],"openai_active":"gone"}"#,
        )
        .unwrap();
        migrate_openai(&mut c);
        assert_eq!(c.openai_active, "a");
    }

    fn vendor(id: &str, name: &str, vendor: &str, use_llm: bool) -> VendorProfile {
        VendorProfile {
            id: id.into(),
            name: name.into(),
            vendor: vendor.into(),
            use_llm,
            enabled: true,
            appid: "app".into(),
            secret: "sec".into(),
            youdao_app_key: "yk".into(),
            youdao_app_secret: "ys".into(),
        }
    }

    fn model(id: &str, name: &str) -> ModelProfile {
        ModelProfile {
            id: id.into(),
            name: name.into(),
            base: format!("https://{id}"),
            model: "m".into(),
            key: "k".into(),
            prompt: String::new(),
            enabled: true,
        }
    }

    /// `active_engine` 是唯一的选择入口：只有前缀与 id 都对上才算选中，
    /// 不会「碰巧」落到列表第一条上（那是会话 30 的旧语义，已经去掉）。
    #[test]
    fn active_profiles_follow_active_engine() {
        let mut c = Config {
            vendor_profiles: vec![vendor("v1", "百度", VENDOR_BAIDU, false)],
            openai_profiles: vec![model("a", "A"), model("b", "B")],
            ..Config::default()
        };

        c.active_engine = "openai:b".into();
        assert_eq!(c.active_openai_profile().unwrap().name, "B");
        assert!(c.active_vendor_profile().is_none());
        assert_eq!(c.active_engine_label(), "OpenAI 兼容 · B");

        c.active_engine = "vendor:v1".into();
        assert_eq!(c.active_vendor_profile().unwrap().name, "百度");
        assert!(c.active_openai_profile().is_none());
        assert_eq!(c.active_engine_label(), "百度");

        // 前缀与 id 不匹配 → 两边都是 None（由 migrate_vendors 负责顺位）
        c.active_engine = "vendor:nope".into();
        assert!(c.active_vendor_profile().is_none());
        assert!(c.active_openai_profile().is_none());
        assert_eq!(c.active_engine_label(), "");
    }

    /// 下拉候选项：厂商在前、OpenAI 在后；开了 use_llm 的厂商出机翻+大模型两项。
    #[test]
    fn engine_choices_list_vendors_then_models() {
        let c = Config {
            vendor_profiles: vec![
                vendor("v1", "百度主账号", VENDOR_BAIDU, false),
                vendor("v2", "有道大模型", VENDOR_YOUDAO, true),
            ],
            openai_profiles: vec![model("a", "DeepSeek")],
            ..Config::default()
        };
        assert_eq!(
            c.engine_choices(),
            vec![
                ("microsoft".to_string(), "微软翻译（免配置）".to_string()),
                ("vendor:v1".to_string(), "百度主账号".to_string()),
                ("vendor:v2".to_string(), "有道大模型 · 机翻".to_string()),
                (
                    format!("vendor:v2{LLM_ENGINE_SUFFIX}"),
                    "有道大模型 · 大模型".to_string()
                ),
                ("openai:a".to_string(), "OpenAI 兼容 · DeepSeek".to_string()),
            ]
        );
        // 什么都没配 → 只有内置微软翻译
        assert_eq!(
            Config::default().engine_choices(),
            vec![("microsoft".to_string(), "微软翻译（免配置）".to_string())]
        );
    }

    /// 会话 52：`:llm` 后缀指向同一实例；机翻/大模型由后缀决定，
    /// `use_llm` 只影响下拉要不要出大模型项。
    #[test]
    fn llm_suffix_selects_mode_not_profile() {
        let mut c = Config {
            vendor_profiles: vec![vendor("v1", "百度", VENDOR_BAIDU, true)],
            ..Config::default()
        };

        c.active_engine = "vendor:v1".into();
        assert_eq!(c.active_vendor_profile().unwrap().id, "v1");
        assert_eq!(c.active_engine_label(), "百度 · 机翻");

        c.active_engine = format!("vendor:v1{LLM_ENGINE_SUFFIX}");
        assert_eq!(c.active_vendor_profile().unwrap().id, "v1");
        assert_eq!(c.active_engine_label(), "百度 · 大模型");
    }

    /// 内置微软翻译是空配置的默认引擎；已有可用 profile 时迁移不覆盖它。
    #[test]
    fn microsoft_is_default_and_migration_preserves_it() {
        let mut empty = Config::default();
        migrate_vendors(&mut empty);
        assert_eq!(empty.active_engine, MICROSOFT_ENGINE);

        let mut explicit = Config {
            active_engine: MICROSOFT_ENGINE.into(),
            vendor_profiles: vec![vendor("v1", "百度", VENDOR_BAIDU, false)],
            ..Config::default()
        };
        migrate_vendors(&mut explicit);
        assert_eq!(explicit.active_engine, MICROSOFT_ENGINE);
        assert_eq!(explicit.active_engine_label(), "微软翻译");
    }

    /// 会话 53：卡片开关关掉的实例不出现在下拉里；active 指向它时
    /// 迁移顺位到第一个**启用**的实例，不会指向停用的。
    #[test]
    fn disabled_profiles_are_hidden_and_skipped_in_fallback() {
        let mut c = Config {
            vendor_profiles: vec![
                vendor("v1", "百度停用", VENDOR_BAIDU, true),
                vendor("v2", "有道启用", VENDOR_YOUDAO, false),
            ],
            openai_profiles: vec![model("a", "DeepSeek")],
            ..Config::default()
        };
        c.vendor_profiles[0].enabled = false;
        c.active_engine = "vendor:v1".into();

        // 下拉里没有 v1（机翻与大模型项都消失）
        let choices = c.engine_choices();
        let ids: Vec<&str> = choices.iter().map(|(id, _)| id.as_str()).collect();
        assert!(!ids.iter().any(|x| x.starts_with("vendor:v1")));
        assert!(ids.contains(&"vendor:v2"));

        // active 指向停用实例 → 顺位到第一个启用的厂商
        migrate_vendors(&mut c);
        assert_eq!(c.active_engine, "vendor:v2");
    }

    /// 列表要能原样存回去（含中文名与空指令）。
    #[test]
    fn openai_profiles_roundtrip() {
        let c = Config {
            openai_profiles: vec![ModelProfile {
                id: "x1".into(),
                name: "硅基流动".into(),
                base: "https://api.siliconflow.cn/v1".into(),
                model: "Qwen/Qwen2.5-72B".into(),
                key: "sk-2".into(),
                prompt: String::new(),
                enabled: true,
            }],
            openai_active: "x1".into(),
            ..Config::default()
        };
        let s = serde_json::to_string(&c).unwrap();
        let d: Config = serde_json::from_str(&s).unwrap();
        assert_eq!(d.openai_profiles, c.openai_profiles);
        assert_eq!(d.openai_active, "x1");
    }

    // ── 会话 29：三模式 + 厂商的迁移 ────────────────────────────

    /// 老配置 `engine=openai` 必须换算成「OpenAI 兼容」，否则用户升级后
    /// 会突然开始用百度翻译（而百度凭据可能是空的，直接报未配置）。
    #[test]
    fn migrate_engine_maps_legacy_openai() {
        let mut c: Config = serde_json::from_str(r#"{"engine":"openai"}"#).unwrap();
        migrate_engine(&mut c);
        assert_eq!(c.engine_mode, MODE_OPENAI);
        assert_eq!(c.vendor, VENDOR_BAIDU);
        assert!(c.engine.is_empty(), "旧字段迁移后要清空，不再落盘");
    }

    #[test]
    fn migrate_engine_maps_legacy_baidu_and_empty() {
        for raw in [r#"{"engine":"baidu"}"#, r#"{}"#] {
            let mut c: Config = serde_json::from_str(raw).unwrap();
            migrate_engine(&mut c);
            assert_eq!(c.engine_mode, MODE_VENDOR_MT, "输入 {raw}");
            assert_eq!(c.vendor, VENDOR_BAIDU, "输入 {raw}");
        }
    }

    /// 迁移过一次之后，旧字段不该再把模式扳回去——用户明确选了有道大模型，
    /// 重启不能被 `engine:"baidu"` 改回机器翻译。
    #[test]
    fn migrate_engine_does_not_override_explicit_mode() {
        let mut c: Config = serde_json::from_str(
            r#"{"engine":"baidu","engine_mode":"vendor_llm","vendor":"youdao"}"#,
        )
        .unwrap();
        migrate_engine(&mut c);
        assert_eq!(c.engine_mode, MODE_VENDOR_LLM);
        assert_eq!(c.vendor, VENDOR_YOUDAO);
    }

    /// 手改 config.json 写成不存在的值时兜底，不能让整个翻译不可用。
    #[test]
    fn migrate_engine_falls_back_on_garbage() {
        let mut c: Config = serde_json::from_str(
            r#"{"engine_mode":"vendor_x","vendor":"klingon"}"#,
        )
        .unwrap();
        migrate_engine(&mut c);
        assert_eq!(c.engine_mode, MODE_VENDOR_MT);
        assert_eq!(c.vendor, VENDOR_BAIDU);
    }

    /// 迁移过的老字段都不该再被序列化，配置文件里只留新结构。
    #[test]
    fn legacy_fields_are_not_serialized() {
        // 默认配置里老字段都是空串，`skip_serializing_if` 会把它们全部略掉
        let v = serde_json::to_value(Config::default()).unwrap();
        let obj = v.as_object().unwrap();
        for old in ["engine", "engine_mode", "vendor", "appid", "openai_active"] {
            assert!(!obj.contains_key(old), "顶层老字段 {old} 还在：{v}");
        }
        assert!(obj.contains_key("vendor_profiles"));
        assert!(obj.contains_key("active_engine"));
    }

    /// 外观字段（外观功能会话）：老配置缺省 → 跟随系统 + 默认主色；
    /// 显式值原样读写。ADR-013：只增字段不破坏旧配置。会话 90 起默认主题
    /// 由 `dark` 改为 `system`（ADR-067）；会话 107 起默认主色为 #11a780。
    #[test]
    fn appearance_fields_default_and_roundtrip() {
        let c: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(c.appearance_theme, "system");
        assert_eq!(c.accent_color, "#11a780");

        let c2: Config = serde_json::from_str(
            r##"{"appearance_theme":"system","accent_color":"#3b82f6"}"##,
        )
        .unwrap();
        assert_eq!(c2.appearance_theme, "system");
        assert_eq!(c2.accent_color, "#3b82f6");
    }

    /// 界面语言（会话 111）：老配置缺省 → 跟随系统；显式值原样读写。
    #[test]
    fn ui_language_defaults_to_system_and_roundtrips() {
        let c: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(c.ui_language, "system");

        let c2: Config = serde_json::from_str(r#"{"ui_language":"ja"}"#).unwrap();
        assert_eq!(c2.ui_language, "ja");
        assert_eq!(
            serde_json::to_value(c2).unwrap()["ui_language"],
            serde_json::json!("ja")
        );
    }

    /// 会话 107：默认主色必须与色板第一位一致。
    #[test]
    fn accent_default_is_first_preset() {
        let first = crate::tools::general::ACCENT_PRESETS[0];
        assert_eq!(first.0, "#11a780");
        assert_eq!(Config::default().accent_color, first.0);
    }

    /// 会话 90：轻量模式默认值改为关（ADR-067）。老配置缺字段同样按关处理，
    /// 显式写了 `true` 的保持开启。
    #[test]
    fn lite_mode_defaults_to_off() {
        let c: Config = serde_json::from_str("{}").unwrap();
        assert!(!c.lite_mode);
        assert!(!Config::default().lite_mode);
        let c2: Config = serde_json::from_str(r#"{"lite_mode":true}"#).unwrap();
        assert!(c2.lite_mode);
    }

    /// 会话 81：老配置缺省时左键 = 打开设置页；命名规则 = 默认时间格式。
    #[test]
    fn tray_left_click_and_screenshot_name_defaults() {
        let c: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(c.tray_left_click, "settings");
        assert_eq!(
            c.screenshot_name_rule,
            "Vexto_$yyyyMMdd_HHmmss$.png"
        );
        let c2: Config = serde_json::from_str(
            r#"{"tray_left_click":"tasks","screenshot_name_rule":"shot_$uuid$.png"}"#,
        )
        .unwrap();
        assert_eq!(c2.tray_left_click, "tasks");
        assert_eq!(c2.screenshot_name_rule, "shot_$uuid$.png");
    }
}
