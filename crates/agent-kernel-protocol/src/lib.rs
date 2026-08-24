//! 独立的进程外 Agent kernel wire contract。
//!
//! 本 crate 只拥有 versioned envelope、typed command/receipt/event DTO 与语义校验；process、
//! request scheduling、runtime lifecycle、host capability 和 TUI projection 属于上层 adapter。

use std::fmt;

use provider_protocol::{ContentBlock, ConversationItem};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use thiserror::Error;

/// Agent kernel wire identity。
pub const PROTOCOL_NAME: &str = "hunea-agent-kernel";
/// 当前 Agent kernel protocol version。
pub const PROTOCOL_VERSION: u16 = 1;
const MAX_ID_BYTES: usize = 128;
const MAX_LABEL_BYTES: usize = 512;

/// initialize 阶段协商的 kernel 行为。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentKernelCapability {
    Events,
    Interrupt,
    PermissionResponse,
    StructuredErrors,
}

impl fmt::Debug for AgentKernelCapability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AgentKernelCapability")
    }
}

/// host 可显式授予 kernel 的 authority vocabulary。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentKernelHostCapability {
    Llm,
    Tools,
    Prompt,
    Session,
    Permission,
}

impl fmt::Debug for AgentKernelHostCapability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AgentKernelHostCapability")
    }
}

/// Host -> kernel request method。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AgentKernelMethod {
    #[serde(rename = "initialize")]
    Initialize,
    #[serde(rename = "agent.command")]
    AgentCommand,
    #[serde(rename = "shutdown")]
    Shutdown,
}

impl AgentKernelMethod {
    /// 返回稳定 wire method name。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Initialize => "initialize",
            Self::AgentCommand => "agent.command",
            Self::Shutdown => "shutdown",
        }
    }
}

/// Host -> kernel request envelope。
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentKernelRequest {
    protocol: String,
    version: u16,
    request_id: String,
    method: AgentKernelMethod,
    params: Value,
    deadline_ms: Option<u64>,
}

impl AgentKernelRequest {
    /// 使用当前 protocol identity 编码 typed request。
    pub fn new(
        request_id: impl Into<String>,
        method: AgentKernelMethod,
        params: impl Serialize,
    ) -> Result<Self, AgentKernelEncodeError> {
        Ok(Self {
            protocol: PROTOCOL_NAME.to_string(),
            version: PROTOCOL_VERSION,
            request_id: request_id.into(),
            method,
            params: serde_json::to_value(params).map_err(|_| AgentKernelEncodeError::Json)?,
            deadline_ms: None,
        })
    }

    /// 设置 host request deadline；零值会被 validation 拒绝。
    pub const fn with_deadline_ms(mut self, deadline_ms: u64) -> Self {
        self.deadline_ms = Some(deadline_ms);
        self
    }

    /// 返回 envelope protocol identity。
    pub fn protocol(&self) -> &str {
        &self.protocol
    }

    /// 返回 envelope protocol version。
    pub const fn version(&self) -> u16 {
        self.version
    }

    /// 返回用于 response correlation 的 request identity。
    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    /// 返回 typed request method。
    pub const fn method(&self) -> AgentKernelMethod {
        self.method
    }

    /// 返回 host deadline，单位为毫秒。
    pub const fn deadline_ms(&self) -> Option<u64> {
        self.deadline_ms
    }

    /// 解码 method owner 指定的 typed params。
    pub fn decode_params<T: DeserializeOwned>(&self) -> Result<T, AgentKernelValidationError> {
        serde_json::from_value(self.params.clone())
            .map_err(|_| AgentKernelValidationError::InvalidParams)
    }

    /// 校验 envelope 与 method-specific params。
    pub fn validate(&self) -> Result<(), AgentKernelValidationError> {
        validate_protocol(&self.protocol, self.version)?;
        validate_id(
            &self.request_id,
            AgentKernelValidationError::InvalidRequestId,
        )?;
        if self.deadline_ms == Some(0) || !self.params.is_object() {
            return Err(AgentKernelValidationError::InvalidParams);
        }
        match self.method {
            AgentKernelMethod::Initialize => self
                .decode_params::<AgentKernelInitializeParams>()?
                .validate(),
            AgentKernelMethod::AgentCommand => {
                self.decode_params::<AgentKernelCommandParams>()?.validate()
            }
            AgentKernelMethod::Shutdown => {
                self.decode_params::<AgentKernelShutdownParams>()?;
                Ok(())
            }
        }
    }
}

impl fmt::Debug for AgentKernelRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentKernelRequest")
            .field("has_protocol", &!self.protocol.is_empty())
            .field("version", &self.version)
            .field("has_request_id", &!self.request_id.is_empty())
            .field("method", &self.method)
            .field("deadline_ms", &self.deadline_ms)
            .field("has_params", &true)
            .finish()
    }
}

/// Kernel -> host response envelope；result 与 error 必须且只能出现一个。
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentKernelResponse {
    protocol: String,
    version: u16,
    request_id: String,
    result: Option<Value>,
    error: Option<AgentKernelError>,
}

impl AgentKernelResponse {
    /// 使用当前 protocol identity 编码 success result。
    pub fn success(
        request_id: impl Into<String>,
        result: impl Serialize,
    ) -> Result<Self, AgentKernelEncodeError> {
        Ok(Self {
            protocol: PROTOCOL_NAME.to_string(),
            version: PROTOCOL_VERSION,
            request_id: request_id.into(),
            result: Some(serde_json::to_value(result).map_err(|_| AgentKernelEncodeError::Json)?),
            error: None,
        })
    }

    /// 使用当前 protocol identity 构造 structured failure。
    pub fn failure(request_id: impl Into<String>, error: AgentKernelError) -> Self {
        Self {
            protocol: PROTOCOL_NAME.to_string(),
            version: PROTOCOL_VERSION,
            request_id: request_id.into(),
            result: None,
            error: Some(error),
        }
    }

    /// 返回与 request 相同的 correlation identity。
    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    /// 返回 remote structured error；success response 返回 `None`。
    pub fn error(&self) -> Option<&AgentKernelError> {
        self.error.as_ref()
    }

    /// 把 success body 解码为 method owner 指定的 typed result。
    pub fn result<T: DeserializeOwned>(&self) -> Result<Option<T>, AgentKernelValidationError> {
        self.result
            .clone()
            .map(|value| {
                serde_json::from_value(value).map_err(|_| AgentKernelValidationError::InvalidResult)
            })
            .transpose()
    }

    /// 校验 protocol identity、correlation identity 与 result/error shape。
    pub fn validate(&self) -> Result<(), AgentKernelValidationError> {
        validate_protocol(&self.protocol, self.version)?;
        validate_id(
            &self.request_id,
            AgentKernelValidationError::InvalidRequestId,
        )?;
        if self.result.is_some() == self.error.is_some() {
            return Err(AgentKernelValidationError::InvalidResponseShape);
        }
        Ok(())
    }
}

impl fmt::Debug for AgentKernelResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentKernelResponse")
            .field("version", &self.version)
            .field("has_request_id", &!self.request_id.is_empty())
            .field("has_result", &self.result.is_some())
            .field("error", &self.error)
            .finish()
    }
}

/// Kernel -> host frame payload。
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "message", rename_all = "snake_case")]
pub enum AgentKernelMessage {
    Response { response: AgentKernelResponse },
    Event { event: AgentKernelEventNotification },
}

impl AgentKernelMessage {
    /// 校验 response 或 unsolicited event envelope。
    pub fn validate(&self) -> Result<(), AgentKernelValidationError> {
        match self {
            Self::Response { response } => response.validate(),
            Self::Event { event } => event.validate(),
        }
    }
}

impl fmt::Debug for AgentKernelMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Response { .. } => formatter.write_str("AgentKernelMessage::Response"),
            Self::Event { .. } => formatter.write_str("AgentKernelMessage::Event"),
        }
    }
}

/// Remote structured error code；message/details 只属于 wire body。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentKernelErrorCode {
    InvalidRequest,
    UnsupportedVersion,
    CapabilityDenied,
    Busy,
    UnknownAgent,
    CommandRejected,
    Internal,
}

/// Remote structured error；正文只属于 wire delivery，不进入 `Debug`。
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentKernelError {
    code: AgentKernelErrorCode,
    message: String,
    retryable: bool,
    details: Option<Value>,
}

impl AgentKernelError {
    /// 创建不含 details 的 remote error DTO。
    pub fn new(code: AgentKernelErrorCode, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code,
            message: message.into(),
            retryable,
            details: None,
        }
    }

    /// 编码可选 structured details body。
    pub fn with_details(mut self, details: impl Serialize) -> Result<Self, AgentKernelEncodeError> {
        self.details =
            Some(serde_json::to_value(details).map_err(|_| AgentKernelEncodeError::Json)?);
        Ok(self)
    }

    /// 返回 host 可安全匹配的 closed error code。
    pub const fn code(&self) -> AgentKernelErrorCode {
        self.code
    }

    /// 返回 remote 声明的 retryability metadata。
    pub const fn retryable(&self) -> bool {
        self.retryable
    }
}

impl fmt::Debug for AgentKernelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentKernelError")
            .field("code", &self.code)
            .field("retryable", &self.retryable)
            .field("has_details", &self.details.is_some())
            .finish()
    }
}

/// Outbound typed DTO 无法编码为 JSON value。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum AgentKernelEncodeError {
    #[error("Agent kernel protocol value could not be encoded")]
    Json,
}

/// Inbound envelope 或 typed payload 的 closed validation error。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum AgentKernelValidationError {
    #[error("Agent kernel protocol identity is unsupported")]
    UnsupportedProtocol,
    #[error("Agent kernel protocol version is unsupported")]
    UnsupportedVersion,
    #[error("Agent kernel request identity is invalid")]
    InvalidRequestId,
    #[error("Agent kernel request parameters are invalid")]
    InvalidParams,
    #[error("Agent kernel response shape is invalid")]
    InvalidResponseShape,
    #[error("Agent kernel response result is invalid")]
    InvalidResult,
    #[error("Agent kernel capability list contains a duplicate")]
    DuplicateCapability,
    #[error("Agent kernel host capability list contains a duplicate")]
    DuplicateHostCapability,
    #[error("Agent kernel command identity is invalid")]
    InvalidCommandId,
    #[error("Agent kernel Agent identity is invalid")]
    InvalidAgentId,
    #[error("Agent kernel turn identity is invalid")]
    InvalidTurnId,
    #[error("Agent kernel target is invalid")]
    InvalidTarget,
    #[error("Agent kernel turn request is invalid")]
    InvalidTurnRequest,
    #[error("Agent kernel permission identity is invalid")]
    InvalidPermission,
    #[error("Agent kernel event sequence is invalid")]
    InvalidEventSequence,
    #[error("Agent kernel event payload is invalid")]
    InvalidEvent,
}

/// Host 初始化请求中的 protocol/capability proposal。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentKernelInitializeParams {
    pub protocol_version: u16,
    pub capabilities: Vec<AgentKernelCapability>,
    pub host_capabilities: Vec<AgentKernelHostCapability>,
}

impl AgentKernelInitializeParams {
    /// 校验 version 与 capability uniqueness。
    pub fn validate(&self) -> Result<(), AgentKernelValidationError> {
        if self.protocol_version != PROTOCOL_VERSION {
            return Err(AgentKernelValidationError::UnsupportedVersion);
        }
        validate_unique_capabilities(&self.capabilities)?;
        validate_unique_host_capabilities(&self.host_capabilities)
    }
}

impl fmt::Debug for AgentKernelInitializeParams {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentKernelInitializeParams")
            .field("protocol_version", &self.protocol_version)
            .field("capability_count", &self.capabilities.len())
            .field("host_capability_count", &self.host_capabilities.len())
            .finish()
    }
}

/// Kernel 初始化结果中的 negotiated capability set。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentKernelInitializeResult {
    pub protocol: String,
    pub version: u16,
    pub capabilities: Vec<AgentKernelCapability>,
    pub accepted_host_capabilities: Vec<AgentKernelHostCapability>,
}

impl AgentKernelInitializeResult {
    /// 校验 protocol identity 与 capability uniqueness。
    pub fn validate(&self) -> Result<(), AgentKernelValidationError> {
        validate_protocol(&self.protocol, self.version)?;
        validate_unique_capabilities(&self.capabilities)?;
        validate_unique_host_capabilities(&self.accepted_host_capabilities)
    }
}

impl fmt::Debug for AgentKernelInitializeResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentKernelInitializeResult")
            .field("has_protocol", &!self.protocol.is_empty())
            .field("version", &self.version)
            .field("capability_count", &self.capabilities.len())
            .field(
                "accepted_host_capability_count",
                &self.accepted_host_capabilities.len(),
            )
            .finish()
    }
}

/// Cooperative shutdown request；当前 version 无附加 authority。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentKernelShutdownParams {}

/// Cooperative shutdown acknowledgement。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentKernelShutdownResult {
    pub drained: bool,
}

/// Wire target；只承载 provider/model identity，不承载 endpoint 或 credential。
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AgentKernelTarget {
    pub provider_id: String,
    pub model_id: String,
}

impl AgentKernelTarget {
    /// 校验 provider/model label。
    pub fn validate(&self) -> Result<(), AgentKernelValidationError> {
        if !valid_label(&self.provider_id) || !valid_label(&self.model_id) {
            return Err(AgentKernelValidationError::InvalidTarget);
        }
        Ok(())
    }
}

impl fmt::Debug for AgentKernelTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentKernelTarget")
            .field("has_provider", &!self.provider_id.is_empty())
            .field("has_model", &!self.model_id.is_empty())
            .finish()
    }
}

/// Provider image detail vocabulary。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentKernelImageDetail {
    Auto,
    Low,
    High,
    Original,
}

/// Transcript-visible user attachment。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentKernelUserAttachment {
    Image {
        data_base64: String,
        mime_type: String,
        uri: Option<String>,
        detail: Option<AgentKernelImageDetail>,
    },
}

impl AgentKernelUserAttachment {
    fn validate(&self) -> Result<(), AgentKernelValidationError> {
        match self {
            Self::Image { mime_type, .. } if valid_label(mime_type) => Ok(()),
            Self::Image { .. } => Err(AgentKernelValidationError::InvalidTurnRequest),
        }
    }
}

impl fmt::Debug for AgentKernelUserAttachment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Image { uri, detail, .. } => formatter
                .debug_struct("Image")
                .field("has_uri", &uri.is_some())
                .field("detail", detail)
                .finish(),
        }
    }
}

/// Transcript-visible user delivery；不含 instruction reference/body。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentKernelUserDelivery {
    pub content: String,
    pub attachments: Vec<AgentKernelUserAttachment>,
}

impl AgentKernelUserDelivery {
    fn validate(&self) -> Result<(), AgentKernelValidationError> {
        self.attachments
            .iter()
            .try_for_each(AgentKernelUserAttachment::validate)
    }
}

impl fmt::Debug for AgentKernelUserDelivery {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentKernelUserDelivery")
            .field("content_chars", &self.content.chars().count())
            .field("attachment_count", &self.attachments.len())
            .finish()
    }
}

/// Instruction reference 的 stable origin metadata。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentKernelPromptOrigin {
    Builtin,
    Global,
    Project,
}

/// Skill control reference；不携带解析后的 instruction body。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentKernelSkillBinding {
    pub skill_name: String,
    pub origin: AgentKernelPromptOrigin,
    pub skill_path: String,
    pub start_char: u64,
    pub end_char: u64,
}

impl AgentKernelSkillBinding {
    fn validate(&self) -> Result<(), AgentKernelValidationError> {
        if !valid_label(&self.skill_name)
            || self.skill_path.trim().is_empty()
            || self.end_char < self.start_char
        {
            return Err(AgentKernelValidationError::InvalidTurnRequest);
        }
        Ok(())
    }
}

impl fmt::Debug for AgentKernelSkillBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentKernelSkillBinding")
            .field("origin", &self.origin)
            .field("has_name", &!self.skill_name.is_empty())
            .field("has_path", &!self.skill_path.is_empty())
            .field("span_chars", &self.end_char.saturating_sub(self.start_char))
            .finish()
    }
}

/// Custom prompt control reference；不携带解析后的 instruction body。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentKernelCustomPromptBinding {
    pub reference_id: String,
    pub origin: AgentKernelPromptOrigin,
    pub start_char: u64,
    pub end_char: u64,
}

impl AgentKernelCustomPromptBinding {
    fn validate(&self) -> Result<(), AgentKernelValidationError> {
        if !valid_label(&self.reference_id) || self.end_char < self.start_char {
            return Err(AgentKernelValidationError::InvalidTurnRequest);
        }
        Ok(())
    }
}

impl fmt::Debug for AgentKernelCustomPromptBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentKernelCustomPromptBinding")
            .field("origin", &self.origin)
            .field("has_reference", &!self.reference_id.is_empty())
            .field("span_chars", &self.end_char.saturating_sub(self.start_char))
            .finish()
    }
}

/// Structured controls 只承载 reference metadata，不承载解析后的 instruction body。
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentKernelTurnControls {
    pub skill_bindings: Vec<AgentKernelSkillBinding>,
    pub custom_prompt_bindings: Vec<AgentKernelCustomPromptBinding>,
}

impl AgentKernelTurnControls {
    fn validate(&self) -> Result<(), AgentKernelValidationError> {
        self.skill_bindings
            .iter()
            .try_for_each(AgentKernelSkillBinding::validate)?;
        self.custom_prompt_bindings
            .iter()
            .try_for_each(AgentKernelCustomPromptBinding::validate)
    }
}

impl fmt::Debug for AgentKernelTurnControls {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentKernelTurnControls")
            .field("skill_binding_count", &self.skill_bindings.len())
            .field(
                "custom_prompt_binding_count",
                &self.custom_prompt_bindings.len(),
            )
            .finish()
    }
}

/// SubmitTurn payload 明确分离 delivery、control 与 provider-visible content。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentKernelTurnRequest {
    pub target: AgentKernelTarget,
    pub delivery: AgentKernelUserDelivery,
    pub controls: AgentKernelTurnControls,
    pub provider_content: Vec<ContentBlock>,
}

impl AgentKernelTurnRequest {
    /// 校验 target、delivery、controls 与 provider content。
    pub fn validate(&self) -> Result<(), AgentKernelValidationError> {
        self.target.validate()?;
        self.delivery.validate()?;
        self.controls.validate()?;
        ConversationItem::user(self.provider_content.clone())
            .validate()
            .map_err(|_| AgentKernelValidationError::InvalidTurnRequest)
    }
}

impl fmt::Debug for AgentKernelTurnRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentKernelTurnRequest")
            .field("target", &self.target)
            .field("delivery", &self.delivery)
            .field("controls", &self.controls)
            .field("provider_content_count", &self.provider_content.len())
            .finish()
    }
}

/// Framework-neutral Agent command 的 wire projection。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentKernelCommand {
    SubmitTurn {
        agent_id: u64,
        turn_id: u64,
        request: Box<AgentKernelTurnRequest>,
    },
    Interrupt {
        agent_id: u64,
        target: Option<AgentKernelTarget>,
    },
    RespondPermission {
        agent_id: u64,
        target: Option<AgentKernelTarget>,
        request_id: String,
        option_id: Option<String>,
    },
}

impl AgentKernelCommand {
    /// 校验 command identity 与 variant-specific payload。
    pub fn validate(&self) -> Result<(), AgentKernelValidationError> {
        match self {
            Self::SubmitTurn {
                agent_id,
                turn_id,
                request,
            } => {
                validate_numeric_id(*agent_id, AgentKernelValidationError::InvalidAgentId)?;
                validate_numeric_id(*turn_id, AgentKernelValidationError::InvalidTurnId)?;
                request.validate()
            }
            Self::Interrupt { agent_id, target } => {
                validate_numeric_id(*agent_id, AgentKernelValidationError::InvalidAgentId)?;
                if let Some(target) = target {
                    target.validate()?;
                }
                Ok(())
            }
            Self::RespondPermission {
                agent_id,
                target,
                request_id,
                option_id,
            } => {
                validate_numeric_id(*agent_id, AgentKernelValidationError::InvalidAgentId)?;
                if let Some(target) = target {
                    target.validate()?;
                }
                validate_id(request_id, AgentKernelValidationError::InvalidPermission)?;
                if let Some(option_id) = option_id {
                    validate_id(option_id, AgentKernelValidationError::InvalidPermission)?;
                }
                Ok(())
            }
        }
    }
}

impl fmt::Debug for AgentKernelCommand {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SubmitTurn { request, .. } => formatter
                .debug_struct("SubmitTurn")
                .field("request", request)
                .finish(),
            Self::Interrupt { target, .. } => formatter
                .debug_struct("Interrupt")
                .field("has_target", &target.is_some())
                .finish(),
            Self::RespondPermission {
                target, option_id, ..
            } => formatter
                .debug_struct("RespondPermission")
                .field("has_target", &target.is_some())
                .field("has_option", &option_id.is_some())
                .finish(),
        }
    }
}

/// Correlated Agent command request body。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentKernelCommandParams {
    pub command_id: u64,
    pub command: AgentKernelCommand,
}

impl AgentKernelCommandParams {
    /// 校验 command correlation identity 与 typed command。
    pub fn validate(&self) -> Result<(), AgentKernelValidationError> {
        validate_numeric_id(
            self.command_id,
            AgentKernelValidationError::InvalidCommandId,
        )?;
        self.command.validate()
    }
}

impl fmt::Debug for AgentKernelCommandParams {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentKernelCommandParams")
            .field("has_command_id", &(self.command_id != 0))
            .field("command", &self.command)
            .finish()
    }
}

/// Kernel 对同步 command admission 的 typed receipt。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentKernelCommandReceipt {
    Accepted,
    TurnStarted {
        turn_id: u64,
        target: AgentKernelTarget,
        activity_label: String,
    },
    Interrupted {
        target: Option<AgentKernelTarget>,
    },
}

impl AgentKernelCommandReceipt {
    /// 校验 receipt variant 的 identity 与 target payload。
    pub fn validate(&self) -> Result<(), AgentKernelValidationError> {
        match self {
            Self::Accepted => Ok(()),
            Self::TurnStarted {
                turn_id,
                target,
                activity_label,
            } => {
                validate_numeric_id(*turn_id, AgentKernelValidationError::InvalidTurnId)?;
                target.validate()?;
                if !valid_label(activity_label) {
                    return Err(AgentKernelValidationError::InvalidResult);
                }
                Ok(())
            }
            Self::Interrupted { target } => {
                if let Some(target) = target {
                    target.validate()?;
                }
                Ok(())
            }
        }
    }
}

impl fmt::Debug for AgentKernelCommandReceipt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Accepted => formatter.write_str("Accepted"),
            Self::TurnStarted { target, .. } => formatter
                .debug_struct("TurnStarted")
                .field("target", target)
                .finish(),
            Self::Interrupted { target } => formatter
                .debug_struct("Interrupted")
                .field("has_target", &target.is_some())
                .finish(),
        }
    }
}

/// 将 command identity 与 typed receipt 关联的 response body。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentKernelCommandResult {
    pub command_id: u64,
    pub receipt: AgentKernelCommandReceipt,
}

impl AgentKernelCommandResult {
    /// 校验 command correlation identity 与 receipt。
    pub fn validate(&self) -> Result<(), AgentKernelValidationError> {
        validate_numeric_id(
            self.command_id,
            AgentKernelValidationError::InvalidCommandId,
        )?;
        self.receipt.validate()
    }
}

impl fmt::Debug for AgentKernelCommandResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentKernelCommandResult")
            .field("has_command_id", &(self.command_id != 0))
            .field("receipt", &self.receipt)
            .finish()
    }
}

/// Tool activity 的 provider-neutral category。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentKernelToolKind {
    Read,
    Write,
    Edit,
    Delete,
    Move,
    Search,
    Execute,
    Think,
    Fetch,
    SwitchMode,
    Other,
}

/// Tool activity lifecycle status。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentKernelToolStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

/// Tool activity 关联的 source location。
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AgentKernelToolLocation {
    pub path: String,
    pub line: Option<u32>,
}

impl fmt::Debug for AgentKernelToolLocation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentKernelToolLocation")
            .field("has_path", &!self.path.is_empty())
            .field("line", &self.line)
            .finish()
    }
}

/// Tool activity 的 typed display/delivery content。
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "payload", rename_all = "snake_case")]
pub enum AgentKernelToolContent {
    Text(String),
    Image {
        mime_type: String,
        uri: Option<String>,
    },
    Audio {
        mime_type: String,
    },
    ResourceLink {
        uri: String,
        name: String,
        title: Option<String>,
    },
    Resource {
        uri: String,
        mime_type: Option<String>,
        text: Option<String>,
    },
    Diff {
        path: String,
        old_text: Option<String>,
        new_text: String,
        is_truncated: bool,
    },
    Terminal {
        terminal_id: String,
    },
    Unknown(String),
}

impl fmt::Debug for AgentKernelToolContent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self {
            Self::Text(_) => "text",
            Self::Image { .. } => "image",
            Self::Audio { .. } => "audio",
            Self::ResourceLink { .. } => "resource_link",
            Self::Resource { .. } => "resource",
            Self::Diff { .. } => "diff",
            Self::Terminal { .. } => "terminal",
            Self::Unknown(_) => "unknown",
        };
        formatter
            .debug_struct("AgentKernelToolContent")
            .field("kind", &kind)
            .finish()
    }
}

/// Tool activity 首次发布的完整 snapshot。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentKernelToolActivity {
    pub activity_id: String,
    pub title: String,
    pub kind: AgentKernelToolKind,
    pub status: AgentKernelToolStatus,
    pub content: Vec<AgentKernelToolContent>,
    pub locations: Vec<AgentKernelToolLocation>,
    pub raw_input: Option<Value>,
    pub raw_output: Option<Value>,
}

impl AgentKernelToolActivity {
    fn validate(&self) -> Result<(), AgentKernelValidationError> {
        validate_id(&self.activity_id, AgentKernelValidationError::InvalidEvent)
    }
}

impl fmt::Debug for AgentKernelToolActivity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentKernelToolActivity")
            .field("kind", &self.kind)
            .field("status", &self.status)
            .field("content_count", &self.content.len())
            .field("location_count", &self.locations.len())
            .field("has_raw_input", &self.raw_input.is_some())
            .field("has_raw_output", &self.raw_output.is_some())
            .finish()
    }
}

/// Tool activity 的 partial update。
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentKernelToolActivityUpdate {
    pub activity_id: String,
    pub title: Option<String>,
    pub kind: Option<AgentKernelToolKind>,
    pub status: Option<AgentKernelToolStatus>,
    pub content: Option<Vec<AgentKernelToolContent>>,
    pub locations: Option<Vec<AgentKernelToolLocation>>,
    pub raw_input: Option<Value>,
    pub raw_output: Option<Value>,
}

impl AgentKernelToolActivityUpdate {
    fn validate(&self) -> Result<(), AgentKernelValidationError> {
        validate_id(&self.activity_id, AgentKernelValidationError::InvalidEvent)
    }
}

impl fmt::Debug for AgentKernelToolActivityUpdate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentKernelToolActivityUpdate")
            .field("has_title", &self.title.is_some())
            .field("kind", &self.kind)
            .field("status", &self.status)
            .field("content_count", &self.content.as_ref().map(Vec::len))
            .field("location_count", &self.locations.as_ref().map(Vec::len))
            .field("has_raw_input", &self.raw_input.is_some())
            .field("has_raw_output", &self.raw_output.is_some())
            .finish()
    }
}

/// Terminal process 的 typed exit status。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentKernelTerminalExitStatus {
    pub exit_code: Option<u32>,
    pub signal: Option<String>,
}

impl fmt::Debug for AgentKernelTerminalExitStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentKernelTerminalExitStatus")
            .field("exit_code", &self.exit_code)
            .field("has_signal", &self.signal.is_some())
            .finish()
    }
}

/// Terminal content/update snapshot；正文不会进入 `Debug`。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentKernelTerminalSnapshot {
    pub terminal_id: String,
    pub command: Option<String>,
    pub cwd: Option<String>,
    pub output: String,
    pub truncated: bool,
    pub exit_status: Option<AgentKernelTerminalExitStatus>,
    pub released: bool,
}

impl AgentKernelTerminalSnapshot {
    fn validate(&self) -> Result<(), AgentKernelValidationError> {
        validate_id(&self.terminal_id, AgentKernelValidationError::InvalidEvent)
    }
}

impl fmt::Debug for AgentKernelTerminalSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentKernelTerminalSnapshot")
            .field("has_command", &self.command.is_some())
            .field("has_cwd", &self.cwd.is_some())
            .field("output_chars", &self.output.chars().count())
            .field("truncated", &self.truncated)
            .field("has_exit_status", &self.exit_status.is_some())
            .field("released", &self.released)
            .finish()
    }
}

/// Permission option 的 closed decision category。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentKernelPermissionOptionKind {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
    Unknown,
}

/// Permission prompt 中的一个 typed choice。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentKernelPermissionOption {
    pub option_id: String,
    pub name: String,
    pub kind: AgentKernelPermissionOptionKind,
}

impl fmt::Debug for AgentKernelPermissionOption {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentKernelPermissionOption")
            .field("kind", &self.kind)
            .finish()
    }
}

/// Turn-scoped permission request fact。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentKernelPermissionRequest {
    pub request_id: String,
    pub title: Option<String>,
    pub tool_activity: Option<AgentKernelToolActivityUpdate>,
    pub options: Vec<AgentKernelPermissionOption>,
}

impl AgentKernelPermissionRequest {
    fn validate(&self) -> Result<(), AgentKernelValidationError> {
        validate_id(
            &self.request_id,
            AgentKernelValidationError::InvalidPermission,
        )?;
        if self.options.is_empty() {
            return Err(AgentKernelValidationError::InvalidPermission);
        }
        for (index, option) in self.options.iter().enumerate() {
            validate_id(
                &option.option_id,
                AgentKernelValidationError::InvalidPermission,
            )?;
            if self.options[..index]
                .iter()
                .any(|prior| prior.option_id == option.option_id)
            {
                return Err(AgentKernelValidationError::InvalidPermission);
            }
        }
        if let Some(activity) = &self.tool_activity {
            activity.validate()?;
        }
        Ok(())
    }
}

impl fmt::Debug for AgentKernelPermissionRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentKernelPermissionRequest")
            .field("has_title", &self.title.is_some())
            .field("has_tool_activity", &self.tool_activity.is_some())
            .field("option_count", &self.options.len())
            .finish()
    }
}

/// Remote request timing/token metrics。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentKernelRequestMetrics {
    pub latency_ms: u64,
    pub output_tokens: u64,
    pub duration_ms: u64,
}

/// Remote context-window usage snapshot。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentKernelContextUsage {
    pub limit: u64,
    pub used: u64,
}

/// Kernel event fact；所有 free-form body 只用于 delivery，不进入 `Debug`。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentKernelEventKind {
    SystemMessage {
        message: String,
    },
    Retrying {
        message: String,
    },
    OutputTokenEstimate {
        total_tokens: u64,
    },
    InputTokenEstimate {
        total_tokens: u64,
    },
    Thinking {
        is_thinking: bool,
    },
    AssistantDelta {
        content: String,
    },
    ReasoningDelta {
        content: String,
    },
    ToolActivityStarted {
        activity: AgentKernelToolActivity,
    },
    ToolActivityUpdated {
        update: AgentKernelToolActivityUpdate,
    },
    TerminalUpdated {
        snapshot: AgentKernelTerminalSnapshot,
    },
    PermissionRequested {
        request: AgentKernelPermissionRequest,
    },
    PreparationWarning {
        message: String,
    },
    TurnFinished {
        items: Vec<ConversationItem>,
        reasoning_duration_ms: Option<u64>,
        metrics: Option<AgentKernelRequestMetrics>,
        context_usage: Option<AgentKernelContextUsage>,
    },
    TurnFailed {
        message: String,
    },
    TurnInterrupted,
}

impl AgentKernelEventKind {
    /// 返回该 fact 是否结束当前 turn。
    pub const fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::TurnFinished { .. } | Self::TurnFailed { .. } | Self::TurnInterrupted
        )
    }

    fn validate(&self) -> Result<(), AgentKernelValidationError> {
        match self {
            Self::ToolActivityStarted { activity } => activity.validate(),
            Self::ToolActivityUpdated { update } => update.validate(),
            Self::TerminalUpdated { snapshot } => snapshot.validate(),
            Self::PermissionRequested { request } => request.validate(),
            Self::TurnFinished {
                items,
                context_usage,
                ..
            } => {
                if items.iter().any(|item| item.validate().is_err())
                    || context_usage.is_some_and(|usage| usage.limit == 0)
                {
                    return Err(AgentKernelValidationError::InvalidEvent);
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

impl fmt::Debug for AgentKernelEventKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::SystemMessage { .. } => "SystemMessage",
            Self::Retrying { .. } => "Retrying",
            Self::OutputTokenEstimate { .. } => "OutputTokenEstimate",
            Self::InputTokenEstimate { .. } => "InputTokenEstimate",
            Self::Thinking { .. } => "Thinking",
            Self::AssistantDelta { .. } => "AssistantDelta",
            Self::ReasoningDelta { .. } => "ReasoningDelta",
            Self::ToolActivityStarted { .. } => "ToolActivityStarted",
            Self::ToolActivityUpdated { .. } => "ToolActivityUpdated",
            Self::TerminalUpdated { .. } => "TerminalUpdated",
            Self::PermissionRequested { .. } => "PermissionRequested",
            Self::PreparationWarning { .. } => "PreparationWarning",
            Self::TurnFinished { .. } => "TurnFinished",
            Self::TurnFailed { .. } => "TurnFailed",
            Self::TurnInterrupted => "TurnInterrupted",
        };
        formatter.write_str(name)
    }
}

/// 带完整 Agent/turn/target identity 的 event fact。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentKernelEvent {
    pub agent_id: u64,
    pub turn_id: u64,
    pub target: AgentKernelTarget,
    pub kind: AgentKernelEventKind,
}

impl AgentKernelEvent {
    /// 校验 event identity、target 与 typed fact payload。
    pub fn validate(&self) -> Result<(), AgentKernelValidationError> {
        validate_numeric_id(self.agent_id, AgentKernelValidationError::InvalidAgentId)?;
        validate_numeric_id(self.turn_id, AgentKernelValidationError::InvalidTurnId)?;
        self.target.validate()?;
        self.kind.validate()
    }
}

impl fmt::Debug for AgentKernelEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentKernelEvent")
            .field("has_agent_id", &(self.agent_id != 0))
            .field("has_turn_id", &(self.turn_id != 0))
            .field("target", &self.target)
            .field("kind", &self.kind)
            .finish()
    }
}

/// 带 connection-local sequence 与 cause command correlation 的 event envelope。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentKernelEventNotification {
    protocol: String,
    version: u16,
    pub sequence: u64,
    pub command_id: u64,
    pub event: AgentKernelEvent,
}

impl AgentKernelEventNotification {
    /// 使用当前 protocol identity 构造 unsolicited event envelope。
    pub fn new(sequence: u64, command_id: u64, event: AgentKernelEvent) -> Self {
        Self {
            protocol: PROTOCOL_NAME.to_string(),
            version: PROTOCOL_VERSION,
            sequence,
            command_id,
            event,
        }
    }

    /// 校验 protocol、sequence、command 与 event payload。
    pub fn validate(&self) -> Result<(), AgentKernelValidationError> {
        validate_protocol(&self.protocol, self.version)?;
        validate_numeric_id(
            self.sequence,
            AgentKernelValidationError::InvalidEventSequence,
        )?;
        validate_numeric_id(
            self.command_id,
            AgentKernelValidationError::InvalidCommandId,
        )?;
        self.event.validate()
    }
}

impl fmt::Debug for AgentKernelEventNotification {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentKernelEventNotification")
            .field("version", &self.version)
            .field("has_sequence", &(self.sequence != 0))
            .field("has_command_id", &(self.command_id != 0))
            .field("event", &self.event)
            .finish()
    }
}

fn validate_protocol(protocol: &str, version: u16) -> Result<(), AgentKernelValidationError> {
    if protocol != PROTOCOL_NAME {
        return Err(AgentKernelValidationError::UnsupportedProtocol);
    }
    if version != PROTOCOL_VERSION {
        return Err(AgentKernelValidationError::UnsupportedVersion);
    }
    Ok(())
}

fn validate_id(
    value: &str,
    error: AgentKernelValidationError,
) -> Result<(), AgentKernelValidationError> {
    if value.is_empty()
        || value.len() > MAX_ID_BYTES
        || !value.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return Err(error);
    }
    Ok(())
}

fn validate_numeric_id(
    value: u64,
    error: AgentKernelValidationError,
) -> Result<(), AgentKernelValidationError> {
    if value == 0 {
        return Err(error);
    }
    Ok(())
}

fn valid_label(value: &str) -> bool {
    !value.trim().is_empty() && value.trim() == value && value.len() <= MAX_LABEL_BYTES
}

fn validate_unique_capabilities(
    capabilities: &[AgentKernelCapability],
) -> Result<(), AgentKernelValidationError> {
    for (index, capability) in capabilities.iter().enumerate() {
        if capabilities[..index].contains(capability) {
            return Err(AgentKernelValidationError::DuplicateCapability);
        }
    }
    Ok(())
}

fn validate_unique_host_capabilities(
    capabilities: &[AgentKernelHostCapability],
) -> Result<(), AgentKernelValidationError> {
    for (index, capability) in capabilities.iter().enumerate() {
        if capabilities[..index].contains(capability) {
            return Err(AgentKernelValidationError::DuplicateHostCapability);
        }
    }
    Ok(())
}
