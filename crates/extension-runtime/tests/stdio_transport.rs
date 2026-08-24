use std::path::PathBuf;

use extension_hook_runtime::{BeforeTurnPayload, ExtensionHookRegistry};
use extension_protocol::{
    ExtensionMethod, ExtensionRequest, InitializeParams, ToolExecuteParams, ToolsListParams,
};
use extension_runtime::{
    ExtensionClient, ExtensionOptions, ExtensionRequestTransport, ExtensionTransportError,
    StdioExtensionTransport, StdioTransportOptions,
};
use provider_protocol::{ConversationItem, Role};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use tool_runtime::{ToolCall, ToolExecutor};

fn fixture_options(mode: &str) -> StdioTransportOptions {
    let executable = std::env::var_os("CARGO_BIN_EXE_stdio_fixture").map_or_else(
        || {
            std::env::current_exe()
                .expect("integration test executable path should be available")
                .parent()
                .and_then(std::path::Path::parent)
                .expect("target directory should be next to integration test executable")
                .join("stdio-fixture")
        },
        PathBuf::from,
    );
    StdioTransportOptions::new(executable).arg(mode)
}

#[tokio::test(flavor = "current_thread")]
async fn stdio_transport_runs_extension_tools_through_catalog() {
    let transport = StdioExtensionTransport::spawn(fixture_options("normal"))
        .expect("fixture transport should spawn");
    let client = ExtensionClient::new(transport, ExtensionOptions::default());
    let set = client.discover().await.expect("handshake should succeed");
    let catalog = tool_runtime::ToolCatalog::default();
    let hooks = ExtensionHookRegistry::new();
    let _mount = set
        .mount(&catalog, &hooks, "stdio-extension")
        .expect("mount should succeed");
    let result = catalog
        .snapshot()
        .execute_tool(
            ToolCall::new("call", "echo", json!({ "value": "hello" })),
            &CancellationToken::new(),
        )
        .await;
    assert_eq!(result.text_content(), "stdio-ok");

    let output = hooks
        .dispatch_before_turn(
            BeforeTurnPayload::try_new(vec![ConversationItem::text(Role::User, "private")])
                .expect("payload should validate"),
            &CancellationToken::new(),
        )
        .await
        .expect("stdio hook should continue");
    assert_eq!(output.items().len(), 2);
    assert_eq!(output.items()[0], output.items()[1]);
}

#[tokio::test(flavor = "current_thread")]
async fn concurrent_requests_are_correlated_by_request_id() {
    let transport = StdioExtensionTransport::spawn(fixture_options("normal"))
        .expect("fixture transport should spawn");
    let first = ExtensionRequest::new(
        "request-one",
        ExtensionMethod::ToolsList,
        ToolsListParams::default(),
    )
    .expect("request should encode");
    let second = ExtensionRequest::new(
        "request-two",
        ExtensionMethod::Initialize,
        InitializeParams {
            protocol_version: extension_protocol::PROTOCOL_VERSION,
            capabilities: vec![
                extension_protocol::ExtensionCapability::Cancel,
                extension_protocol::ExtensionCapability::StructuredErrors,
            ],
        },
    )
    .expect("request should encode");
    let (first, second) = tokio::join!(transport.request(first), transport.request(second));
    assert_eq!(first.expect("first response").request_id(), "request-one");
    assert_eq!(second.expect("second response").request_id(), "request-two");
}

#[tokio::test(flavor = "current_thread")]
async fn child_eof_and_invalid_frame_fail_closed() {
    let eof = StdioExtensionTransport::spawn(fixture_options("exit"))
        .expect("fixture transport should spawn");
    let request = ExtensionRequest::new(
        "request-eof",
        ExtensionMethod::ToolsExecute,
        ToolExecuteParams {
            name: "echo".to_string(),
            arguments: json!({}),
        },
    )
    .expect("request should encode");
    assert_eq!(
        eof.request(request).await,
        Err(ExtensionTransportError::Unavailable)
    );

    let malformed = StdioExtensionTransport::spawn(fixture_options("malformed"))
        .expect("fixture transport should spawn");
    let request = ExtensionRequest::new(
        "request-malformed",
        ExtensionMethod::ToolsList,
        ToolsListParams::default(),
    )
    .expect("request should encode");
    assert_eq!(
        malformed.request(request).await,
        Err(ExtensionTransportError::Protocol)
    );

    let oversized = StdioExtensionTransport::spawn(
        fixture_options("oversized")
            .max_frame_bytes(64)
            .expect("frame limit should be valid"),
    )
    .expect("fixture transport should spawn");
    let request = ExtensionRequest::new(
        "request-oversized",
        ExtensionMethod::ToolsList,
        ToolsListParams::default(),
    )
    .expect("request should encode");
    assert!(matches!(
        oversized.request(request).await,
        Err(ExtensionTransportError::Unavailable)
            | Err(ExtensionTransportError::Protocol)
            | Err(ExtensionTransportError::ShutDown)
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn unsolicited_response_fails_closed() {
    let transport = StdioExtensionTransport::spawn(fixture_options("unknown"))
        .expect("fixture transport should spawn");
    let request = ExtensionRequest::new(
        "request-known",
        ExtensionMethod::ToolsList,
        ToolsListParams::default(),
    )
    .expect("request should encode");
    assert_eq!(
        transport.request(request).await,
        Err(ExtensionTransportError::Protocol)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn stderr_is_drained_without_entering_public_error_projection() {
    let transport = StdioExtensionTransport::spawn(fixture_options("stderr"))
        .expect("fixture transport should spawn");
    let request = ExtensionRequest::new(
        "request-stderr",
        ExtensionMethod::ToolsList,
        ToolsListParams::default(),
    )
    .expect("request should encode");
    let error = transport
        .request(request)
        .await
        .expect_err("stderr-only child should close without a response");
    assert!(!error.to_string().contains("fixture stderr"));
    assert!(!format!("{transport:?}").contains("fixture stderr"));
}

#[tokio::test(flavor = "current_thread")]
async fn shutdown_rejects_new_requests_and_is_idempotent() {
    let transport = StdioExtensionTransport::spawn(fixture_options("normal"))
        .expect("fixture transport should spawn");
    transport.shutdown().expect("shutdown should succeed");
    transport
        .shutdown()
        .expect("second shutdown should succeed");
    let request = ExtensionRequest::new(
        "after-shutdown",
        ExtensionMethod::ToolsList,
        ToolsListParams::default(),
    )
    .expect("request should encode");
    assert_eq!(
        transport.request(request).await,
        Err(ExtensionTransportError::ShutDown)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn dropping_request_future_removes_pending_correlation_state() {
    let transport = StdioExtensionTransport::spawn(fixture_options("hold"))
        .expect("fixture transport should spawn");
    let request = ExtensionRequest::new(
        "dropped-request",
        ExtensionMethod::ToolsList,
        ToolsListParams::default(),
    )
    .expect("request should encode");
    let request = transport.request(request);
    assert!(format!("{transport:?}").contains("pending_count: 1"));

    drop(request);

    assert!(format!("{transport:?}").contains("pending_count: 0"));
}

#[tokio::test(flavor = "current_thread")]
async fn late_response_for_cancelled_request_is_discarded_without_closing_transport() {
    let transport = StdioExtensionTransport::spawn(fixture_options("late"))
        .expect("fixture transport should spawn");
    let first = ExtensionRequest::new(
        "dropped-request",
        ExtensionMethod::ToolsList,
        ToolsListParams::default(),
    )
    .expect("request should encode");
    drop(transport.request(first));

    let reused = ExtensionRequest::new(
        "dropped-request",
        ExtensionMethod::ToolsList,
        ToolsListParams::default(),
    )
    .expect("request should encode");
    assert_eq!(
        transport.request(reused).await,
        Err(ExtensionTransportError::Protocol)
    );

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let second = ExtensionRequest::new(
        "live-request",
        ExtensionMethod::ToolsList,
        ToolsListParams::default(),
    )
    .expect("request should encode");
    assert_eq!(
        transport
            .request(second)
            .await
            .expect("late response should not close the transport")
            .request_id(),
        "live-request"
    );
}
