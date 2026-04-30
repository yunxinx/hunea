use serde_json::Value;

/// `RuntimeToolSchema` 包装工具参数 JSON Schema。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeToolSchema {
    pub value: Value,
}

impl RuntimeToolSchema {
    /// `new` 创建工具参数 schema。
    pub fn new(value: Value) -> Self {
        Self { value }
    }
}
