//! 工具注册表的数据类型：`Field`（一个设置项）+ `Section`（一组设置项）+ `Tool`（一个工具）。
//!
//! **这里是 ADR-015 的落点**：设置页不允许为具体工具写分支。前端只认识
//! `Field` 的 9 种类型，具体有哪些工具、每个工具长什么样，全部从 `Tool::sections()`
//! 读出来。加一个工具时改的是「这个工具自己的文件 + registry 一行」，前端零改动。
//!
//! S2 相比 S0 的两个变化：
//! 1. `Tool::settings_schema() -> Vec<Field>` → `Tool::sections() -> Vec<Section>`，
//!    这样「翻译」这种一个工具内部有多个子功能（引擎 / 划词 / 截图）的能画标签页。
//! 2. `Field` 从 enum 本体变成 `{ key, label, kind, show_if }` 的结构，
//!    `kind` 才是那 9 种类型。**`Field` 的 JSON 形状保持兼容**：`kind` 仍然用
//!    `#[serde(tag = "type", rename_all = "snake_case")]` 平铺进去，前端 `f.type` 不变。
//!    新增的 `show_if` 是可选的联动条件（架构文档 §2.3.3 说「这是唯一需要的联动能力」）。

pub mod about;
pub mod download;
pub mod general;
pub mod registry;
pub mod transfer;
pub mod translate_tool;

use serde::Serialize;

/// 字段的联动条件：只有 `其它字段 == 某值` 时才显示。
///
/// 一行判断，刻意不做表达式（`and`/`or`/`not`）——S2 任务书与架构文档都明确
/// 「不要做更复杂的表达式」，因为 9 种字段里真正需要联动的只有「引擎=百度才显示
/// APPID」这一类，加到 and/or 就是给自己挖坑。
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct ShowIf {
    pub key: &'static str,
    pub equals: &'static str,
}

/// 9 种字段类型。JSON 里以 `"type": "text" | "password" | ...` 区分。
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FieldKind {
    /// 单行文本
    Text {
        #[serde(skip_serializing_if = "Option::is_none")]
        placeholder: Option<String>,
    },
    /// 密码/密钥，前端用 `type=password` 并给个「显示」眼睛
    Password,
    /// 数字，带上下限与单位
    Number {
        min: u32,
        max: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        unit: Option<&'static str>,
    },
    /// 开关
    Switch,
    /// 下拉选择。`(值, 显示名)`
    Select {
        options: &'static [(&'static str, &'static str)],
    },
    /// 热键：前端只读 + 捕获模式，用户点一下直接按组合键
    Hotkey,
    /// 目录选择
    ///
    /// `placeholder` 用来显示运行时才算得出来的提示（例如
    /// 「默认：C:\Users\x\Downloads」）。前端 `settings.js` 的 `dir` 分支
    /// 本来就读 `f.placeholder`，所以这里加上字段**不需要改前端**（ADR-015）。
    Dir {
        #[serde(skip_serializing_if = "Option::is_none")]
        placeholder: Option<String>,
    },
    /// 按钮（点了发命令，不是存值字段）
    Button,
    /// 纯说明文字，没有值。文案在 `text` 里
    Note {
        text: &'static str,
    },
}

/// 一个设置项。`key` 对应 `Config` 里的字段名（`Button`/`Note` 没有要存的 key）。
#[derive(Clone, Debug, Serialize)]
pub struct Field {
    /// 存哪个配置字段。`Button`/`Note` 用不到，但为了结构统一仍要填（给个唯一字符串）
    pub key: &'static str,
    pub label: &'static str,
    #[serde(flatten)]
    pub kind: FieldKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub show_if: Option<ShowIf>,
}

impl Field {
    fn new(key: &'static str, label: &'static str, kind: FieldKind) -> Self {
        Self {
            key,
            label,
            kind,
            show_if: None,
        }
    }

    pub fn text_hint(key: &'static str, label: &'static str, placeholder: &'static str) -> Self {
        Self::new(
            key,
            label,
            FieldKind::Text {
                placeholder: Some(placeholder.to_string()),
            },
        )
    }

    pub fn password(key: &'static str, label: &'static str) -> Self {
        Self::new(key, label, FieldKind::Password)
    }

    pub fn number_unit(
        key: &'static str,
        label: &'static str,
        min: u32,
        max: u32,
        unit: &'static str,
    ) -> Self {
        Self::new(
            key,
            label,
            FieldKind::Number {
                min,
                max,
                unit: Some(unit),
            },
        )
    }

    pub fn switch(key: &'static str, label: &'static str) -> Self {
        Self::new(key, label, FieldKind::Switch)
    }

    pub fn select(
        key: &'static str,
        label: &'static str,
        options: &'static [(&'static str, &'static str)],
    ) -> Self {
        Self::new(key, label, FieldKind::Select { options })
    }

    pub fn hotkey(key: &'static str, label: &'static str) -> Self {
        Self::new(key, label, FieldKind::Hotkey)
    }

    pub fn dir(key: &'static str, label: &'static str) -> Self {
        Self::new(key, label, FieldKind::Dir { placeholder: None })
    }

    /// 目录字段 + 运行时提示。提示由后端算，前端只负责显示成灰色占位符。
    pub fn dir_with_placeholder(
        key: &'static str,
        label: &'static str,
        placeholder: impl Into<String>,
    ) -> Self {
        Self::new(
            key,
            label,
            FieldKind::Dir {
                placeholder: Some(placeholder.into()),
            },
        )
    }

    pub fn button(key: &'static str, label: &'static str) -> Self {
        Self::new(key, label, FieldKind::Button)
    }

    /// 纯说明文字。没有 key（用文案本身当 DOM id），文案放在 `kind.text` 里。
    pub fn note(text: &'static str) -> Self {
        Self::new(text, "", FieldKind::Note { text })
    }

    /// 只在 `key == equals` 时显示。
    pub fn show_if(mut self, key: &'static str, equals: &'static str) -> Self {
        self.show_if = Some(ShowIf { key, equals });
        self
    }
}

/// 一个标签页 / 分组。
///
/// 只有一个 `Section` 时前端**不画标签栏**，直接渲染字段——所以下载/互传/通用/关于
/// 这四个工具零额外点击成本，只有翻译会看到「引擎 | 划词翻译 | 截图翻译」。
#[derive(Clone, Debug, Serialize)]
pub struct Section {
    /// 稳定的机器标识，前端用来记住「上次停在哪个标签」
    pub id: &'static str,
    /// 标签文字
    pub name: &'static str,
    pub fields: Vec<Field>,
}

impl Section {
    pub fn new(id: &'static str, name: &'static str, fields: Vec<Field>) -> Self {
        Self { id, name, fields }
    }
}

/// 一个工具。
///
/// 加新工具：①实现这个 trait ②`registry::all()` 里加一行。其余（托盘、设置页、
/// 模块开关、状态摘要）全部自动生效。
pub trait Tool: Send + Sync {
    /// 稳定标识，写进配置/日志，不许改
    fn id(&self) -> &'static str;
    fn name(&self) -> &'static str;
    /// 一句话说明这个工具干什么
    fn desc(&self) -> &'static str;
    /// 设置页左侧是否可用（`false` 时灰掉但仍能点开看）
    fn enabled(&self) -> bool;
    fn set_enabled(&self, on: bool) -> Result<(), String>;
    /// 状态摘要，显示在工具名下方与分区顶部。例如「百度翻译 · 已就绪」/「未配置」
    fn status_line(&self) -> String;
    /// 设置项分组。多数工具返回一个 Section，翻译返回三个。
    fn sections(&self) -> Vec<Section>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    /// 9 种类型都必须序列化成前端认识的形状：`{"type":"xxx", ...}`。
    #[test]
    fn every_field_kind_serializes_with_type_tag() {
        let cases: Vec<(Field, &str)> = vec![
            (Field::text_hint("a", "A", "提示"), "text"),
            (Field::password("b", "B"), "password"),
            (Field::number_unit("c", "C", 1, 2, "px"), "number"),
            (Field::switch("d", "D"), "switch"),
            (Field::select("e", "E", &[("x", "X")]), "select"),
            (Field::hotkey("f", "F"), "hotkey"),
            (Field::dir("g", "G"), "dir"),
            (Field::button("h", "H"), "button"),
            (Field::note("说明"), "note"),
        ];
        for (f, ty) in cases {
            let v: Value = serde_json::to_value(&f).unwrap();
            assert_eq!(v["type"], ty, "字段 {ty} 的 type 不对：{v}");
            assert_eq!(v["label"], f.label);
        }
    }

    /// `show_if` 只在设置过时才出现，避免前端拿到 `null` 还要判空。
    #[test]
    fn show_if_is_omitted_when_absent() {
        let v: Value = serde_json::to_value(Field::switch("a", "A")).unwrap();
        assert!(v.get("show_if").is_none(), "不该出现 show_if：{v}");
    }

    #[test]
    fn show_if_serializes_key_and_equals() {
        let f = Field::text_hint("appid", "APPID", "填 APPID").show_if("engine", "baidu");
        let v: Value = serde_json::to_value(&f).unwrap();
        assert_eq!(v["show_if"]["key"], "engine");
        assert_eq!(v["show_if"]["equals"], "baidu");
    }

    /// `placeholder` 是 `Option`，没填时不该出现在 JSON 里。
    #[test]
    fn optional_field_parts_are_skipped() {
        let v: Value = serde_json::to_value(Field::text_hint("a", "A", "提示")).unwrap();
        assert_eq!(v["placeholder"], "提示");
        // number 的 unit 也一样
        let v: Value = serde_json::to_value(Field::number_unit("b", "B", 1, 2, "毫秒")).unwrap();
        assert_eq!(v["unit"], "毫秒");
    }

    #[test]
    fn section_serializes_fields_flat() {
        let s = Section::new("s1", "分组", vec![Field::switch("k", "开关")]);
        let v: Value = serde_json::to_value(&s).unwrap();
        assert_eq!(v["id"], "s1");
        assert_eq!(v["name"], "分组");
        assert_eq!(v["fields"][0]["type"], "switch");
    }
}
