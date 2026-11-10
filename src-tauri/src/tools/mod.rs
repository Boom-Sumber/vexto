//! 工具注册表的数据类型：`Field`（一个设置项）+ `Section`（一组设置项）+ `Tool`（一个工具）。
//!
//! **这里是 ADR-015 的落点**：设置页不允许为具体工具写分支。前端只认识
//! `Field` 的 10 种类型，具体有哪些工具、每个工具长什么样，全部从 `Tool::sections()`
//! 读出来。加一个工具时改的是「这个工具自己的文件 + registry 一行」，前端零改动。
//!
//! S2 相比 S0 的两个变化：
//! 1. `Tool::settings_schema() -> Vec<Field>` → `Tool::sections() -> Vec<Section>`，
//!    这样「翻译」这种一个工具内部有多个子功能（引擎 / 划词 / 截图）的能画标签页。
//! 2. `Field` 从 enum 本体变成 `{ key, label, kind, show_if }` 的结构，
//!    `kind` 才是那 10 种类型。**`Field` 的 JSON 形状保持兼容**：`kind` 仍然用
//!    `#[serde(tag = "type", rename_all = "snake_case")]` 平铺进去，前端 `f.type` 不变。
//!    新增的 `show_if` 是可选的联动条件（架构文档 §2.3.3 说「这是唯一需要的联动能力」）。

pub mod about;
pub mod download;
pub mod general;
pub mod registry;
pub mod transfer;
pub mod translate_tool;

use serde::Serialize;

/// 字段的联动条件：全部条件都满足时才显示。
///
/// 仍然**不做任意表达式**（没有 `or` / `not` / 括号 / 嵌套比较），只允许：
/// - 一个 key 命中多个候选值之一（OR，用于「厂商模式下的两个模式都显示厂商」）；
/// - 再 AND 一层同样的条件（用于「厂商=有道 **且** 模式不是 OpenAI」）。
///
/// 会话 29 才加这两条能力：翻译引擎拆成「模式 + 厂商」两个维度后，
/// 凭据字段必须同时看两个字段，单条件表达不了。**深度上限就是两层**，
/// 别在这条路上继续加。
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct ShowIf {
    pub key: &'static str,
    /// 命中其中之一即算满足（单值就是「等于」）
    pub equals: Vec<&'static str>,
    /// 必须同时满足的第二组条件。只允许一层，不嵌套
    #[serde(skip_serializing_if = "Option::is_none")]
    pub also: Option<Box<ShowIf>>,
}

/// 10 种字段类型。JSON 里以 `"type": "text" | "password" | ...` 区分。
///
/// 第 10 种 `Group` 是会话 25 加的：下载设置要在**一个页签里**再分组
/// （下载路径 / 速度与并发 / 通知…）。它不是「值」，只是一个分组标题，
/// 所以没有 key、也不参与 `show_if`。
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FieldKind {
    /// 单行文本
    Text {
        #[serde(skip_serializing_if = "Option::is_none")]
        placeholder: Option<String>,
        /// 输入框下方的一排预设按钮 `(显示名, 值)`。点一下把值填进同一个字段。
        /// 目前只有「默认 User-Agent」用（Chrome / Edge / Safari / Firefox）。
        #[serde(skip_serializing_if = "Option::is_none")]
        presets: Option<&'static [(&'static str, &'static str)]>,
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
    /// 目录选择。
    ///
    /// 会话 17 起**不再用 placeholder 提示默认路径**：用户要求输入框里直接是
    /// 具体路径，默认值由 `get_tool_config` / `save_tool_config` 在留空时回填。
    Dir,
    /// 按钮（点了发命令，不是存值字段）
    Button,
    /// 纯说明文字，没有值。文案在 `text` 里
    Note {
        text: &'static str,
    },
    /// 分组标题 + 一条分隔线。只在一个页签内部再分层时用（会话 25）
    Group {
        title: &'static str,
    },
    /// 可重复的一组配置（会话 30）。
    ///
    /// 第一个用它的地方是「OpenAI 兼容的多套模型」：每套有名称 / BaseURL /
    /// 模型名 / API Key，用户可以加任意多套、各起名字。
    ///
    /// `item_fields` 描述「每一项里有哪些输入框」，它们的 `key` 是**对象内的键名**
    /// （例如 `name` / `base`），不是顶层配置键。整组的值是一个对象数组。
    /// **不支持嵌套 list**，也不支持项内联动——真需要那些就是该换方案了。
    List {
        item_fields: Vec<Field>,
        /// 「使用中」单选被选中时，把那一项的 `id` 写到这个配置键。
        /// `None` = 这组配置没有「当前项」的概念。
        #[serde(skip_serializing_if = "Option::is_none")]
        active_key: Option<&'static str>,
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
                presets: None,
            },
        )
    }

    /// 带一排预设按钮的文本框。点按钮把对应值填进同一个字段并保存。
    pub fn text_presets(
        key: &'static str,
        label: &'static str,
        placeholder: &'static str,
        presets: &'static [(&'static str, &'static str)],
    ) -> Self {
        Self::new(
            key,
            label,
            FieldKind::Text {
                placeholder: Some(placeholder.to_string()),
                presets: Some(presets),
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
        Self::new(key, label, FieldKind::Dir)
    }

    pub fn button(key: &'static str, label: &'static str) -> Self {
        Self::new(key, label, FieldKind::Button)
    }

    /// 纯说明文字。没有 key（用文案本身当 DOM id），文案放在 `kind.text` 里。
    pub fn note(text: &'static str) -> Self {
        Self::new(text, "", FieldKind::Note { text })
    }

    /// 分组标题。没有 key（用标题当 DOM id），也不带 label。
    pub fn group(title: &'static str) -> Self {
        Self::new(title, "", FieldKind::Group { title })
    }

    /// 可重复的一组配置。`items` 里的字段只用于描述每项的输入框，
    /// 它们的 `key` 是对象内的键名（`name` / `base` …），不是顶层配置键。
    pub fn list(key: &'static str, label: &'static str, items: Vec<Field>) -> Self {
        Self::new(
            key,
            label,
            FieldKind::List {
                item_fields: items,
                active_key: None,
            },
        )
    }

    /// 给 list 加「使用中」单选：选中项的 `id` 写到 `active_key`。
    /// 对非 list 字段是空操作。
    pub fn active_key(mut self, key: &'static str) -> Self {
        if let FieldKind::List { active_key, .. } = &mut self.kind {
            *active_key = Some(key);
        }
        self
    }

    /// 只在 `key == equals` 时显示。
    pub fn show_if(mut self, key: &'static str, equals: &'static str) -> Self {
        self.show_if = Some(ShowIf {
            key,
            equals: vec![equals],
            also: None,
        });
        self
    }

    /// 只在 `key` 命中 `equals` 里任一值时显示（OR）。
    pub fn show_if_any(mut self, key: &'static str, equals: &'static [&'static str]) -> Self {
        self.show_if = Some(ShowIf {
            key,
            equals: equals.to_vec(),
            also: None,
        });
        self
    }

    /// 追加一个「同时满足且命中任一值」的条件（AND + OR）。
    pub fn and_if_any(mut self, key: &'static str, equals: &'static [&'static str]) -> Self {
        if let Some(s) = self.show_if.as_mut() {
            // 已经有一层 also 时不再往下加：schema 到这里为止
            if s.also.is_none() {
                s.also = Some(Box::new(ShowIf {
                    key,
                    equals: equals.to_vec(),
                    also: None,
                }));
            }
        }
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
    /// 设置页右上角是否显示「启用」开关。
    ///
    /// 「通用」是配置容器、「关于」是只读信息，两者都**没有可停用的东西**。
    /// 以前是显示开关、点了再回滚 + 弹原因（讲道理但绕）；会话 25 用户要求
    /// 直接不显示——没有意义的选择不该出现在界面上。
    fn switchable(&self) -> bool {
        true
    }
    /// 状态摘要，显示在工具名下方与分区顶部。例如「百度翻译 · 已就绪」/「未配置」
    fn status_line(&self) -> String;
    /// 设置项分组。多数工具返回一个 Section，翻译返回三个。
    fn sections(&self) -> Vec<Section>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    /// 10 种类型都必须序列化成前端认识的形状：`{"type":"xxx", ...}`。
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
            (Field::group("分组"), "group"),
            (Field::list("i", "I", vec![]), "list"),
        ];
        for (f, ty) in cases {
            let v: Value = serde_json::to_value(&f).unwrap();
            assert_eq!(v["type"], ty, "字段 {ty} 的 type 不对：{v}");
            assert_eq!(v["label"], f.label);
        }
    }

    /// 预设按钮是可选属性：没有 presets 的文本字段不该在 JSON 里带这个键
    /// （前端要判空，多余的空数组会让它画一排空白按钮）。
    #[test]
    fn presets_only_appear_when_declared() {
        let v: Value = serde_json::to_value(Field::text_hint("a", "A", "提示")).unwrap();
        assert!(v.get("presets").is_none(), "不该出现 presets：{v}");
        let v: Value = serde_json::to_value(Field::text_presets(
            "ua",
            "UA",
            "选填",
            &[("Chrome", "Mozilla/5.0")],
        ))
        .unwrap();
        assert_eq!(v["presets"][0][0], "Chrome");
        assert_eq!(v["presets"][0][1], "Mozilla/5.0");
    }

    /// 分组标题没有任何值，前端只读 `title`。
    #[test]
    fn group_carries_its_title() {
        let v: Value = serde_json::to_value(Field::group("下载路径")).unwrap();
        assert_eq!(v["type"], "group");
        assert_eq!(v["title"], "下载路径");
        assert_eq!(v["label"], "");
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
        // 会话 29 起 equals 统一是数组（前端按「命中任一」判断）
        assert_eq!(v["show_if"]["equals"], serde_json::json!(["baidu"]));
        assert!(
            v["show_if"].get("also").is_none(),
            "单条件时不该出现 also：{v}"
        );
    }

    /// 多值 + AND：翻译引擎的凭据字段就靠这个同时看「模式」和「厂商」。
    #[test]
    fn show_if_supports_multiple_values_and_one_and() {
        let f = Field::password("youdao_app_secret", "应用密钥")
            .show_if("vendor", "youdao")
            .and_if_any("engine_mode", &["vendor_mt", "vendor_llm"]);
        let v: Value = serde_json::to_value(&f).unwrap();
        assert_eq!(v["show_if"]["key"], "vendor");
        assert_eq!(v["show_if"]["equals"], serde_json::json!(["youdao"]));
        let also = &v["show_if"]["also"];
        assert_eq!(also["key"], "engine_mode");
        assert_eq!(also["equals"], serde_json::json!(["vendor_mt", "vendor_llm"]));
        assert!(also.get("also").is_none(), "只允许一层 AND");
    }

    /// list 的 item_fields 与 active_key 要能序列化给前端：
    /// 前端靠 `item_fields` 决定每张卡片画哪些输入框，靠 `active_key`
    /// 知道「使用中」单选写到哪个配置键。
    #[test]
    fn list_serializes_item_fields_and_active_key() {
        let f = Field::list(
            "profiles",
            "模型列表",
            vec![
                Field::text_hint("name", "名称", "例如 DeepSeek"),
                Field::password("key", "API Key"),
            ],
        )
        .active_key("active_id");
        let v: Value = serde_json::to_value(&f).unwrap();
        assert_eq!(v["type"], "list");
        assert_eq!(v["active_key"], "active_id");
        let items = v["item_fields"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["key"], "name");
        assert_eq!(items[0]["type"], "text");
        assert_eq!(items[1]["type"], "password");
    }

    /// 没设 active_key 时不该出现这个键（前端按「没有它就不画单选」判断）。
    #[test]
    fn list_without_active_key_omits_it() {
        let v: Value = serde_json::to_value(Field::list("p", "P", vec![])).unwrap();
        assert!(v.get("active_key").is_none(), "{v}");
    }

    /// `active_key` 对非 list 字段是空操作，不该 panic 也不该改类型。
    #[test]
    fn active_key_on_non_list_is_noop() {
        let v: Value = serde_json::to_value(Field::switch("a", "A").active_key("b")).unwrap();
        assert_eq!(v["type"], "switch");
        assert!(v.get("active_key").is_none());
    }

    /// 没有 `show_if` 时 `and_if_any` 是空操作，不该凭空造出一个条件。
    #[test]
    fn and_if_without_show_if_is_noop() {
        let f = Field::switch("a", "A").and_if_any("b", &["x"]);
        assert!(f.show_if.is_none());
    }

    /// AND 只允许一层：再加一次不该覆盖或叠加出第三层。
    #[test]
    fn and_if_keeps_single_level() {
        let f = Field::switch("a", "A")
            .show_if("b", "x")
            .and_if_any("c", &["y"])
            .and_if_any("d", &["z"]);
        let s = f.show_if.unwrap();
        assert_eq!(s.also.as_ref().unwrap().key, "c");
        assert!(s.also.unwrap().also.is_none(), "不该出现第三层");
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
