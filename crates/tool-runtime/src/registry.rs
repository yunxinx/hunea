use std::collections::BTreeMap;

use super::ToolDefinition;

/// `ToolRegistry` 保存 runtime 可用工具定义。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolRegistry {
    definitions: BTreeMap<String, ToolDefinition>,
}

impl ToolRegistry {
    /// `new` 创建空工具注册表。
    pub fn new() -> Self {
        Self::default()
    }

    /// `insert` 注册或替换一个工具定义。
    pub fn insert(&mut self, definition: ToolDefinition) {
        self.definitions.insert(definition.name.clone(), definition);
    }

    /// `definition` 返回指定工具定义。
    pub fn definition(&self, name: &str) -> Option<&ToolDefinition> {
        self.definitions.get(name)
    }

    /// `definitions` 返回所有工具定义，按名称稳定排序。
    pub fn definitions(&self) -> impl Iterator<Item = &ToolDefinition> {
        self.definitions.values()
    }

    /// `is_empty` 返回当前是否没有任何工具定义。
    pub fn is_empty(&self) -> bool {
        self.definitions.is_empty()
    }
}
