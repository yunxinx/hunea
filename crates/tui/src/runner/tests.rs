use std::time::Duration;

use super::acp_session::{
    AcpRuntimeState, acp_reject_option_id_for_stale_discard, apply_acp_session_event,
    run_interrupt_acp_prompt_effect, run_respond_acp_permission_effect, run_set_acp_model_effect,
};
use super::effects::{reset_runtime_session_after_clear, run_interrupt_current_turn_effect};
use super::input::{TerminalInputAction, coalesced_input_actions};
use super::native_agent::{apply_native_agent_event, run_send_native_agent_effect};
use super::*;
use crate::{AppEffect, AppEvent, ReasoningDisplayMode, Sender, StatusLineItem};
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use mo_core::acp::{
    AcpAgentCapabilities, AcpAvailableCommand, AcpAvailableCommandInput, AcpInitializeOutcome,
    AcpPromptCapabilities, AcpProtocolVersion, AcpSessionEvent, AcpTerminalExitStatus,
    AcpTerminalSnapshot, AcpToolCall, AcpToolCallContent, AcpToolCallLocation, AcpToolCallStatus,
    AcpToolCallUpdate, AcpToolKind,
};
use mo_core::model_catalog::{ModelSelection, ProviderSyncRequest};
use mo_core::phrases::StatusPhraseOrder;
use mo_core::provider::ProviderKind;
use mo_core::request_policy::RuntimeRequestPolicy;
use mo_core::session::{
    NativeAgentEvent, NativeAgentRequest, NativeAgentResponse, NativeLlmPerformanceMetrics,
    RuntimeTarget,
};
use mo_core::tools::{RuntimeToolCall, RuntimeToolResult};

#[derive(Default)]
struct TestRuntimeDriver {
    native_events: Vec<NativeAgentRuntimeEvent>,
    native_running: bool,
    native_interrupted: bool,
    native_request: Option<NativeAgentRequest>,
    reset_count: usize,
}

impl RuntimeDriver for TestRuntimeDriver {
    fn drain_native_agent_events(&mut self) -> Vec<NativeAgentRuntimeEvent> {
        std::mem::take(&mut self.native_events)
    }

    fn reset_runtime_session(&mut self) {
        self.native_events.clear();
        self.native_running = false;
        self.reset_count += 1;
    }

    fn send_native_agent(&mut self, request: NativeAgentRequest) -> Result<String, String> {
        if self.native_running {
            return Err("Chat request is already running".to_string());
        }

        let activity_label = request.llm_request().model_id.clone();
        self.native_running = true;
        self.native_request = Some(request);
        Ok(activity_label)
    }

    fn interrupt_native_agent(&mut self) -> bool {
        if !self.native_running {
            return false;
        }

        self.native_running = false;
        self.native_interrupted = true;
        true
    }

    fn refresh_model_provider(&mut self, _request: ProviderSyncRequest) -> Result<(), String> {
        Ok(())
    }
}

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
            message: mo_core::acp::debug_protocol_version_system_message(),
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
fn acp_tool_call_update_replaces_same_transcript_item() {
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
        AcpSessionEvent::ToolCall {
            agent_id: "Kimi Code CLI".to_string(),
            call: AcpToolCall {
                tool_call_id: "call-1".to_string(),
                title: "Read Cargo.toml".to_string(),
                kind: AcpToolKind::Read,
                status: AcpToolCallStatus::Pending,
                content: Vec::new(),
                locations: vec![AcpToolCallLocation {
                    path: "Cargo.toml".to_string(),
                    line: Some(1),
                }],
                raw_input: Some("{\"path\":\"Cargo.toml\"}".into()),
                raw_output: None,
            },
        },
    );

    assert_eq!(model.transcript_mut().len(), 1);
    assert_eq!(
        model.transcript_plain_items(),
        vec!["● Read Cargo.toml".to_string()]
    );

    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::ToolCallUpdate {
            agent_id: "Kimi Code CLI".to_string(),
            update: AcpToolCallUpdate {
                tool_call_id: "call-1".to_string(),
                title: None,
                kind: None,
                status: Some(AcpToolCallStatus::Completed),
                content: Some(vec![AcpToolCallContent::Text("read complete".to_string())]),
                locations: None,
                raw_input: None,
                raw_output: Some("{\"ok\":true}".into()),
            },
        },
    );

    assert_eq!(
        model.transcript_mut().len(),
        1,
        "tool_call_update should replace the existing item instead of appending"
    );
    let plain = model.transcript_plain_items().join("\n");
    assert_eq!(plain, "● Read Cargo.toml");
    assert!(!plain.contains("read complete"));
    assert!(!plain.contains("{\"ok\":true}"));
    assert!(!plain.contains("Pending [Read]"));
}

#[test]
fn acp_tool_call_update_without_create_appends_transcript_item() {
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
        AcpSessionEvent::ToolCallUpdate {
            agent_id: "Kimi Code CLI".to_string(),
            update: AcpToolCallUpdate {
                tool_call_id: "call-orphan".to_string(),
                title: Some("Search symbols".to_string()),
                kind: Some(AcpToolKind::Search),
                status: Some(AcpToolCallStatus::InProgress),
                content: Some(vec![AcpToolCallContent::Text("scanning src".to_string())]),
                locations: None,
                raw_input: None,
                raw_output: None,
            },
        },
    );

    assert_eq!(model.transcript_mut().len(), 1);
    let plain = model.transcript_plain_items().join("\n");
    assert!(
        plain.contains("Search symbols"),
        "orphan tool_call_update should create a visible transcript row: {plain}"
    );
}

#[test]
fn acp_tool_call_create_after_update_updates_existing_item() {
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
        AcpSessionEvent::ToolCallUpdate {
            agent_id: "Kimi Code CLI".to_string(),
            update: AcpToolCallUpdate {
                tool_call_id: "call-reordered".to_string(),
                title: Some("Preparing edit".to_string()),
                kind: Some(AcpToolKind::Edit),
                status: Some(AcpToolCallStatus::Pending),
                content: None,
                locations: None,
                raw_input: None,
                raw_output: None,
            },
        },
    );
    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::ToolCall {
            agent_id: "Kimi Code CLI".to_string(),
            call: AcpToolCall {
                tool_call_id: "call-reordered".to_string(),
                title: "Edit Cargo.toml".to_string(),
                kind: AcpToolKind::Edit,
                status: AcpToolCallStatus::InProgress,
                content: Vec::new(),
                locations: vec![AcpToolCallLocation {
                    path: "Cargo.toml".to_string(),
                    line: Some(2),
                }],
                raw_input: Some(r#"{"path":"Cargo.toml"}"#.into()),
                raw_output: None,
            },
        },
    );

    assert_eq!(
        model.transcript_mut().len(),
        1,
        "tool_call arriving after tool_call_update should update the existing row"
    );
    let plain = model.transcript_plain_items().join("\n");
    assert!(
        plain.contains("Cargo.toml"),
        "expected real title/location: {plain}"
    );
    assert!(
        !plain.contains("Preparing edit"),
        "create event should replace provisional update title: {plain}"
    );
}

#[test]
fn acp_late_tool_call_update_after_completion_updates_existing_item() {
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
        AcpSessionEvent::ToolCall {
            agent_id: "Kimi Code CLI".to_string(),
            call: AcpToolCall {
                tool_call_id: "call-late".to_string(),
                title: "Run cargo test".to_string(),
                kind: AcpToolKind::Execute,
                status: AcpToolCallStatus::InProgress,
                content: Vec::new(),
                locations: Vec::new(),
                raw_input: None,
                raw_output: None,
            },
        },
    );
    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::ToolCallUpdate {
            agent_id: "Kimi Code CLI".to_string(),
            update: AcpToolCallUpdate {
                tool_call_id: "call-late".to_string(),
                title: None,
                kind: None,
                status: Some(AcpToolCallStatus::Completed),
                content: None,
                locations: None,
                raw_input: None,
                raw_output: None,
            },
        },
    );
    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::ToolCallUpdate {
            agent_id: "Kimi Code CLI".to_string(),
            update: AcpToolCallUpdate {
                tool_call_id: "call-late".to_string(),
                title: None,
                kind: None,
                status: None,
                content: None,
                locations: None,
                raw_input: None,
                raw_output: Some("test result arrived late".into()),
            },
        },
    );

    assert_eq!(
        model.transcript_mut().len(),
        1,
        "late update after completed status should not append a duplicate row"
    );
    let plain = model.transcript_plain_items().join("\n");
    assert!(
        plain.contains("test result arrived late"),
        "late raw output should update the completed row: {plain}"
    );
}

#[test]
fn acp_execute_tool_call_lifecycle_defers_content_until_completed() {
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
        AcpSessionEvent::ToolCall {
            agent_id: "Kimi Code CLI".to_string(),
            call: AcpToolCall {
                tool_call_id: "call-exec".to_string(),
                title: "Shell: cargo check".to_string(),
                kind: AcpToolKind::Execute,
                status: AcpToolCallStatus::Pending,
                content: vec![AcpToolCallContent::Text(
                    "Requesting approval to perform: Run command `cargo check`".to_string(),
                )],
                locations: Vec::new(),
                raw_input: Some(r#"{"command":"cargo check"}"#.into()),
                raw_output: None,
            },
        },
    );

    let pending_plain = model.transcript_plain_items().join("\n");
    assert!(
        pending_plain.contains("Waiting..."),
        "pending command should render as waiting: {pending_plain}"
    );
    assert!(
        !pending_plain.contains("Requesting approval")
            && !pending_plain.contains(r#"{"command":"cargo check"}"#),
        "pending command should not expose approval copy or raw input: {pending_plain}"
    );

    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::ToolCallUpdate {
            agent_id: "Kimi Code CLI".to_string(),
            update: AcpToolCallUpdate {
                tool_call_id: "call-exec".to_string(),
                title: None,
                kind: None,
                status: Some(AcpToolCallStatus::InProgress),
                content: Some(vec![AcpToolCallContent::Text(
                    "Checking lumos v0.1.0".to_string(),
                )]),
                locations: None,
                raw_input: None,
                raw_output: Some("streamed output should wait".into()),
            },
        },
    );

    let in_progress_plain = model.transcript_plain_items().join("\n");
    assert!(
        in_progress_plain.contains("Waiting..."),
        "running command should still render as waiting: {in_progress_plain}"
    );
    assert!(
        !in_progress_plain.contains("Checking lumos")
            && !in_progress_plain.contains("streamed output should wait"),
        "running command output should not stream into the transcript: {in_progress_plain}"
    );

    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::ToolCallUpdate {
            agent_id: "Kimi Code CLI".to_string(),
            update: AcpToolCallUpdate {
                tool_call_id: "call-exec".to_string(),
                title: None,
                kind: None,
                status: Some(AcpToolCallStatus::Completed),
                content: None,
                locations: None,
                raw_input: None,
                raw_output: Some("Finished dev profile".into()),
            },
        },
    );

    let completed_plain = model.transcript_plain_items().join("\n");
    assert!(
        completed_plain.contains("Finished dev profile"),
        "completed command should reveal final output: {completed_plain}"
    );
    assert!(
        !completed_plain.contains("Waiting...")
            && !completed_plain.contains("Requesting approval")
            && !completed_plain.contains("Checking lumos")
            && !completed_plain.contains("Input:"),
        "completed command should not replay deferred transport details: {completed_plain}"
    );
}

#[test]
fn acp_execute_tool_call_failed_reveals_final_output_without_input() {
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
        AcpSessionEvent::ToolCallUpdate {
            agent_id: "Kimi Code CLI".to_string(),
            update: AcpToolCallUpdate {
                tool_call_id: "call-exec".to_string(),
                title: Some("Shell: cargo check".to_string()),
                kind: Some(AcpToolKind::Execute),
                status: Some(AcpToolCallStatus::Pending),
                content: Some(vec![AcpToolCallContent::Text(
                    "Requesting approval to perform: Run command `cargo check`".to_string(),
                )]),
                locations: None,
                raw_input: Some(r#"{"command":"cargo check"}"#.into()),
                raw_output: None,
            },
        },
    );
    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::ToolCallUpdate {
            agent_id: "Kimi Code CLI".to_string(),
            update: AcpToolCallUpdate {
                tool_call_id: "call-exec".to_string(),
                title: None,
                kind: None,
                status: Some(AcpToolCallStatus::Failed),
                content: None,
                locations: None,
                raw_input: None,
                raw_output: Some("error: could not compile `lumos`".into()),
            },
        },
    );

    let plain = model.transcript_plain_items().join("\n");
    assert!(
        plain.contains("error: could not compile `lumos`"),
        "failed command should reveal final output: {plain}"
    );
    assert!(
        !plain.contains("Requesting approval")
            && !plain.contains(r#"{"command":"cargo check"}"#)
            && !plain.contains("Input:"),
        "failed command should not reveal approval copy or raw transport input: {plain}"
    );
    assert_eq!(
        model.transcript_mut().len(),
        1,
        "execute lifecycle updates should upsert one transcript row"
    );
}

#[test]
fn acp_write_tool_call_stream_updates_token_activity() {
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
        AcpSessionEvent::ToolCall {
            agent_id: "Kimi Code CLI".to_string(),
            call: AcpToolCall {
                tool_call_id: "write-1".to_string(),
                title: "WriteFile: TEMP.md".to_string(),
                kind: AcpToolKind::Other,
                status: AcpToolCallStatus::InProgress,
                content: Vec::new(),
                locations: Vec::new(),
                raw_input: Some(
                    r##"{"path":"TEMP.md","content":"# TEMP\n\n正在写入一段较长的测试内容"}"##
                        .into(),
                ),
                raw_output: None,
            },
        },
    );

    let activity = model
        .current_stream_activity_render_result_at(
            std::time::Instant::now() + Duration::from_millis(120),
        )
        .plain_line;
    assert!(
        activity.contains("tokens"),
        "streaming write tool content should update token activity: {activity:?}"
    );
}

#[test]
fn acp_interrupt_marks_active_tool_calls_failed() {
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
        AcpSessionEvent::ToolCall {
            agent_id: "Kimi Code CLI".to_string(),
            call: AcpToolCall {
                tool_call_id: "call-1".to_string(),
                title: "Run tests".to_string(),
                kind: AcpToolKind::Execute,
                status: AcpToolCallStatus::InProgress,
                content: Vec::new(),
                locations: Vec::new(),
                raw_input: None,
                raw_output: None,
            },
        },
    );

    run_interrupt_acp_prompt_effect(
        &mut model,
        &mut acp_runtime,
        &mut TestRuntimeDriver::default(),
    );

    let plain = model.transcript_plain_items().join("\n");
    assert!(plain.contains("● Run tests"));
    assert!(plain.contains("  └─ Interrupted"));
    assert!(plain.contains("Interrupted"));
}

#[test]
fn acp_prompt_failed_marks_active_tool_calls_failed() {
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
        AcpSessionEvent::ToolCall {
            agent_id: "Kimi Code CLI".to_string(),
            call: AcpToolCall {
                tool_call_id: "call-1".to_string(),
                title: "Run tests".to_string(),
                kind: AcpToolKind::Execute,
                status: AcpToolCallStatus::InProgress,
                content: Vec::new(),
                locations: Vec::new(),
                raw_input: None,
                raw_output: None,
            },
        },
    );

    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::PromptFailed {
            agent_id: "Kimi Code CLI".to_string(),
            message: "transport closed".to_string(),
        },
    );

    let plain = model.transcript_plain_items().join("\n");
    assert!(plain.contains("● Run tests"));
    assert!(
        plain.contains("Tool call ended because the ACP prompt failed"),
        "unfinished tool call should surface why it stopped: {plain}"
    );
    assert!(
        model
            .transcript_mut()
            .active_tool_activity_started_at()
            .is_none(),
        "failed prompt should not leave active tool animations scheduled"
    );
}

#[test]
fn acp_prompt_interrupted_event_marks_active_tool_calls_failed() {
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
        AcpSessionEvent::ToolCall {
            agent_id: "Kimi Code CLI".to_string(),
            call: AcpToolCall {
                tool_call_id: "call-1".to_string(),
                title: "Run tests".to_string(),
                kind: AcpToolKind::Execute,
                status: AcpToolCallStatus::InProgress,
                content: Vec::new(),
                locations: Vec::new(),
                raw_input: None,
                raw_output: None,
            },
        },
    );

    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::PromptInterrupted {
            agent_id: "Kimi Code CLI".to_string(),
        },
    );

    let plain = model.transcript_plain_items().join("\n");
    assert!(plain.contains("● Run tests"));
    assert!(plain.contains("  └─ Interrupted"));
    assert!(
        model
            .transcript_mut()
            .active_tool_activity_started_at()
            .is_none(),
        "interrupted prompt event should not leave active tool animations scheduled"
    );
}

#[test]
fn acp_prompt_response_marks_unfinished_tool_calls_without_final_status() {
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
        AcpSessionEvent::ToolCall {
            agent_id: "Kimi Code CLI".to_string(),
            call: AcpToolCall {
                tool_call_id: "call-1".to_string(),
                title: "Run tests".to_string(),
                kind: AcpToolKind::Execute,
                status: AcpToolCallStatus::InProgress,
                content: Vec::new(),
                locations: Vec::new(),
                raw_input: None,
                raw_output: None,
            },
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

    let plain = model.transcript_plain_items().join("\n");
    assert!(plain.contains("● Run tests"));
    assert!(
        plain.contains("Tool call ended without final status"),
        "unfinished tool call should be finalized when the prompt ends: {plain}"
    );
    assert!(
        model
            .transcript_mut()
            .active_tool_activity_started_at()
            .is_none(),
        "completed prompt should not leave active tool animations scheduled"
    );
}

#[test]
fn acp_stopped_marks_active_tool_calls_failed() {
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
        AcpSessionEvent::ToolCall {
            agent_id: "Kimi Code CLI".to_string(),
            call: AcpToolCall {
                tool_call_id: "call-1".to_string(),
                title: "Run tests".to_string(),
                kind: AcpToolKind::Execute,
                status: AcpToolCallStatus::InProgress,
                content: Vec::new(),
                locations: Vec::new(),
                raw_input: None,
                raw_output: None,
            },
        },
    );

    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::Stopped {
            agent_id: "Kimi Code CLI".to_string(),
            message: None,
        },
    );

    let plain = model.transcript_plain_items().join("\n");
    assert!(plain.contains("● Run tests"));
    assert!(
        plain.contains("Tool call ended because the ACP session stopped"),
        "unfinished tool call should explain that the session stopped: {plain}"
    );
    assert!(
        model
            .transcript_mut()
            .active_tool_activity_started_at()
            .is_none(),
        "stopped ACP session should not leave active tool animations scheduled"
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
                protocol_version: AcpProtocolVersion::V1,
                agent_name: Some("kimi".to_string()),
                agent_title: Some("Kimi Code CLI".to_string()),
                agent_version: Some("1.39.0".to_string()),
                agent_capabilities: AcpAgentCapabilities {
                    load_session: true,
                    prompt_capabilities: AcpPromptCapabilities {
                        image: true,
                        ..AcpPromptCapabilities::default()
                    },
                },
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
                protocol_version: AcpProtocolVersion::V1,
                agent_name: None,
                agent_title: None,
                agent_version: None,
                agent_capabilities: AcpAgentCapabilities::default(),
                auth_method_count: 0,
            },
        },
    );

    let identity = model
        .acp_agent_identities
        .get("Kimi Code CLI")
        .expect("started ACP agent identity snapshot should be saved");
    assert!(!identity.has_agent_info());
    assert_eq!(identity.agent_capabilities, AcpAgentCapabilities::default());
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
            request: mo_core::acp::AcpPermissionRequest {
                request_id: "permission-1".to_string(),
                title: Some("Write file".to_string()),
                tool_call: AcpToolCallUpdate {
                    tool_call_id: "tool-1".to_string(),
                    title: Some("Write file".to_string()),
                    kind: None,
                    status: None,
                    content: None,
                    locations: None,
                    raw_input: None,
                    raw_output: None,
                },
                options: Vec::new(),
            },
        },
    );

    let plain = model.transcript_plain_items().join("\n");
    assert!(plain.contains("需要先确认"));
    assert!(
        plain.contains("Write file"),
        "permission request should upsert the pending tool call row: {plain}"
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

    let plain = model.transcript_plain_items().join("\n");
    assert!(plain.contains("需要先确认"));
    assert!(plain.contains("Write file"));
    assert!(plain.contains("确认后继续"));
}

#[test]
fn acp_terminal_update_renders_embedded_terminal_output() {
    let mut model = Model::new(HeroOptions::default());
    model.transcript_mut().clear();
    let mut acp_runtime = AcpRuntimeState::default();

    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::ToolCall {
            agent_id: "Kimi Code CLI".to_string(),
            call: AcpToolCall {
                tool_call_id: "tool-terminal".to_string(),
                title: "cargo check".to_string(),
                kind: AcpToolKind::Execute,
                status: AcpToolCallStatus::InProgress,
                content: vec![AcpToolCallContent::Terminal {
                    terminal_id: "term-1".to_string(),
                }],
                locations: Vec::new(),
                raw_input: None,
                raw_output: None,
            },
        },
    );
    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::TerminalUpdated {
            agent_id: "Kimi Code CLI".to_string(),
            snapshot: AcpTerminalSnapshot {
                terminal_id: "term-1".to_string(),
                command: Some("cargo check".to_string()),
                cwd: None,
                output: "Checking lumos\nFinished".to_string(),
                truncated: false,
                exit_status: Some(AcpTerminalExitStatus {
                    exit_code: Some(0),
                    signal: None,
                }),
                released: false,
            },
        },
    );

    let transcript = model.transcript_plain_items().join("\n");
    assert!(transcript.contains("cargo check"));
    assert!(transcript.contains("Checking lumos"));
    assert!(transcript.contains("Finished"));
    assert!(transcript.contains("Exited with code 0"));
    assert!(!transcript.contains("terminal/create unsupported"));
}

#[test]
fn acp_prompt_response_keeps_running_terminal_tool_call_active() {
    let mut model = Model::new(HeroOptions::default());
    model.transcript_mut().clear();
    let mut acp_runtime = AcpRuntimeState::default();

    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::ToolCall {
            agent_id: "Kimi Code CLI".to_string(),
            call: AcpToolCall {
                tool_call_id: "tool-dev-server".to_string(),
                title: "npm run dev".to_string(),
                kind: AcpToolKind::Execute,
                status: AcpToolCallStatus::InProgress,
                content: vec![AcpToolCallContent::Terminal {
                    terminal_id: "term-dev".to_string(),
                }],
                locations: Vec::new(),
                raw_input: None,
                raw_output: None,
            },
        },
    );
    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::TerminalUpdated {
            agent_id: "Kimi Code CLI".to_string(),
            snapshot: AcpTerminalSnapshot {
                terminal_id: "term-dev".to_string(),
                command: Some("npm run dev".to_string()),
                cwd: Some("/tmp/project".to_string()),
                output: "ready on http://localhost:3000\n".to_string(),
                truncated: false,
                exit_status: None,
                released: false,
            },
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

    let transcript = model.transcript_plain_items().join("\n");
    assert!(transcript.contains("npm run dev"));
    assert!(transcript.contains("Running..."));
    assert!(transcript.contains("ready on http://localhost:3000"));
    assert!(!transcript.contains("Tool call ended without final status"));
    assert!(
        model
            .transcript_mut()
            .active_tool_activity_started_at()
            .is_some(),
        "running background terminal should keep tool activity animation alive"
    );

    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::TerminalUpdated {
            agent_id: "Kimi Code CLI".to_string(),
            snapshot: AcpTerminalSnapshot {
                terminal_id: "term-dev".to_string(),
                command: Some("npm run dev".to_string()),
                cwd: Some("/tmp/project".to_string()),
                output: "ready on http://localhost:3000\nstopped\n".to_string(),
                truncated: false,
                exit_status: Some(AcpTerminalExitStatus {
                    exit_code: Some(137),
                    signal: None,
                }),
                released: false,
            },
        },
    );
    assert!(
        model
            .transcript_mut()
            .active_tool_activity_started_at()
            .is_none(),
        "exited background terminal should not keep tool activity animation alive"
    );
}

#[test]
fn acp_prompt_response_keeps_terminal_tool_call_active_before_first_snapshot() {
    let mut model = Model::new(HeroOptions::default());
    model.transcript_mut().clear();
    let mut acp_runtime = AcpRuntimeState::default();

    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::ToolCall {
            agent_id: "Kimi Code CLI".to_string(),
            call: AcpToolCall {
                tool_call_id: "tool-dev-server".to_string(),
                title: "npm run dev".to_string(),
                kind: AcpToolKind::Execute,
                status: AcpToolCallStatus::InProgress,
                content: vec![AcpToolCallContent::Terminal {
                    terminal_id: "term-dev".to_string(),
                }],
                locations: Vec::new(),
                raw_input: None,
                raw_output: None,
            },
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

    let transcript = model.transcript_plain_items().join("\n");
    assert!(transcript.contains("npm run dev"));
    assert!(transcript.contains("Waiting..."));
    assert!(!transcript.contains("Tool call ended without final status"));
    assert!(
        model
            .transcript_mut()
            .active_tool_activity_started_at()
            .is_some(),
        "terminal tool activity should remain active until the first terminal snapshot arrives"
    );

    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::TerminalUpdated {
            agent_id: "Kimi Code CLI".to_string(),
            snapshot: AcpTerminalSnapshot {
                terminal_id: "term-dev".to_string(),
                command: Some("npm run dev".to_string()),
                cwd: Some("/tmp/project".to_string()),
                output: "stopped\n".to_string(),
                truncated: false,
                exit_status: Some(AcpTerminalExitStatus {
                    exit_code: Some(0),
                    signal: None,
                }),
                released: false,
            },
        },
    );
    assert!(
        model
            .transcript_mut()
            .active_tool_activity_started_at()
            .is_none(),
        "terminal snapshot with exit status should clear the active marker"
    );
}

#[test]
fn acp_permission_request_without_prior_tool_call_appends_transcript_item() {
    let mut model = Model::new(HeroOptions::default());
    model.set_window(80, 12);
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
        AcpSessionEvent::PermissionRequested {
            agent_id: "Kimi Code CLI".to_string(),
            request: mo_core::acp::AcpPermissionRequest {
                request_id: "permission-exec".to_string(),
                title: Some("Run cargo test".to_string()),
                tool_call: AcpToolCallUpdate {
                    tool_call_id: "tool-exec".to_string(),
                    title: Some("Run cargo test".to_string()),
                    kind: Some(AcpToolKind::Execute),
                    status: Some(AcpToolCallStatus::Pending),
                    content: None,
                    locations: None,
                    raw_input: Some(r#"{"command":"cargo test"}"#.into()),
                    raw_output: None,
                },
                options: vec![
                    mo_core::acp::AcpPermissionOption {
                        option_id: "allow-once".to_string(),
                        name: "Allow once".to_string(),
                        kind: mo_core::acp::AcpPermissionOptionKind::AllowOnce,
                    },
                    mo_core::acp::AcpPermissionOption {
                        option_id: "reject-once".to_string(),
                        name: "Reject".to_string(),
                        kind: mo_core::acp::AcpPermissionOptionKind::RejectOnce,
                    },
                ],
            },
        },
    );

    assert!(model.tool_approval_panel_active());
    assert_eq!(model.transcript_mut().len(), 1);
    let plain = model.transcript_plain_items().join("\n");
    assert!(
        plain.contains("Run cargo test"),
        "permission request should upsert its tool call into transcript: {plain}"
    );
}

#[test]
fn acp_permission_request_content_renders_waiting_instead_of_approval_text() {
    let mut model = Model::new(HeroOptions::default());
    model.set_window(80, 12);
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
        AcpSessionEvent::PermissionRequested {
            agent_id: "Kimi Code CLI".to_string(),
            request: mo_core::acp::AcpPermissionRequest {
                request_id: "permission-exec".to_string(),
                title: Some("cargo check".to_string()),
                tool_call: AcpToolCallUpdate {
                    tool_call_id: "tool-exec".to_string(),
                    title: Some("cargo check".to_string()),
                    kind: Some(AcpToolKind::Other),
                    status: Some(AcpToolCallStatus::Pending),
                    content: Some(vec![AcpToolCallContent::Text(
                        "Requesting approval to perform: Run command `cargo check`".to_string(),
                    )]),
                    locations: None,
                    raw_input: Some(r#"{"command":"cargo check"}"#.into()),
                    raw_output: None,
                },
                options: vec![
                    mo_core::acp::AcpPermissionOption {
                        option_id: "allow-once".to_string(),
                        name: "Allow once".to_string(),
                        kind: mo_core::acp::AcpPermissionOptionKind::AllowOnce,
                    },
                    mo_core::acp::AcpPermissionOption {
                        option_id: "reject-once".to_string(),
                        name: "Reject".to_string(),
                        kind: mo_core::acp::AcpPermissionOptionKind::RejectOnce,
                    },
                ],
            },
        },
    );

    assert!(model.tool_approval_panel_active());
    let plain = model.transcript_plain_items().join("\n");
    assert!(
        plain.contains("cargo check"),
        "permission tool call title should remain visible: {plain}"
    );
    assert!(
        plain.contains("Waiting..."),
        "permission tool call should show a compact waiting row: {plain}"
    );
    assert!(
        !plain.contains("Requesting approval to perform"),
        "main transcript should not duplicate the approval panel request copy: {plain}"
    );
    assert!(
        !plain.contains("Input:") && !plain.contains(r#"{"command":"cargo check"}"#),
        "pending approval rows should not expose raw input while the approval panel is active: {plain}"
    );
}

#[test]
fn acp_permission_request_in_progress_command_renders_waiting_instead_of_approval_text() {
    let mut model = Model::new(HeroOptions::default());
    model.set_window(80, 12);
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
        AcpSessionEvent::PermissionRequested {
            agent_id: "Kimi Code CLI".to_string(),
            request: mo_core::acp::AcpPermissionRequest {
                request_id: "permission-exec".to_string(),
                title: Some("cargo check".to_string()),
                tool_call: AcpToolCallUpdate {
                    tool_call_id: "tool-exec".to_string(),
                    title: Some("cargo check".to_string()),
                    kind: Some(AcpToolKind::Execute),
                    status: Some(AcpToolCallStatus::InProgress),
                    content: Some(vec![AcpToolCallContent::Text(
                        "Requesting approval to perform: Run command `cargo check`".to_string(),
                    )]),
                    locations: None,
                    raw_input: Some(r#"{"command":"cargo check"}"#.into()),
                    raw_output: None,
                },
                options: vec![
                    mo_core::acp::AcpPermissionOption {
                        option_id: "allow-once".to_string(),
                        name: "Allow once".to_string(),
                        kind: mo_core::acp::AcpPermissionOptionKind::AllowOnce,
                    },
                    mo_core::acp::AcpPermissionOption {
                        option_id: "reject-once".to_string(),
                        name: "Reject".to_string(),
                        kind: mo_core::acp::AcpPermissionOptionKind::RejectOnce,
                    },
                ],
            },
        },
    );

    assert!(model.tool_approval_panel_active());
    let plain = model.transcript_plain_items().join("\n");
    assert!(
        plain.contains("cargo check"),
        "permission tool call title should remain visible: {plain}"
    );
    assert!(
        plain.contains("Waiting..."),
        "permission command should show only a compact waiting row while approval is active: {plain}"
    );
    assert!(
        !plain.contains("Requesting approval to perform")
            && !plain.contains(r#"{"command":"cargo check"}"#),
        "main transcript should not duplicate approval copy or raw command input: {plain}"
    );
}

#[test]
fn acp_command_permission_request_does_not_replay_approval_text_after_completion() {
    let mut model = Model::new(HeroOptions::default());
    model.set_window(80, 12);
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
        AcpSessionEvent::PermissionRequested {
            agent_id: "Kimi Code CLI".to_string(),
            request: mo_core::acp::AcpPermissionRequest {
                request_id: "permission-exec".to_string(),
                title: Some("cargo check".to_string()),
                tool_call: AcpToolCallUpdate {
                    tool_call_id: "tool-exec".to_string(),
                    title: Some("cargo check".to_string()),
                    kind: Some(AcpToolKind::Execute),
                    status: Some(AcpToolCallStatus::InProgress),
                    content: Some(vec![AcpToolCallContent::Text(
                        "Requesting approval to perform: Run command `cargo check`".to_string(),
                    )]),
                    locations: None,
                    raw_input: Some(r#"{"command":"cargo check"}"#.into()),
                    raw_output: None,
                },
                options: Vec::new(),
            },
        },
    );
    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::ToolCallUpdate {
            agent_id: "Kimi Code CLI".to_string(),
            update: AcpToolCallUpdate {
                tool_call_id: "tool-exec".to_string(),
                title: None,
                kind: None,
                status: Some(AcpToolCallStatus::Completed),
                content: None,
                locations: None,
                raw_input: None,
                raw_output: Some("Finished dev profile".into()),
            },
        },
    );

    let plain = model.transcript_plain_items().join("\n");
    assert!(
        plain.contains("Finished dev profile"),
        "completed command should reveal deferred output: {plain}"
    );
    assert!(
        !plain.contains("Requesting approval to perform"),
        "permission request copy must not be replayed after command completion: {plain}"
    );
}

#[test]
fn acp_rejected_permission_internal_agent_notice_is_not_shown() {
    let mut model = Model::new(HeroOptions::default());
    model.set_window(80, 12);
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
        AcpSessionEvent::PermissionRequested {
            agent_id: "Kimi Code CLI".to_string(),
            request: mo_core::acp::AcpPermissionRequest {
                request_id: "permission-exec".to_string(),
                title: Some("cargo check".to_string()),
                tool_call: AcpToolCallUpdate {
                    tool_call_id: "tool-exec".to_string(),
                    title: Some("cargo check".to_string()),
                    kind: Some(AcpToolKind::Execute),
                    status: Some(AcpToolCallStatus::Pending),
                    content: None,
                    locations: None,
                    raw_input: Some(r#"{"command":"cargo check"}"#.into()),
                    raw_output: None,
                },
                options: vec![mo_core::acp::AcpPermissionOption {
                    option_id: "reject-once".to_string(),
                    name: "Reject".to_string(),
                    kind: mo_core::acp::AcpPermissionOptionKind::RejectOnce,
                }],
            },
        },
    );

    let effect = model
        .handle_tool_approval_panel_key(KeyEvent::from(KeyCode::Enter))
        .expect("permission panel should handle Enter");
    assert!(matches!(
        effect,
        Some(AppEffect::RespondAcpPermission {
            ref request_id,
            option_id: Some(ref option_id),
            is_rejection,
            rejected_tool_call_id: Some(ref rejected_tool_call_id),
        }) if request_id == "permission-exec"
            && option_id == "reject-once"
            && is_rejection
            && rejected_tool_call_id == "tool-exec"
    ));
    if let Some(AppEffect::RespondAcpPermission {
        request_id,
        option_id,
        is_rejection,
        rejected_tool_call_id,
    }) = effect
    {
        run_respond_acp_permission_effect(
            &mut model,
            &mut acp_runtime,
            &mut TestRuntimeDriver::default(),
            &request_id,
            option_id,
            is_rejection,
            rejected_tool_call_id,
        );
    }

    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::AgentMessageChunk {
            agent_id: "Kimi Code CLI".to_string(),
            content: concat!(
                "The tool call is rejected by the user. ",
                "Stop what you are doing and wait for the user to tell you how to proceed."
            )
            .to_string(),
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

    let plain = model.transcript_plain_items().join("\n");
    assert!(
        !plain.contains("Reject cargo check"),
        "the ACP tool row should carry permission rejection without a redundant reject item: {plain}"
    );
    assert!(
        !plain.contains("Tool call ended without final status") && !plain.contains("Input:"),
        "permission rejection should not fall back to generic failure/raw input details: {plain}"
    );
    assert!(
        !plain.contains("The tool call is rejected by the user")
            && !plain.contains("Stop what you are doing")
            && !plain.contains("wait for the user to tell you how to proceed"),
        "agent-facing rejection instruction must not be shown in the TUI: {plain}"
    );
}

#[test]
fn acp_rejected_permission_tool_call_update_hides_internal_notice_and_skips_reject_result() {
    let mut model = Model::new(HeroOptions::default());
    model.set_window(80, 12);
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
        AcpSessionEvent::PermissionRequested {
            agent_id: "Kimi Code CLI".to_string(),
            request: mo_core::acp::AcpPermissionRequest {
                request_id: "permission-exec".to_string(),
                title: Some("cargo check".to_string()),
                tool_call: AcpToolCallUpdate {
                    tool_call_id: "tool-exec".to_string(),
                    title: Some("cargo check".to_string()),
                    kind: Some(AcpToolKind::Execute),
                    status: Some(AcpToolCallStatus::Pending),
                    content: None,
                    locations: None,
                    raw_input: Some(r#"{"command":"cargo check"}"#.into()),
                    raw_output: None,
                },
                options: vec![mo_core::acp::AcpPermissionOption {
                    option_id: "reject-once".to_string(),
                    name: "Reject".to_string(),
                    kind: mo_core::acp::AcpPermissionOptionKind::RejectOnce,
                }],
            },
        },
    );

    let effect = model
        .handle_tool_approval_panel_key(KeyEvent::from(KeyCode::Enter))
        .expect("permission panel should handle Enter");
    assert!(matches!(
        effect,
        Some(AppEffect::RespondAcpPermission {
            ref request_id,
            option_id: Some(ref option_id),
            is_rejection,
            rejected_tool_call_id: Some(ref rejected_tool_call_id),
        }) if request_id == "permission-exec"
            && option_id == "reject-once"
            && is_rejection
            && rejected_tool_call_id == "tool-exec"
    ));
    if let Some(AppEffect::RespondAcpPermission {
        request_id,
        option_id,
        is_rejection,
        rejected_tool_call_id,
    }) = effect
    {
        run_respond_acp_permission_effect(
            &mut model,
            &mut acp_runtime,
            &mut TestRuntimeDriver::default(),
            &request_id,
            option_id,
            is_rejection,
            rejected_tool_call_id,
        );
    }

    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::ToolCallUpdate {
            agent_id: "Kimi Code CLI".to_string(),
            update: AcpToolCallUpdate {
                tool_call_id: "tool-exec".to_string(),
                title: None,
                kind: None,
                status: Some(AcpToolCallStatus::Failed),
                content: Some(vec![AcpToolCallContent::Text(
                    concat!(
                        "The tool call is rejected by the user. ",
                        "Stop what you are doing and wait for the user to tell you how to proceed."
                    )
                    .to_string(),
                )]),
                locations: None,
                raw_input: None,
                raw_output: None,
            },
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

    let plain = model.transcript_plain_items().join("\n");
    assert!(
        plain.contains("cargo check"),
        "tool row should remain: {plain}"
    );
    assert!(
        !plain.contains("Reject cargo check"),
        "the ACP tool row already carries the rejected state, so a separate reject result is redundant: {plain}"
    );
    assert!(
        !plain.contains("Input:"),
        "rejected permission rows should not reveal transport raw input: {plain}"
    );
    assert!(
        !plain.contains("The tool call is rejected by the user")
            && !plain.contains("Stop what you are doing")
            && !plain.contains("wait for the user to tell you how to proceed"),
        "agent-facing rejection instruction must not be rendered inside the tool row: {plain}"
    );
}

#[test]
fn acp_file_preview_permission_without_prior_tool_call_restores_upserted_item_after_choice() {
    let mut model = Model::new(HeroOptions::default());
    model.set_window(80, 12);
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
        AcpSessionEvent::PermissionRequested {
            agent_id: "Kimi Code CLI".to_string(),
            request: mo_core::acp::AcpPermissionRequest {
                request_id: "permission-write-new".to_string(),
                title: Some("WriteFile: TEMP.md".to_string()),
                tool_call: AcpToolCallUpdate {
                    tool_call_id: "tool-write-new".to_string(),
                    title: Some("WriteFile: TEMP.md".to_string()),
                    kind: Some(AcpToolKind::Edit),
                    status: None,
                    content: Some(vec![AcpToolCallContent::Diff {
                        path: "TEMP.md".to_string(),
                        old_text: None,
                        new_text: "body".to_string(),
                    }]),
                    locations: None,
                    raw_input: Some(r##"{"path":"TEMP.md","content":"body"}"##.into()),
                    raw_output: None,
                },
                options: vec![
                    mo_core::acp::AcpPermissionOption {
                        option_id: "allow-once".to_string(),
                        name: "Allow once".to_string(),
                        kind: mo_core::acp::AcpPermissionOptionKind::AllowOnce,
                    },
                    mo_core::acp::AcpPermissionOption {
                        option_id: "reject-once".to_string(),
                        name: "Reject".to_string(),
                        kind: mo_core::acp::AcpPermissionOptionKind::RejectOnce,
                    },
                ],
            },
        },
    );

    assert_eq!(model.transcript_mut().len(), 1);
    assert!(
        !model
            .transcript_plain_items()
            .join("\n")
            .contains("TEMP.md"),
        "file preview approval should suspend the upserted write row while the panel is open"
    );

    let effect = model
        .handle_tool_approval_panel_key(KeyEvent::from(KeyCode::Enter))
        .expect("file preview panel should handle Enter");
    assert!(matches!(
        effect,
        Some(AppEffect::RespondAcpPermission {
            request_id,
            option_id: Some(option_id),
            is_rejection,
            ..
        }) if request_id == "permission-write-new" && option_id == "allow-once" && !is_rejection
    ));
    assert!(
        model
            .transcript_plain_items()
            .join("\n")
            .contains("TEMP.md"),
        "upserted write row should render after the approval panel closes"
    );
}

#[test]
fn acp_permission_request_renders_file_preview_and_pauses_activity() {
    let mut model = Model::new(HeroOptions::default());
    model.set_window(80, 12);
    model.transcript_mut().clear();
    let mut acp_runtime = AcpRuntimeState::default();

    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::PromptStarted {
            agent_id: "Kimi Code CLI".to_string(),
        },
    );
    assert!(model.current_stream_activity_render_result().has_content);

    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::PermissionRequested {
            agent_id: "Kimi Code CLI".to_string(),
            request: mo_core::acp::AcpPermissionRequest {
                request_id: "permission-write".to_string(),
                title: Some("WriteFile: __lumos_permission_preview__.md".to_string()),
                tool_call: AcpToolCallUpdate {
                    tool_call_id: "tool-write".to_string(),
                    title: Some("WriteFile: __lumos_permission_preview__.md".to_string()),
                    kind: Some(AcpToolKind::Edit),
                    status: None,
                    content: Some(vec![AcpToolCallContent::Diff {
                        path: "__lumos_permission_preview__.md".to_string(),
                        old_text: None,
                        new_text: "# Preview\n\nbody".to_string(),
                    }]),
                    locations: None,
                    raw_input: Some(
                        r##"{"path":"__lumos_permission_preview__.md","content":"# Preview\n\nbody"}"##
                            .into(),
                    ),
                    raw_output: None,
                },
                options: vec![
                    mo_core::acp::AcpPermissionOption {
                        option_id: "allow-once".to_string(),
                        name: "Allow once".to_string(),
                        kind: mo_core::acp::AcpPermissionOptionKind::AllowOnce,
                    },
                    mo_core::acp::AcpPermissionOption {
                        option_id: "allow-always".to_string(),
                        name: "Allow always".to_string(),
                        kind: mo_core::acp::AcpPermissionOptionKind::AllowAlways,
                    },
                    mo_core::acp::AcpPermissionOption {
                        option_id: "reject-once".to_string(),
                        name: "Reject".to_string(),
                        kind: mo_core::acp::AcpPermissionOptionKind::RejectOnce,
                    },
                ],
            },
        },
    );

    let panel = model.current_inline_tool_approval_panel_render_result();
    let text = panel.plain_lines.join("\n");
    assert!(model.tool_approval_panel_active());
    assert!(
        !model.current_stream_activity_render_result().has_content,
        "permission approval should pause the turn activity/status line"
    );
    assert!(
        !text.contains("Create file") && !text.contains("Edit file"),
        "file preview should keep the header to the file path only: {text}"
    );
    assert!(
        text.contains("__lumos_permission_preview__.md"),
        "file path should render: {text}"
    );
    assert!(
        text.contains("      1  # Preview") && text.contains("      3  body"),
        "numbered file content should render in the approval panel: {text}"
    );
    assert!(
        !text.contains("\"path\"") && !text.contains("\"content\""),
        "approval panel should not expose raw transport JSON: {text}"
    );
}

#[test]
fn acp_permission_choice_restores_paused_stream_activity() {
    let mut model = Model::new(HeroOptions::default());
    model.set_window(80, 12);
    model.transcript_mut().clear();
    let mut acp_runtime = AcpRuntimeState::default();

    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::PromptStarted {
            agent_id: "Kimi Code CLI".to_string(),
        },
    );
    assert!(model.current_stream_activity_render_result().has_content);

    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::PermissionRequested {
            agent_id: "Kimi Code CLI".to_string(),
            request: mo_core::acp::AcpPermissionRequest {
                request_id: "permission-write".to_string(),
                title: Some("WriteFile: __lumos_permission_preview__.md".to_string()),
                tool_call: AcpToolCallUpdate {
                    tool_call_id: "tool-write".to_string(),
                    title: Some("WriteFile: __lumos_permission_preview__.md".to_string()),
                    kind: Some(AcpToolKind::Edit),
                    status: None,
                    content: Some(vec![AcpToolCallContent::Diff {
                        path: "__lumos_permission_preview__.md".to_string(),
                        old_text: None,
                        new_text: "# Preview\n\nbody".to_string(),
                    }]),
                    locations: None,
                    raw_input: Some(
                        r##"{"path":"__lumos_permission_preview__.md","content":"# Preview\n\nbody"}"##
                            .into(),
                    ),
                    raw_output: None,
                },
                options: vec![
                    mo_core::acp::AcpPermissionOption {
                        option_id: "allow-once".to_string(),
                        name: "Allow once".to_string(),
                        kind: mo_core::acp::AcpPermissionOptionKind::AllowOnce,
                    },
                    mo_core::acp::AcpPermissionOption {
                        option_id: "reject-once".to_string(),
                        name: "Reject".to_string(),
                        kind: mo_core::acp::AcpPermissionOptionKind::RejectOnce,
                    },
                ],
            },
        },
    );
    assert!(
        !model.current_stream_activity_render_result().has_content,
        "approval panel should hide the active turn status"
    );

    let effect = model
        .handle_tool_approval_panel_key(KeyEvent::from(KeyCode::Enter))
        .expect("permission panel should handle Enter");
    assert!(matches!(
        effect,
        Some(AppEffect::RespondAcpPermission {
            request_id,
            option_id: Some(option_id),
            is_rejection,
            ..
        }) if request_id == "permission-write" && option_id == "allow-once" && !is_rejection
    ));
    assert!(
        model.current_stream_activity_render_result().has_content,
        "turn status should resume after the permission panel closes"
    );
}

#[test]
fn acp_file_preview_permission_hides_active_write_tool_call_until_choice() {
    let mut model = Model::new(HeroOptions::default());
    model.set_window(80, 12);
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
        AcpSessionEvent::ToolCall {
            agent_id: "Kimi Code CLI".to_string(),
            call: AcpToolCall {
                tool_call_id: "tool-write".to_string(),
                title: "WriteFile: TEMP.md".to_string(),
                kind: AcpToolKind::Edit,
                status: AcpToolCallStatus::InProgress,
                content: Vec::new(),
                locations: Vec::new(),
                raw_input: Some(r##"{"path":"TEMP.md","content":"body"}"##.into()),
                raw_output: None,
            },
        },
    );
    assert!(
        model
            .transcript_plain_items()
            .join("\n")
            .contains("Write TEMP.md"),
        "active write tool call should render before permission preview opens"
    );

    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::PermissionRequested {
            agent_id: "Kimi Code CLI".to_string(),
            request: mo_core::acp::AcpPermissionRequest {
                request_id: "permission-write".to_string(),
                title: Some("WriteFile: TEMP.md".to_string()),
                tool_call: AcpToolCallUpdate {
                    tool_call_id: "tool-write".to_string(),
                    title: Some("WriteFile: TEMP.md".to_string()),
                    kind: Some(AcpToolKind::Edit),
                    status: None,
                    content: Some(vec![AcpToolCallContent::Diff {
                        path: "TEMP.md".to_string(),
                        old_text: None,
                        new_text: "body".to_string(),
                    }]),
                    locations: None,
                    raw_input: Some(r##"{"path":"TEMP.md","content":"body"}"##.into()),
                    raw_output: None,
                },
                options: vec![
                    mo_core::acp::AcpPermissionOption {
                        option_id: "allow-once".to_string(),
                        name: "Allow once".to_string(),
                        kind: mo_core::acp::AcpPermissionOptionKind::AllowOnce,
                    },
                    mo_core::acp::AcpPermissionOption {
                        option_id: "reject-once".to_string(),
                        name: "Reject".to_string(),
                        kind: mo_core::acp::AcpPermissionOptionKind::RejectOnce,
                    },
                ],
            },
        },
    );
    assert!(
        !model
            .transcript_plain_items()
            .join("\n")
            .contains("Write TEMP.md"),
        "active write tool call should be hidden while the file preview approval panel is open"
    );

    let effect = model
        .handle_tool_approval_panel_key(KeyEvent::from(KeyCode::Enter))
        .expect("file preview panel should handle Enter");
    assert!(matches!(
        effect,
        Some(AppEffect::RespondAcpPermission {
            request_id,
            option_id: Some(option_id),
            is_rejection,
            ..
        }) if request_id == "permission-write" && option_id == "allow-once" && !is_rejection
    ));
    assert!(
        model
            .transcript_plain_items()
            .join("\n")
            .contains("TEMP.md"),
        "active write tool call should reappear after the approval panel closes"
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
            config: mo_core::acp::AcpModelConfig {
                config_id: Some("model".to_string()),
                current_value: "kimi-k2".to_string(),
                current_name: "Kimi K2".to_string(),
                options: vec![mo_core::acp::AcpModelOption {
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
fn acp_model_config_change_failed_rolls_back_selected_model_and_status_line() {
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
            config: mo_core::acp::AcpModelConfig {
                config_id: Some("model".to_string()),
                current_value: "kimi-k2".to_string(),
                current_name: "Kimi K2".to_string(),
                options: vec![
                    mo_core::acp::AcpModelOption {
                        value: "kimi-k2".to_string(),
                        name: "Kimi K2".to_string(),
                    },
                    mo_core::acp::AcpModelOption {
                        value: "kimi-k1.5".to_string(),
                        name: "Kimi K1.5".to_string(),
                    },
                ],
            },
        },
    );
    model.open_model_panel();
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Down)));

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    assert_eq!(
        effect,
        Some(AppEffect::SetAcpModel {
            config_id: Some("model".to_string()),
            value: "kimi-k1.5".to_string(),
        })
    );
    assert_eq!(
        model.selected_model,
        Some(ModelSelection::new("acp:Kimi Code CLI", "kimi-k1.5"))
    );
    assert_eq!(
        model.current_status_line_parts(),
        vec!["Kimi K1.5".to_string()]
    );

    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::ConfigChangeFailed {
            agent_id: "Kimi Code CLI".to_string(),
            message: "boom".to_string(),
        },
    );

    assert_eq!(
        model.selected_model,
        Some(ModelSelection::new("acp:Kimi Code CLI", "kimi-k2"))
    );
    assert_eq!(
        model.current_status_line_parts(),
        vec!["Kimi K2".to_string()]
    );
}

#[test]
fn acp_model_selection_effect_failure_rolls_back_selected_model_and_status_line() {
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
            config: mo_core::acp::AcpModelConfig {
                config_id: None,
                current_value: "kimi-for-coding".to_string(),
                current_name: "Kimi for Coding".to_string(),
                options: vec![
                    mo_core::acp::AcpModelOption {
                        value: "kimi-for-coding".to_string(),
                        name: "Kimi for Coding".to_string(),
                    },
                    mo_core::acp::AcpModelOption {
                        value: "kimi-for-coding(thinking)".to_string(),
                        name: "Kimi for Coding (thinking)".to_string(),
                    },
                ],
            },
        },
    );
    model.open_model_panel();
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Down)));

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    assert_eq!(
        effect,
        Some(AppEffect::SetAcpModel {
            config_id: None,
            value: "kimi-for-coding(thinking)".to_string(),
        })
    );
    assert_eq!(
        model.selected_model,
        Some(ModelSelection::new(
            "acp:Kimi Code CLI",
            "kimi-for-coding(thinking)"
        ))
    );
    assert_eq!(
        model.current_status_line_parts(),
        vec!["Kimi for Coding (thinking)".to_string()]
    );

    run_set_acp_model_effect(
        &mut model,
        &mut TestRuntimeDriver::default(),
        None,
        "kimi-for-coding(thinking)".to_string(),
    );

    assert_eq!(
        model.selected_model,
        Some(ModelSelection::new("acp:Kimi Code CLI", "kimi-for-coding"))
    );
    assert_eq!(
        model.current_status_line_parts(),
        vec!["Kimi for Coding".to_string()]
    );
}

#[test]
fn acp_model_config_change_succeeded_commits_selected_model_and_status_line() {
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
            config: mo_core::acp::AcpModelConfig {
                config_id: None,
                current_value: "kimi-for-coding".to_string(),
                current_name: "Kimi for Coding".to_string(),
                options: vec![
                    mo_core::acp::AcpModelOption {
                        value: "kimi-for-coding".to_string(),
                        name: "Kimi for Coding".to_string(),
                    },
                    mo_core::acp::AcpModelOption {
                        value: "kimi-for-coding(thinking)".to_string(),
                        name: "Kimi for Coding (thinking)".to_string(),
                    },
                ],
            },
        },
    );
    model.open_model_panel();
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Down)));

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    assert_eq!(
        effect,
        Some(AppEffect::SetAcpModel {
            config_id: None,
            value: "kimi-for-coding(thinking)".to_string(),
        })
    );

    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::ConfigChangeSucceeded {
            agent_id: "Kimi Code CLI".to_string(),
        },
    );
    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::ConfigChangeFailed {
            agent_id: "Kimi Code CLI".to_string(),
            message: "stale failure".to_string(),
        },
    );

    assert_eq!(
        model.selected_model,
        Some(ModelSelection::new(
            "acp:Kimi Code CLI",
            "kimi-for-coding(thinking)"
        ))
    );
    assert_eq!(
        model.current_status_line_parts(),
        vec!["Kimi for Coding (thinking)".to_string()]
    );
}

#[test]
fn acp_available_commands_are_saved_and_cleared_with_session_lifecycle() {
    let mut model = Model::new(HeroOptions::default());
    model.selected_acp_agent = Some("Kimi Code CLI".to_string());
    let mut acp_runtime = AcpRuntimeState::default();

    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::Started {
            agent_id: "Kimi Code CLI".to_string(),
            session_id: "test-session".to_string(),
            outcome: AcpInitializeOutcome {
                protocol_version: AcpProtocolVersion::V1,
                agent_name: Some("kimi".to_string()),
                agent_title: Some("Kimi Code CLI".to_string()),
                agent_version: Some("1.0.0".to_string()),
                agent_capabilities: AcpAgentCapabilities::default(),
                auth_method_count: 0,
            },
        },
    );

    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::AvailableCommandsChanged {
            agent_id: "Kimi Code CLI".to_string(),
            commands: vec![
                AcpAvailableCommand {
                    name: "web".to_string(),
                    description: "Search the web".to_string(),
                    input: Some(AcpAvailableCommandInput::Unstructured {
                        hint: "query to search for".to_string(),
                    }),
                },
                AcpAvailableCommand {
                    name: "clear".to_string(),
                    description: "ACP clear".to_string(),
                    input: None,
                },
            ],
        },
    );

    let commands = model.selected_acp_available_commands();
    assert_eq!(
        commands
            .iter()
            .map(|command| command.name.as_str())
            .collect::<Vec<_>>(),
        vec!["web", "clear"]
    );
    assert!(matches!(
        commands[0].input,
        Some(AcpAvailableCommandInput::Unstructured { ref hint }) if hint == "query to search for"
    ));

    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::Stopped {
            agent_id: "Kimi Code CLI".to_string(),
            message: None,
        },
    );

    assert!(model.selected_acp_available_commands().is_empty());
}

#[test]
fn clear_runtime_discards_stale_native_agent_event() {
    let mut model = Model::new(HeroOptions::default());
    model.transcript_mut().clear();
    model.show_stream_activity("qwen3");
    let mut acp_runtime = AcpRuntimeState::default();
    let mut runtime_driver = TestRuntimeDriver {
        native_events: vec![NativeAgentRuntimeEvent {
            target: Some(RuntimeTarget::native_agent("local", "qwen3")),
            event: NativeAgentEvent::Finished {
                response: NativeAgentResponse {
                    content: "stale response".to_string(),
                    reasoning_content: None,
                    reasoning_duration: None,
                    ..Default::default()
                },
                metrics: None,
            },
        }],
        native_running: true,
        ..TestRuntimeDriver::default()
    };

    model.reset_to_initial_tui_state();
    reset_runtime_session_after_clear(&mut acp_runtime, &mut runtime_driver);
    drain_runtime_driver_events(&mut model, &mut acp_runtime, &mut runtime_driver);

    assert!(
        model
            .transcript_plain_items()
            .iter()
            .all(|item| !item.contains("stale response"))
    );
    assert!(!model.current_stream_activity_render_result().has_content);
    assert_eq!(runtime_driver.reset_count, 1);
}

#[test]
fn clear_runtime_discards_stale_acp_prompt_output_without_exiting_acp_mode() {
    let mut model = Model::new(HeroOptions::default());
    model.transcript_mut().clear();
    model.selected_acp_agent = Some("Kimi Code CLI".to_string());
    let mut acp_runtime = AcpRuntimeState::default();
    let mut runtime_driver = TestRuntimeDriver::default();

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
    reset_runtime_session_after_clear(&mut acp_runtime, &mut runtime_driver);
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
    let mut runtime_driver = TestRuntimeDriver::default();

    acp_runtime.mark_prompt_submitted();
    model.reset_to_initial_tui_state();
    reset_runtime_session_after_clear(&mut acp_runtime, &mut runtime_driver);
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
    let mut runtime_driver = TestRuntimeDriver::default();

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
    reset_runtime_session_after_clear(&mut acp_runtime, &mut runtime_driver);
    apply_acp_session_event(
        &mut model,
        &mut acp_runtime,
        AcpSessionEvent::PermissionRequested {
            agent_id: "Kimi Code CLI".to_string(),
            request: mo_core::acp::AcpPermissionRequest {
                request_id: "stale-permission".to_string(),
                title: Some("旧请求写文件".to_string()),
                tool_call: AcpToolCallUpdate {
                    tool_call_id: "tool-1".to_string(),
                    title: Some("旧请求写文件".to_string()),
                    kind: None,
                    status: None,
                    content: None,
                    locations: None,
                    raw_input: None,
                    raw_output: None,
                },
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
fn cancelled_acp_prompt_cancels_late_permission_request() {
    let mut model = Model::new(HeroOptions::default());
    let mut acp_runtime = AcpRuntimeState::default();
    acp_runtime.mark_prompt_submitted();
    run_interrupt_acp_prompt_effect(
        &mut model,
        &mut acp_runtime,
        &mut TestRuntimeDriver::default(),
    );

    let option_id = acp_runtime
        .permission_option_id_for_discarded_prompt(&acp_permission_request_with_reject_always());

    assert_eq!(
        option_id, None,
        "ACP session/cancel requires late permission requests to resolve as Cancelled"
    );
}

#[test]
fn stale_acp_permission_reject_fallback_uses_reject_always() {
    let mut acp_runtime = AcpRuntimeState::default();
    acp_runtime.mark_prompt_submitted();
    acp_runtime.reset_after_clear();

    let option_id = acp_runtime
        .permission_option_id_for_discarded_prompt(&acp_permission_request_with_reject_always());

    assert_eq!(option_id, Some("reject-always".to_string()));
}

#[test]
fn acp_permission_stale_reject_fallback_prefers_reject_always() {
    let option_id =
        acp_reject_option_id_for_stale_discard(&acp_permission_request_with_reject_always());

    assert_eq!(option_id, Some("reject-always".to_string()));
}

fn acp_permission_request_with_reject_always() -> mo_core::acp::AcpPermissionRequest {
    use mo_core::acp::{AcpPermissionOption, AcpPermissionOptionKind, AcpPermissionRequest};

    AcpPermissionRequest {
        request_id: "permission-session-only".to_string(),
        title: Some("Run command".to_string()),
        tool_call: AcpToolCallUpdate {
            tool_call_id: "tool-1".to_string(),
            title: Some("Run command".to_string()),
            kind: None,
            status: None,
            content: None,
            locations: None,
            raw_input: None,
            raw_output: None,
        },
        options: vec![AcpPermissionOption {
            option_id: "reject-always".to_string(),
            name: "Reject in session".to_string(),
            kind: AcpPermissionOptionKind::RejectAlways,
        }],
    }
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
    model.set_palette(crate::theme::default_palette(), true);
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
    model.set_palette(crate::theme::default_palette(), true);
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
    model.set_palette(crate::theme::default_palette(), true);
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
    let mut runtime_driver = TestRuntimeDriver::default();
    let request = NativeAgentRequest::new(
        "local",
        ProviderKind::OpenAiCompatible,
        "qwen3",
        None,
        None,
        None,
        vec![],
    );

    run_send_native_agent_effect(&mut model, &mut runtime_driver, request);

    assert_eq!(
        runtime_driver
            .native_request
            .as_ref()
            .map(NativeAgentRequest::target),
        Some(RuntimeTarget::native_agent("local", "qwen3"))
    );
    assert!(model.current_stream_activity_render_result().has_content);
}

#[test]
fn native_agent_request_keeps_runtime_target_in_core_dto() {
    let request = NativeAgentRequest::new(
        "local",
        ProviderKind::OpenAiCompatible,
        "qwen3",
        None,
        None,
        None,
        vec![],
    );

    assert_eq!(
        request.target(),
        RuntimeTarget::native_agent("local", "qwen3")
    );
}

#[test]
fn interrupt_native_agent_clears_runtime_and_appends_system_message() {
    let mut runtime_driver = TestRuntimeDriver {
        native_running: true,
        ..TestRuntimeDriver::default()
    };
    let mut model = Model::new(HeroOptions::default());
    model.transcript_mut().clear();
    model.show_stream_activity("qwen3");

    apply_effect_if_needed_for_test(
        &mut model,
        &mut runtime_driver,
        Some(AppEffect::InterruptCurrentTurn),
    );

    assert!(runtime_driver.native_interrupted);
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

    run_interrupt_acp_prompt_effect(
        &mut model,
        &mut acp_runtime,
        &mut TestRuntimeDriver::default(),
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
    runtime_driver: &mut TestRuntimeDriver,
    effect: Option<AppEffect>,
) {
    if let Some(AppEffect::InterruptCurrentTurn) = effect {
        run_interrupt_current_turn_effect(model, &mut AcpRuntimeState::default(), runtime_driver);
    }
}
