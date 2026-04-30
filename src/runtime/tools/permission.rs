/// `ToolPermissionPolicy` 描述工具调用前是否需要用户确认。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolPermissionPolicy {
    #[default]
    Never,
    Ask,
    Always,
}
