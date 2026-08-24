//! 版本化 extension tool protocol 的 transport-neutral host adapter。
//!
//! 本 crate 拥有 protocol negotiation、host tool metadata、取消与生命周期；具体 process/stdio
//! transport 由后续 integration layer 提供。

use std::{
    collections::BTreeSet,
    fmt,
    future::Future,
    num::NonZeroU64,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use extension_protocol::{
    ExtensionCapability, ExtensionErrorCode, ExtensionMethod, ExtensionRequest, ExtensionResponse,
    InitializeParams, InitializeResult, ToolCancelParams, ToolCancelResult, ToolContent,
    ToolDescriptor, ToolExecuteParams, ToolExecuteResult, ToolsListParams, ToolsListResult,
};
use serde_json::Value;
use thiserror::Error;
use tokio::{time::sleep, time::timeout};
use tokio_util::sync::CancellationToken;
use tool_runtime::{
    Tool, ToolCall, ToolDefinition, ToolExecutionContext, ToolExecutionFuture, ToolKind,
    ToolPermissionPolicy, ToolResult, ToolResultContent,
};

/// 一个 request 在 transport 中等待的 future。
pub type ExtensionRequestFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ExtensionResponse, ExtensionTransportError>> + Send + 'a>>;

/// extension-runtime 消费的最小 transport contract。
///
/// transport 只能接收 typed protocol request；process、stdio、socket 与底层错误由具体实现
/// 在本 crate 之外拥有。`shutdown` 必须幂等。
pub trait ExtensionRequestTransport: Send + Sync {
    /// 发送一个 request 并等待对应 response。
    fn request(&self, request: ExtensionRequest) -> ExtensionRequestFuture<'_>;

    /// 停止 transport；重复调用必须安全。
    fn shutdown(&self) -> Result<(), ExtensionTransportError>;
}

/// transport 错误的 closed projection；不携带 I/O、process、path 或 endpoint source。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ExtensionTransportError {
    #[error("extension transport is unavailable")]
    Unavailable,
    #[error("extension transport is shut down")]
    ShutDown,
    #[error("extension transport rejected the protocol exchange")]
    Protocol,
}

/// host 为 extension tool 注入的超时与权限策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtensionToolOptions {
    request_deadline_ms: NonZeroU64,
    cancel_grace_ms: NonZeroU64,
    permission_policy: ToolPermissionPolicy,
}

impl ExtensionToolOptions {
    /// 创建 extension tool options。
    pub const fn new(
        request_deadline_ms: NonZeroU64,
        cancel_grace_ms: NonZeroU64,
        permission_policy: ToolPermissionPolicy,
    ) -> Self {
        Self {
            request_deadline_ms,
            cancel_grace_ms,
            permission_policy,
        }
    }

    /// 返回 execute request 的 host deadline。
    pub const fn request_deadline_ms(self) -> NonZeroU64 {
        self.request_deadline_ms
    }

    /// 返回 cancel acknowledgement 的最大等待时间。
    pub const fn cancel_grace_ms(self) -> NonZeroU64 {
        self.cancel_grace_ms
    }

    /// 返回 host 注入的 permission policy。
    pub const fn permission_policy(self) -> ToolPermissionPolicy {
        self.permission_policy
    }
}

impl Default for ExtensionToolOptions {
    fn default() -> Self {
        Self::new(
            NonZeroU64::new(30_000).expect("literal deadline is non-zero"),
            NonZeroU64::new(1_000).expect("literal grace is non-zero"),
            ToolPermissionPolicy::Never,
        )
    }
}

/// discovery 失败时的安全错误投影。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ExtensionDiscoveryError {
    #[error("extension transport request failed")]
    Transport(ExtensionTransportError),
    #[error("extension protocol response was invalid")]
    Protocol,
    #[error("extension response did not correlate with its request")]
    Correlation,
    #[error("extension response did not contain the expected result")]
    MissingResult,
    #[error("extension does not grant the required cancellation capability")]
    MissingCancelCapability,
    #[error("extension tool descriptor is invalid")]
    InvalidDescriptor,
    #[error("extension tool names contain a duplicate")]
    DuplicateTool,
    #[error("extension returned a remote error")]
    Remote(ExtensionErrorCode),
}

/// mount 失败时的安全错误投影。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ExtensionMountError {
    #[error("extension tool registration conflicted with an existing tool")]
    DuplicateTool,
}

/// host-owned extension client；同一 client 可被多个 tool execution 共享。
#[derive(Clone)]
pub struct ExtensionToolClient {
    inner: Arc<ExtensionToolClientInner>,
}

struct ExtensionToolClientInner {
    transport: Arc<dyn ExtensionRequestTransport>,
    options: ExtensionToolOptions,
    next_request_id: AtomicU64,
}

impl fmt::Debug for ExtensionToolClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExtensionToolClient")
            .field(
                "request_deadline_ms",
                &self.inner.options.request_deadline_ms,
            )
            .field("cancel_grace_ms", &self.inner.options.cancel_grace_ms)
            .finish_non_exhaustive()
    }
}

impl ExtensionToolClient {
    /// 创建尚未执行 handshake 的 client。
    pub fn new<T>(transport: T, options: ExtensionToolOptions) -> Self
    where
        T: ExtensionRequestTransport + 'static,
    {
        Self {
            inner: Arc::new(ExtensionToolClientInner {
                transport: Arc::new(transport),
                options,
                next_request_id: AtomicU64::new(1),
            }),
        }
    }

    /// 完成 initialize 与 tools.list，并在失败时关闭 transport。
    pub async fn discover(&self) -> Result<ExtensionToolSet, ExtensionDiscoveryError> {
        let result = self.discover_inner().await;
        if result.is_err() {
            let _ = self.inner.transport.shutdown();
        }
        result
    }

    async fn discover_inner(&self) -> Result<ExtensionToolSet, ExtensionDiscoveryError> {
        let capabilities = [
            ExtensionCapability::Cancel,
            ExtensionCapability::StructuredErrors,
        ];
        let initialize = InitializeParams {
            protocol_version: extension_protocol::PROTOCOL_VERSION,
            capabilities: capabilities.to_vec(),
        };
        let request = ExtensionRequest::new(
            self.next_request_id(),
            ExtensionMethod::Initialize,
            initialize,
        )
        .map_err(|_| ExtensionDiscoveryError::Protocol)?
        .with_capabilities(capabilities);
        let response = self.request(request).await.map_err(map_discovery_error)?;
        let initialize = response_result::<InitializeResult>(&response)?;
        initialize
            .validate()
            .map_err(|_| ExtensionDiscoveryError::Protocol)?;
        if !initialize
            .capabilities
            .contains(&ExtensionCapability::Cancel)
        {
            return Err(ExtensionDiscoveryError::MissingCancelCapability);
        }

        let request = ExtensionRequest::new(
            self.next_request_id(),
            ExtensionMethod::ToolsList,
            ToolsListParams::default(),
        )
        .map_err(|_| ExtensionDiscoveryError::Protocol)?;
        let response = self.request(request).await.map_err(map_discovery_error)?;
        let list = response_result::<ToolsListResult>(&response)?;
        validate_descriptors(&list.tools)?;

        Ok(ExtensionToolSet {
            client: self.clone(),
            descriptors: list.tools,
            transport_guard: Some(ExtensionTransportGuard::new(Arc::clone(
                &self.inner.transport,
            ))),
        })
    }

    async fn request(&self, request: ExtensionRequest) -> Result<ExtensionResponse, RequestError> {
        request.validate().map_err(|_| RequestError::Protocol)?;
        let request_id = request.request_id().to_string();
        let response = self
            .inner
            .transport
            .request(request)
            .await
            .map_err(RequestError::Transport)?;
        response.validate().map_err(|_| RequestError::Protocol)?;
        if response.request_id() != request_id {
            return Err(RequestError::Correlation);
        }
        Ok(response)
    }

    async fn execute(
        &self,
        call: ToolCall,
        tool_name: String,
        context: ToolExecutionContext<'_>,
    ) -> ToolResult {
        let call_id = call.call_id.clone();
        let cancellation = context.cancellation();
        if cancellation.is_cancelled() {
            return ToolResult::error(call_id, "Extension tool execution was cancelled");
        }

        let execute_id = self.next_request_id();
        let request = match ExtensionRequest::new(
            execute_id.clone(),
            ExtensionMethod::ToolsExecute,
            ToolExecuteParams {
                name: tool_name,
                arguments: call.arguments,
            },
        ) {
            Ok(request) => request.with_deadline_ms(self.inner.options.request_deadline_ms.get()),
            Err(_) => return ToolResult::error(call_id, "Extension tool request was invalid"),
        };

        let response = self.request(request);
        tokio::pin!(response);
        let deadline = sleep(Duration::from_millis(
            self.inner.options.request_deadline_ms.get(),
        ));
        tokio::pin!(deadline);
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                self.cancel_execution(&execute_id).await;
                ToolResult::error(call_id, "Extension tool execution was cancelled")
            }
            _ = &mut deadline => {
                self.cancel_execution(&execute_id).await;
                ToolResult::error(call_id, "Extension tool execution timed out")
            }
            response = &mut response => {
                self.map_execution_response(call_id, response).await
            }
        }
    }

    async fn cancel_execution(&self, execute_id: &str) {
        let request = match ExtensionRequest::new(
            self.next_request_id(),
            ExtensionMethod::ToolsCancel,
            ToolCancelParams {
                request_id: execute_id.to_string(),
            },
        ) {
            Ok(request) => request,
            Err(_) => return,
        };
        let cancel_response = timeout(
            Duration::from_millis(self.inner.options.cancel_grace_ms.get()),
            self.request(request),
        )
        .await;
        let Ok(Ok(response)) = cancel_response else {
            return;
        };
        if response.error().is_some() {
            return;
        }
        let Ok(Some(ToolCancelResult { accepted: true })) = response.result() else {
            return;
        };
    }

    async fn map_execution_response(
        &self,
        call_id: String,
        response: Result<ExtensionResponse, RequestError>,
    ) -> ToolResult {
        let response = match response {
            Ok(response) => response,
            Err(RequestError::Transport(_)) => {
                return ToolResult::error(call_id, "Extension tool transport failed");
            }
            Err(RequestError::Protocol) => {
                return ToolResult::error(call_id, "Extension tool response was invalid");
            }
            Err(RequestError::Correlation) => {
                return ToolResult::error(call_id, "Extension tool response did not correlate");
            }
        };
        if let Some(error) = response.error() {
            return ToolResult::error(call_id, safe_remote_error_text(error.code()));
        }
        let result = match response.result::<ToolExecuteResult>() {
            Ok(Some(result)) => result,
            _ => return ToolResult::error(call_id, "Extension tool response was invalid"),
        };
        let content = result
            .content
            .into_iter()
            .map(|content| match content {
                ToolContent::Text(text) => ToolResultContent::Text(text),
                ToolContent::Json(value) => ToolResultContent::Text(compact_json(value)),
            })
            .collect::<Vec<_>>();
        if result.is_error {
            ToolResult::error(call_id, "Extension tool reported a failure")
        } else {
            ToolResult::success_content(call_id, content)
        }
    }

    fn next_request_id(&self) -> String {
        let id = self
            .inner
            .next_request_id
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .expect("extension request id space should be unreachable");
        format!("extension-{id}")
    }
}

/// handshake 后已验证的 immutable extension tool set。
pub struct ExtensionToolSet {
    client: ExtensionToolClient,
    descriptors: Vec<ToolDescriptor>,
    transport_guard: Option<ExtensionTransportGuard>,
}

impl fmt::Debug for ExtensionToolSet {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExtensionToolSet")
            .field("tool_count", &self.descriptors.len())
            .finish_non_exhaustive()
    }
}

impl ExtensionToolSet {
    /// 返回 host-owned tool definitions；permission 仅来自 options。
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.descriptors
            .iter()
            .map(|descriptor| definition_for(descriptor, self.client.inner.options))
            .collect()
    }

    /// 将整组 tool 原子注册到 catalog，并返回拥有 inverse 的 mount。
    pub fn mount(
        mut self,
        catalog: &tool_runtime::ToolCatalog,
        owner: impl Into<String>,
    ) -> Result<ExtensionToolMount, ExtensionMountError> {
        let tools = self
            .descriptors
            .iter()
            .cloned()
            .map(|descriptor| ExtensionTool {
                client: self.client.clone(),
                descriptor,
            })
            .collect::<Vec<_>>();
        let registration = catalog
            .register_batch(owner, tools)
            .map_err(|_| ExtensionMountError::DuplicateTool)?;
        let transport_guard = self
            .transport_guard
            .take()
            .expect("discovered extension tool set owns a transport guard");
        Ok(ExtensionToolMount {
            registration: Some(registration),
            transport_guard: Some(transport_guard),
        })
    }
}

/// extension tool visibility 与 transport shutdown 的组合 inverse。
pub struct ExtensionToolMount {
    registration: Option<tool_runtime::ToolRegistration>,
    transport_guard: Option<ExtensionTransportGuard>,
}

impl fmt::Debug for ExtensionToolMount {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExtensionToolMount")
            .field("has_registration", &self.registration.is_some())
            .field("has_transport", &self.transport_guard.is_some())
            .finish()
    }
}

impl ExtensionToolMount {
    /// 按可观察性逆序撤销：先移除 tool，再停止 transport。
    pub fn dispose(&mut self) -> Result<(), ExtensionTransportError> {
        if let Some(mut registration) = self.registration.take() {
            registration.dispose();
        }
        self.transport_guard
            .take()
            .map_or(Ok(()), |mut guard| guard.shutdown())
    }
}

impl Drop for ExtensionToolMount {
    fn drop(&mut self) {
        let _ = self.dispose();
    }
}

struct ExtensionTransportGuard {
    transport: Option<Arc<dyn ExtensionRequestTransport>>,
}

impl ExtensionTransportGuard {
    fn new(transport: Arc<dyn ExtensionRequestTransport>) -> Self {
        Self {
            transport: Some(transport),
        }
    }

    fn shutdown(&mut self) -> Result<(), ExtensionTransportError> {
        self.transport
            .take()
            .map_or(Ok(()), |transport| transport.shutdown())
    }
}

impl Drop for ExtensionTransportGuard {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

struct ExtensionTool {
    client: ExtensionToolClient,
    descriptor: ToolDescriptor,
}

impl Tool for ExtensionTool {
    fn definition(&self) -> ToolDefinition {
        definition_for(&self.descriptor, self.client.inner.options)
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        cancellation: &'a CancellationToken,
    ) -> ToolExecutionFuture<'a> {
        self.execute_with_context(call, ToolExecutionContext::new(cancellation))
    }

    fn execute_with_context<'a>(
        &'a self,
        call: ToolCall,
        context: ToolExecutionContext<'a>,
    ) -> ToolExecutionFuture<'a> {
        let client = self.client.clone();
        let tool_name = self.descriptor.name.clone();
        Box::pin(async move { client.execute(call, tool_name, context).await })
    }
}

fn definition_for(descriptor: &ToolDescriptor, options: ExtensionToolOptions) -> ToolDefinition {
    let mut definition = ToolDefinition::new(descriptor.name.clone())
        .with_kind(ToolKind::Other)
        .with_permission_policy(options.permission_policy());
    if let Some(description) = descriptor.description.clone() {
        definition = definition.with_description(description);
    }
    if let Some(schema) = descriptor.input_schema.clone() {
        definition = definition.with_input_schema(schema);
    }
    definition
}

fn validate_descriptors(descriptors: &[ToolDescriptor]) -> Result<(), ExtensionDiscoveryError> {
    let mut names = BTreeSet::new();
    for descriptor in descriptors {
        if descriptor.name.is_empty()
            || descriptor.name.trim() != descriptor.name
            || !descriptor.name.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return Err(ExtensionDiscoveryError::InvalidDescriptor);
        }
        if !names.insert(descriptor.name.as_str()) {
            return Err(ExtensionDiscoveryError::DuplicateTool);
        }
    }
    Ok(())
}

fn response_result<T: serde::de::DeserializeOwned>(
    response: &ExtensionResponse,
) -> Result<T, ExtensionDiscoveryError> {
    if let Some(error) = response.error() {
        return Err(ExtensionDiscoveryError::Remote(error.code()));
    }
    response
        .result::<T>()
        .map_err(|_| ExtensionDiscoveryError::Protocol)?
        .ok_or(ExtensionDiscoveryError::MissingResult)
}

#[derive(Debug)]
enum RequestError {
    Transport(ExtensionTransportError),
    Protocol,
    Correlation,
}

fn map_discovery_error(error: RequestError) -> ExtensionDiscoveryError {
    match error {
        RequestError::Transport(error) => ExtensionDiscoveryError::Transport(error),
        RequestError::Protocol => ExtensionDiscoveryError::Protocol,
        RequestError::Correlation => ExtensionDiscoveryError::Correlation,
    }
}

fn compact_json(value: Value) -> String {
    value.to_string()
}

fn safe_remote_error_text(code: ExtensionErrorCode) -> &'static str {
    match code {
        ExtensionErrorCode::Cancelled => "Extension tool execution was cancelled",
        ExtensionErrorCode::DeadlineExceeded => "Extension tool execution timed out",
        ExtensionErrorCode::ToolNotFound => "Extension tool was not found",
        ExtensionErrorCode::ToolRejected => "Extension tool rejected the request",
        ExtensionErrorCode::CapabilityDenied => "Extension capability was denied",
        ExtensionErrorCode::InvalidRequest | ExtensionErrorCode::ProtocolViolation => {
            "Extension request was invalid"
        }
        ExtensionErrorCode::UnsupportedVersion => "Extension protocol version is unsupported",
        ExtensionErrorCode::Internal => "Extension tool failed",
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use serde_json::json;
    use tokio::sync::oneshot;
    use tool_runtime::{ToolExecutor, ToolResultOutcome};

    use super::*;

    #[derive(Default)]
    struct ScriptedTransport {
        state: Arc<Mutex<ScriptState>>,
    }

    struct ScriptState {
        requests: Vec<(ExtensionMethod, String)>,
        shutdown_count: usize,
        execute_waiter: Option<oneshot::Receiver<()>>,
        execute_mode: ExecuteMode,
        grant_cancel: bool,
        duplicate_descriptors: bool,
    }

    impl Default for ScriptState {
        fn default() -> Self {
            Self {
                requests: Vec::new(),
                shutdown_count: 0,
                execute_waiter: None,
                execute_mode: ExecuteMode::Success,
                grant_cancel: true,
                duplicate_descriptors: false,
            }
        }
    }

    #[derive(Clone, Copy, Default)]
    enum ExecuteMode {
        #[default]
        Success,
        MismatchedResponse,
        RemoteFailure,
        ResultFailure,
    }

    impl ExtensionRequestTransport for ScriptedTransport {
        fn request(&self, request: ExtensionRequest) -> ExtensionRequestFuture<'_> {
            let state = Arc::clone(&self.state);
            let method = request.method();
            let request_id = request.request_id().to_string();
            state
                .lock()
                .expect("script lock")
                .requests
                .push((method, request_id.clone()));
            Box::pin(async move {
                match method {
                    ExtensionMethod::Initialize => ExtensionResponse::success(
                        request_id,
                        InitializeResult {
                            protocol: extension_protocol::PROTOCOL_NAME.to_string(),
                            version: extension_protocol::PROTOCOL_VERSION,
                            capabilities: {
                                let mut capabilities = vec![ExtensionCapability::StructuredErrors];
                                if state.lock().expect("script lock").grant_cancel {
                                    capabilities.insert(0, ExtensionCapability::Cancel);
                                }
                                capabilities
                            },
                        },
                    )
                    .map_err(|_| ExtensionTransportError::Protocol),
                    ExtensionMethod::ToolsList => ExtensionResponse::success(
                        request_id,
                        ToolsListResult {
                            tools: {
                                let descriptor = ToolDescriptor {
                                    name: "echo".to_string(),
                                    description: Some("echoes a value".to_string()),
                                    input_schema: Some(json!({
                                        "type": "object",
                                        "properties": { "value": { "type": "string" } },
                                        "required": ["value"],
                                        "additionalProperties": false
                                    })),
                                };
                                let mut tools = vec![descriptor.clone()];
                                if state.lock().expect("script lock").duplicate_descriptors {
                                    tools.push(descriptor);
                                }
                                tools
                            },
                        },
                    )
                    .map_err(|_| ExtensionTransportError::Protocol),
                    ExtensionMethod::ToolsExecute => {
                        let mode = state.lock().expect("script lock").execute_mode;
                        let waiter = state.lock().expect("script lock").execute_waiter.take();
                        if let Some(waiter) = waiter {
                            let _ = waiter.await;
                        }
                        if matches!(mode, ExecuteMode::MismatchedResponse) {
                            return ExtensionResponse::success(
                                "wrong-response-id",
                                ToolExecuteResult {
                                    content: vec![ToolContent::Text("unexpected".to_string())],
                                    is_error: false,
                                },
                            )
                            .map_err(|_| ExtensionTransportError::Protocol);
                        }
                        if matches!(mode, ExecuteMode::RemoteFailure) {
                            return Ok(ExtensionResponse::failure(
                                request_id,
                                extension_protocol::ExtensionError::new(
                                    ExtensionErrorCode::ToolRejected,
                                    "remote secret instruction and argument",
                                    false,
                                )
                                .with_details(&json!({ "secret": "remote result" }))
                                .expect("details should encode"),
                            ));
                        }
                        if matches!(mode, ExecuteMode::ResultFailure) {
                            return ExtensionResponse::success(
                                request_id,
                                ToolExecuteResult {
                                    content: vec![ToolContent::Text(
                                        "secret instruction and path".to_string(),
                                    )],
                                    is_error: true,
                                },
                            )
                            .map_err(|_| ExtensionTransportError::Protocol);
                        }
                        ExtensionResponse::success(
                            request_id,
                            ToolExecuteResult {
                                content: vec![ToolContent::Text("ok".to_string())],
                                is_error: false,
                            },
                        )
                        .map_err(|_| ExtensionTransportError::Protocol)
                    }
                    ExtensionMethod::ToolsCancel => {
                        ExtensionResponse::success(request_id, ToolCancelResult { accepted: true })
                            .map_err(|_| ExtensionTransportError::Protocol)
                    }
                    ExtensionMethod::Shutdown => ExtensionResponse::success(
                        request_id,
                        extension_protocol::ShutdownResult { drained: true },
                    )
                    .map_err(|_| ExtensionTransportError::Protocol),
                }
            })
        }

        fn shutdown(&self) -> Result<(), ExtensionTransportError> {
            self.state.lock().expect("script lock").shutdown_count += 1;
            Ok(())
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn discovery_maps_definitions_and_host_permission_policy() {
        let transport = ScriptedTransport::default();
        let state = Arc::clone(&transport.state);
        let options = ExtensionToolOptions::new(
            NonZeroU64::new(100).expect("nonzero"),
            NonZeroU64::new(10).expect("nonzero"),
            ToolPermissionPolicy::Ask,
        );
        let client = ExtensionToolClient::new(transport, options);
        let set = client.discover().await.expect("discovery should succeed");
        let definition = set.definitions().pop().expect("one definition");
        assert_eq!(definition.name, "echo");
        assert_eq!(definition.permission_policy, ToolPermissionPolicy::Ask);
        assert_eq!(
            definition.input_schema.expect("schema"),
            json!({
                "type": "object",
                "properties": { "value": { "type": "string" } },
                "required": ["value"],
                "additionalProperties": false
            })
        );
        assert_eq!(state.lock().expect("script lock").requests.len(), 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn schema_is_validated_before_transport_execute() {
        let transport = ScriptedTransport::default();
        let state = Arc::clone(&transport.state);
        let client = ExtensionToolClient::new(transport, ExtensionToolOptions::default());
        let set = client.discover().await.expect("discovery should succeed");
        let catalog = tool_runtime::ToolCatalog::default();
        let _mount = set
            .mount(&catalog, "extension")
            .expect("mount should succeed");
        let result = catalog
            .snapshot()
            .execute_tool(
                ToolCall::new("call", "echo", json!({})),
                &CancellationToken::new(),
            )
            .await;
        assert_eq!(result.outcome(), ToolResultOutcome::Error);
        assert_eq!(state.lock().expect("script lock").requests.len(), 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mount_drop_removes_tools_before_shutdown_and_is_idempotent() {
        let transport = ScriptedTransport::default();
        let state = Arc::clone(&transport.state);
        let client = ExtensionToolClient::new(transport, ExtensionToolOptions::default());
        let set = client.discover().await.expect("discovery should succeed");
        let catalog = tool_runtime::ToolCatalog::default();
        let mut mount = set
            .mount(&catalog, "extension")
            .expect("mount should succeed");
        assert_eq!(catalog.definitions().len(), 1);
        mount.dispose().expect("shutdown should succeed");
        mount.dispose().expect("second dispose should succeed");
        assert!(catalog.definitions().is_empty());
        assert_eq!(state.lock().expect("script lock").shutdown_count, 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn dropping_unmounted_tool_set_shuts_down_transport() {
        let transport = ScriptedTransport::default();
        let state = Arc::clone(&transport.state);
        let client = ExtensionToolClient::new(transport, ExtensionToolOptions::default());
        let set = client.discover().await.expect("discovery should succeed");
        drop(set);
        assert_eq!(state.lock().expect("script lock").shutdown_count, 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn missing_cancel_capability_fails_discovery_and_shuts_down() {
        let transport = ScriptedTransport::default();
        let state = Arc::clone(&transport.state);
        state.lock().expect("script lock").grant_cancel = false;
        let client = ExtensionToolClient::new(transport, ExtensionToolOptions::default());
        let error = client
            .discover()
            .await
            .expect_err("cancel is required for v1 mount");
        assert_eq!(error, ExtensionDiscoveryError::MissingCancelCapability);
        assert_eq!(state.lock().expect("script lock").shutdown_count, 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn duplicate_descriptors_fail_before_mount_and_shut_down() {
        let transport = ScriptedTransport::default();
        let state = Arc::clone(&transport.state);
        state.lock().expect("script lock").duplicate_descriptors = true;
        let client = ExtensionToolClient::new(transport, ExtensionToolOptions::default());
        let error = client
            .discover()
            .await
            .expect_err("duplicate descriptors must fail closed");
        assert_eq!(error, ExtensionDiscoveryError::DuplicateTool);
        assert_eq!(state.lock().expect("script lock").shutdown_count, 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn registration_collision_keeps_catalog_unchanged_and_shuts_down() {
        let transport = ScriptedTransport::default();
        let state = Arc::clone(&transport.state);
        let client = ExtensionToolClient::new(transport, ExtensionToolOptions::default());
        let set = client.discover().await.expect("discovery should succeed");
        let catalog = tool_runtime::ToolCatalog::default();
        let _existing = catalog
            .register("existing", StubTool)
            .expect("fixture should register");
        let error = set
            .mount(&catalog, "extension")
            .expect_err("existing name must reject mount");
        assert_eq!(error, ExtensionMountError::DuplicateTool);
        assert_eq!(catalog.definitions().len(), 1);
        assert_eq!(state.lock().expect("script lock").shutdown_count, 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancellation_sends_cancel_for_execute_request_and_returns_safe_text() {
        let transport = ScriptedTransport::default();
        let state = Arc::clone(&transport.state);
        let (release_tx, release_rx) = oneshot::channel();
        state.lock().expect("script lock").execute_waiter = Some(release_rx);
        let client = ExtensionToolClient::new(
            transport,
            ExtensionToolOptions::new(
                NonZeroU64::new(10_000).expect("nonzero"),
                NonZeroU64::new(50).expect("nonzero"),
                ToolPermissionPolicy::Never,
            ),
        );
        let set = client.discover().await.expect("discovery should succeed");
        let catalog = tool_runtime::ToolCatalog::default();
        let _mount = set
            .mount(&catalog, "extension")
            .expect("mount should succeed");
        let cancellation = CancellationToken::new();
        let snapshot = catalog.snapshot();
        let execution = snapshot.execute_tool(
            ToolCall::new("call", "echo", json!({ "value": "x" })),
            &cancellation,
        );
        tokio::pin!(execution);
        tokio::select! {
            result = &mut execution => panic!("execution unexpectedly completed: {result:?}"),
            _ = tokio::task::yield_now() => {}
        }
        cancellation.cancel();
        let result = execution.await;
        assert!(
            release_tx.send(()).is_err(),
            "late response receiver must be dropped"
        );
        assert_eq!(result.outcome(), ToolResultOutcome::Error);
        assert!(result.text_content().contains("cancelled"));
        let requests = state.lock().expect("script lock").requests.clone();
        assert!(
            requests
                .iter()
                .any(|(method, _)| *method == ExtensionMethod::ToolsCancel)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pre_cancelled_execution_does_not_send_execute_request() {
        let transport = ScriptedTransport::default();
        let state = Arc::clone(&transport.state);
        let client = ExtensionToolClient::new(transport, ExtensionToolOptions::default());
        let set = client.discover().await.expect("discovery should succeed");
        let catalog = tool_runtime::ToolCatalog::default();
        let _mount = set
            .mount(&catalog, "extension")
            .expect("mount should succeed");
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let result = catalog
            .snapshot()
            .execute_tool(
                ToolCall::new("call", "echo", json!({ "value": "x" })),
                &cancellation,
            )
            .await;
        assert_eq!(result.outcome(), ToolResultOutcome::Error);
        assert!(result.text_content().contains("cancelled"));
        assert_eq!(state.lock().expect("script lock").requests.len(), 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mismatched_response_is_rejected_without_remote_body() {
        let transport = ScriptedTransport::default();
        let state = Arc::clone(&transport.state);
        state.lock().expect("script lock").execute_mode = ExecuteMode::MismatchedResponse;
        let client = ExtensionToolClient::new(transport, ExtensionToolOptions::default());
        let set = client.discover().await.expect("discovery should succeed");
        let catalog = tool_runtime::ToolCatalog::default();
        let _mount = set
            .mount(&catalog, "extension")
            .expect("mount should succeed");
        let snapshot = catalog.snapshot();
        let result = snapshot
            .execute_tool(
                ToolCall::new("call", "echo", json!({ "value": "x" })),
                &CancellationToken::new(),
            )
            .await;
        assert_eq!(result.outcome(), ToolResultOutcome::Error);
        assert!(result.text_content().contains("correlate"));
        assert!(!result.text_content().contains("unexpected"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn remote_error_projects_only_stable_host_text() {
        let transport = ScriptedTransport::default();
        let state = Arc::clone(&transport.state);
        state.lock().expect("script lock").execute_mode = ExecuteMode::RemoteFailure;
        let client = ExtensionToolClient::new(transport, ExtensionToolOptions::default());
        let set = client.discover().await.expect("discovery should succeed");
        let catalog = tool_runtime::ToolCatalog::default();
        let _mount = set
            .mount(&catalog, "extension")
            .expect("mount should succeed");
        let snapshot = catalog.snapshot();
        let result = snapshot
            .execute_tool(
                ToolCall::new("call", "echo", json!({ "value": "secret argument" })),
                &CancellationToken::new(),
            )
            .await;
        assert_eq!(result.outcome(), ToolResultOutcome::Error);
        assert!(result.text_content().contains("rejected"));
        assert!(!result.text_content().contains("secret"));
        assert!(!format!("{result:?}").contains("remote"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn error_result_content_is_not_delivered_as_host_error_text() {
        let transport = ScriptedTransport::default();
        let state = Arc::clone(&transport.state);
        state.lock().expect("script lock").execute_mode = ExecuteMode::ResultFailure;
        let client = ExtensionToolClient::new(transport, ExtensionToolOptions::default());
        let set = client.discover().await.expect("discovery should succeed");
        let catalog = tool_runtime::ToolCatalog::default();
        let _mount = set
            .mount(&catalog, "extension")
            .expect("mount should succeed");
        let snapshot = catalog.snapshot();
        let result = snapshot
            .execute_tool(
                ToolCall::new("call", "echo", json!({ "value": "secret argument" })),
                &CancellationToken::new(),
            )
            .await;
        assert_eq!(result.outcome(), ToolResultOutcome::Error);
        assert!(result.text_content().contains("reported a failure"));
        assert!(!result.text_content().contains("secret"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn deadline_sends_cancel_and_returns_timeout_text() {
        let transport = ScriptedTransport::default();
        let state = Arc::clone(&transport.state);
        let (_release_tx, release_rx) = oneshot::channel();
        state.lock().expect("script lock").execute_waiter = Some(release_rx);
        let client = ExtensionToolClient::new(
            transport,
            ExtensionToolOptions::new(
                NonZeroU64::new(1).expect("nonzero"),
                NonZeroU64::new(50).expect("nonzero"),
                ToolPermissionPolicy::Never,
            ),
        );
        let set = client.discover().await.expect("discovery should succeed");
        let catalog = tool_runtime::ToolCatalog::default();
        let _mount = set
            .mount(&catalog, "extension")
            .expect("mount should succeed");
        let snapshot = catalog.snapshot();
        let result = snapshot
            .execute_tool(
                ToolCall::new("call", "echo", json!({ "value": "x" })),
                &CancellationToken::new(),
            )
            .await;
        assert_eq!(result.outcome(), ToolResultOutcome::Error);
        assert!(result.text_content().contains("timed out"));
        assert!(
            state
                .lock()
                .expect("script lock")
                .requests
                .iter()
                .any(|(method, _)| *method == ExtensionMethod::ToolsCancel)
        );
    }

    #[test]
    fn transport_and_protocol_errors_have_no_payload_source() {
        let error = ExtensionTransportError::Unavailable;
        assert_eq!(format!("{error:?}"), "Unavailable");
        assert!(!format!("{error}").contains("/"));
    }

    struct StubTool;

    impl Tool for StubTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition::new("echo")
        }

        fn execute<'a>(
            &'a self,
            call: ToolCall,
            _cancellation: &'a CancellationToken,
        ) -> ToolExecutionFuture<'a> {
            Box::pin(async move { ToolResult::success(call.call_id, "fixture") })
        }
    }
}
