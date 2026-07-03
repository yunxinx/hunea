use serde_json::Value;

use super::{ToolKind, ToolPermissionPolicy};

/// `ToolDefinition` 描述可暴露给 runtime/agent 的工具元数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDefinition {
    pub name: String,
    pub label: Option<String>,
    pub kind: ToolKind,
    pub description: Option<String>,
    pub input_schema: Option<Value>,
    pub permission_policy: ToolPermissionPolicy,
    /// 动态注入 system prompt 的工具使用指南；为 None 时不参与 tool guidelines 装配。
    pub prompt_guidelines: Option<String>,
}

impl ToolDefinition {
    /// `new` 创建一个工具定义。
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            label: None,
            kind: ToolKind::Other,
            description: None,
            input_schema: None,
            permission_policy: ToolPermissionPolicy::Never,
            prompt_guidelines: None,
        }
    }

    /// `with_label` 设置适合 TUI 展示的工具名称。
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// `with_kind` 设置工具的 runtime activity 语义分类。
    pub const fn with_kind(mut self, kind: ToolKind) -> Self {
        self.kind = kind;
        self
    }

    /// `with_description` 设置给模型使用的工具说明。
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// `with_input_schema` 设置工具参数 JSON Schema。
    pub fn with_input_schema(mut self, schema: Value) -> Self {
        self.input_schema = Some(schema);
        self
    }

    /// `with_permission_policy` 设置工具执行前的权限策略。
    pub const fn with_permission_policy(mut self, policy: ToolPermissionPolicy) -> Self {
        self.permission_policy = policy;
        self
    }

    /// `with_prompt_guidelines` 设置动态注入 system prompt 的工具使用指南。
    pub fn with_prompt_guidelines(mut self, guidelines: impl Into<String>) -> Self {
        self.prompt_guidelines = Some(guidelines.into());
        self
    }
}
