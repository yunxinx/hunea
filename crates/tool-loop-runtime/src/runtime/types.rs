use std::time::{Duration, Instant};

use provider_protocol::ConversationItem;
use runtime_domain::session::{
    ManagedSearchTool, ProviderRequestMetrics, RuntimeTerminalSnapshot, RuntimeToolActivity,
    RuntimeToolActivityUpdate,
};
use tool_runtime::{
    DefaultToolErrorFormatter, SharedToolErrorFormatter, SharedToolPermissionHandler,
};

/// `ToolLoopProgress` describes runtime progress and provider-context session deltas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolLoopProgress {
    SystemMessage { message: String },
    ProviderTurnStarted,
    ProviderContextItem { item: ConversationItem },
    OutputTokens { total_tokens: usize },
    InputTokens { total_tokens: usize },
    Thinking { is_thinking: bool },
    AssistantDelta { content: String },
    ReasoningDelta { content: String },
    ToolActivityStarted { activity: RuntimeToolActivity },
    ToolActivityUpdated { update: RuntimeToolActivityUpdate },
    TerminalUpdated { snapshot: RuntimeTerminalSnapshot },
    ManagedSearchToolAuthorization { tool: ManagedSearchTool },
}

/// `ToolLoopResponse` 保存本轮运行时输出的完整 provider-visible items。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolLoopResponse {
    pub items: Vec<ConversationItem>,
    pub reasoning_duration: Option<Duration>,
}

impl ToolLoopResponse {
    /// `new` 从本轮新增的 provider-visible items 创建响应。
    pub fn new(items: Vec<ConversationItem>, reasoning_duration: Option<Duration>) -> Self {
        Self {
            items,
            reasoning_duration,
        }
    }
}

/// `ToolLoopCompletion` is returned when a runtime turn completes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolLoopCompletion {
    pub response: ToolLoopResponse,
    pub metrics: Option<ProviderRequestMetrics>,
    pub upstream_context_tokens: Option<usize>,
}

/// `ToolLoopClock` 抽象 runtime 计时来源，方便测试审批等待时间剔除逻辑。
#[derive(Clone)]
pub struct ToolLoopClock {
    now: std::sync::Arc<dyn Fn() -> Instant + Send + Sync>,
}

impl Default for ToolLoopClock {
    fn default() -> Self {
        Self {
            now: std::sync::Arc::new(Instant::now),
        }
    }
}

impl ToolLoopClock {
    /// `new` 创建自定义计时来源。
    pub fn new(now: impl Fn() -> Instant + Send + Sync + 'static) -> Self {
        Self {
            now: std::sync::Arc::new(now),
        }
    }

    pub(super) fn now(&self) -> Instant {
        (self.now)()
    }
}

/// `ToolLoopOptions` controls runtime-owned tool loop behavior.
#[derive(Clone)]
pub struct ToolLoopOptions {
    pub tool_max_turns: Option<usize>,
    pub permission_handler: Option<SharedToolPermissionHandler>,
    pub error_formatter: SharedToolErrorFormatter,
    pub clock: ToolLoopClock,
}

impl Default for ToolLoopOptions {
    fn default() -> Self {
        Self {
            tool_max_turns: None,
            permission_handler: None,
            error_formatter: std::sync::Arc::new(DefaultToolErrorFormatter),
            clock: ToolLoopClock::default(),
        }
    }
}
