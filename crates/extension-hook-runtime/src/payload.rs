use std::{fmt, future::Future, pin::Pin};

use provider_protocol::ConversationItem;
use tokio_util::sync::CancellationToken;
use tool_runtime::{ToolCall, ToolResult};

use crate::{HookFailureKind, HookRejectionKind};

/// Hook trait object 使用的 owned async result。
pub type HookFuture<T> = Pin<Box<dyn Future<Output = Result<T, HookFailureKind>> + Send + 'static>>;

/// `before_turn` 可以变换的 provider-visible conversation items。
pub struct BeforeTurnPayload {
    items: Vec<ConversationItem>,
}

impl BeforeTurnPayload {
    /// 从非空 provider-visible items 创建 payload。
    pub fn try_new(items: Vec<ConversationItem>) -> Result<Self, BeforeTurnPayloadError> {
        if items.is_empty() {
            return Err(BeforeTurnPayloadError::EmptyItems);
        }
        Ok(Self { items })
    }

    /// 返回 hook 可检查的 provider-visible items。
    pub fn items(&self) -> &[ConversationItem] {
        &self.items
    }

    /// 用新的非空 items 替换当前 serial-transform value。
    pub fn replace_items(
        self,
        items: Vec<ConversationItem>,
    ) -> Result<Self, BeforeTurnPayloadError> {
        Self::try_new(items)
    }

    /// 消费 payload 并返回变换后的 items。
    pub fn into_items(self) -> Vec<ConversationItem> {
        self.items
    }
}

impl fmt::Debug for BeforeTurnPayload {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BeforeTurnPayload")
            .field("item_count", &self.items.len())
            .finish()
    }
}

/// `BeforeTurnPayload` 的封闭校验错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BeforeTurnPayloadError {
    #[error("before_turn_items_must_not_be_empty")]
    EmptyItems,
}

/// `before_turn` 的 serial-transform result。
pub enum BeforeTurnDecision {
    Continue(BeforeTurnPayload),
    Reject(HookRejectionKind),
}

/// `before_turn` hook interface。
pub trait BeforeTurnHook: Send + Sync {
    fn call(
        &self,
        payload: BeforeTurnPayload,
        cancellation: CancellationToken,
    ) -> HookFuture<BeforeTurnDecision>;
}

impl<F, Fut> BeforeTurnHook for F
where
    F: Fn(BeforeTurnPayload, CancellationToken) -> Fut + Send + Sync,
    Fut: Future<Output = Result<BeforeTurnDecision, HookFailureKind>> + Send + 'static,
{
    fn call(
        &self,
        payload: BeforeTurnPayload,
        cancellation: CancellationToken,
    ) -> HookFuture<BeforeTurnDecision> {
        Box::pin(self(payload, cancellation))
    }
}

/// `before_tool_execute` 只读持有已经过 permission 的 parsed call。
pub struct BeforeToolExecutePayload {
    call: ToolCall,
}

impl BeforeToolExecutePayload {
    /// 创建只读 tool gate payload。
    pub fn new(call: ToolCall) -> Self {
        Self { call }
    }

    /// 返回 hook 可检查但不可替换的 parsed call。
    pub fn call(&self) -> &ToolCall {
        &self.call
    }
}

impl fmt::Debug for BeforeToolExecutePayload {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BeforeToolExecutePayload")
            .finish_non_exhaustive()
    }
}

/// `before_tool_execute` 的 serial-gate result。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BeforeToolExecuteDecision {
    Continue,
    Reject(HookRejectionKind),
}

/// `before_tool_execute` hook interface。
pub trait BeforeToolExecuteHook: Send + Sync {
    fn call(
        &self,
        payload: BeforeToolExecutePayload,
        cancellation: CancellationToken,
    ) -> HookFuture<BeforeToolExecuteDecision>;
}

impl<F, Fut> BeforeToolExecuteHook for F
where
    F: Fn(BeforeToolExecutePayload, CancellationToken) -> Fut + Send + Sync,
    Fut: Future<Output = Result<BeforeToolExecuteDecision, HookFailureKind>> + Send + 'static,
{
    fn call(
        &self,
        payload: BeforeToolExecutePayload,
        cancellation: CancellationToken,
    ) -> HookFuture<BeforeToolExecuteDecision> {
        Box::pin(self(payload, cancellation))
    }
}

/// `after_tool_result` 可以 serial-transform 的 raw tool result。
pub struct AfterToolResultPayload {
    tool_name: String,
    result: ToolResult,
}

impl AfterToolResultPayload {
    /// 创建 raw tool result payload；registry 会验证 serial output identity。
    pub fn new(tool_name: impl Into<String>, result: ToolResult) -> Self {
        Self {
            tool_name: tool_name.into(),
            result,
        }
    }

    /// 返回当前 tool name。
    pub fn tool_name(&self) -> &str {
        &self.tool_name
    }

    /// 返回当前 serial-transform result。
    pub fn result(&self) -> &ToolResult {
        &self.result
    }

    /// 替换 result；call identity 不匹配时在 mutation 前拒绝。
    pub fn replace_result(self, result: ToolResult) -> Result<Self, AfterToolResultPayloadError> {
        if result.call_id() != self.result.call_id() {
            return Err(AfterToolResultPayloadError::CallIdentityChanged);
        }
        Ok(Self {
            tool_name: self.tool_name,
            result,
        })
    }

    /// 消费 payload 并返回变换后的 raw result。
    pub fn into_result(self) -> ToolResult {
        self.result
    }

    pub(crate) fn has_identity(&self, tool_name: &str, call_id: &str) -> bool {
        self.tool_name == tool_name && self.result.call_id() == call_id
    }
}

impl fmt::Debug for AfterToolResultPayload {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AfterToolResultPayload")
            .field("outcome", &self.result.outcome())
            .field("content_block_count", &self.result.content().iter().count())
            .field(
                "has_display_content",
                &self.result.display_content().is_some(),
            )
            .field("has_details", &self.result.details().is_some())
            .finish()
    }
}

/// `AfterToolResultPayload` 的封闭校验错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AfterToolResultPayloadError {
    #[error("after_tool_result_call_identity_changed")]
    CallIdentityChanged,
}

/// `after_tool_result` 的 serial-transform result。
pub enum AfterToolResultDecision {
    Continue(AfterToolResultPayload),
}

/// `after_tool_result` hook interface。
pub trait AfterToolResultHook: Send + Sync {
    fn call(
        &self,
        payload: AfterToolResultPayload,
        cancellation: CancellationToken,
    ) -> HookFuture<AfterToolResultDecision>;
}

impl<F, Fut> AfterToolResultHook for F
where
    F: Fn(AfterToolResultPayload, CancellationToken) -> Fut + Send + Sync,
    Fut: Future<Output = Result<AfterToolResultDecision, HookFailureKind>> + Send + 'static,
{
    fn call(
        &self,
        payload: AfterToolResultPayload,
        cancellation: CancellationToken,
    ) -> HookFuture<AfterToolResultDecision> {
        Box::pin(self(payload, cancellation))
    }
}
