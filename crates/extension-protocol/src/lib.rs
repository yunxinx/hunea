//! 版本化的进程外扩展协议值与 Content-Length framing。
//!
//! 本 crate 只负责 wire boundary，不拥有 process、permission policy、cancellation task 或
//! runtime registry；这些职责属于 host adapter。

use std::fmt;

use provider_protocol::ConversationItem;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use thiserror::Error;

/// Wire envelope 使用的固定 protocol name。
pub const PROTOCOL_NAME: &str = "hunea-extension";
/// 当前 wire protocol version。
pub const PROTOCOL_VERSION: u16 = 2;
/// 默认允许的最大 frame body 大小。
pub const DEFAULT_MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;
const MAX_HEADER_LINE_BYTES: usize = 8 * 1024;
const MAX_HEADER_BYTES: usize = 32 * 1024;
const MAX_REQUEST_ID_BYTES: usize = 128;

/// 在 initialization 阶段协商的显式 capability grant。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionCapability {
    Progress,
    Cancel,
    StructuredErrors,
    Hooks,
}

impl fmt::Debug for ExtensionCapability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ExtensionCapability")
    }
}

/// 协议支持的初始化、tool、typed hook 与关闭 method。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ExtensionMethod {
    #[serde(rename = "initialize")]
    Initialize,
    #[serde(rename = "tools.list")]
    ToolsList,
    #[serde(rename = "tools.execute")]
    ToolsExecute,
    #[serde(rename = "tools.cancel")]
    ToolsCancel,
    #[serde(rename = "hooks.list")]
    HooksList,
    #[serde(rename = "hooks.before_turn")]
    HooksBeforeTurn,
    #[serde(rename = "hooks.before_tool_execute")]
    HooksBeforeToolExecute,
    #[serde(rename = "hooks.after_tool_result")]
    HooksAfterToolResult,
    #[serde(rename = "hooks.cancel")]
    HooksCancel,
    #[serde(rename = "shutdown")]
    Shutdown,
}

impl ExtensionMethod {
    /// 返回稳定的 wire method name。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Initialize => "initialize",
            Self::ToolsList => "tools.list",
            Self::ToolsExecute => "tools.execute",
            Self::ToolsCancel => "tools.cancel",
            Self::HooksList => "hooks.list",
            Self::HooksBeforeTurn => "hooks.before_turn",
            Self::HooksBeforeToolExecute => "hooks.before_tool_execute",
            Self::HooksAfterToolResult => "hooks.after_tool_result",
            Self::HooksCancel => "hooks.cancel",
            Self::Shutdown => "shutdown",
        }
    }
}

/// Request envelope；params 只由对应 method owner 解码。
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtensionRequest {
    protocol: String,
    version: u16,
    request_id: String,
    method: ExtensionMethod,
    params: Value,
    deadline_ms: Option<u64>,
    capabilities: Vec<ExtensionCapability>,
}

impl ExtensionRequest {
    /// 使用当前 protocol identity 创建 request，并编码 typed params。
    pub fn new(
        request_id: impl Into<String>,
        method: ExtensionMethod,
        params: impl Serialize,
    ) -> Result<Self, ProtocolEncodeError> {
        Ok(Self {
            protocol: PROTOCOL_NAME.to_string(),
            version: PROTOCOL_VERSION,
            request_id: request_id.into(),
            method,
            params: serde_json::to_value(params).map_err(|_| ProtocolEncodeError::Json)?,
            deadline_ms: None,
            capabilities: Vec::new(),
        })
    }

    /// 设置 request deadline；`validate` 会拒绝零值。
    pub fn with_deadline_ms(mut self, deadline_ms: u64) -> Self {
        self.deadline_ms = Some(deadline_ms);
        self
    }

    /// 设置 request 声明的 capability 集合。
    pub fn with_capabilities(
        mut self,
        capabilities: impl IntoIterator<Item = ExtensionCapability>,
    ) -> Self {
        self.capabilities = capabilities.into_iter().collect();
        self
    }

    /// 返回 wire protocol name。
    pub fn protocol(&self) -> &str {
        &self.protocol
    }

    /// 返回 wire protocol version。
    pub const fn version(&self) -> u16 {
        self.version
    }

    /// 返回 request identity。
    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    /// 返回 typed method。
    pub const fn method(&self) -> ExtensionMethod {
        self.method
    }

    /// 返回可选 deadline milliseconds。
    pub const fn deadline_ms(&self) -> Option<u64> {
        self.deadline_ms
    }

    /// 返回 request 声明的 capability。
    pub fn capabilities(&self) -> &[ExtensionCapability] {
        &self.capabilities
    }

    /// 将 params 解码为 method owner 指定的 DTO。
    pub fn decode_params<T: DeserializeOwned>(&self) -> Result<T, ProtocolValidationError> {
        serde_json::from_value(self.params.clone())
            .map_err(|_| ProtocolValidationError::InvalidParams)
    }

    /// 验证 envelope 与 method-specific params。
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        validate_protocol(&self.protocol, self.version)?;
        validate_request_id(&self.request_id)?;
        if self.deadline_ms == Some(0) {
            return Err(ProtocolValidationError::InvalidDeadline);
        }
        validate_capabilities(&self.capabilities)?;
        if !self.params.is_object() {
            return Err(ProtocolValidationError::InvalidParams);
        }
        match self.method {
            ExtensionMethod::Initialize => self.decode_params::<InitializeParams>()?.validate(),
            ExtensionMethod::ToolsList => {
                self.decode_params::<ToolsListParams>()?;
                Ok(())
            }
            ExtensionMethod::ToolsExecute => self.decode_params::<ToolExecuteParams>()?.validate(),
            ExtensionMethod::ToolsCancel => self.decode_params::<ToolCancelParams>()?.validate(),
            ExtensionMethod::HooksList => {
                self.decode_params::<HooksListParams>()?;
                Ok(())
            }
            ExtensionMethod::HooksBeforeTurn => {
                self.decode_params::<BeforeTurnHookParams>()?.validate()
            }
            ExtensionMethod::HooksBeforeToolExecute => self
                .decode_params::<BeforeToolExecuteHookParams>()?
                .validate(),
            ExtensionMethod::HooksAfterToolResult => self
                .decode_params::<AfterToolResultHookParams>()?
                .validate(),
            ExtensionMethod::HooksCancel => self.decode_params::<HookCancelParams>()?.validate(),
            ExtensionMethod::Shutdown => {
                self.decode_params::<ShutdownParams>()?;
                Ok(())
            }
        }
    }
}

impl fmt::Debug for ExtensionRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExtensionRequest")
            .field("has_protocol", &!self.protocol.is_empty())
            .field("version", &self.version)
            .field("has_request_id", &!self.request_id.is_empty())
            .field("method", &self.method)
            .field("has_params", &true)
            .field("deadline_ms", &self.deadline_ms)
            .field("capability_count", &self.capabilities.len())
            .finish()
    }
}

/// Response envelope；构造后必须且只能存在 result 或 error 之一。
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtensionResponse {
    protocol: String,
    version: u16,
    request_id: String,
    result: Option<Value>,
    error: Option<ExtensionError>,
}

impl ExtensionResponse {
    /// 创建只含 typed result 的 success response。
    pub fn success<T: Serialize>(
        request_id: impl Into<String>,
        result: T,
    ) -> Result<Self, ProtocolEncodeError> {
        Ok(Self {
            protocol: PROTOCOL_NAME.to_string(),
            version: PROTOCOL_VERSION,
            request_id: request_id.into(),
            result: Some(serde_json::to_value(result).map_err(|_| ProtocolEncodeError::Json)?),
            error: None,
        })
    }

    /// 创建只含 structured error 的 failure response。
    pub fn failure(request_id: impl Into<String>, error: ExtensionError) -> Self {
        Self {
            protocol: PROTOCOL_NAME.to_string(),
            version: PROTOCOL_VERSION,
            request_id: request_id.into(),
            result: None,
            error: Some(error),
        }
    }

    /// 验证 protocol identity、request identity 与 response shape。
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        validate_protocol(&self.protocol, self.version)?;
        validate_request_id(&self.request_id)?;
        if self.result.is_some() == self.error.is_some() {
            return Err(ProtocolValidationError::InvalidResponseShape);
        }
        Ok(())
    }

    /// 返回关联的 request identity。
    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    /// 将 success result 解码为调用方指定的 DTO。
    pub fn result<T: DeserializeOwned>(&self) -> Result<Option<T>, ProtocolValidationError> {
        self.result
            .as_ref()
            .map(|value| {
                serde_json::from_value(value.clone())
                    .map_err(|_| ProtocolValidationError::InvalidResult)
            })
            .transpose()
    }

    /// 返回 failure response 的 structured error。
    pub fn error(&self) -> Option<&ExtensionError> {
        self.error.as_ref()
    }
}

impl fmt::Debug for ExtensionResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExtensionResponse")
            .field("has_protocol", &!self.protocol.is_empty())
            .field("version", &self.version)
            .field("has_request_id", &!self.request_id.is_empty())
            .field("has_result", &self.result.is_some())
            .field("error_code", &self.error.as_ref().map(ExtensionError::code))
            .finish()
    }
}

/// 稳定的 machine-readable error；message 必须由 host 生成且可安全展示。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionErrorCode {
    InvalidRequest,
    UnsupportedVersion,
    CapabilityDenied,
    ToolNotFound,
    ToolRejected,
    Cancelled,
    DeadlineExceeded,
    ProtocolViolation,
    Internal,
}

/// 可跨进程传输的 structured error。
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtensionError {
    code: ExtensionErrorCode,
    message: String,
    retryable: bool,
    #[serde(default)]
    details: Option<Value>,
}

impl ExtensionError {
    /// 使用稳定 code、安全 message 与 retry policy 创建 error。
    pub fn new(code: ExtensionErrorCode, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code,
            message: message.into(),
            retryable,
            details: None,
        }
    }

    /// 添加 structured details；`Debug` 不输出 details body。
    pub fn with_details<T: Serialize>(mut self, details: &T) -> Result<Self, ProtocolEncodeError> {
        self.details = Some(serde_json::to_value(details).map_err(|_| ProtocolEncodeError::Json)?);
        Ok(self)
    }

    /// 返回 machine-readable error code。
    pub const fn code(&self) -> ExtensionErrorCode {
        self.code
    }

    /// 返回由 host 投影的安全 message。
    pub fn message(&self) -> &str {
        &self.message
    }

    /// 返回调用方是否可以 retry。
    pub const fn retryable(&self) -> bool {
        self.retryable
    }

    /// 返回可选 structured details。
    pub fn details(&self) -> Option<&Value> {
        self.details.as_ref()
    }
}

impl fmt::Debug for ExtensionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExtensionError")
            .field("code", &self.code)
            .field("has_message", &!self.message.is_empty())
            .field("retryable", &self.retryable)
            .field("has_details", &self.details.is_some())
            .finish()
    }
}

/// 协议 DTO 无法编码时的安全错误投影。
#[derive(Debug, Error)]
pub enum ProtocolEncodeError {
    #[error("protocol value could not be encoded")]
    Json,
}

/// Envelope、method params 或 response 校验失败。
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolValidationError {
    #[error("unsupported protocol name")]
    UnsupportedProtocol,
    #[error("unsupported protocol version")]
    UnsupportedVersion,
    #[error("request id is empty or too long")]
    InvalidRequestId,
    #[error("deadline must be greater than zero")]
    InvalidDeadline,
    #[error("capability is declared more than once")]
    DuplicateCapability,
    #[error("hook id is invalid")]
    InvalidHookId,
    #[error("hook descriptor is declared more than once")]
    DuplicateHookDescriptor,
    #[error("hook conversation items are invalid")]
    InvalidConversationItems,
    #[error("hook tool call is invalid")]
    InvalidHookToolCall,
    #[error("hook tool result is invalid")]
    InvalidHookToolResult,
    #[error("request params are invalid")]
    InvalidParams,
    #[error("response must contain exactly one result or error")]
    InvalidResponseShape,
    #[error("response result is invalid")]
    InvalidResult,
}

/// `initialize` request params。
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct InitializeParams {
    pub protocol_version: u16,
    pub capabilities: Vec<ExtensionCapability>,
}

impl InitializeParams {
    /// 验证 protocol version 与 capability 唯一性。
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        if self.protocol_version != PROTOCOL_VERSION {
            return Err(ProtocolValidationError::UnsupportedVersion);
        }
        validate_capabilities(&self.capabilities)
    }
}

impl fmt::Debug for InitializeParams {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InitializeParams")
            .field("protocol_version", &self.protocol_version)
            .field("capability_count", &self.capabilities.len())
            .finish()
    }
}

/// `initialize` success result。
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct InitializeResult {
    pub protocol: String,
    pub version: u16,
    pub capabilities: Vec<ExtensionCapability>,
}

impl InitializeResult {
    /// 验证 protocol identity 与 capability 唯一性。
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        validate_protocol(&self.protocol, self.version)?;
        validate_capabilities(&self.capabilities)
    }
}

impl fmt::Debug for InitializeResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InitializeResult")
            .field("has_protocol", &!self.protocol.is_empty())
            .field("version", &self.version)
            .field("capability_count", &self.capabilities.len())
            .finish()
    }
}

/// `tools.list` request params。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolsListParams {}

/// Extension 暴露给 host 的 tool metadata。
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDescriptor {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: Option<Value>,
}

impl fmt::Debug for ToolDescriptor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ToolDescriptor")
            .field("has_name", &!self.name.is_empty())
            .field("has_description", &self.description.is_some())
            .field("has_input_schema", &self.input_schema.is_some())
            .finish()
    }
}

/// `tools.list` success result。
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolsListResult {
    pub tools: Vec<ToolDescriptor>,
}

impl fmt::Debug for ToolsListResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ToolsListResult")
            .field("tool_count", &self.tools.len())
            .finish()
    }
}

/// `tools.execute` request params。
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolExecuteParams {
    pub name: String,
    pub arguments: Value,
}

impl fmt::Debug for ToolExecuteParams {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ToolExecuteParams")
            .field("has_name", &!self.name.is_empty())
            .field("argument_kind", &json_kind(&self.arguments))
            .finish()
    }
}

impl ToolExecuteParams {
    /// 验证 tool identity 非空。
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        if self.name.trim().is_empty() {
            return Err(ProtocolValidationError::InvalidParams);
        }
        Ok(())
    }
}

/// `tools.cancel` request params。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCancelParams {
    pub request_id: String,
}

impl ToolCancelParams {
    /// 验证待取消 request 的 identity。
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        validate_request_id(&self.request_id)
    }
}

impl fmt::Debug for ToolCancelParams {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ToolCancelParams")
            .field("has_request_id", &!self.request_id.is_empty())
            .finish()
    }
}

/// `tools.cancel` success result。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCancelResult {
    pub accepted: bool,
}

/// Typed hook phase；wire 不接受任意字符串 phase。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookPhase {
    BeforeTurn,
    BeforeToolExecute,
    AfterToolResult,
}

/// Remote hook descriptor；timeout 与 cancellation grace 始终由 host 决定。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookDescriptor {
    pub hook_id: String,
    pub phase: HookPhase,
    pub priority: i32,
}

impl HookDescriptor {
    /// 验证可用于 deterministic registration 的 hook identity。
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        validate_hook_id(&self.hook_id)
    }
}

impl fmt::Debug for HookDescriptor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HookDescriptor")
            .field("has_hook_id", &!self.hook_id.is_empty())
            .field("phase", &self.phase)
            .field("priority", &self.priority)
            .finish()
    }
}

/// `hooks.list` request params。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HooksListParams {}

/// `hooks.list` success result。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HooksListResult {
    pub hooks: Vec<HookDescriptor>,
}

impl HooksListResult {
    /// 验证全部 descriptor，并拒绝同一 phase 内的 duplicate hook identity。
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        let mut identities = std::collections::BTreeSet::new();
        for descriptor in &self.hooks {
            descriptor.validate()?;
            if !identities.insert((descriptor.phase, descriptor.hook_id.as_str())) {
                return Err(ProtocolValidationError::DuplicateHookDescriptor);
            }
        }
        Ok(())
    }
}

impl fmt::Debug for HooksListResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HooksListResult")
            .field("hook_count", &self.hooks.len())
            .finish()
    }
}

/// Hook gate 可以返回的封闭 rejection code。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookRejectionCode {
    PolicyDenied,
    UnsupportedOperation,
}

/// `hooks.before_turn` request params。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BeforeTurnHookParams {
    pub hook_id: String,
    pub items: Vec<ConversationItem>,
}

impl BeforeTurnHookParams {
    /// 验证 hook identity 与 provider-neutral conversation item 语义。
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        validate_hook_id(&self.hook_id)?;
        validate_conversation_items(&self.items)
    }
}

impl fmt::Debug for BeforeTurnHookParams {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BeforeTurnHookParams")
            .field("has_hook_id", &!self.hook_id.is_empty())
            .field("item_count", &self.items.len())
            .finish()
    }
}

/// `hooks.before_turn` typed success result。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum BeforeTurnHookResult {
    Continue { items: Vec<ConversationItem> },
    Reject { code: HookRejectionCode },
}

impl BeforeTurnHookResult {
    /// 验证 transform output；continue 必须保留非空、语义合法的 item list。
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        match self {
            Self::Continue { items } => validate_conversation_items(items),
            Self::Reject { .. } => Ok(()),
        }
    }
}

impl fmt::Debug for BeforeTurnHookResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Continue { items } => formatter
                .debug_struct("Continue")
                .field("item_count", &items.len())
                .finish(),
            Self::Reject { code } => formatter
                .debug_struct("Reject")
                .field("code", code)
                .finish(),
        }
    }
}

/// Hook protocol 自有的 pure tool call DTO。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookToolCall {
    pub call_id: String,
    pub name: String,
    pub arguments: Value,
}

impl HookToolCall {
    /// 验证 correlation identity 与 tool name。
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        validate_request_id(&self.call_id)
            .map_err(|_| ProtocolValidationError::InvalidHookToolCall)?;
        validate_tool_name(&self.name)
    }
}

impl fmt::Debug for HookToolCall {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HookToolCall")
            .field("has_call_id", &!self.call_id.is_empty())
            .field("has_name", &!self.name.is_empty())
            .field("argument_kind", &json_kind(&self.arguments))
            .finish()
    }
}

/// `hooks.before_tool_execute` request params。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BeforeToolExecuteHookParams {
    pub hook_id: String,
    pub call: HookToolCall,
}

impl BeforeToolExecuteHookParams {
    /// 验证 hook identity 与 parsed call DTO。
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        validate_hook_id(&self.hook_id)?;
        self.call.validate()
    }
}

impl fmt::Debug for BeforeToolExecuteHookParams {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BeforeToolExecuteHookParams")
            .field("has_hook_id", &!self.hook_id.is_empty())
            .field("has_call", &true)
            .finish()
    }
}

/// `hooks.before_tool_execute` typed success result。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum BeforeToolExecuteHookResult {
    Continue,
    Reject { code: HookRejectionCode },
}

/// Hook tool result 的控制语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookToolResultOutcome {
    Success,
    Error,
    Terminate,
}

/// Hook tool result image 的 provider detail hint。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HookToolImageDetail {
    High,
    Original,
}

/// Hook protocol 自有的 pure tool result content DTO。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HookToolResultContent {
    Text {
        text: String,
    },
    Image {
        data_base64: String,
        mime_type: String,
        uri: Option<String>,
        detail: Option<HookToolImageDetail>,
    },
}

impl HookToolResultContent {
    fn validate(&self) -> Result<(), ProtocolValidationError> {
        match self {
            Self::Text { .. } => Ok(()),
            Self::Image {
                data_base64,
                mime_type,
                ..
            } if !data_base64.is_empty() && !mime_type.trim().is_empty() => Ok(()),
            Self::Image { .. } => Err(ProtocolValidationError::InvalidHookToolResult),
        }
    }
}

impl fmt::Debug for HookToolResultContent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text { text } => formatter
                .debug_struct("Text")
                .field("char_count", &text.chars().count())
                .finish(),
            Self::Image {
                data_base64,
                mime_type,
                uri,
                detail,
            } => formatter
                .debug_struct("Image")
                .field("encoded_len", &data_base64.len())
                .field("has_mime_type", &!mime_type.is_empty())
                .field("has_uri", &uri.is_some())
                .field("detail", detail)
                .finish(),
        }
    }
}

/// Hook protocol 自有的完整 tool result DTO。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookToolResult {
    pub call_id: String,
    pub content: Vec<HookToolResultContent>,
    pub outcome: HookToolResultOutcome,
    pub display_content: Option<String>,
    pub details: Option<Value>,
}

impl HookToolResult {
    /// 验证 correlation identity 与结构化 content。
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        validate_request_id(&self.call_id)
            .map_err(|_| ProtocolValidationError::InvalidHookToolResult)?;
        self.content
            .iter()
            .try_for_each(HookToolResultContent::validate)
    }
}

impl fmt::Debug for HookToolResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HookToolResult")
            .field("has_call_id", &!self.call_id.is_empty())
            .field("content_count", &self.content.len())
            .field("outcome", &self.outcome)
            .field("has_display_content", &self.display_content.is_some())
            .field("has_details", &self.details.is_some())
            .finish()
    }
}

/// `hooks.after_tool_result` request params。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AfterToolResultHookParams {
    pub hook_id: String,
    pub tool_name: String,
    pub result: HookToolResult,
}

impl AfterToolResultHookParams {
    /// 验证 hook、tool 与 call correlation identity。
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        validate_hook_id(&self.hook_id)?;
        validate_tool_name(&self.tool_name)?;
        self.result.validate()
    }
}

impl fmt::Debug for AfterToolResultHookParams {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AfterToolResultHookParams")
            .field("has_hook_id", &!self.hook_id.is_empty())
            .field("has_tool_name", &!self.tool_name.is_empty())
            .field("outcome", &self.result.outcome)
            .finish()
    }
}

/// `hooks.after_tool_result` typed success result。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum AfterToolResultHookResult {
    Continue { result: HookToolResult },
}

impl AfterToolResultHookResult {
    /// 验证 remote transform 返回的完整 tool result。
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        match self {
            Self::Continue { result } => result.validate(),
        }
    }
}

impl fmt::Debug for AfterToolResultHookResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Continue { result } => formatter
                .debug_struct("Continue")
                .field("outcome", &result.outcome)
                .field("content_count", &result.content.len())
                .finish(),
        }
    }
}

/// `hooks.cancel` request params。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookCancelParams {
    pub request_id: String,
}

impl HookCancelParams {
    /// 验证待取消 hook invocation 的 request identity。
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        validate_request_id(&self.request_id)
    }
}

impl fmt::Debug for HookCancelParams {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HookCancelParams")
            .field("has_request_id", &!self.request_id.is_empty())
            .finish()
    }
}

/// `hooks.cancel` success result。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookCancelResult {
    pub accepted: bool,
}

/// `shutdown` request params。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShutdownParams {}

/// `shutdown` success result。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShutdownResult {
    pub drained: bool,
}

/// `tools.execute` success result。
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolExecuteResult {
    pub content: Vec<ToolContent>,
    pub is_error: bool,
}

impl fmt::Debug for ToolExecuteResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ToolExecuteResult")
            .field("content_count", &self.content.len())
            .field(
                "content_chars",
                &self
                    .content
                    .iter()
                    .map(ToolContent::char_count)
                    .sum::<usize>(),
            )
            .field("is_error", &self.is_error)
            .finish()
    }
}

/// Tool delivery body；`Debug` 只投影 kind 与长度。
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value")]
pub enum ToolContent {
    #[serde(rename = "text")]
    Text(String),
    #[serde(rename = "json")]
    Json(Value),
}

impl ToolContent {
    fn char_count(&self) -> usize {
        match self {
            Self::Text(text) => text.chars().count(),
            Self::Json(value) => value.to_string().chars().count(),
        }
    }
}

impl fmt::Debug for ToolContent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self {
            Self::Text(_) => "text",
            Self::Json(value) => json_kind(value),
        };
        formatter
            .debug_struct("ToolContent")
            .field("kind", &kind)
            .field("char_count", &self.char_count())
            .finish()
    }
}

/// Extension 主动发送的 progress/cancellation notification。
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum ExtensionNotification {
    #[serde(rename = "progress")]
    Progress {
        request_id: String,
        completed: u64,
        total: Option<u64>,
    },
    #[serde(rename = "cancelled")]
    Cancelled { request_id: String },
}

impl fmt::Debug for ExtensionNotification {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Progress {
                request_id,
                completed,
                total,
            } => formatter
                .debug_struct("Progress")
                .field("has_request_id", &!request_id.is_empty())
                .field("completed", completed)
                .field("has_total", &total.is_some())
                .finish(),
            Self::Cancelled { request_id } => formatter
                .debug_struct("Cancelled")
                .field("has_request_id", &!request_id.is_empty())
                .finish(),
        }
    }
}

/// 可由 blocking 与 async process adapter 复用的同步 Content-Length framing。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameCodec {
    max_frame_bytes: usize,
}

impl Default for FrameCodec {
    fn default() -> Self {
        Self {
            max_frame_bytes: DEFAULT_MAX_FRAME_BYTES,
        }
    }
}

impl FrameCodec {
    /// 使用非零 max frame body 大小创建 codec。
    pub fn new(max_frame_bytes: usize) -> Result<Self, FrameError> {
        if max_frame_bytes == 0 {
            return Err(FrameError::InvalidMaxFrame);
        }
        Ok(Self { max_frame_bytes })
    }

    /// 返回允许的最大 frame body 大小。
    pub const fn max_frame_bytes(self) -> usize {
        self.max_frame_bytes
    }

    /// 读取一个完整 frame body，并保留 reader 中的后续 bytes。
    pub fn read_frame<R: std::io::Read>(&self, reader: &mut R) -> Result<Vec<u8>, FrameError> {
        let mut content_length = None;
        let mut header_bytes: usize = 0;
        loop {
            let line = read_header_line(reader)?;
            header_bytes = header_bytes
                .checked_add(line.len() + 2)
                .ok_or(FrameError::HeadersTooLarge)?;
            if header_bytes > MAX_HEADER_BYTES {
                return Err(FrameError::HeadersTooLarge);
            }
            if line.is_empty() {
                break;
            }
            let (name, value) = line.split_once(':').ok_or(FrameError::InvalidHeader)?;
            if name.eq_ignore_ascii_case("content-length") {
                if content_length.is_some() {
                    return Err(FrameError::DuplicateContentLength);
                }
                let value = value.trim_matches([' ', '\t']);
                if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err(FrameError::InvalidContentLength);
                }
                let length = value
                    .parse::<usize>()
                    .map_err(|_| FrameError::InvalidContentLength)?;
                if length > self.max_frame_bytes {
                    return Err(FrameError::FrameTooLarge {
                        length,
                        max: self.max_frame_bytes,
                    });
                }
                content_length = Some(length);
            }
        }
        let length = content_length.ok_or(FrameError::MissingContentLength)?;
        let mut body = vec![0; length];
        read_exact_with_count(reader, &mut body)?;
        Ok(body)
    }

    /// 读取并解码一个 UTF-8 JSON frame。
    pub fn read_json<R: std::io::Read, T: DeserializeOwned>(
        &self,
        reader: &mut R,
    ) -> Result<T, FrameError> {
        let body = self.read_frame(reader)?;
        let text = std::str::from_utf8(&body).map_err(|_| FrameError::InvalidUtf8)?;
        serde_json::from_str(text).map_err(|_| FrameError::InvalidJson)
    }

    /// 编码 JSON 并写入一个完整 frame。
    pub fn write_json<W: std::io::Write, T: Serialize>(
        &self,
        writer: &mut W,
        value: &T,
    ) -> Result<(), FrameError> {
        let body = serde_json::to_vec(value).map_err(|_| FrameError::Encode)?;
        self.write_frame(writer, &body)
    }

    /// 写入 header/body，随后 flush 且不追加 newline。
    pub fn write_frame<W: std::io::Write>(
        &self,
        writer: &mut W,
        body: &[u8],
    ) -> Result<(), FrameError> {
        if body.len() > self.max_frame_bytes {
            return Err(FrameError::FrameTooLarge {
                length: body.len(),
                max: self.max_frame_bytes,
            });
        }
        write!(writer, "Content-Length: {}\r\n\r\n", body.len()).map_err(|_| FrameError::Io)?;
        writer.write_all(body).map_err(|_| FrameError::Io)?;
        writer.flush().map_err(|_| FrameError::Io)
    }
}

/// Content-Length framing 或 JSON codec 的安全错误投影。
#[derive(Debug, Error)]
pub enum FrameError {
    #[error("frame maximum must be greater than zero")]
    InvalidMaxFrame,
    #[error("frame header is truncated")]
    TruncatedHeader,
    #[error("frame header is invalid")]
    InvalidHeader,
    #[error("frame headers exceed the maximum size")]
    HeadersTooLarge,
    #[error("frame has duplicate Content-Length headers")]
    DuplicateContentLength,
    #[error("frame has no Content-Length header")]
    MissingContentLength,
    #[error("Content-Length is invalid")]
    InvalidContentLength,
    #[error("frame body is truncated")]
    TruncatedBody,
    #[error("frame length {length} exceeds maximum {max}")]
    FrameTooLarge { length: usize, max: usize },
    #[error("frame is not valid UTF-8")]
    InvalidUtf8,
    #[error("frame is not valid JSON")]
    InvalidJson,
    #[error("protocol value could not be encoded")]
    Encode,
    #[error("frame I/O failed")]
    Io,
}

fn validate_protocol(protocol: &str, version: u16) -> Result<(), ProtocolValidationError> {
    if protocol != PROTOCOL_NAME {
        return Err(ProtocolValidationError::UnsupportedProtocol);
    }
    if version != PROTOCOL_VERSION {
        return Err(ProtocolValidationError::UnsupportedVersion);
    }
    Ok(())
}

fn validate_request_id(request_id: &str) -> Result<(), ProtocolValidationError> {
    if request_id.is_empty()
        || request_id.len() > MAX_REQUEST_ID_BYTES
        || !request_id.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return Err(ProtocolValidationError::InvalidRequestId);
    }
    Ok(())
}

fn validate_capabilities(
    capabilities: &[ExtensionCapability],
) -> Result<(), ProtocolValidationError> {
    for (index, capability) in capabilities.iter().enumerate() {
        if capabilities[..index].contains(capability) {
            return Err(ProtocolValidationError::DuplicateCapability);
        }
    }
    Ok(())
}

fn validate_hook_id(hook_id: &str) -> Result<(), ProtocolValidationError> {
    const MAX_HOOK_ID_BYTES: usize = 64;

    let bytes = hook_id.as_bytes();
    if hook_id.is_empty()
        || hook_id.len() > MAX_HOOK_ID_BYTES
        || !bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        || !bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        || bytes
            .iter()
            .any(|byte| !byte.is_ascii_lowercase() && !byte.is_ascii_digit() && *byte != b'-')
        || bytes.windows(2).any(|pair| pair == b"--")
    {
        return Err(ProtocolValidationError::InvalidHookId);
    }
    Ok(())
}

fn validate_conversation_items(items: &[ConversationItem]) -> Result<(), ProtocolValidationError> {
    if items.is_empty() || items.iter().any(|item| item.validate().is_err()) {
        return Err(ProtocolValidationError::InvalidConversationItems);
    }
    Ok(())
}

fn validate_tool_name(name: &str) -> Result<(), ProtocolValidationError> {
    if name.trim().is_empty()
        || name.trim() != name
        || !name.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return Err(ProtocolValidationError::InvalidHookToolCall);
    }
    Ok(())
}

fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn read_header_line<R: std::io::Read>(reader: &mut R) -> Result<String, FrameError> {
    let mut bytes = Vec::new();
    loop {
        let mut byte = [0; 1];
        let count = reader.read(&mut byte).map_err(|_| FrameError::Io)?;
        if count == 0 {
            return Err(FrameError::TruncatedHeader);
        }
        match byte[0] {
            b'\r' => {
                let mut line_end = [0; 1];
                let count = reader.read(&mut line_end).map_err(|_| FrameError::Io)?;
                if count == 0 {
                    return Err(FrameError::TruncatedHeader);
                }
                if line_end[0] != b'\n' {
                    return Err(FrameError::InvalidHeader);
                }
                return String::from_utf8(bytes).map_err(|_| FrameError::InvalidHeader);
            }
            b'\n' => return Err(FrameError::InvalidHeader),
            byte if byte == b'\t' || byte == b' ' || byte.is_ascii_graphic() => bytes.push(byte),
            _ => return Err(FrameError::InvalidHeader),
        }
        if bytes.len() > MAX_HEADER_LINE_BYTES {
            return Err(FrameError::InvalidHeader);
        }
    }
}

fn read_exact_with_count<R: std::io::Read>(
    reader: &mut R,
    body: &mut [u8],
) -> Result<(), FrameError> {
    let mut offset = 0;
    while offset < body.len() {
        let count = reader
            .read(&mut body[offset..])
            .map_err(|_| FrameError::Io)?;
        if count == 0 {
            return Err(FrameError::TruncatedBody);
        }
        offset += count;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::{self, Cursor, Write};

    use super::*;

    #[test]
    fn request_round_trip_and_validation_preserve_method_without_exposing_params() {
        let request = ExtensionRequest::new(
            "req-1",
            ExtensionMethod::ToolsExecute,
            ToolExecuteParams {
                name: "read_file".to_string(),
                arguments: serde_json::json!({"path": "/workspace/secret.txt"}),
            },
        )
        .expect("request should encode")
        .with_deadline_ms(30_000)
        .with_capabilities([ExtensionCapability::Cancel]);
        request.validate().expect("request should validate");
        let json = serde_json::to_string(&request).expect("request should serialize");
        let decoded: ExtensionRequest = serde_json::from_str(&json).expect("request should decode");
        let params: ToolExecuteParams = decoded.decode_params().expect("params should decode");
        assert_eq!(decoded.method(), ExtensionMethod::ToolsExecute);
        assert_eq!(params.name, "read_file");
        let debug = format!("{decoded:?}");
        assert!(!debug.contains("/workspace/secret.txt"));
    }

    #[test]
    fn response_error_and_notification_round_trip() {
        let response = ExtensionResponse::failure(
            "req-2",
            ExtensionError::new(ExtensionErrorCode::ToolRejected, "permission denied", false)
                .with_details(&serde_json::json!({"path": "/workspace/secret.txt"}))
                .expect("error details should encode"),
        );
        response
            .validate()
            .expect("failure response should validate");
        assert_eq!(
            response.error().expect("error should exist").code(),
            ExtensionErrorCode::ToolRejected
        );
        assert!(!format!("{response:?}").contains("permission denied"));
        assert!(!format!("{response:?}").contains("secret.txt"));
        assert_eq!(
            response.error().and_then(ExtensionError::details),
            Some(&serde_json::json!({"path": "/workspace/secret.txt"}))
        );
        let error_debug = format!("{:?}", response.error().expect("error should exist"));
        assert!(!error_debug.contains("permission denied"));
        assert!(!error_debug.contains("secret.txt"));

        let encoded = serde_json::to_string(&response).expect("response should encode");
        let decoded: ExtensionResponse =
            serde_json::from_str(&encoded).expect("response should decode");
        decoded.validate().expect("response should validate");
        assert_eq!(
            decoded.error().expect("error should exist").code(),
            ExtensionErrorCode::ToolRejected
        );

        let notification = ExtensionNotification::Progress {
            request_id: "req-2".to_string(),
            completed: 2,
            total: Some(4),
        };
        let decoded: ExtensionNotification = serde_json::from_str(
            &serde_json::to_string(&notification).expect("notification should encode"),
        )
        .expect("notification should decode");
        assert!(matches!(
            decoded,
            ExtensionNotification::Progress { completed: 2, .. }
        ));

        let cancelled = ExtensionNotification::Cancelled {
            request_id: "req-2".to_string(),
        };
        let decoded: ExtensionNotification = serde_json::from_str(
            &serde_json::to_string(&cancelled).expect("notification should encode"),
        )
        .expect("notification should decode");
        assert!(matches!(
            decoded,
            ExtensionNotification::Cancelled { request_id } if request_id == "req-2"
        ));
    }

    #[test]
    fn content_length_codec_reads_exact_frame_and_flushes_writes() {
        let codec = FrameCodec::new(128).expect("codec should construct");
        let mut output = FlushTrackingWriter::default();
        codec
            .write_json(&mut output, &serde_json::json!({"method": "tools.list"}))
            .expect("frame should write");
        assert!(output.flushed);
        assert_eq!(
            output.bytes,
            b"Content-Length: 23\r\n\r\n{\"method\":\"tools.list\"}".to_vec()
        );

        let mut reader = Cursor::new(output.bytes);
        let decoded: Value = codec.read_json(&mut reader).expect("frame should read");
        assert_eq!(decoded["method"], "tools.list");

        let mut two_frames =
            Cursor::new(b"Content-Length: 2\r\n\r\n{}Content-Length: 2\r\n\r\n[]".to_vec());
        assert_eq!(
            codec.read_frame(&mut two_frames).expect("first frame"),
            b"{}".to_vec()
        );
        assert_eq!(
            codec.read_frame(&mut two_frames).expect("second frame"),
            b"[]".to_vec()
        );
    }

    #[test]
    fn content_length_codec_rejects_malformed_and_oversized_frames() {
        let codec = FrameCodec::new(4).expect("codec should construct");
        let mut missing = Cursor::new(b"\r\n{}".to_vec());
        assert!(matches!(
            codec.read_frame(&mut missing),
            Err(FrameError::MissingContentLength)
        ));

        let mut duplicate =
            Cursor::new(b"Content-Length: 2\r\nContent-Length: 2\r\n\r\n{}".to_vec());
        assert!(matches!(
            codec.read_frame(&mut duplicate),
            Err(FrameError::DuplicateContentLength)
        ));

        let mut too_large = Cursor::new(b"Content-Length: 5\r\n\r\n12345".to_vec());
        assert!(matches!(
            codec.read_frame(&mut too_large),
            Err(FrameError::FrameTooLarge { .. })
        ));

        let mut truncated = Cursor::new(b"Content-Length: 3\r\n\r\n{}".to_vec());
        assert!(matches!(
            codec.read_frame(&mut truncated),
            Err(FrameError::TruncatedBody)
        ));

        let mut bare_lf = Cursor::new(b"Content-Length: 2\n\n{}".to_vec());
        assert!(matches!(
            codec.read_frame(&mut bare_lf),
            Err(FrameError::InvalidHeader)
        ));

        let mut mixed_ending = Cursor::new(b"Content-Length: 2\rX\n{}".to_vec());
        assert!(matches!(
            codec.read_frame(&mut mixed_ending),
            Err(FrameError::InvalidHeader)
        ));

        let mut signed_length = Cursor::new(b"Content-Length: +2\r\n\r\n{}".to_vec());
        assert!(matches!(
            codec.read_frame(&mut signed_length),
            Err(FrameError::InvalidContentLength)
        ));

        let mut ascii_ows = Cursor::new(b"Content-Length:\t2 \t\r\n\r\n{}".to_vec());
        assert_eq!(
            codec
                .read_frame(&mut ascii_ows)
                .expect("ASCII OWS should be accepted"),
            b"{}".to_vec()
        );

        let mut non_ascii_length =
            Cursor::new("Content-Length: \u{a0}2\r\n\r\n{}".as_bytes().to_vec());
        assert!(matches!(
            codec.read_frame(&mut non_ascii_length),
            Err(FrameError::InvalidHeader)
        ));

        let oversized_line = format!("X-Header: {}\r\n\r\n", "x".repeat(MAX_HEADER_LINE_BYTES));
        let mut oversized_line = Cursor::new(oversized_line.into_bytes());
        assert!(matches!(
            FrameCodec::default().read_frame(&mut oversized_line),
            Err(FrameError::InvalidHeader)
        ));

        let mut headers = String::new();
        for _ in 0..5 {
            headers.push_str("X-Header: ");
            headers.push_str(&"x".repeat(8_000));
            headers.push_str("\r\n");
        }
        headers.push_str("\r\n");
        let mut headers = Cursor::new(headers.into_bytes());
        assert!(matches!(
            FrameCodec::default().read_frame(&mut headers),
            Err(FrameError::HeadersTooLarge)
        ));

        let mut encoded_value = Vec::new();
        assert!(matches!(
            FrameCodec::default().write_json(&mut encoded_value, &Unencodable),
            Err(FrameError::Encode)
        ));
    }

    #[test]
    fn request_validation_rejects_invalid_identity_and_duplicate_capability() {
        let request = ExtensionRequest::new(
            "",
            ExtensionMethod::Initialize,
            InitializeParams {
                protocol_version: PROTOCOL_VERSION,
                capabilities: Vec::new(),
            },
        )
        .expect("request should encode");
        assert_eq!(
            request.validate(),
            Err(ProtocolValidationError::InvalidRequestId)
        );

        let request = ExtensionRequest::new(
            "req-deadline",
            ExtensionMethod::ToolsList,
            ToolsListParams::default(),
        )
        .expect("request should encode")
        .with_deadline_ms(0);
        assert_eq!(
            request.validate(),
            Err(ProtocolValidationError::InvalidDeadline)
        );

        let request = ExtensionRequest::new(
            "req-7",
            ExtensionMethod::Initialize,
            InitializeParams {
                protocol_version: PROTOCOL_VERSION + 1,
                capabilities: Vec::new(),
            },
        )
        .expect("request should encode");
        assert_eq!(
            request.validate(),
            Err(ProtocolValidationError::UnsupportedVersion)
        );

        let request = ExtensionRequest::new(
            "req-8",
            ExtensionMethod::Initialize,
            InitializeParams {
                protocol_version: PROTOCOL_VERSION,
                capabilities: vec![ExtensionCapability::Cancel, ExtensionCapability::Cancel],
            },
        )
        .expect("request should encode");
        assert_eq!(
            request.validate(),
            Err(ProtocolValidationError::DuplicateCapability)
        );

        let request =
            ExtensionRequest::new("req-9", ExtensionMethod::ToolsList, serde_json::json!([]))
                .expect("request should encode");
        assert_eq!(
            request.validate(),
            Err(ProtocolValidationError::InvalidParams)
        );

        let request = ExtensionRequest::new(
            "req-3",
            ExtensionMethod::Initialize,
            InitializeParams {
                protocol_version: PROTOCOL_VERSION,
                capabilities: Vec::new(),
            },
        )
        .expect("request should encode")
        .with_capabilities([ExtensionCapability::Progress, ExtensionCapability::Progress]);
        assert_eq!(
            request.validate(),
            Err(ProtocolValidationError::DuplicateCapability)
        );

        let request = ExtensionRequest::new(
            "req-5",
            ExtensionMethod::ToolsExecute,
            ToolExecuteParams {
                name: "  ".to_string(),
                arguments: Value::Null,
            },
        )
        .expect("request should encode");
        assert_eq!(
            request.validate(),
            Err(ProtocolValidationError::InvalidParams)
        );

        let request = ExtensionRequest::new(
            "req-6",
            ExtensionMethod::ToolsCancel,
            ToolCancelParams {
                request_id: "".to_string(),
            },
        )
        .expect("request should encode");
        assert_eq!(
            request.validate(),
            Err(ProtocolValidationError::InvalidRequestId)
        );
    }

    #[test]
    fn request_validation_rejects_wire_protocol_drift() {
        let request = ExtensionRequest::new(
            "req-4",
            ExtensionMethod::ToolsList,
            ToolsListParams::default(),
        )
        .expect("request should encode");
        let mut value = serde_json::to_value(&request).expect("request should serialize");
        value["version"] = serde_json::json!(PROTOCOL_VERSION + 1);
        let decoded: ExtensionRequest =
            serde_json::from_value(value).expect("request should decode");
        assert_eq!(
            decoded.validate(),
            Err(ProtocolValidationError::UnsupportedVersion)
        );

        let mut value = serde_json::to_value(&request).expect("request should serialize");
        value["protocol"] = serde_json::json!("other-extension");
        let decoded: ExtensionRequest =
            serde_json::from_value(value).expect("request should decode");
        assert_eq!(
            decoded.validate(),
            Err(ProtocolValidationError::UnsupportedProtocol)
        );
    }

    #[test]
    fn protocol_v2_methods_and_tool_dtos_have_stable_wire_names() {
        let methods = [
            (ExtensionMethod::Initialize, "initialize"),
            (ExtensionMethod::ToolsList, "tools.list"),
            (ExtensionMethod::ToolsExecute, "tools.execute"),
            (ExtensionMethod::ToolsCancel, "tools.cancel"),
            (ExtensionMethod::HooksList, "hooks.list"),
            (ExtensionMethod::HooksBeforeTurn, "hooks.before_turn"),
            (
                ExtensionMethod::HooksBeforeToolExecute,
                "hooks.before_tool_execute",
            ),
            (
                ExtensionMethod::HooksAfterToolResult,
                "hooks.after_tool_result",
            ),
            (ExtensionMethod::HooksCancel, "hooks.cancel"),
            (ExtensionMethod::Shutdown, "shutdown"),
        ];
        for (method, wire_name) in methods {
            let json = serde_json::to_string(&method).expect("method should serialize");
            assert_eq!(json, format!("\"{wire_name}\""));
            let decoded: ExtensionMethod =
                serde_json::from_str(&json).expect("method should deserialize");
            assert_eq!(decoded, method);
        }

        let result = ToolExecuteResult {
            content: vec![
                ToolContent::Text("ok".to_string()),
                ToolContent::Json(serde_json::json!({"status": "done"})),
            ],
            is_error: false,
        };
        let json = serde_json::to_string(&result).expect("tool result should serialize");
        let decoded: ToolExecuteResult =
            serde_json::from_str(&json).expect("tool result should deserialize");
        assert_eq!(decoded, result);

        let tools = ToolsListResult {
            tools: vec![ToolDescriptor {
                name: "read_file".to_string(),
                description: Some("Read a file".to_string()),
                input_schema: Some(serde_json::json!({"type": "object"})),
            }],
        };
        assert_eq!(round_trip(&tools), tools);
        let list_params = ToolsListParams::default();
        assert_eq!(round_trip(&list_params), list_params);

        let cancel_params = ToolCancelParams {
            request_id: "req-11".to_string(),
        };
        assert_eq!(round_trip(&cancel_params), cancel_params);
        let cancel_result = ToolCancelResult { accepted: true };
        assert_eq!(round_trip(&cancel_result), cancel_result);

        let shutdown_params = ShutdownParams::default();
        assert_eq!(round_trip(&shutdown_params), shutdown_params);
        let shutdown_result = ShutdownResult { drained: true };
        assert_eq!(round_trip(&shutdown_result), shutdown_result);
    }

    #[test]
    fn typed_hook_dtos_round_trip_and_validate() {
        let descriptors = HooksListResult {
            hooks: vec![
                HookDescriptor {
                    hook_id: "turn-policy".to_string(),
                    phase: HookPhase::BeforeTurn,
                    priority: -10,
                },
                HookDescriptor {
                    hook_id: "tool-policy".to_string(),
                    phase: HookPhase::BeforeToolExecute,
                    priority: 0,
                },
                HookDescriptor {
                    hook_id: "tool-policy".to_string(),
                    phase: HookPhase::AfterToolResult,
                    priority: 10,
                },
            ],
        };
        descriptors.validate().expect("descriptors should validate");
        assert_eq!(round_trip(&HooksListParams::default()), HooksListParams {});
        assert_eq!(round_trip(&descriptors), descriptors);

        let items = vec![ConversationItem::text(
            provider_protocol::Role::User,
            "private instruction",
        )];
        let before_turn_params = BeforeTurnHookParams {
            hook_id: "turn-policy".to_string(),
            items: items.clone(),
        };
        before_turn_params
            .validate()
            .expect("params should validate");
        assert_eq!(round_trip(&before_turn_params), before_turn_params);
        let before_turn_result = BeforeTurnHookResult::Continue {
            items: items.clone(),
        };
        before_turn_result
            .validate()
            .expect("result should validate");
        assert_eq!(round_trip(&before_turn_result), before_turn_result);
        assert_eq!(
            round_trip(&BeforeTurnHookResult::Reject {
                code: HookRejectionCode::PolicyDenied,
            }),
            BeforeTurnHookResult::Reject {
                code: HookRejectionCode::PolicyDenied,
            }
        );

        let call = HookToolCall {
            call_id: "call-typed-1".to_string(),
            name: "read_file".to_string(),
            arguments: serde_json::json!({"path": "/private/path"}),
        };
        let before_tool_params = BeforeToolExecuteHookParams {
            hook_id: "tool-policy".to_string(),
            call,
        };
        before_tool_params
            .validate()
            .expect("params should validate");
        assert_eq!(round_trip(&before_tool_params), before_tool_params);
        assert_eq!(
            round_trip(&BeforeToolExecuteHookResult::Continue),
            BeforeToolExecuteHookResult::Continue
        );
        assert_eq!(
            round_trip(&BeforeToolExecuteHookResult::Reject {
                code: HookRejectionCode::UnsupportedOperation,
            }),
            BeforeToolExecuteHookResult::Reject {
                code: HookRejectionCode::UnsupportedOperation,
            }
        );

        let result = sample_hook_tool_result();
        result.validate().expect("tool result should validate");
        let after_result_params = AfterToolResultHookParams {
            hook_id: "tool-policy".to_string(),
            tool_name: "read_file".to_string(),
            result: result.clone(),
        };
        after_result_params
            .validate()
            .expect("params should validate");
        assert_eq!(round_trip(&after_result_params), after_result_params);
        let after_result = AfterToolResultHookResult::Continue { result };
        after_result.validate().expect("result should validate");
        assert_eq!(round_trip(&after_result), after_result);

        let cancel_params = HookCancelParams {
            request_id: "hook-request-1".to_string(),
        };
        cancel_params.validate().expect("cancel should validate");
        assert_eq!(round_trip(&cancel_params), cancel_params);
        assert_eq!(
            round_trip(&HookCancelResult { accepted: true }),
            HookCancelResult { accepted: true }
        );
    }

    #[test]
    fn hook_request_validation_is_method_specific() {
        let valid_requests = [
            ExtensionRequest::new(
                "req-hook-list",
                ExtensionMethod::HooksList,
                HooksListParams::default(),
            )
            .unwrap(),
            ExtensionRequest::new(
                "req-before-turn",
                ExtensionMethod::HooksBeforeTurn,
                BeforeTurnHookParams {
                    hook_id: "turn-policy".to_string(),
                    items: vec![ConversationItem::text(
                        provider_protocol::Role::User,
                        "secret",
                    )],
                },
            )
            .unwrap(),
            ExtensionRequest::new(
                "req-before-tool",
                ExtensionMethod::HooksBeforeToolExecute,
                BeforeToolExecuteHookParams {
                    hook_id: "tool-policy".to_string(),
                    call: HookToolCall {
                        call_id: "call-1".to_string(),
                        name: "read_file".to_string(),
                        arguments: serde_json::json!({"secret": true}),
                    },
                },
            )
            .unwrap(),
            ExtensionRequest::new(
                "req-after-result",
                ExtensionMethod::HooksAfterToolResult,
                AfterToolResultHookParams {
                    hook_id: "tool-policy".to_string(),
                    tool_name: "read_file".to_string(),
                    result: sample_hook_tool_result(),
                },
            )
            .unwrap(),
            ExtensionRequest::new(
                "req-hook-cancel",
                ExtensionMethod::HooksCancel,
                HookCancelParams {
                    request_id: "req-after-result".to_string(),
                },
            )
            .unwrap(),
        ];
        for request in valid_requests {
            request.validate().expect("hook request should validate");
        }

        let wrong_params = ExtensionRequest::new(
            "req-wrong-shape",
            ExtensionMethod::HooksBeforeTurn,
            HooksListParams::default(),
        )
        .unwrap();
        assert_eq!(
            wrong_params.validate(),
            Err(ProtocolValidationError::InvalidParams)
        );

        let empty_items = BeforeTurnHookParams {
            hook_id: "turn-policy".to_string(),
            items: Vec::new(),
        };
        assert_eq!(
            empty_items.validate(),
            Err(ProtocolValidationError::InvalidConversationItems)
        );

        let invalid_item = ConversationItem::user(vec![provider_protocol::ContentBlock::ToolCall(
            provider_protocol::ToolCall::new("call-private", "read_file", "{}"),
        )]);
        assert_eq!(
            BeforeTurnHookResult::Continue {
                items: vec![invalid_item]
            }
            .validate(),
            Err(ProtocolValidationError::InvalidConversationItems)
        );

        let duplicate_descriptors = HooksListResult {
            hooks: vec![
                HookDescriptor {
                    hook_id: "same-hook".to_string(),
                    phase: HookPhase::BeforeTurn,
                    priority: 0,
                },
                HookDescriptor {
                    hook_id: "same-hook".to_string(),
                    phase: HookPhase::BeforeTurn,
                    priority: 1,
                },
            ],
        };
        assert_eq!(
            duplicate_descriptors.validate(),
            Err(ProtocolValidationError::DuplicateHookDescriptor)
        );
        assert_eq!(
            HookDescriptor {
                hook_id: "Unsafe--Hook".to_string(),
                phase: HookPhase::BeforeTurn,
                priority: 0,
            }
            .validate(),
            Err(ProtocolValidationError::InvalidHookId)
        );

        let invalid_call = HookToolCall {
            call_id: "call 1".to_string(),
            name: " read_file ".to_string(),
            arguments: Value::Null,
        };
        assert_eq!(
            invalid_call.validate(),
            Err(ProtocolValidationError::InvalidHookToolCall)
        );
        let invalid_result = HookToolResult {
            call_id: "call-1".to_string(),
            content: vec![HookToolResultContent::Image {
                data_base64: String::new(),
                mime_type: String::new(),
                uri: None,
                detail: None,
            }],
            outcome: HookToolResultOutcome::Success,
            display_content: None,
            details: None,
        };
        assert_eq!(
            invalid_result.validate(),
            Err(ProtocolValidationError::InvalidHookToolResult)
        );
    }

    #[test]
    fn hook_diagnostics_omit_all_delivery_and_remote_bodies() {
        let params = BeforeTurnHookParams {
            hook_id: "private-hook-id".to_string(),
            items: vec![ConversationItem::text(
                provider_protocol::Role::User,
                "private instruction body",
            )],
        };
        let call = BeforeToolExecuteHookParams {
            hook_id: "private-call-hook".to_string(),
            call: HookToolCall {
                call_id: "private-call-id".to_string(),
                name: "private-tool-name".to_string(),
                arguments: serde_json::json!({
                    "credential": "secret-token",
                    "path": "/private/path",
                    "endpoint": "https://private.invalid"
                }),
            },
        };
        let result = AfterToolResultHookParams {
            hook_id: "private-result-hook".to_string(),
            tool_name: "private-tool-name".to_string(),
            result: sample_hook_tool_result(),
        };
        let values = [
            format!("{params:?}"),
            format!("{call:?}"),
            format!("{result:?}"),
            format!("{:?}", call.call),
            format!("{:?}", result.result),
            format!("{:?}", result.result.content[0]),
            format!(
                "{:?}",
                BeforeTurnHookResult::Continue {
                    items: params.items.clone()
                }
            ),
            format!(
                "{:?}",
                AfterToolResultHookResult::Continue {
                    result: result.result.clone()
                }
            ),
        ];
        for diagnostic in values {
            for sentinel in [
                "private instruction body",
                "private-hook-id",
                "private-call-hook",
                "private-call-id",
                "private-result-hook",
                "private-tool-name",
                "secret-token",
                "/private/path",
                "https://private.invalid",
                "private-image-body",
                "private-display-body",
                "private-details-body",
                "private-uri",
            ] {
                assert!(
                    !diagnostic.contains(sentinel),
                    "diagnostic leaked sentinel: {sentinel}"
                );
            }
        }
    }

    fn sample_hook_tool_result() -> HookToolResult {
        HookToolResult {
            call_id: "call-typed-1".to_string(),
            content: vec![
                HookToolResultContent::Text {
                    text: "private tool body".to_string(),
                },
                HookToolResultContent::Image {
                    data_base64: "private-image-body".to_string(),
                    mime_type: "image/png".to_string(),
                    uri: Some("private-uri".to_string()),
                    detail: Some(HookToolImageDetail::Original),
                },
            ],
            outcome: HookToolResultOutcome::Success,
            display_content: Some("private-display-body".to_string()),
            details: Some(serde_json::json!({"private": "private-details-body"})),
        }
    }

    fn round_trip<T>(value: &T) -> T
    where
        T: Serialize + DeserializeOwned,
    {
        let json = serde_json::to_string(value).expect("value should serialize");
        serde_json::from_str(&json).expect("value should deserialize")
    }

    #[derive(Default)]
    struct FlushTrackingWriter {
        bytes: Vec<u8>,
        flushed: bool,
    }

    struct Unencodable;

    impl Serialize for Unencodable {
        fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            Err(serde::ser::Error::custom("sensitive serializer detail"))
        }
    }

    impl Write for FlushTrackingWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            self.flushed = true;
            Ok(())
        }
    }

    #[test]
    fn redacted_debug_for_tool_result_omits_delivery_body() {
        let result = ToolExecuteResult {
            content: vec![ToolContent::Text("user-visible tool output".to_string())],
            is_error: false,
        };
        let debug = format!("{result:?}");
        assert!(debug.contains("content_chars"));
        assert!(!debug.contains("user-visible tool output"));

        let content = ToolContent::Text("direct user-visible output".to_string());
        assert!(!format!("{content:?}").contains("direct user-visible output"));

        let params = ToolExecuteParams {
            name: "read_file".to_string(),
            arguments: serde_json::json!({"path": "/workspace/secret.txt"}),
        };
        assert!(!format!("{params:?}").contains("secret.txt"));

        let descriptor = ToolDescriptor {
            name: "read_file".to_string(),
            description: Some("private description".to_string()),
            input_schema: Some(serde_json::json!({"secret": "schema detail"})),
        };
        let debug = format!("{descriptor:?}");
        assert!(!debug.contains("private description"));
        assert!(!debug.contains("schema detail"));

        let response = ExtensionResponse::success(
            "req-redacted",
            ToolExecuteResult {
                content: vec![ToolContent::Text("private result".to_string())],
                is_error: false,
            },
        )
        .expect("response should encode");
        assert!(!format!("{response:?}").contains("private result"));
    }

    #[test]
    fn protocol_diagnostics_omit_remote_control_values() {
        let request = ExtensionRequest::new(
            "private-request-id",
            ExtensionMethod::ToolsExecute,
            ToolExecuteParams {
                name: "private-tool-name".to_string(),
                arguments: serde_json::json!({"credential": "private-credential"}),
            },
        )
        .expect("request should encode");
        let response =
            ExtensionResponse::success("private-response-id", ShutdownResult { drained: true })
                .expect("response should encode");
        let descriptor = ToolDescriptor {
            name: "/private/tool/path".to_string(),
            description: Some("private-description".to_string()),
            input_schema: Some(serde_json::json!({"endpoint": "private-endpoint"})),
        };
        let execute = ToolExecuteParams {
            name: "private-execute-name".to_string(),
            arguments: serde_json::json!({"credential": "private-execute-credential"}),
        };
        let cancel = ToolCancelParams {
            request_id: "private-cancel-id".to_string(),
        };
        let progress = ExtensionNotification::Progress {
            request_id: "private-progress-id".to_string(),
            completed: 1,
            total: Some(2),
        };
        let cancelled = ExtensionNotification::Cancelled {
            request_id: "private-notification-id".to_string(),
        };
        let mut untrusted_request = serde_json::to_value(&request).expect("request should encode");
        untrusted_request["protocol"] = serde_json::json!("private-request-protocol");
        let untrusted_request: ExtensionRequest =
            serde_json::from_value(untrusted_request).expect("request should decode");
        let mut untrusted_response =
            serde_json::to_value(&response).expect("response should encode");
        untrusted_response["protocol"] = serde_json::json!("private-response-protocol");
        let untrusted_response: ExtensionResponse =
            serde_json::from_value(untrusted_response).expect("response should decode");
        let initialize_result = InitializeResult {
            protocol: "private-initialize-protocol".to_string(),
            version: PROTOCOL_VERSION,
            capabilities: Vec::new(),
        };

        let diagnostics = [
            format!("{untrusted_request:?}"),
            format!("{untrusted_response:?}"),
            format!("{initialize_result:?}"),
            format!("{descriptor:?}"),
            format!("{execute:?}"),
            format!("{cancel:?}"),
            format!("{progress:?}"),
            format!("{cancelled:?}"),
        ];
        for diagnostic in diagnostics {
            for sentinel in [
                "private-request-id",
                "private-response-id",
                "private-request-protocol",
                "private-response-protocol",
                "private-initialize-protocol",
                "/private/tool/path",
                "private-description",
                "private-endpoint",
                "private-execute-name",
                "private-execute-credential",
                "private-cancel-id",
                "private-progress-id",
                "private-notification-id",
            ] {
                assert!(
                    !diagnostic.contains(sentinel),
                    "diagnostic leaked sentinel: {sentinel}"
                );
            }
        }
    }

    #[test]
    fn redacted_debug_for_initialization_omits_capability_values() {
        let params = InitializeParams {
            protocol_version: PROTOCOL_VERSION,
            capabilities: vec![ExtensionCapability::StructuredErrors],
        };
        let result = InitializeResult {
            protocol: PROTOCOL_NAME.to_string(),
            version: PROTOCOL_VERSION,
            capabilities: vec![ExtensionCapability::StructuredErrors],
        };
        result.validate().expect("result should validate");
        assert_eq!(round_trip(&params), params);
        assert_eq!(round_trip(&result), result);
        assert!(format!("{params:?}").contains("capability_count"));
        assert!(!format!("{params:?}").contains("structured_errors"));
        assert!(format!("{result:?}").contains("capability_count"));
        assert!(!format!("{result:?}").contains("structured_errors"));
        assert_eq!(
            format!("{:?}", ExtensionCapability::StructuredErrors),
            "ExtensionCapability"
        );
    }

    #[test]
    fn success_response_round_trips_and_rejects_invalid_shapes() {
        let response = ExtensionResponse::success("req-12", ShutdownResult { drained: true })
            .expect("response should encode");
        let decoded: ExtensionResponse = round_trip(&response);
        decoded.validate().expect("response should validate");
        assert_eq!(
            decoded
                .result::<ShutdownResult>()
                .expect("result should decode"),
            Some(ShutdownResult { drained: true })
        );

        let mut invalid_shape = serde_json::to_value(&response).expect("response should encode");
        invalid_shape["error"] = serde_json::json!({
            "code": "internal",
            "message": "safe",
            "retryable": false,
            "details": null
        });
        let invalid_shape: ExtensionResponse =
            serde_json::from_value(invalid_shape).expect("response should decode");
        assert_eq!(
            invalid_shape.validate(),
            Err(ProtocolValidationError::InvalidResponseShape)
        );

        let mut invalid_result = serde_json::to_value(&response).expect("response should encode");
        invalid_result["result"] = Value::String("wrong shape".to_string());
        let invalid_result: ExtensionResponse =
            serde_json::from_value(invalid_result).expect("response should decode");
        assert_eq!(
            invalid_result.result::<ShutdownResult>(),
            Err(ProtocolValidationError::InvalidResult)
        );
    }

    #[test]
    fn protocol_errors_drop_upstream_error_text() {
        let encode_error = ExtensionRequest::new("req-10", ExtensionMethod::ToolsList, Unencodable)
            .expect_err("serialization should fail");
        assert!(std::error::Error::source(&encode_error).is_none());
        assert!(!format!("{encode_error:?}").contains("sensitive serializer detail"));

        let mut reader = FailingReader;
        let io_error = FrameCodec::default()
            .read_frame(&mut reader)
            .expect_err("read should fail");
        assert!(std::error::Error::source(&io_error).is_none());
        assert!(!format!("{io_error:?}").contains("/workspace/secret.txt"));
    }

    #[test]
    fn frame_codec_rejects_invalid_utf8_and_json() {
        let codec = FrameCodec::new(32).expect("codec should construct");
        let mut invalid_utf8 = Cursor::new(b"Content-Length: 1\r\n\r\n\xff".to_vec());
        assert!(matches!(
            codec.read_json::<_, Value>(&mut invalid_utf8),
            Err(FrameError::InvalidUtf8)
        ));

        let mut invalid_json = Cursor::new(b"Content-Length: 1\r\n\r\n{".to_vec());
        assert!(matches!(
            codec.read_json::<_, Value>(&mut invalid_json),
            Err(FrameError::InvalidJson)
        ));
    }

    struct FailingReader;

    impl io::Read for FailingReader {
        fn read(&mut self, _bytes: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("/workspace/secret.txt"))
        }
    }
}
