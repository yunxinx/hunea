//! 版本化 out-of-process extension protocol 的 transport-neutral host adapter。
//!
//! 本 crate 把同一 transport owner 暴露的 tools 与 typed hooks 映射到 host registries；stdio
//! process transport 仍隔离在 concrete adapter module，不进入 protocol-neutral contract。

use std::{
    collections::BTreeSet,
    fmt,
    future::Future,
    num::NonZeroU64,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};

use extension_hook_runtime::{
    AfterToolResultDecision, AfterToolResultHook, AfterToolResultPayload,
    BeforeToolExecuteDecision, BeforeToolExecuteHook, BeforeToolExecutePayload, BeforeTurnDecision,
    BeforeTurnHook, BeforeTurnPayload, ExtensionHookRegistry, HookFailureKind, HookFuture, HookId,
    HookOwnerId, HookPriority, HookRegistration, HookRegistrationOptions, HookRejectionKind,
};
use extension_protocol::{
    AfterToolResultHookParams, AfterToolResultHookResult, BeforeToolExecuteHookParams,
    BeforeToolExecuteHookResult, BeforeTurnHookParams, BeforeTurnHookResult, ExtensionCapability,
    ExtensionErrorCode, ExtensionMethod, ExtensionRequest, ExtensionResponse, HookCancelParams,
    HookCancelResult, HookDescriptor, HookPhase, HookRejectionCode, HookToolCall,
    HookToolImageDetail, HookToolResult, HookToolResultContent, HookToolResultOutcome,
    HooksListParams, HooksListResult, InitializeParams, InitializeResult, ToolCancelParams,
    ToolCancelResult, ToolContent, ToolDescriptor, ToolExecuteParams, ToolExecuteResult,
    ToolsListParams, ToolsListResult,
};
use serde_json::Value;
use thiserror::Error;
use tokio::{time::sleep, time::timeout};
use tokio_util::sync::CancellationToken;
use tool_runtime::{
    Tool, ToolCall, ToolDefinition, ToolExecutionContext, ToolExecutionFuture, ToolImageDetail,
    ToolKind, ToolPermissionPolicy, ToolResult, ToolResultContent, ToolResultOutcome,
};

mod stdio;

pub use stdio::{
    StdioExtensionSource, StdioExtensionTransport, StdioTransportError, StdioTransportOptions,
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

/// 用于 lifecycle dependency 恢复的 host-owned extension discovery source。
///
/// source 只返回已完成 handshake/descriptor validation 的 opaque set；具体 launch policy
/// 与 transport implementation 留在 extension-runtime 或更上层 host。
pub trait ExtensionBundleSource: Send + Sync {
    /// 启动 fresh transport、完成 discovery，并返回一个未 mount 的 bundle。
    fn discover(&self) -> Result<ExtensionBundle, ExtensionDiscoveryError>;
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

/// host 为 extension 注入的超时、取消与 tool permission 策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtensionOptions {
    request_deadline_ms: NonZeroU64,
    cancel_grace_ms: NonZeroU64,
    permission_policy: ToolPermissionPolicy,
}

impl ExtensionOptions {
    /// 创建 extension options。
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

impl Default for ExtensionOptions {
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
    #[error("extension hook descriptor is invalid")]
    InvalidHookDescriptor,
    #[error("extension hook descriptors contain a duplicate")]
    DuplicateHook,
    #[error("extension client discovery has already started")]
    AlreadyDiscovered,
    #[error("extension returned a remote error")]
    Remote(ExtensionErrorCode),
}

/// mount 失败时的安全错误投影。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ExtensionMountError {
    #[error("extension tool registration conflicted with an existing tool")]
    DuplicateTool,
    #[error("extension hook owner identity is invalid")]
    InvalidHookOwner,
    #[error("extension hook registration conflicted with an existing hook")]
    DuplicateHook,
}

/// host-owned extension client；tools 与 hooks 共享 request sequence 和 transport。
#[derive(Clone)]
pub struct ExtensionClient {
    inner: Arc<ExtensionClientInner>,
}

struct ExtensionClientInner {
    transport: Arc<dyn ExtensionRequestTransport>,
    options: ExtensionOptions,
    next_request_id: AtomicU64,
    discovery_started: AtomicBool,
    is_closing: AtomicBool,
    shutdown_started: AtomicBool,
    active_hook_invocations: AtomicUsize,
    closing: CancellationToken,
}

impl fmt::Debug for ExtensionClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExtensionClient")
            .field(
                "request_deadline_ms",
                &self.inner.options.request_deadline_ms,
            )
            .field("cancel_grace_ms", &self.inner.options.cancel_grace_ms)
            .finish_non_exhaustive()
    }
}

impl ExtensionClient {
    /// 创建尚未执行 handshake 的 client。
    pub fn new<T>(transport: T, options: ExtensionOptions) -> Self
    where
        T: ExtensionRequestTransport + 'static,
    {
        Self {
            inner: Arc::new(ExtensionClientInner {
                transport: Arc::new(transport),
                options,
                next_request_id: AtomicU64::new(1),
                discovery_started: AtomicBool::new(false),
                is_closing: AtomicBool::new(false),
                shutdown_started: AtomicBool::new(false),
                active_hook_invocations: AtomicUsize::new(0),
                closing: CancellationToken::new(),
            }),
        }
    }

    /// 完成 initialize、tools.list 与可选 hooks.list，并在失败时关闭 transport。
    pub async fn discover(&self) -> Result<ExtensionBundle, ExtensionDiscoveryError> {
        if self.inner.discovery_started.swap(true, Ordering::AcqRel) {
            return Err(ExtensionDiscoveryError::AlreadyDiscovered);
        }
        let result = self.discover_inner().await;
        if result.is_err() {
            let _ = self.force_shutdown();
        }
        result
    }

    async fn discover_inner(&self) -> Result<ExtensionBundle, ExtensionDiscoveryError> {
        let capabilities = [
            ExtensionCapability::Cancel,
            ExtensionCapability::StructuredErrors,
            ExtensionCapability::Hooks,
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
        validate_tool_descriptors(&list.tools)?;

        let hook_descriptors = if initialize
            .capabilities
            .contains(&ExtensionCapability::Hooks)
        {
            let request = ExtensionRequest::new(
                self.next_request_id(),
                ExtensionMethod::HooksList,
                HooksListParams::default(),
            )
            .map_err(|_| ExtensionDiscoveryError::Protocol)?;
            let response = self.request(request).await.map_err(map_discovery_error)?;
            let list = response_result::<HooksListResult>(&response)?;
            list.validate().map_err(|error| match error {
                extension_protocol::ProtocolValidationError::DuplicateHookDescriptor => {
                    ExtensionDiscoveryError::DuplicateHook
                }
                _ => ExtensionDiscoveryError::InvalidHookDescriptor,
            })?;
            validate_hook_descriptors(&list.hooks)?;
            list.hooks
        } else {
            Vec::new()
        };

        Ok(ExtensionBundle {
            client: self.clone(),
            tool_descriptors: list.tools,
            hook_descriptors,
            transport_guard: Some(ExtensionTransportGuard::new(self.clone())),
            source: None,
        })
    }

    async fn request(&self, request: ExtensionRequest) -> Result<ExtensionResponse, RequestError> {
        if self.inner.is_closing.load(Ordering::Acquire)
            && request.method() != ExtensionMethod::HooksCancel
        {
            return Err(RequestError::Transport(ExtensionTransportError::ShutDown));
        }
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

    async fn execute_tool(
        &self,
        call: ToolCall,
        tool_name: String,
        context: ToolExecutionContext<'_>,
    ) -> ToolResult {
        let call_id = call.call_id.clone();
        let cancellation = context.cancellation();
        if cancellation.is_cancelled() || self.inner.is_closing.load(Ordering::Acquire) {
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
                self.cancel_tool_execution(&execute_id).await;
                ToolResult::error(call_id, "Extension tool execution was cancelled")
            }
            _ = self.inner.closing.cancelled() => {
                ToolResult::error(call_id, "Extension tool transport failed")
            }
            _ = &mut deadline => {
                self.cancel_tool_execution(&execute_id).await;
                ToolResult::error(call_id, "Extension tool execution timed out")
            }
            response = &mut response => {
                self.map_execution_response(call_id, response).await
            }
        }
    }

    async fn cancel_tool_execution(&self, execute_id: &str) {
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

    fn hook_deadline(&self) -> Duration {
        Duration::from_millis(self.inner.options.request_deadline_ms.get())
    }

    fn hook_cancellation_grace(&self) -> Duration {
        Duration::from_millis(self.inner.options.cancel_grace_ms.get())
    }

    async fn request_hook(
        &self,
        request: ExtensionRequest,
        cancellation: &CancellationToken,
    ) -> Result<ExtensionResponse, HookFailureKind> {
        let _active_invocation = ActiveHookInvocation::try_new(&self.inner)?;
        let invocation_request_id = request.request_id().to_string();
        let response = self.request(request);
        tokio::pin!(response);
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                self.cancel_hook_invocation(&invocation_request_id).await;
                Err(HookFailureKind::Unavailable)
            }
            response = &mut response => response.map_err(map_hook_request_error),
        }
    }

    async fn cancel_hook_invocation(&self, invocation_request_id: &str) {
        let Ok(request) = ExtensionRequest::new(
            self.next_request_id(),
            ExtensionMethod::HooksCancel,
            HookCancelParams {
                request_id: invocation_request_id.to_string(),
            },
        ) else {
            return;
        };
        let Ok(Ok(response)) = timeout(self.hook_cancellation_grace(), self.request(request)).await
        else {
            return;
        };
        if response.error().is_some() {
            return;
        }
        let Ok(Some(HookCancelResult { accepted: true })) = response.result() else {
            return;
        };
    }

    async fn invoke_before_turn(
        &self,
        hook_id: String,
        payload: BeforeTurnPayload,
        cancellation: CancellationToken,
    ) -> Result<BeforeTurnDecision, HookFailureKind> {
        let request = ExtensionRequest::new(
            self.next_request_id(),
            ExtensionMethod::HooksBeforeTurn,
            BeforeTurnHookParams {
                hook_id,
                items: payload.into_items(),
            },
        )
        .map_err(|_| HookFailureKind::InvalidInput)?
        .with_deadline_ms(self.inner.options.request_deadline_ms.get());
        let response = self.request_hook(request, &cancellation).await?;
        let result = hook_response_result::<BeforeTurnHookResult>(&response)?;
        result
            .validate()
            .map_err(|_| HookFailureKind::InvalidInput)?;
        match result {
            BeforeTurnHookResult::Continue { items } => BeforeTurnPayload::try_new(items)
                .map(BeforeTurnDecision::Continue)
                .map_err(|_| HookFailureKind::InvalidInput),
            BeforeTurnHookResult::Reject { code } => {
                Ok(BeforeTurnDecision::Reject(map_hook_rejection(code)))
            }
        }
    }

    async fn invoke_before_tool_execute(
        &self,
        hook_id: String,
        payload: BeforeToolExecutePayload,
        cancellation: CancellationToken,
    ) -> Result<BeforeToolExecuteDecision, HookFailureKind> {
        let call = payload.call();
        let request = ExtensionRequest::new(
            self.next_request_id(),
            ExtensionMethod::HooksBeforeToolExecute,
            BeforeToolExecuteHookParams {
                hook_id,
                call: HookToolCall {
                    call_id: call.call_id.clone(),
                    name: call.name.clone(),
                    arguments: call.arguments.clone(),
                },
            },
        )
        .map_err(|_| HookFailureKind::InvalidInput)?
        .with_deadline_ms(self.inner.options.request_deadline_ms.get());
        let response = self.request_hook(request, &cancellation).await?;
        match hook_response_result::<BeforeToolExecuteHookResult>(&response)? {
            BeforeToolExecuteHookResult::Continue => Ok(BeforeToolExecuteDecision::Continue),
            BeforeToolExecuteHookResult::Reject { code } => {
                Ok(BeforeToolExecuteDecision::Reject(map_hook_rejection(code)))
            }
        }
    }

    async fn invoke_after_tool_result(
        &self,
        hook_id: String,
        payload: AfterToolResultPayload,
        cancellation: CancellationToken,
    ) -> Result<AfterToolResultDecision, HookFailureKind> {
        let tool_name = payload.tool_name().to_string();
        let request = ExtensionRequest::new(
            self.next_request_id(),
            ExtensionMethod::HooksAfterToolResult,
            AfterToolResultHookParams {
                hook_id,
                tool_name: tool_name.clone(),
                result: hook_tool_result_from_local(payload.result()),
            },
        )
        .map_err(|_| HookFailureKind::InvalidInput)?
        .with_deadline_ms(self.inner.options.request_deadline_ms.get());
        let response = self.request_hook(request, &cancellation).await?;
        let result = hook_response_result::<AfterToolResultHookResult>(&response)?;
        result
            .validate()
            .map_err(|_| HookFailureKind::InvalidInput)?;
        match result {
            AfterToolResultHookResult::Continue { result } => {
                let result = local_tool_result_from_hook(result)?;
                Ok(AfterToolResultDecision::Continue(
                    AfterToolResultPayload::new(tool_name, result),
                ))
            }
        }
    }

    fn begin_shutdown(&self) -> Result<(), ExtensionTransportError> {
        self.inner.is_closing.store(true, Ordering::Release);
        self.inner.closing.cancel();
        if self.inner.active_hook_invocations.load(Ordering::Acquire) == 0 {
            self.force_shutdown()
        } else {
            Ok(())
        }
    }

    fn force_shutdown(&self) -> Result<(), ExtensionTransportError> {
        if self.inner.shutdown_started.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        self.inner.transport.shutdown()
    }
}

struct ActiveHookInvocation {
    client: Arc<ExtensionClientInner>,
}

impl ActiveHookInvocation {
    fn try_new(client: &Arc<ExtensionClientInner>) -> Result<Self, HookFailureKind> {
        if client.is_closing.load(Ordering::Acquire) {
            return Err(HookFailureKind::Unavailable);
        }
        client
            .active_hook_invocations
            .fetch_add(1, Ordering::AcqRel);
        if client.is_closing.load(Ordering::Acquire) {
            if client
                .active_hook_invocations
                .fetch_sub(1, Ordering::AcqRel)
                == 1
            {
                shutdown_client_inner(client);
            }
            return Err(HookFailureKind::Unavailable);
        }
        Ok(Self {
            client: Arc::clone(client),
        })
    }
}

impl Drop for ActiveHookInvocation {
    fn drop(&mut self) {
        if self
            .client
            .active_hook_invocations
            .fetch_sub(1, Ordering::AcqRel)
            == 1
            && self.client.is_closing.load(Ordering::Acquire)
        {
            shutdown_client_inner(&self.client);
        }
    }
}

fn shutdown_client_inner(client: &ExtensionClientInner) {
    if !client.shutdown_started.swap(true, Ordering::AcqRel) {
        let _ = client.transport.shutdown();
    }
}

/// Handshake 后已验证的 immutable extension contribution bundle。
pub struct ExtensionBundle {
    client: ExtensionClient,
    tool_descriptors: Vec<ToolDescriptor>,
    hook_descriptors: Vec<HookDescriptor>,
    transport_guard: Option<ExtensionTransportGuard>,
    source: Option<Arc<dyn ExtensionBundleSource>>,
}

impl fmt::Debug for ExtensionBundle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExtensionBundle")
            .field("tool_count", &self.tool_descriptors.len())
            .field("hook_count", &self.hook_descriptors.len())
            .field("has_rediscovery_source", &self.source.is_some())
            .finish_non_exhaustive()
    }
}

impl ExtensionBundle {
    /// 绑定一个可在 dependency generation 恢复时重新 discover 的 host source。
    pub fn with_rediscovery_source(mut self, source: Arc<dyn ExtensionBundleSource>) -> Self {
        self.source = Some(source);
        self
    }

    /// 返回 source 的 opaque handle；不会暴露 launch 参数或 process internals。
    pub fn rediscovery_source(&self) -> Option<Arc<dyn ExtensionBundleSource>> {
        self.source.clone()
    }

    /// 返回 host-owned tool definitions；permission 仅来自 options。
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tool_descriptors
            .iter()
            .map(|descriptor| definition_for(descriptor, self.client.inner.options))
            .collect()
    }

    /// 将全部 tool/hook contributions 原子注册，并返回组合 inverse。
    pub fn mount(
        mut self,
        catalog: &tool_runtime::ToolCatalog,
        hook_registry: &ExtensionHookRegistry,
        owner: impl Into<String>,
    ) -> Result<ExtensionMount, ExtensionMountError> {
        let owner = owner.into();
        let hook_owner = HookOwnerId::try_new(owner.clone())
            .map_err(|_| ExtensionMountError::InvalidHookOwner)?;
        let tools = self
            .tool_descriptors
            .iter()
            .cloned()
            .map(|descriptor| ExtensionTool {
                client: self.client.clone(),
                descriptor,
            })
            .collect::<Vec<_>>();
        let tool_registration = catalog
            .register_batch(owner, tools)
            .map_err(|_| ExtensionMountError::DuplicateTool)?;
        let hook_options = |priority| {
            HookRegistrationOptions::try_new_with_cancellation_grace(
                HookPriority::new(priority),
                self.client.hook_deadline(),
                self.client.hook_cancellation_grace(),
            )
            .expect("ExtensionOptions stores non-zero durations")
        };
        let mut hook_registrations = Vec::with_capacity(self.hook_descriptors.len());
        for descriptor in &self.hook_descriptors {
            let hook_id = HookId::try_new(descriptor.hook_id.clone())
                .map_err(|_| ExtensionMountError::DuplicateHook)?;
            let registration = match descriptor.phase {
                HookPhase::BeforeTurn => hook_registry.register_before_turn(
                    hook_owner.clone(),
                    hook_id.clone(),
                    hook_options(descriptor.priority),
                    Arc::new(RemoteBeforeTurnHook {
                        client: self.client.clone(),
                        hook_id: descriptor.hook_id.clone(),
                    }),
                ),
                HookPhase::BeforeToolExecute => hook_registry.register_before_tool_execute(
                    hook_owner.clone(),
                    hook_id.clone(),
                    hook_options(descriptor.priority),
                    Arc::new(RemoteBeforeToolExecuteHook {
                        client: self.client.clone(),
                        hook_id: descriptor.hook_id.clone(),
                    }),
                ),
                HookPhase::AfterToolResult => hook_registry.register_after_tool_result(
                    hook_owner.clone(),
                    hook_id,
                    hook_options(descriptor.priority),
                    Arc::new(RemoteAfterToolResultHook {
                        client: self.client.clone(),
                        hook_id: descriptor.hook_id.clone(),
                    }),
                ),
            }
            .map_err(|_| ExtensionMountError::DuplicateHook)?;
            hook_registrations.push(registration);
        }
        let transport_guard = self
            .transport_guard
            .take()
            .expect("discovered extension bundle owns a transport guard");
        Ok(ExtensionMount {
            hook_registrations,
            tool_registration: Some(tool_registration),
            transport_guard: Some(transport_guard),
        })
    }
}

/// Extension visibility 与 transport shutdown 的组合 inverse。
pub struct ExtensionMount {
    hook_registrations: Vec<HookRegistration>,
    tool_registration: Option<tool_runtime::ToolRegistration>,
    transport_guard: Option<ExtensionTransportGuard>,
}

impl fmt::Debug for ExtensionMount {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExtensionMount")
            .field("hook_registration_count", &self.hook_registrations.len())
            .field("has_tool_registration", &self.tool_registration.is_some())
            .field("has_transport", &self.transport_guard.is_some())
            .finish()
    }
}

impl ExtensionMount {
    /// 按可观察性逆序撤销：先移除 hooks，再移除 tools，最后停止 transport。
    pub fn dispose(&mut self) -> Result<(), ExtensionTransportError> {
        for registration in &mut self.hook_registrations {
            registration.dispose();
        }
        self.hook_registrations.clear();
        if let Some(mut registration) = self.tool_registration.take() {
            registration.dispose();
        }
        self.transport_guard
            .take()
            .map_or(Ok(()), |mut guard| guard.shutdown())
    }
}

impl Drop for ExtensionMount {
    fn drop(&mut self) {
        let _ = self.dispose();
    }
}

struct ExtensionTransportGuard {
    client: Option<ExtensionClient>,
}

impl ExtensionTransportGuard {
    fn new(client: ExtensionClient) -> Self {
        Self {
            client: Some(client),
        }
    }

    fn shutdown(&mut self) -> Result<(), ExtensionTransportError> {
        self.client
            .take()
            .map_or(Ok(()), |client| client.begin_shutdown())
    }
}

impl Drop for ExtensionTransportGuard {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

struct ExtensionTool {
    client: ExtensionClient,
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
        Box::pin(async move { client.execute_tool(call, tool_name, context).await })
    }
}

struct RemoteBeforeTurnHook {
    client: ExtensionClient,
    hook_id: String,
}

impl BeforeTurnHook for RemoteBeforeTurnHook {
    fn call(
        &self,
        payload: BeforeTurnPayload,
        cancellation: CancellationToken,
    ) -> HookFuture<BeforeTurnDecision> {
        let client = self.client.clone();
        let hook_id = self.hook_id.clone();
        Box::pin(async move {
            client
                .invoke_before_turn(hook_id, payload, cancellation)
                .await
        })
    }
}

struct RemoteBeforeToolExecuteHook {
    client: ExtensionClient,
    hook_id: String,
}

impl BeforeToolExecuteHook for RemoteBeforeToolExecuteHook {
    fn call(
        &self,
        payload: BeforeToolExecutePayload,
        cancellation: CancellationToken,
    ) -> HookFuture<BeforeToolExecuteDecision> {
        let client = self.client.clone();
        let hook_id = self.hook_id.clone();
        Box::pin(async move {
            client
                .invoke_before_tool_execute(hook_id, payload, cancellation)
                .await
        })
    }
}

struct RemoteAfterToolResultHook {
    client: ExtensionClient,
    hook_id: String,
}

impl AfterToolResultHook for RemoteAfterToolResultHook {
    fn call(
        &self,
        payload: AfterToolResultPayload,
        cancellation: CancellationToken,
    ) -> HookFuture<AfterToolResultDecision> {
        let client = self.client.clone();
        let hook_id = self.hook_id.clone();
        Box::pin(async move {
            client
                .invoke_after_tool_result(hook_id, payload, cancellation)
                .await
        })
    }
}

fn hook_response_result<T: serde::de::DeserializeOwned>(
    response: &ExtensionResponse,
) -> Result<T, HookFailureKind> {
    if let Some(error) = response.error() {
        return Err(map_remote_hook_error(error.code()));
    }
    response
        .result::<T>()
        .map_err(|_| HookFailureKind::InvalidInput)?
        .ok_or(HookFailureKind::InvalidInput)
}

fn map_hook_request_error(error: RequestError) -> HookFailureKind {
    match error {
        RequestError::Transport(_) => HookFailureKind::Unavailable,
        RequestError::Protocol | RequestError::Correlation => HookFailureKind::InvalidInput,
    }
}

fn map_remote_hook_error(code: ExtensionErrorCode) -> HookFailureKind {
    match code {
        ExtensionErrorCode::Cancelled
        | ExtensionErrorCode::DeadlineExceeded
        | ExtensionErrorCode::CapabilityDenied
        | ExtensionErrorCode::UnsupportedVersion => HookFailureKind::Unavailable,
        ExtensionErrorCode::InvalidRequest | ExtensionErrorCode::ProtocolViolation => {
            HookFailureKind::InvalidInput
        }
        ExtensionErrorCode::ToolNotFound
        | ExtensionErrorCode::ToolRejected
        | ExtensionErrorCode::Internal => HookFailureKind::Internal,
    }
}

fn map_hook_rejection(code: HookRejectionCode) -> HookRejectionKind {
    match code {
        HookRejectionCode::PolicyDenied => HookRejectionKind::PolicyDenied,
        HookRejectionCode::UnsupportedOperation => HookRejectionKind::UnsupportedOperation,
    }
}

fn hook_tool_result_from_local(result: &ToolResult) -> HookToolResult {
    HookToolResult {
        call_id: result.call_id().to_string(),
        content: result
            .content()
            .iter()
            .map(|content| match content {
                ToolResultContent::Text(text) => HookToolResultContent::Text { text: text.clone() },
                ToolResultContent::Image {
                    data_base64,
                    mime_type,
                    uri,
                    detail,
                } => HookToolResultContent::Image {
                    data_base64: data_base64.clone(),
                    mime_type: mime_type.clone(),
                    uri: uri.clone(),
                    detail: detail.map(|detail| match detail {
                        ToolImageDetail::High => HookToolImageDetail::High,
                        ToolImageDetail::Original => HookToolImageDetail::Original,
                    }),
                },
            })
            .collect(),
        outcome: match result.outcome() {
            ToolResultOutcome::Success => HookToolResultOutcome::Success,
            ToolResultOutcome::Error => HookToolResultOutcome::Error,
            ToolResultOutcome::Terminate => HookToolResultOutcome::Terminate,
        },
        display_content: result.display_content().map(str::to_string),
        details: result.details().cloned(),
    }
}

fn local_tool_result_from_hook(result: HookToolResult) -> Result<ToolResult, HookFailureKind> {
    result
        .validate()
        .map_err(|_| HookFailureKind::InvalidInput)?;
    let content = result
        .content
        .into_iter()
        .map(|content| match content {
            HookToolResultContent::Text { text } => ToolResultContent::Text(text),
            HookToolResultContent::Image {
                data_base64,
                mime_type,
                uri,
                detail,
            } => ToolResultContent::Image {
                data_base64,
                mime_type,
                uri,
                detail: detail.map(|detail| match detail {
                    HookToolImageDetail::High => ToolImageDetail::High,
                    HookToolImageDetail::Original => ToolImageDetail::Original,
                }),
            },
        })
        .collect();
    let outcome = match result.outcome {
        HookToolResultOutcome::Success => ToolResultOutcome::Success,
        HookToolResultOutcome::Error => ToolResultOutcome::Error,
        HookToolResultOutcome::Terminate => ToolResultOutcome::Terminate,
    };
    let mut local = ToolResult::from_content(result.call_id, content, outcome);
    if let Some(display_content) = result.display_content {
        local = local.with_display_content(display_content);
    }
    if let Some(details) = result.details {
        local = local.with_details(details);
    }
    Ok(local)
}

fn definition_for(descriptor: &ToolDescriptor, options: ExtensionOptions) -> ToolDefinition {
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

fn validate_tool_descriptors(
    descriptors: &[ToolDescriptor],
) -> Result<(), ExtensionDiscoveryError> {
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

fn validate_hook_descriptors(
    descriptors: &[HookDescriptor],
) -> Result<(), ExtensionDiscoveryError> {
    let mut identities = BTreeSet::new();
    for descriptor in descriptors {
        HookId::try_new(descriptor.hook_id.clone())
            .map_err(|_| ExtensionDiscoveryError::InvalidHookDescriptor)?;
        if !identities.insert((descriptor.phase, descriptor.hook_id.as_str())) {
            return Err(ExtensionDiscoveryError::DuplicateHook);
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
        grant_hooks: bool,
        hook_descriptors: Vec<HookDescriptor>,
        before_turn_behavior: BeforeTurnBehavior,
        before_tool_behavior: BeforeToolBehavior,
        after_result_behavior: AfterResultBehavior,
        hook_waiter: Option<oneshot::Receiver<()>>,
        hook_cancel_targets: Vec<String>,
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
                grant_hooks: false,
                hook_descriptors: Vec::new(),
                before_turn_behavior: BeforeTurnBehavior::Identity,
                before_tool_behavior: BeforeToolBehavior::Continue,
                after_result_behavior: AfterResultBehavior::Identity,
                hook_waiter: None,
                hook_cancel_targets: Vec::new(),
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

    #[derive(Clone, Copy, Default)]
    enum BeforeTurnBehavior {
        #[default]
        Identity,
        Append,
        Reject,
        RemoteFailure,
        InvalidEmpty,
        MismatchedResponse,
    }

    #[derive(Clone, Copy, Default)]
    enum BeforeToolBehavior {
        #[default]
        Continue,
        Reject,
        RemoteFailure,
        MismatchedResponse,
    }

    #[derive(Clone, Copy, Default)]
    enum AfterResultBehavior {
        #[default]
        Identity,
        Transform,
        RemoteFailure,
        ChangeCallIdentity,
        MismatchedResponse,
    }

    impl ExtensionRequestTransport for ScriptedTransport {
        fn request(&self, request: ExtensionRequest) -> ExtensionRequestFuture<'_> {
            let state = Arc::clone(&self.state);
            let method = request.method();
            let request_id = request.request_id().to_string();
            let request_for_response = request.clone();
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
                                if state.lock().expect("script lock").grant_hooks {
                                    capabilities.push(ExtensionCapability::Hooks);
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
                    ExtensionMethod::HooksList => ExtensionResponse::success(
                        request_id,
                        HooksListResult {
                            hooks: state.lock().expect("script lock").hook_descriptors.clone(),
                        },
                    )
                    .map_err(|_| ExtensionTransportError::Protocol),
                    ExtensionMethod::HooksBeforeTurn => {
                        let waiter = state.lock().expect("script lock").hook_waiter.take();
                        if let Some(waiter) = waiter {
                            let _ = waiter.await;
                        }
                        let params = request_for_response
                            .decode_params::<BeforeTurnHookParams>()
                            .map_err(|_| ExtensionTransportError::Protocol)?;
                        match state.lock().expect("script lock").before_turn_behavior {
                            BeforeTurnBehavior::Identity => ExtensionResponse::success(
                                request_id,
                                BeforeTurnHookResult::Continue {
                                    items: params.items,
                                },
                            ),
                            BeforeTurnBehavior::Append => {
                                let mut items = params.items;
                                items.push(provider_protocol::ConversationItem::text(
                                    provider_protocol::Role::Assistant,
                                    "remote-transform",
                                ));
                                ExtensionResponse::success(
                                    request_id,
                                    BeforeTurnHookResult::Continue { items },
                                )
                            }
                            BeforeTurnBehavior::Reject => ExtensionResponse::success(
                                request_id,
                                BeforeTurnHookResult::Reject {
                                    code: HookRejectionCode::PolicyDenied,
                                },
                            ),
                            BeforeTurnBehavior::RemoteFailure => Ok(ExtensionResponse::failure(
                                request_id,
                                extension_protocol::ExtensionError::new(
                                    ExtensionErrorCode::Internal,
                                    "private remote hook message",
                                    false,
                                )
                                .with_details(&json!({"private": "remote details"}))
                                .expect("details should encode"),
                            )),
                            BeforeTurnBehavior::InvalidEmpty => ExtensionResponse::success(
                                request_id,
                                BeforeTurnHookResult::Continue { items: Vec::new() },
                            ),
                            BeforeTurnBehavior::MismatchedResponse => ExtensionResponse::success(
                                "mismatched-hook-response",
                                BeforeTurnHookResult::Continue {
                                    items: params.items,
                                },
                            ),
                        }
                        .map_err(|_| ExtensionTransportError::Protocol)
                    }
                    ExtensionMethod::HooksBeforeToolExecute => {
                        match state.lock().expect("script lock").before_tool_behavior {
                            BeforeToolBehavior::Continue => ExtensionResponse::success(
                                request_id,
                                BeforeToolExecuteHookResult::Continue,
                            ),
                            BeforeToolBehavior::Reject => ExtensionResponse::success(
                                request_id,
                                BeforeToolExecuteHookResult::Reject {
                                    code: HookRejectionCode::UnsupportedOperation,
                                },
                            ),
                            BeforeToolBehavior::RemoteFailure => Ok(ExtensionResponse::failure(
                                request_id,
                                extension_protocol::ExtensionError::new(
                                    ExtensionErrorCode::ProtocolViolation,
                                    "private tool hook failure",
                                    false,
                                ),
                            )),
                            BeforeToolBehavior::MismatchedResponse => ExtensionResponse::success(
                                "mismatched-hook-response",
                                BeforeToolExecuteHookResult::Continue,
                            ),
                        }
                        .map_err(|_| ExtensionTransportError::Protocol)
                    }
                    ExtensionMethod::HooksAfterToolResult => {
                        let params = request_for_response
                            .decode_params::<AfterToolResultHookParams>()
                            .map_err(|_| ExtensionTransportError::Protocol)?;
                        match state.lock().expect("script lock").after_result_behavior {
                            AfterResultBehavior::Identity => ExtensionResponse::success(
                                request_id,
                                AfterToolResultHookResult::Continue {
                                    result: params.result,
                                },
                            ),
                            AfterResultBehavior::Transform => {
                                let mut result = params.result;
                                result.content = vec![HookToolResultContent::Image {
                                    data_base64: "transformed-image".to_string(),
                                    mime_type: "image/png".to_string(),
                                    uri: Some("memory://transformed".to_string()),
                                    detail: Some(HookToolImageDetail::Original),
                                }];
                                result.outcome = HookToolResultOutcome::Terminate;
                                result.display_content = Some("transformed-display".to_string());
                                result.details = Some(json!({"transformed": true}));
                                ExtensionResponse::success(
                                    request_id,
                                    AfterToolResultHookResult::Continue { result },
                                )
                            }
                            AfterResultBehavior::RemoteFailure => Ok(ExtensionResponse::failure(
                                request_id,
                                extension_protocol::ExtensionError::new(
                                    ExtensionErrorCode::Internal,
                                    "private result hook failure",
                                    false,
                                ),
                            )),
                            AfterResultBehavior::ChangeCallIdentity => {
                                let mut result = params.result;
                                result.call_id = "different-call".to_string();
                                ExtensionResponse::success(
                                    request_id,
                                    AfterToolResultHookResult::Continue { result },
                                )
                            }
                            AfterResultBehavior::MismatchedResponse => ExtensionResponse::success(
                                "mismatched-hook-response",
                                AfterToolResultHookResult::Continue {
                                    result: params.result,
                                },
                            ),
                        }
                        .map_err(|_| ExtensionTransportError::Protocol)
                    }
                    ExtensionMethod::HooksCancel => {
                        let params = request_for_response
                            .decode_params::<HookCancelParams>()
                            .map_err(|_| ExtensionTransportError::Protocol)?;
                        state
                            .lock()
                            .expect("script lock")
                            .hook_cancel_targets
                            .push(params.request_id);
                        ExtensionResponse::success(request_id, HookCancelResult { accepted: true })
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

    fn all_hook_descriptors() -> Vec<HookDescriptor> {
        vec![
            HookDescriptor {
                hook_id: "turn-hook".to_string(),
                phase: HookPhase::BeforeTurn,
                priority: 0,
            },
            HookDescriptor {
                hook_id: "tool-hook".to_string(),
                phase: HookPhase::BeforeToolExecute,
                priority: 0,
            },
            HookDescriptor {
                hook_id: "result-hook".to_string(),
                phase: HookPhase::AfterToolResult,
                priority: 0,
            },
        ]
    }

    fn enable_hooks(state: &Arc<Mutex<ScriptState>>) {
        let mut state = state.lock().expect("script lock");
        state.grant_hooks = true;
        state.hook_descriptors = all_hook_descriptors();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn tool_only_discovery_skips_hook_methods_and_maps_host_policy() {
        let transport = ScriptedTransport::default();
        let state = Arc::clone(&transport.state);
        let options = ExtensionOptions::new(
            NonZeroU64::new(100).expect("nonzero"),
            NonZeroU64::new(10).expect("nonzero"),
            ToolPermissionPolicy::Ask,
        );
        let client = ExtensionClient::new(transport, options);
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
        assert_eq!(
            state
                .lock()
                .expect("script lock")
                .requests
                .iter()
                .map(|(method, _)| *method)
                .collect::<Vec<_>>(),
            [ExtensionMethod::Initialize, ExtensionMethod::ToolsList]
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn hooks_capability_discovers_one_bundle_in_protocol_order() {
        let transport = ScriptedTransport::default();
        let state = Arc::clone(&transport.state);
        enable_hooks(&state);
        let client = ExtensionClient::new(transport, ExtensionOptions::default());

        let bundle = client.discover().await.expect("discovery should succeed");

        assert_eq!(bundle.definitions().len(), 1);
        assert_eq!(
            state
                .lock()
                .expect("script lock")
                .requests
                .iter()
                .map(|(method, _)| *method)
                .collect::<Vec<_>>(),
            [
                ExtensionMethod::Initialize,
                ExtensionMethod::ToolsList,
                ExtensionMethod::HooksList,
            ]
        );
        assert_eq!(
            client
                .discover()
                .await
                .expect_err("one client must create only one bundle"),
            ExtensionDiscoveryError::AlreadyDiscovered
        );
        drop(bundle);
        assert_eq!(state.lock().expect("script lock").shutdown_count, 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn duplicate_hook_descriptor_fails_discovery_and_shuts_down() {
        let transport = ScriptedTransport::default();
        let state = Arc::clone(&transport.state);
        enable_hooks(&state);
        state
            .lock()
            .expect("script lock")
            .hook_descriptors
            .push(HookDescriptor {
                hook_id: "turn-hook".to_string(),
                phase: HookPhase::BeforeTurn,
                priority: 99,
            });
        let client = ExtensionClient::new(transport, ExtensionOptions::default());

        let error = client
            .discover()
            .await
            .expect_err("duplicates must fail discovery");

        assert_eq!(error, ExtensionDiscoveryError::DuplicateHook);
        assert_eq!(state.lock().expect("script lock").shutdown_count, 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn invalid_hook_descriptor_fails_discovery_and_shuts_down() {
        let transport = ScriptedTransport::default();
        let state = Arc::clone(&transport.state);
        enable_hooks(&state);
        state.lock().expect("script lock").hook_descriptors[0].hook_id = "INVALID-HOOK".to_string();
        let client = ExtensionClient::new(transport, ExtensionOptions::default());

        let error = client
            .discover()
            .await
            .expect_err("invalid hook identity must fail discovery");

        assert_eq!(error, ExtensionDiscoveryError::InvalidHookDescriptor);
        assert_eq!(state.lock().expect("script lock").shutdown_count, 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn remote_and_local_before_turn_hooks_share_registry_order() {
        let transport = ScriptedTransport::default();
        let state = Arc::clone(&transport.state);
        enable_hooks(&state);
        state.lock().expect("script lock").before_turn_behavior = BeforeTurnBehavior::Append;
        let bundle = ExtensionClient::new(transport, ExtensionOptions::default())
            .discover()
            .await
            .expect("discovery should succeed");
        let catalog = tool_runtime::ToolCatalog::default();
        let registry = ExtensionHookRegistry::new();
        let first = registry
            .register_before_turn(
                HookOwnerId::try_new("local-first").unwrap(),
                HookId::try_new("turn").unwrap(),
                HookRegistrationOptions::try_new(HookPriority::new(-1), Duration::from_secs(1))
                    .unwrap(),
                Arc::new(|payload: BeforeTurnPayload, _| async move {
                    Ok(BeforeTurnDecision::Continue(
                        payload
                            .replace_items(vec![provider_protocol::ConversationItem::text(
                                provider_protocol::Role::User,
                                "local-before",
                            )])
                            .unwrap(),
                    ))
                }),
            )
            .unwrap();
        let remote_was_visible = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let remote_was_visible_for_hook = Arc::clone(&remote_was_visible);
        let last = registry
            .register_before_turn(
                HookOwnerId::try_new("local-last").unwrap(),
                HookId::try_new("turn").unwrap(),
                HookRegistrationOptions::try_new(HookPriority::new(1), Duration::from_secs(1))
                    .unwrap(),
                Arc::new(move |payload: BeforeTurnPayload, _| {
                    let remote_was_visible = Arc::clone(&remote_was_visible_for_hook);
                    async move {
                        remote_was_visible.store(
                            payload.items().len() == 2
                                && payload.items()[0].text_content() == "local-before"
                                && payload.items()[1].text_content() == "remote-transform",
                            Ordering::SeqCst,
                        );
                        Ok(BeforeTurnDecision::Continue(payload))
                    }
                }),
            )
            .unwrap();
        let mount = bundle
            .mount(&catalog, &registry, "external")
            .expect("mount should succeed");

        let output = registry
            .dispatch_before_turn(
                BeforeTurnPayload::try_new(vec![provider_protocol::ConversationItem::text(
                    provider_protocol::Role::User,
                    "initial-private",
                )])
                .unwrap(),
                &CancellationToken::new(),
            )
            .await
            .expect("hooks should continue");

        assert!(remote_was_visible.load(Ordering::SeqCst));
        assert_eq!(output.items()[0].text_content(), "local-before");
        assert_eq!(output.items()[1].text_content(), "remote-transform");
        drop((mount, first, last));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn remote_tool_gate_rejection_stays_closed() {
        let transport = ScriptedTransport::default();
        let state = Arc::clone(&transport.state);
        enable_hooks(&state);
        state.lock().expect("script lock").before_tool_behavior = BeforeToolBehavior::Reject;
        let bundle = ExtensionClient::new(transport, ExtensionOptions::default())
            .discover()
            .await
            .unwrap();
        let registry = ExtensionHookRegistry::new();
        let catalog = tool_runtime::ToolCatalog::default();
        let _mount = bundle.mount(&catalog, &registry, "external").unwrap();

        let error = registry
            .dispatch_before_tool_execute(
                BeforeToolExecutePayload::new(ToolCall::new(
                    "private-call",
                    "echo",
                    json!({"credential": "private"}),
                )),
                &CancellationToken::new(),
            )
            .await
            .expect_err("remote rejection must fail closed");

        assert_eq!(
            error.kind(),
            extension_hook_runtime::HookDispatchErrorKind::Rejected(
                HookRejectionKind::UnsupportedOperation
            )
        );
        let diagnostic = format!("{error:?} {error}");
        assert!(!diagnostic.contains("private-call"));
        assert!(!diagnostic.contains("credential"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn remote_after_result_transform_preserves_complete_typed_result() {
        let transport = ScriptedTransport::default();
        let state = Arc::clone(&transport.state);
        enable_hooks(&state);
        state.lock().expect("script lock").after_result_behavior = AfterResultBehavior::Transform;
        let bundle = ExtensionClient::new(transport, ExtensionOptions::default())
            .discover()
            .await
            .unwrap();
        let registry = ExtensionHookRegistry::new();
        let catalog = tool_runtime::ToolCatalog::default();
        let _mount = bundle.mount(&catalog, &registry, "external").unwrap();

        let output = registry
            .dispatch_after_tool_result(
                AfterToolResultPayload::new(
                    "echo",
                    ToolResult::success("call-1", "private-result")
                        .with_display_content("private-display")
                        .with_details(json!({"private": true})),
                ),
                &CancellationToken::new(),
            )
            .await
            .expect("transform should succeed");

        assert_eq!(output.tool_name(), "echo");
        assert_eq!(output.result().call_id(), "call-1");
        assert_eq!(output.result().outcome(), ToolResultOutcome::Terminate);
        assert_eq!(
            output.result().display_content(),
            Some("transformed-display")
        );
        assert_eq!(
            output.result().details(),
            Some(&json!({"transformed": true}))
        );
        assert!(matches!(
            output.result().content().as_slice(),
            [ToolResultContent::Image {
                data_base64,
                mime_type,
                uri: Some(uri),
                detail: Some(ToolImageDetail::Original),
            }] if data_base64 == "transformed-image"
                && mime_type == "image/png"
                && uri == "memory://transformed"
        ));
    }

    async fn before_turn_error(
        behavior: BeforeTurnBehavior,
    ) -> extension_hook_runtime::HookDispatchError {
        let transport = ScriptedTransport::default();
        let state = Arc::clone(&transport.state);
        enable_hooks(&state);
        state.lock().expect("script lock").before_turn_behavior = behavior;
        let bundle = ExtensionClient::new(transport, ExtensionOptions::default())
            .discover()
            .await
            .unwrap();
        let registry = ExtensionHookRegistry::new();
        let catalog = tool_runtime::ToolCatalog::default();
        let _mount = bundle.mount(&catalog, &registry, "external").unwrap();
        registry
            .dispatch_before_turn(
                BeforeTurnPayload::try_new(vec![provider_protocol::ConversationItem::text(
                    provider_protocol::Role::User,
                    "private-input",
                )])
                .unwrap(),
                &CancellationToken::new(),
            )
            .await
            .expect_err("behavior should fail")
    }

    #[tokio::test(flavor = "current_thread")]
    async fn remote_hook_failure_invalid_output_and_correlation_are_closed() {
        let rejection = before_turn_error(BeforeTurnBehavior::Reject).await;
        assert_eq!(
            rejection.kind(),
            extension_hook_runtime::HookDispatchErrorKind::Rejected(
                HookRejectionKind::PolicyDenied
            )
        );
        let remote = before_turn_error(BeforeTurnBehavior::RemoteFailure).await;
        assert_eq!(
            remote.kind(),
            extension_hook_runtime::HookDispatchErrorKind::Failed(HookFailureKind::Internal)
        );
        let invalid = before_turn_error(BeforeTurnBehavior::InvalidEmpty).await;
        assert_eq!(
            invalid.kind(),
            extension_hook_runtime::HookDispatchErrorKind::Failed(HookFailureKind::InvalidInput)
        );
        let correlation = before_turn_error(BeforeTurnBehavior::MismatchedResponse).await;
        assert_eq!(
            correlation.kind(),
            extension_hook_runtime::HookDispatchErrorKind::Failed(HookFailureKind::InvalidInput)
        );
        let diagnostics = format!(
            "{rejection:?} {rejection} {remote:?} {remote} {invalid:?} {invalid} {correlation:?} {correlation}"
        );
        for sentinel in [
            "private-input",
            "private remote hook message",
            "remote details",
        ] {
            assert!(!diagnostics.contains(sentinel));
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn changed_remote_call_identity_is_rejected_by_registry() {
        let transport = ScriptedTransport::default();
        let state = Arc::clone(&transport.state);
        enable_hooks(&state);
        state.lock().expect("script lock").after_result_behavior =
            AfterResultBehavior::ChangeCallIdentity;
        let bundle = ExtensionClient::new(transport, ExtensionOptions::default())
            .discover()
            .await
            .unwrap();
        let registry = ExtensionHookRegistry::new();
        let catalog = tool_runtime::ToolCatalog::default();
        let _mount = bundle.mount(&catalog, &registry, "external").unwrap();

        let error = registry
            .dispatch_after_tool_result(
                AfterToolResultPayload::new(
                    "echo",
                    ToolResult::success("original-call", "private-result"),
                ),
                &CancellationToken::new(),
            )
            .await
            .expect_err("identity mutation must fail");

        assert_eq!(
            error.kind(),
            extension_hook_runtime::HookDispatchErrorKind::InvalidOutput
        );
        assert!(!format!("{error:?} {error}").contains("original-call"));
    }

    async fn before_tool_error(
        behavior: BeforeToolBehavior,
    ) -> extension_hook_runtime::HookDispatchError {
        let transport = ScriptedTransport::default();
        let state = Arc::clone(&transport.state);
        enable_hooks(&state);
        state.lock().expect("script lock").before_tool_behavior = behavior;
        let bundle = ExtensionClient::new(transport, ExtensionOptions::default())
            .discover()
            .await
            .unwrap();
        let registry = ExtensionHookRegistry::new();
        let catalog = tool_runtime::ToolCatalog::default();
        let _mount = bundle.mount(&catalog, &registry, "external").unwrap();
        registry
            .dispatch_before_tool_execute(
                BeforeToolExecutePayload::new(ToolCall::new(
                    "private-call",
                    "echo",
                    json!({"private": true}),
                )),
                &CancellationToken::new(),
            )
            .await
            .expect_err("behavior should fail")
    }

    async fn after_result_error(
        behavior: AfterResultBehavior,
    ) -> extension_hook_runtime::HookDispatchError {
        let transport = ScriptedTransport::default();
        let state = Arc::clone(&transport.state);
        enable_hooks(&state);
        state.lock().expect("script lock").after_result_behavior = behavior;
        let bundle = ExtensionClient::new(transport, ExtensionOptions::default())
            .discover()
            .await
            .unwrap();
        let registry = ExtensionHookRegistry::new();
        let catalog = tool_runtime::ToolCatalog::default();
        let _mount = bundle.mount(&catalog, &registry, "external").unwrap();
        registry
            .dispatch_after_tool_result(
                AfterToolResultPayload::new(
                    "echo",
                    ToolResult::success("private-call", "private-result"),
                ),
                &CancellationToken::new(),
            )
            .await
            .expect_err("behavior should fail")
    }

    #[tokio::test(flavor = "current_thread")]
    async fn every_remote_phase_maps_failure_and_correlation_without_body() {
        for error in [
            before_tool_error(BeforeToolBehavior::RemoteFailure).await,
            before_tool_error(BeforeToolBehavior::MismatchedResponse).await,
            after_result_error(AfterResultBehavior::RemoteFailure).await,
            after_result_error(AfterResultBehavior::MismatchedResponse).await,
        ] {
            assert!(matches!(
                error.kind(),
                extension_hook_runtime::HookDispatchErrorKind::Failed(
                    HookFailureKind::Internal | HookFailureKind::InvalidInput
                )
            ));
            let diagnostic = format!("{error:?} {error}");
            for sentinel in [
                "private-call",
                "private-result",
                "private tool hook failure",
                "private result hook failure",
            ] {
                assert!(!diagnostic.contains(sentinel));
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn caller_cancellation_sends_correlated_hook_cancel_within_registry_grace() {
        let transport = ScriptedTransport::default();
        let state = Arc::clone(&transport.state);
        enable_hooks(&state);
        let (release_tx, release_rx) = oneshot::channel();
        state.lock().expect("script lock").hook_waiter = Some(release_rx);
        let options = ExtensionOptions::new(
            NonZeroU64::new(10_000).unwrap(),
            NonZeroU64::new(100).unwrap(),
            ToolPermissionPolicy::Never,
        );
        let bundle = ExtensionClient::new(transport, options)
            .discover()
            .await
            .unwrap();
        let registry = ExtensionHookRegistry::new();
        let catalog = tool_runtime::ToolCatalog::default();
        let _mount = bundle.mount(&catalog, &registry, "external").unwrap();
        let cancellation = CancellationToken::new();
        let dispatch = registry.dispatch_before_turn(
            BeforeTurnPayload::try_new(vec![provider_protocol::ConversationItem::text(
                provider_protocol::Role::User,
                "private",
            )])
            .unwrap(),
            &cancellation,
        );
        tokio::pin!(dispatch);
        tokio::select! {
            result = &mut dispatch => panic!("hook unexpectedly completed: {result:?}"),
            _ = tokio::task::yield_now() => {}
        }
        let invocation_request_id = state
            .lock()
            .expect("script lock")
            .requests
            .iter()
            .find(|(method, _)| *method == ExtensionMethod::HooksBeforeTurn)
            .map(|(_, request_id)| request_id.clone())
            .expect("hook request should be recorded");

        cancellation.cancel();
        let error = dispatch.await.expect_err("cancellation should fail closed");

        assert_eq!(
            error.kind(),
            extension_hook_runtime::HookDispatchErrorKind::CallerCancelled
        );
        assert!(release_tx.send(()).is_err());
        assert_eq!(
            state.lock().expect("script lock").hook_cancel_targets,
            [invocation_request_id]
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn hook_timeout_sends_correlated_cancel_and_drops_late_response() {
        let transport = ScriptedTransport::default();
        let state = Arc::clone(&transport.state);
        enable_hooks(&state);
        let (release_tx, release_rx) = oneshot::channel();
        state.lock().expect("script lock").hook_waiter = Some(release_rx);
        let options = ExtensionOptions::new(
            NonZeroU64::new(5).unwrap(),
            NonZeroU64::new(100).unwrap(),
            ToolPermissionPolicy::Never,
        );
        let bundle = ExtensionClient::new(transport, options)
            .discover()
            .await
            .unwrap();
        let registry = ExtensionHookRegistry::new();
        let catalog = tool_runtime::ToolCatalog::default();
        let _mount = bundle.mount(&catalog, &registry, "external").unwrap();

        let error = tokio::time::timeout(
            Duration::from_millis(500),
            registry.dispatch_before_turn(
                BeforeTurnPayload::try_new(vec![provider_protocol::ConversationItem::text(
                    provider_protocol::Role::User,
                    "private",
                )])
                .unwrap(),
                &CancellationToken::new(),
            ),
        )
        .await
        .expect("hook timeout plus cancellation grace must stay bounded")
        .expect_err("hook should time out");
        let invocation_request_id = state
            .lock()
            .expect("script lock")
            .requests
            .iter()
            .find(|(method, _)| *method == ExtensionMethod::HooksBeforeTurn)
            .map(|(_, request_id)| request_id.clone())
            .expect("hook request should be recorded");

        assert_eq!(
            error.kind(),
            extension_hook_runtime::HookDispatchErrorKind::TimedOut
        );
        assert_eq!(
            state.lock().expect("script lock").hook_cancel_targets,
            [invocation_request_id]
        );
        assert!(
            release_tx.send(()).is_err(),
            "the timed-out invocation must drop its late response receiver"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn hook_collision_rolls_back_tools_and_transport_without_touching_existing_hook() {
        let transport = ScriptedTransport::default();
        let state = Arc::clone(&transport.state);
        enable_hooks(&state);
        let bundle = ExtensionClient::new(transport, ExtensionOptions::default())
            .discover()
            .await
            .unwrap();
        let registry = ExtensionHookRegistry::new();
        let catalog = tool_runtime::ToolCatalog::default();
        let existing = registry
            .register_before_turn(
                HookOwnerId::try_new("external").unwrap(),
                HookId::try_new("turn-hook").unwrap(),
                HookRegistrationOptions::try_new(HookPriority::default(), Duration::from_secs(1))
                    .unwrap(),
                Arc::new(|payload: BeforeTurnPayload, _| async move {
                    Ok(BeforeTurnDecision::Continue(payload))
                }),
            )
            .unwrap();

        let error = bundle
            .mount(&catalog, &registry, "external")
            .expect_err("hook collision must roll back the whole mount");

        assert_eq!(error, ExtensionMountError::DuplicateHook);
        assert!(catalog.definitions().is_empty());
        assert_eq!(registry.snapshot().len(), 1);
        assert_eq!(state.lock().expect("script lock").shutdown_count, 1);
        drop(existing);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mount_disposal_cancels_in_flight_hook_before_transport_shutdown() {
        let transport = ScriptedTransport::default();
        let state = Arc::clone(&transport.state);
        enable_hooks(&state);
        let (_release_tx, release_rx) = oneshot::channel();
        state.lock().expect("script lock").hook_waiter = Some(release_rx);
        let options = ExtensionOptions::new(
            NonZeroU64::new(10_000).unwrap(),
            NonZeroU64::new(100).unwrap(),
            ToolPermissionPolicy::Never,
        );
        let bundle = ExtensionClient::new(transport, options)
            .discover()
            .await
            .unwrap();
        let registry = ExtensionHookRegistry::new();
        let catalog = tool_runtime::ToolCatalog::default();
        let mut mount = bundle.mount(&catalog, &registry, "external").unwrap();
        let dispatch_registry = registry.clone();
        let dispatch = tokio::spawn(async move {
            dispatch_registry
                .dispatch_before_turn(
                    BeforeTurnPayload::try_new(vec![provider_protocol::ConversationItem::text(
                        provider_protocol::Role::User,
                        "private",
                    )])
                    .unwrap(),
                    &CancellationToken::new(),
                )
                .await
        });
        while !state
            .lock()
            .expect("script lock")
            .requests
            .iter()
            .any(|(method, _)| *method == ExtensionMethod::HooksBeforeTurn)
        {
            tokio::task::yield_now().await;
        }

        mount.dispose().expect("dispose should succeed");
        let error = dispatch
            .await
            .unwrap()
            .expect_err("disposed registration must cancel delivery");

        assert_eq!(
            error.kind(),
            extension_hook_runtime::HookDispatchErrorKind::RegistrationDisposed
        );
        assert!(registry.snapshot().is_empty());
        assert!(catalog.definitions().is_empty());
        assert_eq!(state.lock().expect("script lock").shutdown_count, 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn schema_is_validated_before_transport_execute() {
        let transport = ScriptedTransport::default();
        let state = Arc::clone(&transport.state);
        let client = ExtensionClient::new(transport, ExtensionOptions::default());
        let set = client.discover().await.expect("discovery should succeed");
        let catalog = tool_runtime::ToolCatalog::default();
        let _mount = set
            .mount(&catalog, &ExtensionHookRegistry::new(), "extension")
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
        let client = ExtensionClient::new(transport, ExtensionOptions::default());
        let set = client.discover().await.expect("discovery should succeed");
        let catalog = tool_runtime::ToolCatalog::default();
        let mut mount = set
            .mount(&catalog, &ExtensionHookRegistry::new(), "extension")
            .expect("mount should succeed");
        assert_eq!(catalog.definitions().len(), 1);
        mount.dispose().expect("shutdown should succeed");
        mount.dispose().expect("second dispose should succeed");
        assert!(catalog.definitions().is_empty());
        assert_eq!(state.lock().expect("script lock").shutdown_count, 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn dropping_unmounted_bundle_shuts_down_transport() {
        let transport = ScriptedTransport::default();
        let state = Arc::clone(&transport.state);
        let client = ExtensionClient::new(transport, ExtensionOptions::default());
        let set = client.discover().await.expect("discovery should succeed");
        drop(set);
        assert_eq!(state.lock().expect("script lock").shutdown_count, 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn missing_cancel_capability_fails_discovery_and_shuts_down() {
        let transport = ScriptedTransport::default();
        let state = Arc::clone(&transport.state);
        state.lock().expect("script lock").grant_cancel = false;
        let client = ExtensionClient::new(transport, ExtensionOptions::default());
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
        let client = ExtensionClient::new(transport, ExtensionOptions::default());
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
        let client = ExtensionClient::new(transport, ExtensionOptions::default());
        let set = client.discover().await.expect("discovery should succeed");
        let catalog = tool_runtime::ToolCatalog::default();
        let _existing = catalog
            .register("existing", StubTool)
            .expect("fixture should register");
        let error = set
            .mount(&catalog, &ExtensionHookRegistry::new(), "extension")
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
        let client = ExtensionClient::new(
            transport,
            ExtensionOptions::new(
                NonZeroU64::new(10_000).expect("nonzero"),
                NonZeroU64::new(50).expect("nonzero"),
                ToolPermissionPolicy::Never,
            ),
        );
        let set = client.discover().await.expect("discovery should succeed");
        let catalog = tool_runtime::ToolCatalog::default();
        let _mount = set
            .mount(&catalog, &ExtensionHookRegistry::new(), "extension")
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
        let client = ExtensionClient::new(transport, ExtensionOptions::default());
        let set = client.discover().await.expect("discovery should succeed");
        let catalog = tool_runtime::ToolCatalog::default();
        let _mount = set
            .mount(&catalog, &ExtensionHookRegistry::new(), "extension")
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
        let client = ExtensionClient::new(transport, ExtensionOptions::default());
        let set = client.discover().await.expect("discovery should succeed");
        let catalog = tool_runtime::ToolCatalog::default();
        let _mount = set
            .mount(&catalog, &ExtensionHookRegistry::new(), "extension")
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
        let client = ExtensionClient::new(transport, ExtensionOptions::default());
        let set = client.discover().await.expect("discovery should succeed");
        let catalog = tool_runtime::ToolCatalog::default();
        let _mount = set
            .mount(&catalog, &ExtensionHookRegistry::new(), "extension")
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
        let client = ExtensionClient::new(transport, ExtensionOptions::default());
        let set = client.discover().await.expect("discovery should succeed");
        let catalog = tool_runtime::ToolCatalog::default();
        let _mount = set
            .mount(&catalog, &ExtensionHookRegistry::new(), "extension")
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
        let client = ExtensionClient::new(
            transport,
            ExtensionOptions::new(
                NonZeroU64::new(1).expect("nonzero"),
                NonZeroU64::new(50).expect("nonzero"),
                ToolPermissionPolicy::Never,
            ),
        );
        let set = client.discover().await.expect("discovery should succeed");
        let catalog = tool_runtime::ToolCatalog::default();
        let _mount = set
            .mount(&catalog, &ExtensionHookRegistry::new(), "extension")
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
