/// `AttentionPillState` 保存左侧常驻 attention pill 的待办提示状态。
///
/// 与右侧 toast 的"自动消失的结果反馈"语义不同，pill 表示条件满足才消失的待办：
/// 新消息 pill 在用户回到贴底后清除，审批 pill 在审批面板可见或被处理后清除，
/// 因此不复用 `ToastState` 的超时状态机。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AttentionPillState {
    /// 离底或被全屏层遮挡期间累计的最终消息数；`None` 表示无新消息 pill。
    pub(super) new_message_count: Option<usize>,
    /// 审批面板打开但不可见（被全屏层遮挡或非贴底）时置位。
    pub(super) approval_pending: bool,
}

/// `AttentionPillKind` 区分可点击的 pill 类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttentionPillKind {
    /// 有工具等待审批（审批面板打开但不可见）。
    ToolApproval,
    /// 离底期间到达的最终消息计数。
    NewMessages,
}
