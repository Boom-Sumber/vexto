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
pub mod screenshot;
pub mod transfer;
pub mod translate_tool;

use serde::Serialize;

/// 字段的联动条件：`key` 命中 `equals` 里任一值时就显示。
///
/// 仍然**不做任意表达式**（没有 `or` / `not` / 括号 / 嵌套比较）。`equals` 是数组
/// 而不是单值，是为了「一家厂商的凭据只在两种模式下显示」这类需求。
///
/// 会话 29 曾经加过一层 AND（`also`），会话 31 把引擎页签改成「选一个已配置实例」
/// 之后不再需要，构造手段已删——字段留着，真需要时直接构造即可，不必改序列化形状。
/// **别在这条路上继续加**，再复杂就该换方案了。
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
        /// 选项由前端**动态**算出来，`options` 作废。
        ///
        /// 目前只有 `"engines"`：翻译模式的下拉要列出用户配好的实例，数量随配置
        /// 变，静态表表达不了。前端按这个名字去找数据源。
        #[serde(skip_serializing_if = "Option::is_none")]
        dynamic: Option<&'static str>,
    },
    /// 主色调（外观功能）：预设色板 `(色名, #rrggbb)` + 前端自定义取色。
    /// 值是 `#rrggbb` 字符串，空串 = 跟随默认色。色板是**数据**（用户可选的
    /// 颜色），不是 UI 装饰色，所以由 Rust 侧给出、前端原样渲染。
    Color {
        presets: &'static [(&'static str, &'static str)],
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
    /// 可重复的一组配置（会话 30，会话 31 改成折叠卡片）。
    ///
    /// 用它的是「厂商管理」与「大模型管理」：用户可以加任意多套、各起名字。
    ///
    /// `item_fields` 描述「每一项里有哪些输入框」，它们的 `key` 是**对象内的键名**
    /// （例如 `name` / `vendor`），不是顶层配置键。整组的值是一个对象数组。
    /// **不支持嵌套 list**，也不支持项内 `show_if` 的跨项联动（只能在项内判断）。
    List {
        item_fields: Vec<Field>,
        /// 折叠时显示的一行摘要用哪几个字段（按顺序用「 · 」连起来）。
        /// 空 = 不折叠，卡片直接展开（会话 30 的样式）。
        #[serde(skip_serializing_if = "<[&str]>::is_empty")]
        summary: &'static [&'static str],
        /// 新建走弹窗（而不是在列表末尾插一张空卡）。
        /// 弹窗里只能填 `item_fields`，保存后 append 进列表。
        #[serde(default)]
        add_dialog: bool,
        /// 卡片头 / 新建弹窗里的「测试」按钮调哪个后端命令（会话 32）。
        /// 传当前项（可以是没保存的草稿）过去，后端用临时配置实测、不落盘。
        /// 只有「这套配置到底能不能用」需要验证的列表才给：厂商与模型。
        #[serde(skip_serializing_if = "Option::is_none")]
        test_action: Option<&'static str>,
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
    /// 字段旁的「?」悬浮说明。设置页不再单独画说明行。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<&'static str>,
    /// true = 改完**立即保存生效**，不进设置页的「未保存」列表（会话 57 用户裁定：
    /// 外观类字段所见即所得，不该再弹保存/放弃）。数据驱动，前端不认 key 名。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auto_apply: bool,
}

impl Field {
    fn new(key: &'static str, label: &'static str, kind: FieldKind) -> Self {
        Self {
            key,
            label,
            kind,
            show_if: None,
            help: None,
            auto_apply: false,
        }
    }

    /// 给字段挂一段「?」悬浮说明。设置页不再用独立的说明行。
    pub fn help(mut self, text: &'static str) -> Self {
        self.help = Some(text);
        self
    }

    /// 改完立即保存生效（外观类字段）。设置页对带此标记的字段不走 pending。
    pub fn auto_apply(mut self) -> Self {
        self.auto_apply = true;
        self
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
        Self::new(
            key,
            label,
            FieldKind::Select {
                options,
                dynamic: None,
            },
        )
    }

    /// 选项由前端动态生成的下拉（`source` 目前只有 `"engines"`）。
    pub fn select_dynamic(key: &'static str, label: &'static str, source: &'static str) -> Self {
        Self::new(
            key,
            label,
            FieldKind::Select {
                options: &[],
                dynamic: Some(source),
            },
        )
    }

    /// 主色调：预设色板 + 前端自定义取色（外观功能）。
    pub fn color(
        key: &'static str,
        label: &'static str,
        presets: &'static [(&'static str, &'static str)],
    ) -> Self {
        Self::new(key, label, FieldKind::Color { presets })
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
                summary: &[],
                add_dialog: false,
                test_action: None,
            },
        )
    }

    /// 折叠成单行卡片，摘要取这几个字段（按顺序）。对非 list 字段是空操作。
    pub fn summary(mut self, fields: &'static [&'static str]) -> Self {
        if let FieldKind::List { summary, .. } = &mut self.kind {
            *summary = fields;
        }
        self
    }

    /// 新建走弹窗。对非 list 字段是空操作。
    pub fn add_dialog(mut self) -> Self {
        if let FieldKind::List { add_dialog, .. } = &mut self.kind {
            *add_dialog = true;
        }
        self
    }

    /// 给卡片头和新建弹窗挂「测试」按钮，`cmd` 是后端命令名。
    /// 对非 list 字段是空操作。
    pub fn test_action(mut self, cmd: &'static str) -> Self {
        if let FieldKind::List { test_action, .. } = &mut self.kind {
            *test_action = Some(cmd);
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
    /// 设置页侧栏的图标名（ui/icons.js 的注册名）。
    /// 空 = 前端退回圆点。**纯展示字段**：不参与任何行为，前端也不允许
    /// 按工具名分支（图标跟着数据走，ADR-015 口径不变）。
    fn icon(&self) -> &'static str {
        ""
    }
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
    /// 状态摘要的颜色语义（会话 52）：`"ok"` 绿 / `"warn"` 黄 / `"error"` 红。
    /// 默认绿；服务启动失败这类必须红的，由工具自己覆盖。
    fn status_kind(&self) -> &'static str {
        "ok"
    }
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

    /// 主色调字段（外观功能）：type=color，色板原样带出。
    #[test]
    fn color_field_carries_presets() {
        let v: Value = serde_json::to_value(Field::color(
            "accent_color",
            "主色调",
            &[("#7c5cff", "高级紫")],
        ))
        .unwrap();
        assert_eq!(v["type"], "color");
        assert_eq!(v["presets"][0][0], "#7c5cff");
        assert_eq!(v["presets"][0][1], "高级紫");
    }

    /// auto_apply 只该出现在显式标记的字段上（外观类），其余字段不带这个键。
    #[test]
    fn auto_apply_is_omitted_when_false() {
        let v: Value = serde_json::to_value(Field::switch("a", "A")).unwrap();
        assert!(v.get("auto_apply").is_none(), "不该出现 auto_apply：{v}");
        let v: Value = serde_json::to_value(Field::color("b", "B", &[]).auto_apply()).unwrap();
        assert_eq!(v["auto_apply"], true);
    }

    /// general 的两个外观字段必须标记 auto_apply（会话 57 用户裁定：改完即存），
    /// 且除它们之外不该有别的字段带标记。
    #[test]
    fn appearance_fields_are_auto_apply() {
        let t = super::registry::by_id("general").expect("general tool");
        let flagged: Vec<&str> = t
            .sections()
            .iter()
            .flat_map(|s| s.fields.iter())
            .filter(|f| f.auto_apply)
            .map(|f| f.key)
            .collect();
        assert_eq!(flagged, vec!["appearance_theme", "accent_color"]);
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

    #[test]
    fn list_serializes_item_fields_summary_and_dialog() {
        let f = Field::list(
            "profiles",
            "模型列表",
            vec![
                Field::text_hint("name", "名称", "例如 DeepSeek"),
                Field::password("key", "API Key"),
            ],
        )
        .summary(&["name", "key"])
        .add_dialog();
        let v: Value = serde_json::to_value(&f).unwrap();
        assert_eq!(v["type"], "list");
        assert_eq!(v["summary"], serde_json::json!(["name", "key"]));
        assert_eq!(v["add_dialog"], true);
        let items = v["item_fields"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["key"], "name");
        assert_eq!(items[0]["type"], "text");
        assert_eq!(items[1]["type"], "password");
    }

    /// 不折叠时不写 `summary`、不弹窗时 `add_dialog` 是 false —— 前端据此
    /// 退回会话 30 的「整张展开 + 末尾插空卡」样式。
    #[test]
    fn list_without_summary_and_dialog_serializes_plain() {
        let v: Value = serde_json::to_value(Field::list("p", "P", vec![])).unwrap();
        assert!(v.get("summary").is_none(), "{v}");
        assert_eq!(v["add_dialog"], false);
    }

    /// `summary` / `add_dialog` 对非 list 字段是空操作，不该 panic 也不该改类型。
    #[test]
    fn list_builders_on_non_list_are_noops() {
        let v: Value =
            serde_json::to_value(Field::switch("a", "A").summary(&["x"]).add_dialog()).unwrap();
        assert_eq!(v["type"], "switch");
        assert!(v.get("summary").is_none());
        assert!(v.get("add_dialog").is_none());
    }

    /// 动态下拉要带上数据源名，前端按它去找候选项（`engines` = 已配置实例）。
    #[test]
    fn select_dynamic_serializes_source() {
        let v: Value = serde_json::to_value(Field::select_dynamic("active_engine", "翻译模式", "engines"))
            .unwrap();
        assert_eq!(v["type"], "select");
        assert_eq!(v["dynamic"], "engines");
        assert_eq!(v["options"].as_array().unwrap().len(), 0, "动态下拉不带静态选项");
        // 普通下拉不该出现 dynamic
        let v2: Value = serde_json::to_value(Field::select("s", "S", &[("a", "A")])).unwrap();
        assert!(v2.get("dynamic").is_none(), "{v2}");
    }

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
