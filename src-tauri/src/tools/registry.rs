//! 工具注册表：**所有工具的唯一定义处**。
//!
//! 加一个新工具就两件事：①在 `src/tools/` 下实现 `Tool` ②在这个文件里加一行。
//! 托盘、设置页左侧列表、模块开关、状态摘要全部从这里读，前端零改动（ADR-015）。

use super::{about::About, download::Download, general::General, transfer::Transfer, Tool};
use super::translate_tool::Translate;

/// 所有工具，按设置页左侧列表的顺序返回。
///
/// 顺序是有意义的：「通用」永远第一个，「关于」永远最后，中间按使用频率排。
pub fn all() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(General),
        Box::new(Translate),
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
                    // Note 用文案当 key，Button 用动作名，都可能重复（两个 note 内容一样时），跳过
                    let is_value_field = !matches!(
                        f.kind,
                        FieldKind::Note { .. } | FieldKind::Button
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

    /// 翻译工具必须有三个标签（引擎 / 划词 / 截图），这是用户 m01468 定的信息架构。
    #[test]
    fn translate_has_three_tabs() {
        let t = super::by_id("translate").expect("必须有 translate 工具");
        let ids: Vec<&str> = t.sections().iter().map(|s| s.id).collect();
        assert_eq!(ids, vec!["engine", "selection", "ocr"]);
    }

    /// 只有翻译是多标签；其余四个工具都必须只有一个 Section，
    /// 这样前端不会给它们画一层没意义的标签栏。
    #[test]
    fn only_translate_has_multiple_sections() {
        for t in all() {
            if t.id() == "translate" {
                continue;
            }
            assert_eq!(
                t.sections().len(),
                1,
                "工具 {} 不该有多个 Section",
                t.id()
            );
        }
    }
}
