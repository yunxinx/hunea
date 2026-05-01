use std::sync::mpsc;
use std::time::Duration;

use agent_client_protocol::schema::{AgentCapabilities, PromptCapabilities};

use super::acp_session::{
    AcpRuntimeState, acp_reject_option_id_for_cancel, apply_acp_session_event,
    run_interrupt_acp_prompt_effect,
};
use super::effects::{reset_runtime_session_after_clear, run_interrupt_current_turn_effect};
use super::input::{TerminalInputAction, coalesced_input_actions};
use super::native_agent::{
    apply_native_agent_event, drain_native_agent_runtime_events, native_agent_workspace_tools,
    run_send_native_agent_effect,
};
use super::*;
use crate::frontend::tui::{AppEffect, ReasoningDisplayMode, Sender, StatusLineItem};
use crate::runtime::acp::{AcpInitializeOutcome, AcpSessionEvent};
use crate::runtime::model_catalog::ModelSelection;
use crate::runtime::native::{
    CancellationToken, NativeAgentEvent, NativeAgentRequest, NativeAgentResponse,
    NativeLlmPerformanceMetrics,
};
use crate::runtime::phrases::StatusPhraseOrder;
use crate::runtime::session::RuntimeTarget;
use crate::runtime::tools::{RuntimeToolCall, RuntimeToolResult};
use agent_client_protocol::schema::ProtocolVersion;
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

#[test]
fn acp_chunks_buffer_until_prompt_response() {
    let mut model = Model::new(HeroOptions::default());
    model.transcript_mut().clear();
    let mut acp_runtime = AcpRuntimeState::default();

    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::PromptStarted {
            agent_id: "Kimi Code CLI".to_string(),
        },
    );
    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::AgentMessageChunk {
            agent_id: "Kimi Code CLI".to_string(),
            content: "你好".to_string(),
        },
    );
    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::AgentMessageChunk {
            agent_id: "Kimi Code CLI".to_string(),
            content: "！我是 Kimi Code CLI".to_string(),
        },
    );

    assert!(model.transcript_plain_items().is_empty());
    assert!(model.current_stream_activity_render_result().has_content);

    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::PromptResponse {
            agent_id: "Kimi Code CLI".to_string(),
            content: String::new(),
            stop_reason: "EndTurn".to_string(),
        },
    );

    assert_eq!(
        model.transcript_plain_items(),
        vec!["你好！我是 Kimi Code CLI".to_string()]
    );
    assert!(!model.current_stream_activity_render_result().has_content);
}

#[test]
fn acp_system_message_event_appends_runtime_system_message() {
    let mut model = Model::new(HeroOptions::default());
    model.transcript_mut().clear();
    let mut acp_runtime = AcpRuntimeState::default();

    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::SystemMessage {
            agent_id: "kimi".to_string(),
            message: crate::runtime::acp::debug_protocol_version_system_message(),
        },
    );

    let items = model.transcript_plain_items();
    assert!(
        items
            .iter()
            .any(|item| item.contains("ACP protocol version mismatch")),
        "expected protocol mismatch system message, got: {items:?}"
    );
}

#[test]
fn acp_started_uses_agent_title_and_version_in_current_model_status_line() {
    let mut model = Model::new_with_options(
        HeroOptions::default(),
        ModelOptions {
            status_line_items: vec![StatusLineItem::CurrentModel],
            ..ModelOptions::default()
        },
    );
    model.selected_acp_agent = Some("kimi".to_string());
    let mut acp_runtime = AcpRuntimeState::default();

    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::Started {
            agent_id: "kimi".to_string(),
            session_id: "test-session".to_string(),
            outcome: AcpInitializeOutcome {
                protocol_version: ProtocolVersion::V1,
                agent_name: Some("kimi".to_string()),
                agent_title: Some("Kimi Code CLI".to_string()),
                agent_version: Some("1.39.0".to_string()),
                agent_capabilities: AgentCapabilities::new()
                    .load_session(true)
                    .prompt_capabilities(PromptCapabilities::new().image(true)),
                auth_method_count: 0,
            },
        },
    );

    let identity = model
        .acp_agent_identities
        .get("kimi")
        .expect("started ACP agent identity should be saved");
    assert!(identity.agent_capabilities.load_session);
    assert!(identity.agent_capabilities.prompt_capabilities.image);

    let status = model.current_status_line_parts().join(" ");
    assert!(
        status.contains("Kimi Code CLI (1.39.0)"),
        "expected ACP identity with version in current-model status line, got: {status:?}"
    );
}

#[test]
fn acp_started_without_agent_info_keeps_configured_agent_label_in_status_line() {
    let mut model = Model::new_with_options(
        HeroOptions::default(),
        ModelOptions {
            status_line_items: vec![StatusLineItem::CurrentModel],
            ..ModelOptions::default()
        },
    );
    model.selected_acp_agent = Some("Kimi Code CLI".to_string());
    let mut acp_runtime = AcpRuntimeState::default();

    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::Started {
            agent_id: "Kimi Code CLI".to_string(),
            session_id: "test-session".to_string(),
            outcome: AcpInitializeOutcome {
                protocol_version: ProtocolVersion::V1,
                agent_name: None,
                agent_title: None,
                agent_version: None,
                agent_capabilities: AgentCapabilities::new(),
                auth_method_count: 0,
            },
        },
    );

    let identity = model
        .acp_agent_identities
        .get("Kimi Code CLI")
        .expect("started ACP agent identity snapshot should be saved");
    assert!(!identity.has_agent_info());
    assert_eq!(identity.agent_capabilities, AgentCapabilities::new());
    assert_eq!(
        model.current_status_line_parts(),
        vec!["Kimi Code CLI".to_string()]
    );
}

#[test]
fn acp_permission_request_flushes_buffered_agent_text() {
    let mut model = Model::new(HeroOptions::default());
    model.transcript_mut().clear();
    let mut acp_runtime = AcpRuntimeState::default();

    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::PromptStarted {
            agent_id: "Kimi Code CLI".to_string(),
        },
    );
    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::AgentMessageChunk {
            agent_id: "Kimi Code CLI".to_string(),
            content: "需要先确认".to_string(),
        },
    );
    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::PermissionRequested {
            agent_id: "Kimi Code CLI".to_string(),
            request: crate::runtime::acp::AcpPermissionRequest {
                request_id: "permission-1".to_string(),
                title: Some("Write file".to_string()),
                options: Vec::new(),
            },
        },
    );

    assert_eq!(
        model.transcript_plain_items(),
        vec!["需要先确认".to_string()]
    );
    assert!(model.current_status_notice_text().is_empty());
    assert!(model.tool_approval_panel_active());

    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::AgentMessageChunk {
            agent_id: "Kimi Code CLI".to_string(),
            content: "确认后继续".to_string(),
        },
    );
    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::PromptResponse {
            agent_id: "Kimi Code CLI".to_string(),
            content: String::new(),
            stop_reason: "EndTurn".to_string(),
        },
    );

    assert_eq!(
        model.transcript_plain_items(),
        vec!["需要先确认".to_string(), "确认后继续".to_string()]
    );
}

#[test]
fn acp_agent_chunks_update_token_activity_without_flushing_transcript() {
    let mut model = Model::new(HeroOptions::default());
    model.set_window(80, 6);
    model.transcript_mut().clear();
    let mut acp_runtime = AcpRuntimeState::default();

    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::PromptStarted {
            agent_id: "Kimi Code CLI".to_string(),
        },
    );
    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::AgentMessageChunk {
            agent_id: "Kimi Code CLI".to_string(),
            content: "hello from acp".to_string(),
        },
    );

    let activity = model
        .current_stream_activity_render_result_at(
            std::time::Instant::now() + std::time::Duration::from_millis(120),
        )
        .plain_line;
    assert!(activity.contains("↓"));
    assert!(activity.contains("tokens"));
    assert!(model.transcript_plain_items().is_empty());
}

#[test]
fn acp_prompt_started_keeps_submitted_activity_status_line() {
    let mut model = Model::new_with_options(
        HeroOptions::default(),
        ModelOptions {
            status_phrases: vec!["Submitted".to_string(), "Started".to_string()],
            status_phrase_order: StatusPhraseOrder::Cycle,
            ..ModelOptions::default()
        },
    );
    model.set_window(80, 6);
    model.show_stream_activity("Kimi Code CLI");
    let before = model.current_stream_activity_render_result().plain_line;
    let mut acp_runtime = AcpRuntimeState::default();

    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::PromptStarted {
            agent_id: "Kimi Code CLI".to_string(),
        },
    );

    let after = model.current_stream_activity_render_result().plain_line;
    assert!(before.contains("Submitted (0s"));
    assert_eq!(after, before);
}

#[test]
fn acp_prompt_response_updates_last_request_metrics() {
    let mut model = Model::new_with_options(
        HeroOptions::default(),
        ModelOptions {
            status_line_items: vec![StatusLineItem::Throughput, StatusLineItem::Latency],
            ..ModelOptions::default()
        },
    );
    let mut acp_runtime = AcpRuntimeState::default();

    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::PromptStarted {
            agent_id: "Kimi Code CLI".to_string(),
        },
    );
    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::AgentMessageChunk {
            agent_id: "Kimi Code CLI".to_string(),
            content: "hello".to_string(),
        },
    );
    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::PromptResponse {
            agent_id: "Kimi Code CLI".to_string(),
            content: String::new(),
            stop_reason: "EndTurn".to_string(),
        },
    );

    let parts = model.current_status_line_parts();
    assert_eq!(parts.len(), 2);
    assert!(parts[0].ends_with("tps"));
    assert!(parts[1].ends_with('s'));
}

#[test]
fn acp_thought_chunks_append_reasoning_and_toggle_activity() {
    let mut model = Model::new_with_options(
        HeroOptions::default(),
        ModelOptions {
            show_reasoning_content: true,
            reasoning_display_mode: ReasoningDisplayMode::Expanded,
            ..ModelOptions::default()
        },
    );
    model.set_window(80, 8);
    model.transcript_mut().clear();
    let mut acp_runtime = AcpRuntimeState::default();

    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::PromptStarted {
            agent_id: "Kimi Code CLI".to_string(),
        },
    );
    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::AgentThoughtChunk {
            agent_id: "Kimi Code CLI".to_string(),
            content: "先分析".to_string(),
        },
    );

    assert!(
        model
            .current_stream_activity_render_result()
            .plain_line
            .contains("thinking")
    );

    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::AgentMessageChunk {
            agent_id: "Kimi Code CLI".to_string(),
            content: "结论".to_string(),
        },
    );
    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::PromptResponse {
            agent_id: "Kimi Code CLI".to_string(),
            content: String::new(),
            stop_reason: "EndTurn".to_string(),
        },
    );

    assert_eq!(
        model.transcript_plain_items(),
        vec![
            "[Hide reasoning · thoughts <1s]\n先分析".to_string(),
            "结论".to_string()
        ]
    );
    assert_eq!(
        model.transcript_mut().source_messages(),
        vec![(Sender::Assistant, "结论".to_string())]
    );
}

#[test]
fn acp_thought_chunks_update_token_activity_like_native_reasoning() {
    let mut model = Model::new(HeroOptions::default());
    model.set_window(80, 8);
    model.transcript_mut().clear();
    let mut acp_runtime = AcpRuntimeState::default();

    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::PromptStarted {
            agent_id: "Kimi Code CLI".to_string(),
        },
    );
    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::AgentThoughtChunk {
            agent_id: "Kimi Code CLI".to_string(),
            content: "先分析这个问题的约束和实现路径。".to_string(),
        },
    );

    let activity = model
        .current_stream_activity_render_result_at(
            std::time::Instant::now() + std::time::Duration::from_millis(120),
        )
        .plain_line;
    assert!(activity.contains("thinking"));
    assert!(activity.contains("↓"));
    assert!(activity.contains("tokens"));
}

#[test]
fn acp_model_config_changed_updates_current_model_status_line_and_models_panel() {
    let mut model = Model::new_with_options(
        HeroOptions::default(),
        ModelOptions {
            status_line_items: vec![StatusLineItem::CurrentModel],
            ..ModelOptions::default()
        },
    );
    model.selected_acp_agent = Some("Kimi Code CLI".to_string());
    let mut acp_runtime = AcpRuntimeState::default();

    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::ModelConfigChanged {
            agent_id: "Kimi Code CLI".to_string(),
            config: crate::runtime::acp::AcpModelConfig {
                config_id: "model".to_string(),
                current_value: "kimi-k2".to_string(),
                current_name: "Kimi K2".to_string(),
                options: vec![crate::runtime::acp::AcpModelOption {
                    value: "kimi-k2".to_string(),
                    name: "Kimi K2".to_string(),
                }],
            },
        },
    );

    assert_eq!(
        model.current_status_line_parts(),
        vec!["Kimi K2".to_string()]
    );
    let provider = model
        .model_catalog
        .enabled_provider_by_id("acp:Kimi Code CLI")
        .expect("ACP provider should replace model catalog");
    assert_eq!(provider.models[0].id, "kimi-k2");
    assert_eq!(
        model.selected_model,
        Some(ModelSelection::new("acp:Kimi Code CLI", "kimi-k2"))
    );
}

#[test]
fn clear_runtime_discards_stale_native_agent_event() {
    let mut model = Model::new(HeroOptions::default());
    model.transcript_mut().clear();
    model.show_stream_activity("qwen3");
    let mut acp_runtime = AcpRuntimeState::default();
    let mut native_agent_runtime = NativeAgentRuntimeState::default();
    let (sender, receiver) = mpsc::channel();
    native_agent_runtime.receiver = Some(receiver);

    sender
        .send(NativeAgentEvent::Finished {
            response: NativeAgentResponse {
                content: "stale response".to_string(),
                reasoning_content: None,
                reasoning_duration: None,
                ..Default::default()
            },
            metrics: None,
        })
        .expect("stale native event should still be produced by worker");
    model.reset_to_initial_tui_state();
    reset_runtime_session_after_clear(
        &mut acp_runtime,
        &mut native_agent_runtime,
        &mut ModelProviderRefreshRuntimeState::default(),
    );
    drain_native_agent_runtime_events(&mut model, &mut native_agent_runtime);

    assert!(
        model
            .transcript_plain_items()
            .iter()
            .all(|item| !item.contains("stale response"))
    );
    assert!(!model.current_stream_activity_render_result().has_content);
}

#[test]
fn clear_runtime_discards_stale_acp_prompt_output_without_exiting_acp_mode() {
    let mut model = Model::new(HeroOptions::default());
    model.transcript_mut().clear();
    model.selected_acp_agent = Some("Kimi Code CLI".to_string());
    let mut acp_runtime = AcpRuntimeState::default();
    let mut native_agent_runtime = NativeAgentRuntimeState::default();

    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::PromptStarted {
            agent_id: "Kimi Code CLI".to_string(),
        },
    );
    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::AgentMessageChunk {
            agent_id: "Kimi Code CLI".to_string(),
            content: "old partial".to_string(),
        },
    );

    model.reset_to_initial_tui_state();
    reset_runtime_session_after_clear(
        &mut acp_runtime,
        &mut native_agent_runtime,
        &mut ModelProviderRefreshRuntimeState::default(),
    );
    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::AgentMessageChunk {
            agent_id: "Kimi Code CLI".to_string(),
            content: " stale response".to_string(),
        },
    );
    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::PromptResponse {
            agent_id: "Kimi Code CLI".to_string(),
            content: " tail".to_string(),
            stop_reason: "EndTurn".to_string(),
        },
    );

    assert_eq!(model.selected_acp_agent(), Some("Kimi Code CLI"));
    assert!(
        model
            .transcript_plain_items()
            .iter()
            .all(|item| !item.contains("old partial") && !item.contains("stale response"))
    );
    assert!(!model.current_stream_activity_render_result().has_content);
}

#[test]
fn clear_runtime_discards_stale_acp_prompt_start_activity() {
    let mut model = Model::new(HeroOptions::default());
    model.transcript_mut().clear();
    model.selected_acp_agent = Some("Kimi Code CLI".to_string());
    let mut acp_runtime = AcpRuntimeState::default();
    let mut native_agent_runtime = NativeAgentRuntimeState::default();

    acp_runtime.mark_prompt_submitted();
    model.reset_to_initial_tui_state();
    reset_runtime_session_after_clear(
        &mut acp_runtime,
        &mut native_agent_runtime,
        &mut ModelProviderRefreshRuntimeState::default(),
    );
    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::PromptStarted {
            agent_id: "Kimi Code CLI".to_string(),
        },
    );

    assert_eq!(model.selected_acp_agent(), Some("Kimi Code CLI"));
    assert!(!model.current_stream_activity_render_result().has_content);
}

#[test]
fn clear_runtime_discards_stale_acp_permission_request() {
    let mut model = Model::new(HeroOptions::default());
    model.transcript_mut().clear();
    model.selected_acp_agent = Some("Kimi Code CLI".to_string());
    let mut acp_runtime = AcpRuntimeState::default();
    let mut native_agent_runtime = NativeAgentRuntimeState::default();

    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::PromptStarted {
            agent_id: "Kimi Code CLI".to_string(),
        },
    );
    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::AgentMessageChunk {
            agent_id: "Kimi Code CLI".to_string(),
            content: "旧请求需要权限".to_string(),
        },
    );

    model.reset_to_initial_tui_state();
    reset_runtime_session_after_clear(
        &mut acp_runtime,
        &mut native_agent_runtime,
        &mut ModelProviderRefreshRuntimeState::default(),
    );
    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::PermissionRequested {
            agent_id: "Kimi Code CLI".to_string(),
            request: crate::runtime::acp::AcpPermissionRequest {
                request_id: "stale-permission".to_string(),
                title: Some("旧请求写文件".to_string()),
                options: Vec::new(),
            },
        },
    );

    assert_eq!(model.selected_acp_agent(), Some("Kimi Code CLI"));
    assert!(model.current_status_notice_text().is_empty());
    assert!(
        model
            .transcript_plain_items()
            .iter()
            .all(|item| !item.contains("旧请求"))
    );
}

#[test]
fn acp_permission_cancel_reject_fallback_uses_reject_always() {
    use crate::runtime::acp::{AcpPermissionOption, AcpPermissionOptionKind, AcpPermissionRequest};

    let option_id = acp_reject_option_id_for_cancel(&AcpPermissionRequest {
        request_id: "permission-session-only".to_string(),
        title: Some("Run command".to_string()),
        options: vec![AcpPermissionOption {
            option_id: "reject-always".to_string(),
            name: "Reject in session".to_string(),
            kind: AcpPermissionOptionKind::RejectAlways,
        }],
    });

    assert_eq!(option_id, Some("reject-always".to_string()));
}

#[test]
fn native_agent_completion_appends_assistant_message_after_request_finishes() {
    let mut model = Model::new(HeroOptions::default());
    model.transcript_mut().clear();
    model.show_stream_activity("qwen3");

    apply_native_agent_event(
        &mut model,
        None,
        NativeAgentEvent::Finished {
            response: NativeAgentResponse {
                content: "你好，我是本地模型".to_string(),
                reasoning_content: None,
                reasoning_duration: None,
                ..Default::default()
            },
            metrics: None,
        },
    );

    assert_eq!(
        model.transcript_plain_items(),
        vec!["你好，我是本地模型".to_string()]
    );
    assert!(!model.current_stream_activity_render_result().has_content);
}

#[test]
fn native_agent_completion_updates_last_request_metrics() {
    let mut model = Model::new_with_options(
        HeroOptions::default(),
        ModelOptions {
            status_line_items: vec![StatusLineItem::Throughput, StatusLineItem::Latency],
            ..ModelOptions::default()
        },
    );

    apply_native_agent_event(
        &mut model,
        None,
        NativeAgentEvent::Finished {
            response: NativeAgentResponse {
                content: "完成".to_string(),
                reasoning_content: None,
                reasoning_duration: None,
                ..Default::default()
            },
            metrics: Some(NativeLlmPerformanceMetrics {
                latency: std::time::Duration::from_millis(250),
                output_tokens: 80,
                duration: std::time::Duration::from_secs(2),
            }),
        },
    );

    assert_eq!(
        model.current_status_line_parts(),
        vec!["40tps".to_string(), "0.25s".to_string()]
    );
}

#[test]
fn native_agent_completion_collapses_reasoning_by_default() {
    let mut model = Model::new_with_options(
        HeroOptions::default(),
        ModelOptions {
            show_reasoning_content: true,
            ..ModelOptions::default()
        },
    );
    model.transcript_mut().clear();
    model.show_stream_activity("qwen3");

    apply_native_agent_event(
        &mut model,
        None,
        NativeAgentEvent::Finished {
            response: NativeAgentResponse {
                content: "结论".to_string(),
                reasoning_content: Some("先分析".to_string()),
                reasoning_duration: Some(std::time::Duration::from_secs(3)),
                ..Default::default()
            },
            metrics: None,
        },
    );

    assert_eq!(
        model.transcript_plain_items(),
        vec![
            "[Show reasoning · thoughts 3s]".to_string(),
            "结论".to_string()
        ]
    );
    assert_eq!(
        model.transcript_mut().source_messages(),
        vec![(Sender::Assistant, "结论".to_string())]
    );
}

#[test]
fn native_agent_completion_keeps_reasoning_body_gap_to_one_line() {
    let mut model = Model::new_with_options(
        HeroOptions::default(),
        ModelOptions {
            show_reasoning_content: true,
            reasoning_display_mode: ReasoningDisplayMode::Expanded,
            ..ModelOptions::default()
        },
    );
    model.transcript_mut().clear();
    model.transcript_mut().set_width(40);
    model.show_stream_activity("qwen3");

    apply_native_agent_event(
        &mut model,
        None,
        NativeAgentEvent::Finished {
            response: NativeAgentResponse {
                content: "结论".to_string(),
                reasoning_content: Some("先分析".to_string()),
                reasoning_duration: Some(std::time::Duration::from_secs(3)),
                ..Default::default()
            },
            metrics: None,
        },
    );

    let render = model.transcript_mut().render();

    assert_eq!(
        render.all_plain_lines(),
        vec!["[Hide reasoning · thoughts 3s]", "先分析", "", "结论"]
    );
}

#[test]
fn native_agent_reasoning_header_click_toggles_visibility_without_changing_source_messages() {
    let mut model = Model::new_with_options(
        HeroOptions::default(),
        ModelOptions {
            show_reasoning_content: true,
            ..ModelOptions::default()
        },
    );
    model.set_palette(crate::frontend::tui::theme::default_palette(), true);
    model.set_window(40, 8);
    model.transcript_mut().clear();

    apply_native_agent_event(
        &mut model,
        None,
        NativeAgentEvent::Finished {
            response: NativeAgentResponse {
                content: "结论".to_string(),
                reasoning_content: Some("先分析".to_string()),
                reasoning_duration: Some(std::time::Duration::from_secs(3)),
                ..Default::default()
            },
            metrics: None,
        },
    );

    assert_eq!(
        model.transcript_plain_items(),
        vec![
            "[Show reasoning · thoughts 3s]".to_string(),
            "结论".to_string()
        ]
    );

    assert!(
        model
            .update(AppEvent::MouseDown {
                button: MouseButton::Left,
                column: 2,
                row: 0,
            })
            .is_none()
    );
    assert!(
        model
            .update(AppEvent::MouseUp {
                button: MouseButton::Left,
                column: 2,
                row: 0,
            })
            .is_none()
    );

    assert_eq!(
        model.transcript_plain_items(),
        vec![
            "[Hide reasoning · thoughts 3s]\n先分析".to_string(),
            "结论".to_string()
        ]
    );
    assert_eq!(
        model.transcript_mut().source_messages(),
        vec![(Sender::Assistant, "结论".to_string())]
    );
}

#[test]
fn native_agent_reasoning_header_drag_does_not_toggle() {
    let mut model = Model::new_with_options(
        HeroOptions::default(),
        ModelOptions {
            show_reasoning_content: true,
            ..ModelOptions::default()
        },
    );
    model.set_palette(crate::frontend::tui::theme::default_palette(), true);
    model.set_window(40, 8);
    model.transcript_mut().clear();

    apply_native_agent_event(
        &mut model,
        None,
        NativeAgentEvent::Finished {
            response: NativeAgentResponse {
                content: "结论".to_string(),
                reasoning_content: Some("先分析".to_string()),
                reasoning_duration: Some(std::time::Duration::from_secs(3)),
                ..Default::default()
            },
            metrics: None,
        },
    );

    assert!(
        model
            .update(AppEvent::MouseDown {
                button: MouseButton::Left,
                column: 2,
                row: 0,
            })
            .is_none()
    );
    assert!(
        model
            .update(AppEvent::MouseDrag {
                button: MouseButton::Left,
                column: 8,
                row: 0,
            })
            .is_none()
    );
    assert!(
        model
            .update(AppEvent::MouseUp {
                button: MouseButton::Left,
                column: 8,
                row: 0,
            })
            .is_none()
    );

    assert_eq!(
        model.transcript_plain_items(),
        vec![
            "[Show reasoning · thoughts 3s]".to_string(),
            "结论".to_string()
        ]
    );
}

#[test]
fn native_agent_reasoning_header_click_outside_label_does_not_toggle() {
    let mut model = Model::new_with_options(
        HeroOptions::default(),
        ModelOptions {
            show_reasoning_content: true,
            ..ModelOptions::default()
        },
    );
    model.set_palette(crate::frontend::tui::theme::default_palette(), true);
    model.set_window(40, 8);
    model.transcript_mut().clear();

    apply_native_agent_event(
        &mut model,
        None,
        NativeAgentEvent::Finished {
            response: NativeAgentResponse {
                content: "结论".to_string(),
                reasoning_content: Some("先分析".to_string()),
                reasoning_duration: Some(std::time::Duration::from_secs(3)),
                ..Default::default()
            },
            metrics: None,
        },
    );

    assert!(
        model
            .update(AppEvent::MouseDown {
                button: MouseButton::Left,
                column: 38,
                row: 0,
            })
            .is_none()
    );
    assert!(
        model
            .update(AppEvent::MouseUp {
                button: MouseButton::Left,
                column: 38,
                row: 0,
            })
            .is_none()
    );

    assert_eq!(
        model.transcript_plain_items(),
        vec![
            "[Show reasoning · thoughts 3s]".to_string(),
            "结论".to_string()
        ]
    );
}

#[test]
fn native_agent_completion_hides_reasoning_when_configured_off() {
    let mut model = Model::new(HeroOptions::default());
    model.transcript_mut().clear();
    model.show_stream_activity("qwen3");

    apply_native_agent_event(
        &mut model,
        None,
        NativeAgentEvent::Finished {
            response: NativeAgentResponse {
                content: "结论".to_string(),
                reasoning_content: Some("先分析".to_string()),
                reasoning_duration: Some(std::time::Duration::from_secs(3)),
                ..Default::default()
            },
            metrics: None,
        },
    );

    assert_eq!(model.transcript_plain_items(), vec!["结论".to_string()]);
}

#[test]
fn native_agent_thinking_event_toggles_activity_segment() {
    let mut model = Model::new(HeroOptions::default());
    model.set_window(80, 6);
    model.transcript_mut().clear();
    model.show_stream_activity("qwen3");

    apply_native_agent_event(
        &mut model,
        None,
        NativeAgentEvent::Thinking { is_thinking: true },
    );

    assert!(
        model
            .current_stream_activity_render_result()
            .plain_line
            .contains("thinking")
    );

    apply_native_agent_event(
        &mut model,
        None,
        NativeAgentEvent::Thinking { is_thinking: false },
    );

    assert!(
        !model
            .current_stream_activity_render_result()
            .plain_line
            .contains("thinking")
    );
}

#[test]
fn native_agent_failure_appends_system_message_in_transcript() {
    let mut model = Model::new(HeroOptions::default());
    model.transcript_mut().clear();
    model.show_stream_activity("qwen3");

    apply_native_agent_event(
        &mut model,
        None,
        NativeAgentEvent::Failed {
            message: "request /v1/chat/completions: connection refused".to_string(),
        },
    );

    assert_eq!(
        model.transcript_plain_items(),
        vec!["■ Chat failed: request /v1/chat/completions: connection refused".to_string()]
    );
    assert!(model.current_status_notice_text().is_empty());
    assert!(!model.current_stream_activity_render_result().has_content);
}

#[test]
fn native_agent_retry_event_shows_reconnecting_activity() {
    let mut model = Model::new(HeroOptions::default());
    model.transcript_mut().clear();
    model.show_stream_activity("qwen3");

    apply_native_agent_event(
        &mut model,
        None,
        NativeAgentEvent::Retrying {
            message: "Reconnecting... 1/3".to_string(),
        },
    );

    let activity = model.current_stream_activity_render_result().plain_line;
    assert!(activity.contains("Reconnecting... 1/3"));
    assert!(model.transcript_plain_items().is_empty());
}

#[test]
fn runtime_request_policy_uses_configured_delay_and_timeout() {
    let policy = RuntimeRequestPolicy::new(5, vec![1, 3, 5, 5, 5], 120);

    assert_eq!(policy.attempts(), 5);
    assert_eq!(policy.delay_for_retry(1), Duration::from_secs(1));
    assert_eq!(policy.delay_for_retry(2), Duration::from_secs(3));
    assert_eq!(policy.delay_for_retry(3), Duration::from_secs(5));
    assert_eq!(policy.delay_for_retry(5), Duration::from_secs(5));
    assert_eq!(policy.timeout(), Duration::from_secs(120));
}

#[test]
fn native_agent_token_estimate_updates_activity_without_finishing_request() {
    let mut model = Model::new(HeroOptions::default());
    model.set_window(70, 6);
    model.transcript_mut().clear();
    model.show_stream_activity("qwen3");

    apply_native_agent_event(
        &mut model,
        None,
        NativeAgentEvent::OutputTokenEstimate { total_tokens: 32 },
    );

    let activity = model
        .current_stream_activity_render_result_at(
            std::time::Instant::now() + std::time::Duration::from_millis(120),
        )
        .plain_line;
    assert!(activity.contains("↓ 32 tokens"));
    assert!(model.current_stream_activity_render_result().has_content);
    assert!(model.transcript_plain_items().is_empty());
}

#[test]
fn native_agent_tool_started_updates_activity_header() {
    let mut model = Model::new(HeroOptions::default());
    model.set_window(70, 6);
    model.transcript_mut().clear();
    model.show_stream_activity("qwen3");

    apply_native_agent_event(
        &mut model,
        Some(RuntimeTarget::native_agent("local", "qwen3")),
        NativeAgentEvent::ToolExecutionStarted {
            call: RuntimeToolCall::new(
                "call-1",
                "file_read",
                serde_json::json!({ "path": "Cargo.toml" }),
            ),
        },
    );

    let activity = model.current_stream_activity_render_result().plain_line;
    assert!(activity.contains("Running file_read Cargo.toml"));
    assert!(model.transcript_plain_items().is_empty());
}

#[test]
fn native_agent_tool_finished_appends_transcript_only_result() {
    let mut model = Model::new(HeroOptions::default());
    model.set_window(70, 6);
    model.transcript_mut().clear();
    model.show_stream_activity("qwen3");

    apply_native_agent_event(
        &mut model,
        Some(RuntimeTarget::native_agent("local", "qwen3")),
        NativeAgentEvent::ToolExecutionFinished {
            call: RuntimeToolCall::new(
                "call-1",
                "file_read",
                serde_json::json!({ "path": "Cargo.toml" }),
            ),
            result: RuntimeToolResult::success("call-1", "1\t[package]"),
        },
    );

    let transcript = model.transcript_plain_items();
    assert_eq!(transcript.len(), 1);
    assert!(transcript[0].contains("file_read Cargo.toml"));
    assert!(model.transcript_mut().source_messages().is_empty());
}

#[test]
fn native_agent_send_effect_starts_native_agent_target() {
    let mut model = Model::new(HeroOptions::default());
    let mut runtime = NativeAgentRuntimeState::default();
    let request = NativeAgentRequest::new(
        "local",
        crate::runtime::native::ProviderKind::OpenAiCompatible,
        "qwen3",
        None,
        None,
        None,
        vec![],
    );

    run_send_native_agent_effect(
        &mut model,
        &mut runtime,
        request,
        RuntimeRequestPolicy::default(),
    );

    assert_eq!(
        runtime.current_target(),
        Some(&RuntimeTarget::native_agent("local", "qwen3"))
    );
}

#[test]
fn native_agent_request_attaches_workspace_tools() {
    let tools = native_agent_workspace_tools();
    let request = NativeAgentRequest::new(
        "local",
        crate::runtime::native::ProviderKind::OpenAiCompatible,
        "qwen3",
        None,
        None,
        None,
        vec![],
    )
    .with_tools(tools.definitions());

    assert_eq!(
        request.target(),
        RuntimeTarget::native_agent("local", "qwen3")
    );
    assert!(request.tools().definition("file_read").is_some());
    assert!(request.tools().definition("list_dir").is_some());
}

#[test]
fn native_agent_runtime_keeps_receiver_after_retry_event() {
    let (sender, receiver) = mpsc::channel();
    let mut runtime = NativeAgentRuntimeState {
        receiver: Some(receiver),
        cancellation: Some(CancellationToken::default()),
        target: Some(RuntimeTarget::native_agent("provider", "model")),
    };

    sender
        .send(NativeAgentEvent::Retrying {
            message: "Reconnecting... 1/3".to_string(),
        })
        .expect("retry event should be queued");

    assert_eq!(
        runtime.try_recv_event(),
        Some(NativeAgentEvent::Retrying {
            message: "Reconnecting... 1/3".to_string(),
        })
    );
    assert!(runtime.is_running());

    sender
        .send(NativeAgentEvent::Finished {
            response: NativeAgentResponse {
                content: "完成".to_string(),
                reasoning_content: None,
                reasoning_duration: None,
                ..Default::default()
            },
            metrics: None,
        })
        .expect("finish event should be queued");

    assert_eq!(
        runtime.try_recv_event(),
        Some(NativeAgentEvent::Finished {
            response: NativeAgentResponse {
                content: "完成".to_string(),
                reasoning_content: None,
                reasoning_duration: None,
                ..Default::default()
            },
            metrics: None,
        })
    );
    assert!(!runtime.is_running());
}

#[test]
fn native_agent_runtime_keeps_receiver_after_token_estimate_event() {
    let (sender, receiver) = mpsc::channel();
    let mut runtime = NativeAgentRuntimeState {
        receiver: Some(receiver),
        cancellation: Some(CancellationToken::default()),
        target: Some(RuntimeTarget::native_agent("provider", "model")),
    };

    sender
        .send(NativeAgentEvent::OutputTokenEstimate { total_tokens: 12 })
        .expect("token estimate event should be queued");

    assert_eq!(
        runtime.try_recv_event(),
        Some(NativeAgentEvent::OutputTokenEstimate { total_tokens: 12 })
    );
    assert!(runtime.is_running());
}

#[test]
fn interrupt_native_agent_clears_runtime_and_appends_system_message() {
    let (_sender, receiver) = mpsc::channel();
    let mut runtime = NativeAgentRuntimeState {
        receiver: Some(receiver),
        cancellation: Some(CancellationToken::default()),
        target: Some(RuntimeTarget::native_agent("provider", "model")),
    };
    let mut model = Model::new(HeroOptions::default());
    model.transcript_mut().clear();
    model.show_stream_activity("qwen3");

    apply_effect_if_needed_for_test(
        &mut model,
        &mut runtime,
        Some(AppEffect::InterruptCurrentTurn),
    );

    assert!(!runtime.is_running());
    assert!(!model.current_stream_activity_render_result().has_content);
    assert_eq!(
        model.transcript_plain_items(),
        vec!["■ Chat interrupted".to_string()]
    );
}

#[test]
fn interrupt_acp_prompt_discards_stale_output_and_keeps_session_selected() {
    let mut model = Model::new(HeroOptions::default());
    model.transcript_mut().clear();
    model.selected_acp_agent = Some("Kimi Code CLI".to_string());
    let mut acp_runtime = AcpRuntimeState::default();

    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::PromptStarted {
            agent_id: "Kimi Code CLI".to_string(),
        },
    );
    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::AgentMessageChunk {
            agent_id: "Kimi Code CLI".to_string(),
            content: "partial before interrupt".to_string(),
        },
    );

    run_interrupt_acp_prompt_effect(&mut model, &mut acp_runtime);

    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::AgentMessageChunk {
            agent_id: "Kimi Code CLI".to_string(),
            content: " stale response".to_string(),
        },
    );
    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::PromptResponse {
            agent_id: "Kimi Code CLI".to_string(),
            content: " tail".to_string(),
            stop_reason: "EndTurn".to_string(),
        },
    );

    assert_eq!(model.selected_acp_agent(), Some("Kimi Code CLI"));
    assert!(!model.current_stream_activity_render_result().has_content);
    assert_eq!(
        model.transcript_plain_items(),
        vec!["■ Chat interrupted".to_string()]
    );
}

#[test]
fn ready_input_batch_coalesces_wheel_burst_before_key() {
    let events = (0..128)
        .map(|_| {
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::ScrollUp,
                column: 0,
                row: 0,
                modifiers: KeyModifiers::empty(),
            })
        })
        .chain(std::iter::once(Event::Key(KeyEvent::from(KeyCode::Char(
            'x',
        )))))
        .collect::<Vec<_>>();

    let actions = coalesced_input_actions(events);

    assert_eq!(actions.len(), 2);
    assert_eq!(
        actions[0],
        TerminalInputAction::App(AppEvent::MouseWheel {
            delta_lines: -128 * Model::document_mouse_wheel_delta(),
        })
    );
    assert_eq!(
        actions[1],
        TerminalInputAction::App(AppEvent::Key(KeyEvent::from(KeyCode::Char('x'))))
    );
}

fn apply_effect_if_needed_for_test(
    model: &mut Model,
    native_agent_runtime: &mut NativeAgentRuntimeState,
    effect: Option<AppEffect>,
) {
    if let Some(AppEffect::InterruptCurrentTurn) = effect {
        run_interrupt_current_turn_effect(
            model,
            &mut AcpRuntimeState::default(),
            native_agent_runtime,
        );
    }
}
