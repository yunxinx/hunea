/// `ToolPermissionPolicy` 描述工具调用的默认许可策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolPermissionPolicy {
    #[default]
    Never,
    Ask,
    Always,
}
