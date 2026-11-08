//! 工具注册表单例列表。
//!
//! S0 只有接口没有实现，所以这里返回空数组 —— 这是**预期状态**，不是未完成。
//! S2 起每个工具在这里加一行。

use super::Tool;

pub fn all() -> Vec<Box<dyn Tool>> {
    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::Field;
    use std::collections::HashSet;

    /// S0 的空注册表是**预期状态**，不是漏写。
    /// 这个测试同时守住两件事：`all()` 能被调用且不 panic、
    /// 以及 S2 往里加实现之后 id 不许重复（id 是配置 key 和设置页分区 id）。
    #[test]
    fn registry_ids_are_unique() {
        let tools = all();
        let mut seen = HashSet::new();
        for t in &tools {
            assert!(
                seen.insert(t.id()),
                "工具 id 重复：{}（id 是配置里的 key，一旦发布不许改）",
                t.id()
            );
            assert!(!t.id().is_empty(), "工具 id 不能为空");
        }
    }

    /// `Field` 的序列化形状是前端契约（`ui/settings.js` 照着 `type` 分派渲染）。
    /// 这里把它钉死，避免 S2 改枚举时悄悄换了 tag 名而前端毫无察觉。
    #[test]
    fn field_serializes_with_snake_case_type_tag() {
        let f = Field::Password {
            key: "secret",
            label: "密钥",
        };
        let v = serde_json::to_value(&f).expect("Field 必须能序列化成 JSON");
        assert_eq!(v["type"], "password");
        assert_eq!(v["key"], "secret");

        let n = Field::Note {
            text: "标准版 QPS 为 1",
        };
        let v = serde_json::to_value(&n).expect("Field 必须能序列化成 JSON");
        assert_eq!(v["type"], "note");
    }
}
