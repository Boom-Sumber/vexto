//! 工具注册表：**所有工具的唯一定义处**。
//!
//! 加一个新工具就两件事：①在 `src/tools/` 下实现 `Tool` ②在这个文件里加一行。
//! 托盘、设置页左侧列表、模块开关、状态摘要全部从这里读，前端零改动（ADR-015）。

use super::{about::About, download::Download, general::General, screenshot::Screenshot, transfer::Transfer, Tool};
use super::translate_tool::Translate;

/// 所有工具，按设置页左侧列表的顺序返回。
///
/// 顺序是有意义的：「常规」永远第一个，「关于」永远最后，中间按使用频率排。
pub fn all() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(General),
        Box::new(Translate),
        Box::new(Screenshot),
        Box::new(Download),
        Box::new(Transfer),
        Box::new(About),
    ]
}

/// 按 id 取工具。未知 id 返回 `None`，由调用方给中文错误。
pub fn by_id(id: &str) -> Option<Box<dyn Tool>> {
    all().into_iter().find(|t| t.id() == id)
}

#[cfg(test)]
mod tests {
    use super::all;
    use crate::tools::FieldKind;
    use std::collections::HashSet;

    /// id 必须唯一且非空，否则设置页左列表会出现两个一样的项，
    /// 保存配置时也会写错工具。
    #[test]
    fn registry_ids_are_unique() {
        let mut seen = HashSet::new();
        for t in all() {
            let id = t.id();
            assert!(!id.is_empty(), "工具 id 不能为空");
            assert!(seen.insert(id), "工具 id 重复：{id}");
        }
    }

    /// S7 截图工具必须真的注册进注册表，否则设置页左侧不会出现这一项。
    #[test]
    fn screenshot_tool_is_registered() {
        assert!(super::by_id("screenshot").is_some());
    }

    /// 每个工具都必须至少有一个 Section，否则设置页点进去是空白。
    /// 每组也必须有 id/name，且同一个工具内 section id 唯一（前端靠它记标签）。
    #[test]
    fn every_tool_has_named_sections() {
        for t in all() {
            let secs = t.sections();
            assert!(!secs.is_empty(), "工具 {} 没有任何 Section", t.id());
            let mut ids = HashSet::new();
            for s in &secs {
                assert!(!s.id.is_empty(), "工具 {} 有 Section 缺 id", t.id());
                assert!(!s.name.is_empty(), "工具 {} 有 Section 缺 name", t.id());
                assert!(ids.insert(s.id), "工具 {} 的 Section id 重复：{}", t.id(), s.id);
            }
        }
    }

    /// `show_if` 引用的 key 必须真的存在于同一工具的某个字段里，
    /// 否则前端等着一个永远不存在的值，字段永远不会显示（这种 bug 极难发现）。
    #[test]
    fn show_if_refers_to_existing_field() {
        for t in all() {
            let secs = t.sections();
            let keys: HashSet<&str> = secs
                .iter()
                .flat_map(|s| s.fields.iter())
                .map(|f| f.key)
                .collect();
            for s in &secs {
                for f in &s.fields {
                    if let Some(cond) = &f.show_if {
                        assert!(
                            keys.contains(cond.key),
                            "工具 {} 的字段 {} 依赖不存在的 {}",
                            t.id(),
                            f.key,
                            cond.key
                        );
                    }
                }
            }
        }
    }

    /// 同一个工具里 `Value` 类字段（要存进 Config 的）的 key 不许重复，
    /// 否则两个控件绑同一个值，改一个另一个跟着变。
    #[test]
    fn field_keys_are_unique_within_tool() {
        for t in all() {
            let mut seen = HashSet::new();
            for s in t.sections() {
                for f in s.fields {
                    // Note 用文案当 key，Button 用动作名，Group 用标题，
                    // 三者都可能重复（两个同名分组 / 两条一样的说明），跳过
                    let is_value_field = !matches!(
                        f.kind,
                        FieldKind::Note { .. }
                            | FieldKind::Button
                            | FieldKind::ButtonRow { .. }
                            | FieldKind::Group { .. }
                            | FieldKind::About { .. }
                    );
                    if !is_value_field {
                        continue;
                    }
                    assert!(
                        seen.insert(f.key),
                        "工具 {} 里字段 key 重复：{}",
                        t.id(),
                        f.key
                    );
                }
            }
        }
    }

    /// 翻译工具必须有两个标签（配置 / 服务管理），这是去开发化后的信息架构。
    #[test]
    fn translate_has_two_tabs() {
        let t = super::by_id("translate").expect("必须有 translate 工具");
        let ids: Vec<&str> = t.sections().iter().map(|s| s.id).collect();
        assert_eq!(ids, vec!["config", "services"]);
    }

    /// 常规三个标签（常规 / 工具管理 / 快捷键）、翻译两个、下载三个、互传两个；
    /// 截图与关于只有一个 Section。
    #[test]
    fn multi_section_tools_are_the_expected_ones() {
        for t in all() {
            let id = t.id();
            let n = t.sections().len();
            match id {
                "general" => assert_eq!(n, 3, "工具 {id} 应该是三个标签"),
                "translate" => assert_eq!(n, 2, "工具 {id} 应该是两个标签"),
                "download" => assert_eq!(n, 3, "工具 {id} 应该是三个标签"),
                "transfer" => assert_eq!(n, 2, "工具 transfer 应该是两个标签"),
                _ => assert_eq!(n, 1, "工具 {id} 不该有多个 Section"),
            }
        }
    }

    /// 下载的三个页签下标必须与设置页里记的「上次停在哪」对得上。
    #[test]
    fn download_tabs_are_general_network_advanced() {
        let t = super::by_id("download").expect("必须有 download 工具");
        let ids: Vec<&str> = t.sections().iter().map(|s| s.id).collect();
        assert_eq!(ids, vec!["general", "network", "advanced"]);
    }

    /// 「常规」「关于」是配置容器与只读信息，不该有启用开关（会话 25 用户要求）。
    #[test]
    fn general_and_about_are_not_switchable() {
        for id in ["general", "about"] {
            let t = super::by_id(id).expect(id);
            assert!(!t.switchable(), "{id} 不该显示启用开关");
        }
        // 其余可停用的工具仍要有开关，不能一刀切全关掉
        for id in ["translate", "download", "transfer"] {
            let t = super::by_id(id).expect(id);
            assert!(t.switchable(), "{id} 应该有启用开关");
        }
    }
}
