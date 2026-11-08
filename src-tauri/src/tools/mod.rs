//! 工具注册表。设置页的分区、启用开关、状态摘要都从这里来。
//!
//! 加一个新工具 = 新建一个实现了 `Tool` 的结构体 + 在 `registry.rs` 里加一行。
//! **不要为此改 `ui/settings.js`**（ADR-015）。

use serde::Serialize;

pub mod registry;

/// 设置页里的一个字段。
///
/// 只支持下面这些 `type`，不要自己发明新的 —— 新增一种 `type` 意味着
/// `settings.js` 要加分支，那是在为单个工具打特例。
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Field {
    Text {
        key: &'static str,
        label: &'static str,
        placeholder: &'static str,
    },
    Password {
        key: &'static str,
        label: &'static str,
    },
    Number {
        key: &'static str,
        label: &'static str,
        min: u32,
        max: u32,
        unit: &'static str,
    },
    Switch {
        key: &'static str,
        label: &'static str,
    },
    Select {
        key: &'static str,
        label: &'static str,
        options: &'static [(&'static str, &'static str)],
    },
    Hotkey {
        key: &'static str,
        label: &'static str,
    },
    Dir {
        key: &'static str,
        label: &'static str,
    },
    Button {
        key: &'static str,
        label: &'static str,
    },
    Note {
        text: &'static str,
    },
}

/// 一个可插拔的工具（划词翻译 / 下载 / 传文件 / OCR）。
pub trait Tool: Send + Sync {
    /// 稳定标识，也是配置里的 key 和设置页分区 id。一旦发布不许改。
    fn id(&self) -> &'static str;
    /// 显示名，用于托盘菜单和设置页左侧列表。
    fn name(&self) -> &'static str;
    /// 一句话说明用途，鼠标悬停 / 分区副标题用。
    fn desc(&self) -> &'static str;
    /// 是否启用（读配置）。
    fn enabled(&self) -> bool;

    /// 启用/停用的副作用：注册或反注册热键、起停后台任务、开关监听。
    ///
    /// **允许失败**（端口被占、设备忙、没有权限）。错误串要能直接给用户看，
    /// 设置页会显示在该分区顶部；不许 `let _ = ...` 吞掉。
    fn set_enabled(&self, on: bool) -> Result<(), String>;

    /// 一行状态摘要，显示在分区标题下方，如「已启用 · 3 个任务 · 1.2 MB/s」。
    fn status_line(&self) -> String;

    /// 设置页该分区要渲染的字段描述，前端照着画。
    fn settings_schema(&self) -> Vec<Field>;
}
