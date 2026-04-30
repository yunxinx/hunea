use std::collections::BTreeMap;

use super::RuntimeToolDefinition;

/// `RuntimeToolRegistry` 保存 runtime 可用工具定义。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeToolRegistry {
    definitions: BTreeMap<String, RuntimeToolDefinition>,
}

impl RuntimeToolRegistry {
    /// `new` 创建空工具注册表。
    pub fn new() -> Self {
        Self::default()
    }

    /// `insert` 注册或替换一个工具定义。
    pub fn insert(&mut self, definition: RuntimeToolDefinition) {
        self.definitions.insert(definition.name.clone(), definition);
    }

    /// `definition` 返回指定工具定义。
    pub fn definition(&self, name: &str) -> Option<&RuntimeToolDefinition> {
        self.definitions.get(name)
    }

    /// `definitions` 返回所有工具定义，按名称稳定排序。
    pub fn definitions(&self) -> impl Iterator<Item = &RuntimeToolDefinition> {
        self.definitions.values()
    }
}
