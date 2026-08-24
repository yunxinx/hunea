//! 版本化的进程外扩展协议值与 Content-Length framing。
//!
//! 本 crate 只负责 wire boundary，不拥有 process、permission policy、cancellation task 或
//! runtime registry；这些职责属于 host adapter。

use std::fmt;

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use thiserror::Error;

/// Wire envelope 使用的固定 protocol name。
pub const PROTOCOL_NAME: &str = "hunea-extension";
/// 当前 wire protocol version。
pub const PROTOCOL_VERSION: u16 = 1;
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
}

impl fmt::Debug for ExtensionCapability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ExtensionCapability")
    }
}

/// 第一批协议 method，限定在初始化、tool 操作与关闭流程。
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
            .field("protocol", &self.protocol)
            .field("version", &self.version)
            .field("request_id", &self.request_id)
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
            .field("protocol", &self.protocol)
            .field("version", &self.version)
            .field("request_id", &self.request_id)
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
            .field("protocol", &self.protocol)
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
            .field("name", &self.name)
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
            .field("name", &self.name)
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCancelParams {
    pub request_id: String,
}

impl ToolCancelParams {
    /// 验证待取消 request 的 identity。
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        validate_request_id(&self.request_id)
    }
}

/// `tools.cancel` success result。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCancelResult {
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
                .field("request_id", request_id)
                .field("completed", completed)
                .field("has_total", &total.is_some())
                .finish(),
            Self::Cancelled { request_id } => formatter
                .debug_struct("Cancelled")
                .field("request_id", request_id)
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
    fn first_batch_methods_and_tool_dtos_have_stable_wire_names() {
        let methods = [
            (ExtensionMethod::Initialize, "initialize"),
            (ExtensionMethod::ToolsList, "tools.list"),
            (ExtensionMethod::ToolsExecute, "tools.execute"),
            (ExtensionMethod::ToolsCancel, "tools.cancel"),
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
