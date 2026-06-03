use std::time::Duration;

use super::conversation::{apply_conversation_event, run_send_conversation_turn_effect};
use super::effects::run_interrupt_current_turn_effect;
use super::input::{TerminalInputAction, coalesced_input_actions};
use super::*;
use crate::{
    AppEffect, AppEvent, ReasoningDisplayMode, Sender, StatusLineItem, runtime::RuntimeEventApply,
    theme::default_palette, transcript::TranscriptItem,
};
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::style::Color;
use runtime_domain::model_catalog::ProviderSyncRequest;
use runtime_domain::provider::ProviderKind;
use runtime_domain::request_policy::RuntimeRequestPolicy;
use runtime_domain::session::{
    ChatMessage, ConversationEvent, ConversationResponse, ConversationTurnRequest,
    ProviderRequestMetrics, RuntimeCommand, RuntimeCommandReceipt, RuntimeEvent, RuntimeTarget,
    RuntimeToolActivity, RuntimeToolActivityContent, RuntimeToolActivityStatus,
    RuntimeToolActivityUpdate, RuntimeToolKind,
};

#[derive(Default)]
struct TestRuntimeCoordinator {
    runtime_events: Vec<RuntimeEvent>,
    conversation_running: bool,
    conversation_interrupted: bool,
    conversation_request: Option<ConversationTurnRequest>,
    reset_count: usize,
    conversation_retained_user_turns: Option<usize>,
}

impl RuntimeCoordinator for TestRuntimeCoordinator {
    fn drain_runtime_events(&mut self) -> Vec<RuntimeEvent> {
        std::mem::take(&mut self.runtime_events)
    }

    fn dispatch_runtime_command(
        &mut self,
        command: RuntimeCommand,
    ) -> Result<RuntimeCommandReceipt, String> {
        match command {
            RuntimeCommand::Reset => {
                self.runtime_events.clear();
                self.conversation_running = false;
                self.reset_count += 1;
                Ok(RuntimeCommandReceipt::Accepted)
            }
            RuntimeCommand::TruncateConversation {
                retained_user_turns,
            } => {
                self.conversation_retained_user_turns = Some(retained_user_turns);
                Ok(RuntimeCommandReceipt::Accepted)
            }
            RuntimeCommand::SubmitConversationTurn { request, .. } => {
                if self.conversation_running {
                    return Err("Chat request is already running".to_string());
                }

                let activity_label = request.model_id().to_string();
                self.conversation_running = true;
                self.conversation_request = Some(request);
                Ok(RuntimeCommandReceipt::ConversationStarted { activity_label })
            }
            RuntimeCommand::Interrupt { target } => {
                if !self.conversation_running {
                    return Ok(RuntimeCommandReceipt::Accepted);
                }

                self.conversation_running = false;
                self.conversation_interrupted = true;
                Ok(RuntimeCommandReceipt::Interrupted {
                    target: target.or_else(|| Some(RuntimeTarget::provider("local", "qwen3"))),
                })
            }
            RuntimeCommand::RespondPermission { .. } => Ok(RuntimeCommandReceipt::Accepted),
        }
    }

    fn refresh_model_provider(&mut self, _request: ProviderSyncRequest) -> Result<(), String> {
        Ok(())
    }
}

#[test]
fn conversation_completion_appends_assistant_message_after_request_finishes() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.transcript_mut().clear();
    model.show_stream_activity("qwen3");

    apply_conversation_event(
        &mut model,
        None,
        ConversationEvent::Finished {
            response: ConversationResponse {
                content: "你好，我是本地模型".to_string(),
                reasoning_content: None,
                reasoning_duration: None,
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
fn runtime_deltas_flush_before_conversation_tool_activity() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            show_reasoning_content: true,
            reasoning_display_mode: ReasoningDisplayMode::Expanded,
            ..ModelOptions::default()
        },
    );
    let target = RuntimeTarget::provider("local", "qwen3");
    model.transcript_mut().clear();
    model.show_stream_activity("qwen3");

    model.apply_runtime_event(RuntimeEvent::ReasoningDelta {
        target: target.clone(),
        content: "先分析目录结构".to_string(),
    });
    model.apply_runtime_event(RuntimeEvent::AssistantDelta {
        target: target.clone(),
        content: "我先看一下 src。".to_string(),
    });

    assert_eq!(
        model.transcript_plain_items(),
        vec!["先分析目录结构".to_string()]
    );

    model.apply_runtime_event(RuntimeEvent::ToolActivityStarted {
        target,
        activity: RuntimeToolActivity {
            activity_id: "call-1".to_string(),
            title: "List Directory src".to_string(),
            kind: RuntimeToolKind::Search,
            status: RuntimeToolActivityStatus::InProgress,
            content: Vec::new(),
            locations: Vec::new(),
            raw_input: None,
            raw_output: None,
        },
    });

    let items = model.transcript_plain_items();
    assert_eq!(items.len(), 3, "{items:#?}");
    assert_eq!(items[0], "先分析目录结构");
    assert_eq!(items[1], "我先看一下 src。");
    assert_eq!(items[2], "● List src");
}

#[test]
fn runtime_expanded_reasoning_flushes_before_message_finish() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            show_reasoning_content: true,
            reasoning_display_mode: ReasoningDisplayMode::Expanded,
            ..ModelOptions::default()
        },
    );
    let target = RuntimeTarget::provider("local", "qwen3");
    model.transcript_mut().clear();
    model.show_stream_activity("qwen3");

    model.apply_runtime_event(RuntimeEvent::ReasoningDelta {
        target: target.clone(),
        content: "先分析目录结构".to_string(),
    });
    model.apply_runtime_event(RuntimeEvent::AssistantDelta {
        target: target.clone(),
        content: "我先看一下 src。".to_string(),
    });

    assert_eq!(
        model.transcript_plain_items(),
        vec!["先分析目录结构".to_string()]
    );

    model.apply_runtime_event(RuntimeEvent::MessageFinished {
        target: Some(target),
        content: "我先看一下 src。".to_string(),
        reasoning_content: Some("先分析目录结构".to_string()),
        reasoning_duration: Some(Duration::from_secs(2)),
        finish_reason: None,
        metrics: None,
    });

    assert_eq!(
        model.transcript_plain_items(),
        vec!["先分析目录结构".to_string(), "我先看一下 src。".to_string()]
    );
}

#[test]
fn runtime_expanded_simplified_reasoning_flushes_before_message_finish() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            show_reasoning_content: true,
            reasoning_display_mode: ReasoningDisplayMode::ExpandedSimplified,
            ..ModelOptions::default()
        },
    );
    let target = RuntimeTarget::provider("local", "qwen3");
    model.transcript_mut().clear();
    model.show_stream_activity("qwen3");

    model.apply_runtime_event(RuntimeEvent::ReasoningDelta {
        target: target.clone(),
        content: "先分析目录结构".to_string(),
    });
    model.apply_runtime_event(RuntimeEvent::AssistantDelta {
        target: target.clone(),
        content: "我先看一下 src。".to_string(),
    });

    assert_eq!(
        model.transcript_plain_items(),
        vec!["先分析目录结构".to_string()]
    );

    model.apply_runtime_event(RuntimeEvent::MessageFinished {
        target: Some(target),
        content: "我先看一下 src。".to_string(),
        reasoning_content: Some("先分析目录结构".to_string()),
        reasoning_duration: Some(Duration::from_secs(2)),
        finish_reason: None,
        metrics: None,
    });

    assert_eq!(
        model.transcript_plain_items(),
        vec!["先分析目录结构".to_string(), "我先看一下 src。".to_string()]
    );
}

#[test]
fn runtime_final_response_keeps_streamed_reasoning_flushed_across_tool_boundaries() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            show_reasoning_content: true,
            reasoning_display_mode: ReasoningDisplayMode::Expanded,
            ..ModelOptions::default()
        },
    );
    let target = RuntimeTarget::provider("local", "qwen3");
    model.transcript_mut().clear();
    model.show_stream_activity("qwen3");

    model.apply_runtime_event(RuntimeEvent::ReasoningDelta {
        target: target.clone(),
        content: "我需要先查看当前目录内容，然后再整理回复。".to_string(),
    });
    model.apply_runtime_event(RuntimeEvent::ToolActivityStarted {
        target: target.clone(),
        activity: RuntimeToolActivity {
            activity_id: "call-1".to_string(),
            title: "List Directory".to_string(),
            kind: RuntimeToolKind::Search,
            status: RuntimeToolActivityStatus::Completed,
            content: vec![RuntimeToolActivityContent::Text(
                "Cargo.toml\ncrates/\n".to_string(),
            )],
            locations: Vec::new(),
            raw_input: Some(serde_json::json!({ "path": "." }).into()),
            raw_output: Some("Cargo.toml\ncrates/\n".into()),
        },
    });
    model.apply_runtime_event(RuntimeEvent::ReasoningDelta {
        target: target.clone(),
        content: "我已经拿到目录结果，现在只保留最终回复前需要展示的推理。".to_string(),
    });
    model.apply_runtime_event(RuntimeEvent::AssistantDelta {
        target: target.clone(),
        content: "当前目录包含 Cargo.toml 和 crates/。".to_string(),
    });

    assert_eq!(
        model.transcript_plain_items(),
        vec![
            "我需要先查看当前目录内容，然后再整理回复。".to_string(),
            "● List .".to_string(),
            "我已经拿到目录结果，现在只保留最终回复前需要展示的推理。".to_string(),
        ]
    );

    model.apply_runtime_event(RuntimeEvent::MessageFinished {
        target: Some(target),
        content: "当前目录包含 Cargo.toml 和 crates/。".to_string(),
        reasoning_content: Some(
            "我已经拿到目录结果，现在只保留最终回复前需要展示的推理。".to_string(),
        ),
        reasoning_duration: Some(Duration::from_secs(2)),
        finish_reason: None,
        metrics: None,
    });

    assert_eq!(
        model.transcript_plain_items(),
        vec![
            "我需要先查看当前目录内容，然后再整理回复。".to_string(),
            "● List .".to_string(),
            "我已经拿到目录结果，现在只保留最终回复前需要展示的推理。".to_string(),
            "当前目录包含 Cargo.toml 和 crates/。".to_string(),
        ]
    );
}

#[test]
fn runtime_final_response_extends_buffered_reasoning_tail_after_earlier_tool_boundary() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            show_reasoning_content: true,
            reasoning_display_mode: ReasoningDisplayMode::Expanded,
            ..ModelOptions::default()
        },
    );
    let target = RuntimeTarget::provider("local", "qwen3");
    model.transcript_mut().clear();
    model.show_stream_activity("qwen3");

    model.apply_runtime_event(RuntimeEvent::ReasoningDelta {
        target: target.clone(),
        content: "我需要先查看当前目录。".to_string(),
    });
    model.apply_runtime_event(RuntimeEvent::ToolActivityStarted {
        target: target.clone(),
        activity: RuntimeToolActivity {
            activity_id: "call-1".to_string(),
            title: "List Directory".to_string(),
            kind: RuntimeToolKind::Search,
            status: RuntimeToolActivityStatus::Completed,
            content: vec![RuntimeToolActivityContent::Text(
                "Cargo.toml\ncrates/\n".to_string(),
            )],
            locations: Vec::new(),
            raw_input: Some(serde_json::json!({ "path": "." }).into()),
            raw_output: Some("Cargo.toml\ncrates/\n".into()),
        },
    });
    model.apply_runtime_event(RuntimeEvent::ReasoningDelta {
        target: target.clone(),
        content: "我已经拿到目录结果，接下来整".to_string(),
    });

    assert_eq!(
        model.transcript_plain_items(),
        vec!["我需要先查看当前目录。".to_string(), "● List .".to_string(),]
    );

    model.apply_runtime_event(RuntimeEvent::MessageFinished {
        target: Some(target),
        content: "当前目录包含 Cargo.toml 和 crates/。".to_string(),
        reasoning_content: Some(
            "我需要先查看当前目录。我已经拿到目录结果，接下来整理回复。".to_string(),
        ),
        reasoning_duration: Some(Duration::from_secs(2)),
        finish_reason: None,
        metrics: None,
    });

    assert_eq!(
        model.transcript_plain_items(),
        vec![
            "我需要先查看当前目录。".to_string(),
            "● List .".to_string(),
            "我已经拿到目录结果，接下来整理回复。".to_string(),
            "当前目录包含 Cargo.toml 和 crates/。".to_string(),
        ]
    );
}

#[test]
fn runtime_first_assistant_delta_finalizes_completed_exploration_marker() {
    let palette = default_palette();
    let mut model = Model::new(StartupBannerOptions::default());
    let target = RuntimeTarget::provider("local", "qwen3");
    model.set_palette(palette, true);
    model.transcript_mut().clear();
    model.show_stream_activity("qwen3");

    model.apply_runtime_event(RuntimeEvent::ToolActivityStarted {
        target: target.clone(),
        activity: RuntimeToolActivity {
            activity_id: "call-1".to_string(),
            title: "List Directory src".to_string(),
            kind: RuntimeToolKind::Search,
            status: RuntimeToolActivityStatus::Completed,
            content: vec![RuntimeToolActivityContent::Text(
                "main.rs\nlib.rs".to_string(),
            )],
            locations: Vec::new(),
            raw_input: Some(serde_json::json!({ "path": "src" }).into()),
            raw_output: Some("main.rs\nlib.rs".into()),
        },
    });

    assert_eq!(
        first_tool_result_marker_color(&mut model),
        Some(palette.main)
    );

    model.apply_runtime_event(RuntimeEvent::AssistantDelta {
        target,
        content: "继续检查实现细节。".to_string(),
    });

    assert_eq!(
        model.transcript_plain_items(),
        vec!["● List src".to_string()]
    );
    assert_eq!(
        first_tool_result_marker_color(&mut model),
        Some(palette.quote)
    );
}

#[test]
fn runtime_final_response_does_not_duplicate_buffered_delta() {
    let mut model = Model::new(StartupBannerOptions::default());
    let target = RuntimeTarget::provider("local", "qwen3");
    model.transcript_mut().clear();
    model.show_stream_activity("qwen3");

    model.apply_runtime_event(RuntimeEvent::AssistantDelta {
        target: target.clone(),
        content: "最终结论".to_string(),
    });
    model.apply_runtime_event(RuntimeEvent::MessageFinished {
        target: Some(target),
        content: "最终结论".to_string(),
        reasoning_content: None,
        reasoning_duration: None,
        finish_reason: None,
        metrics: None,
    });

    assert_eq!(model.transcript_plain_items(), vec!["最终结论".to_string()]);
    assert!(!model.current_stream_activity_render_result().has_content);
}

#[test]
fn runtime_final_response_does_not_overwrite_flushed_streamed_reasoning() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            show_reasoning_content: true,
            reasoning_display_mode: ReasoningDisplayMode::Expanded,
            ..ModelOptions::default()
        },
    );
    let target = RuntimeTarget::provider("local", "qwen3");
    model.transcript_mut().clear();
    model.show_stream_activity("qwen3");

    model.apply_runtime_event(RuntimeEvent::ReasoningDelta {
        target: target.clone(),
        content: "先".to_string(),
    });
    model.apply_runtime_event(RuntimeEvent::AssistantDelta {
        target: target.clone(),
        content: "最终".to_string(),
    });
    model.apply_runtime_event(RuntimeEvent::MessageFinished {
        target: Some(target),
        content: "最终结论".to_string(),
        reasoning_content: Some("先分析完整".to_string()),
        reasoning_duration: Some(Duration::from_secs(2)),
        finish_reason: None,
        metrics: None,
    });

    assert_eq!(
        model.transcript_plain_items(),
        vec!["先".to_string(), "最终结论".to_string(),]
    );
    assert!(!model.current_stream_activity_render_result().has_content);
}

#[test]
fn runtime_final_response_uses_final_reasoning_when_no_boundary_arrives() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            show_reasoning_content: true,
            reasoning_display_mode: ReasoningDisplayMode::Expanded,
            ..ModelOptions::default()
        },
    );
    let target = RuntimeTarget::provider("local", "qwen3");
    model.transcript_mut().clear();
    model.show_stream_activity("qwen3");

    model.apply_runtime_event(RuntimeEvent::ReasoningDelta {
        target: target.clone(),
        content: "先分析".to_string(),
    });
    model.apply_runtime_event(RuntimeEvent::MessageFinished {
        target: Some(target),
        content: "最终结论".to_string(),
        reasoning_content: Some("先分析完整".to_string()),
        reasoning_duration: Some(Duration::from_secs(2)),
        finish_reason: None,
        metrics: None,
    });

    assert_eq!(
        model.transcript_plain_items(),
        vec!["先分析完整".to_string(), "最终结论".to_string(),]
    );
    assert!(!model.current_stream_activity_render_result().has_content);
}

#[test]
fn runtime_system_message_flushes_buffered_delta_in_order() {
    let mut model = Model::new(StartupBannerOptions::default());
    let target = RuntimeTarget::provider("local", "qwen3");
    model.transcript_mut().clear();
    model.show_stream_activity("qwen3");

    model.apply_runtime_event(RuntimeEvent::AssistantDelta {
        target,
        content: "先输出的片段".to_string(),
    });
    model.apply_runtime_event(RuntimeEvent::SystemMessage {
        target: None,
        message: "运行时提示".to_string(),
    });

    assert_eq!(
        model.transcript_plain_items(),
        vec!["先输出的片段".to_string(), "■ 运行时提示".to_string(),]
    );
}

#[test]
fn runtime_interruption_flushes_buffered_delta_before_notice() {
    let mut model = Model::new(StartupBannerOptions::default());
    let target = RuntimeTarget::provider("local", "qwen3");
    model.transcript_mut().clear();
    model.show_stream_activity("qwen3");
    model.backdate_stream_activity_started_at_for_test(Duration::from_secs(38));

    model.apply_runtime_event(RuntimeEvent::AssistantDelta {
        target: target.clone(),
        content: "已输出的片段".to_string(),
    });
    model.apply_runtime_event(RuntimeEvent::Interrupted {
        target: Some(target),
    });

    assert_eq!(
        model.transcript_plain_items(),
        vec!["已输出的片段".to_string(), "■ Chat interrupted".to_string(),]
    );
    assert!(!model.current_stream_activity_render_result().has_content);
}

#[test]
fn runtime_final_response_after_four_tool_calls_inserts_divider_before_body() {
    let mut model = Model::new(StartupBannerOptions::default());
    let target = RuntimeTarget::provider("local", "qwen3");
    model.set_window(40, 8);
    model.transcript_mut().clear();
    model.apply_runtime_event(RuntimeEvent::TurnStarted {
        target: target.clone(),
        label: "qwen3".to_string(),
    });

    apply_runtime_tool_starts(&mut model, &target, 4);
    model.apply_runtime_event(RuntimeEvent::MessageFinished {
        target: Some(target),
        content: "最终正文".to_string(),
        reasoning_content: None,
        reasoning_duration: None,
        finish_reason: None,
        metrics: None,
    });

    let items = model.transcript_plain_items();
    let body_index = items
        .iter()
        .position(|item| item == "最终正文")
        .expect("final body should be appended");
    assert!(
        body_index > 0 && is_plain_divider(&items[body_index - 1]),
        "final body should be separated from prior tool activity: {items:#?}"
    );
}

#[test]
fn runtime_final_response_after_three_tool_calls_does_not_insert_divider() {
    let mut model = Model::new(StartupBannerOptions::default());
    let target = RuntimeTarget::provider("local", "qwen3");
    model.set_window(40, 8);
    model.transcript_mut().clear();
    model.apply_runtime_event(RuntimeEvent::TurnStarted {
        target: target.clone(),
        label: "qwen3".to_string(),
    });

    apply_runtime_tool_starts(&mut model, &target, 3);
    model.apply_runtime_event(RuntimeEvent::MessageFinished {
        target: Some(target),
        content: "最终正文".to_string(),
        reasoning_content: None,
        reasoning_duration: None,
        finish_reason: None,
        metrics: None,
    });

    let items = model.transcript_plain_items();
    assert_eq!(items.last().map(String::as_str), Some("最终正文"));
    assert!(
        !items.iter().any(|item| is_plain_divider(item)),
        "three tool calls should not insert a divider: {items:#?}"
    );
}

#[test]
fn runtime_reasoning_after_four_tool_calls_inserts_divider_before_final_body() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            show_reasoning_content: true,
            reasoning_display_mode: ReasoningDisplayMode::Expanded,
            ..ModelOptions::default()
        },
    );
    let target = RuntimeTarget::provider("local", "qwen3");
    model.set_window(40, 8);
    model.transcript_mut().clear();
    model.apply_runtime_event(RuntimeEvent::TurnStarted {
        target: target.clone(),
        label: "qwen3".to_string(),
    });

    apply_runtime_tool_starts(&mut model, &target, 4);
    model.apply_runtime_event(RuntimeEvent::MessageFinished {
        target: Some(target),
        content: "最终正文".to_string(),
        reasoning_content: Some("最终前的思考".to_string()),
        reasoning_duration: Some(Duration::from_secs(1)),
        finish_reason: None,
        metrics: None,
    });

    let items = model.transcript_plain_items();
    let reasoning_index = items
        .iter()
        .position(|item| item == "最终前的思考")
        .expect("reasoning should be appended");
    let body_index = items
        .iter()
        .position(|item| item == "最终正文")
        .expect("final body should be appended");
    assert!(
        reasoning_index + 2 == body_index && is_plain_divider(&items[reasoning_index + 1]),
        "divider should be placed after visible reasoning and before final body: {items:#?}"
    );
}

#[test]
fn runtime_intermediate_text_after_four_tool_calls_does_not_insert_divider_before_next_tool() {
    let mut model = Model::new(StartupBannerOptions::default());
    let target = RuntimeTarget::provider("local", "qwen3");
    model.set_window(40, 8);
    model.transcript_mut().clear();
    model.apply_runtime_event(RuntimeEvent::TurnStarted {
        target: target.clone(),
        label: "qwen3".to_string(),
    });

    apply_runtime_tool_starts(&mut model, &target, 4);
    model.apply_runtime_event(RuntimeEvent::AssistantDelta {
        target: target.clone(),
        content: "还要继续检查".to_string(),
    });
    apply_runtime_tool_starts(&mut model, &target, 1);

    let intermediate_items = model.transcript_plain_items();
    assert!(
        intermediate_items.iter().any(|item| item == "还要继续检查"),
        "intermediate assistant text should still flush before the next tool: {intermediate_items:#?}"
    );
    assert!(
        !intermediate_items.iter().any(|item| is_plain_divider(item)),
        "divider should be reserved for the final body, not intermediate tool-call text: {intermediate_items:#?}"
    );

    model.apply_runtime_event(RuntimeEvent::MessageFinished {
        target: Some(target),
        content: "最终正文".to_string(),
        reasoning_content: None,
        reasoning_duration: None,
        finish_reason: None,
        metrics: None,
    });

    let items = model.transcript_plain_items();
    let body_index = items
        .iter()
        .position(|item| item == "最终正文")
        .expect("final body should be appended");
    assert!(
        body_index > 0 && is_plain_divider(&items[body_index - 1]),
        "final body should still be separated after later tools: {items:#?}"
    );
}

fn apply_runtime_tool_starts(model: &mut Model, target: &RuntimeTarget, count: usize) {
    for index in 0..count {
        model.apply_runtime_event(RuntimeEvent::ToolActivityStarted {
            target: target.clone(),
            activity: RuntimeToolActivity {
                activity_id: format!("call-{index}"),
                title: format!("Tool {index}"),
                kind: RuntimeToolKind::Other,
                status: RuntimeToolActivityStatus::InProgress,
                content: vec![RuntimeToolActivityContent::Text(format!("input {index}"))],
                locations: Vec::new(),
                raw_input: None,
                raw_output: None,
            },
        });
    }
}

fn is_plain_divider(item: &str) -> bool {
    !item.is_empty() && item.chars().all(|ch| ch == '─')
}

#[test]
fn conversation_delta_event_uses_runtime_boundary_buffer() {
    let mut model = Model::new(StartupBannerOptions::default());
    let target = RuntimeTarget::provider("local", "qwen3");
    model.transcript_mut().clear();
    model.show_stream_activity("qwen3");

    apply_conversation_event(
        &mut model,
        Some(target.clone()),
        ConversationEvent::AssistantDelta {
            content: "我先看一下 src。".to_string(),
        },
    );

    assert!(model.transcript_plain_items().is_empty());

    apply_conversation_event(
        &mut model,
        Some(target),
        ConversationEvent::ToolActivityStarted {
            activity: RuntimeToolActivity {
                activity_id: "call-1".to_string(),
                title: "List Directory src".to_string(),
                kind: RuntimeToolKind::Search,
                status: RuntimeToolActivityStatus::InProgress,
                content: Vec::new(),
                locations: Vec::new(),
                raw_input: None,
                raw_output: None,
            },
        },
    );

    assert_eq!(
        model.transcript_plain_items(),
        vec!["我先看一下 src。".to_string(), "● List src".to_string(),]
    );
}

#[test]
fn conversation_completion_updates_last_request_metrics() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            status_line_items: vec![StatusLineItem::Throughput, StatusLineItem::Latency],
            ..ModelOptions::default()
        },
    );

    apply_conversation_event(
        &mut model,
        None,
        ConversationEvent::Finished {
            response: ConversationResponse {
                content: "完成".to_string(),
                reasoning_content: None,
                reasoning_duration: None,
            },
            metrics: Some(ProviderRequestMetrics {
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

fn first_tool_result_marker_color(model: &mut Model) -> Option<Color> {
    let palette = model.palette;
    let items = model.transcript_mut().items_snapshot();
    let item = items.iter().find_map(|item| match item.as_ref() {
        TranscriptItem::ToolResult(item) => Some(item),
        _ => None,
    })?;
    item.render_lines(80, palette)
        .first()
        .and_then(|line| line.spans.first())
        .and_then(|span| span.style.fg)
}

#[test]
fn conversation_completion_collapses_reasoning_by_default() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            show_reasoning_content: true,
            ..ModelOptions::default()
        },
    );
    model.transcript_mut().clear();
    model.show_stream_activity("qwen3");

    apply_conversation_event(
        &mut model,
        None,
        ConversationEvent::Finished {
            response: ConversationResponse {
                content: "结论".to_string(),
                reasoning_content: Some("先分析".to_string()),
                reasoning_duration: Some(std::time::Duration::from_secs(3)),
            },
            metrics: None,
        },
    );

    assert_eq!(
        model.transcript_plain_items(),
        vec![
            "[Show reasoning · thoughts 3s]".to_string(),
            "结论".to_string(),
        ]
    );
    assert_eq!(
        model.transcript_mut().source_messages(),
        vec![(Sender::Assistant, "结论".to_string())]
    );
}

#[test]
fn conversation_completion_keeps_reasoning_body_gap_to_one_line() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            show_reasoning_content: true,
            reasoning_display_mode: ReasoningDisplayMode::Expanded,
            ..ModelOptions::default()
        },
    );
    model.transcript_mut().clear();
    model.transcript_mut().set_width(40);
    model.show_stream_activity("qwen3");

    apply_conversation_event(
        &mut model,
        None,
        ConversationEvent::Finished {
            response: ConversationResponse {
                content: "结论".to_string(),
                reasoning_content: Some("先分析".to_string()),
                reasoning_duration: Some(std::time::Duration::from_secs(3)),
            },
            metrics: None,
        },
    );

    let render = model.transcript_mut().render();

    assert_eq!(render.all_plain_lines(), vec!["先分析", "", "结论"]);
}

#[test]
fn conversation_reasoning_header_click_toggles_visibility_without_changing_source_messages() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            show_reasoning_content: true,
            ..ModelOptions::default()
        },
    );
    model.set_palette(crate::theme::default_palette(), true);
    model.set_window(40, 8);
    model.transcript_mut().clear();

    apply_conversation_event(
        &mut model,
        None,
        ConversationEvent::Finished {
            response: ConversationResponse {
                content: "结论".to_string(),
                reasoning_content: Some("先分析".to_string()),
                reasoning_duration: Some(std::time::Duration::from_secs(3)),
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
fn conversation_reasoning_header_drag_does_not_toggle() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            show_reasoning_content: true,
            ..ModelOptions::default()
        },
    );
    model.set_palette(crate::theme::default_palette(), true);
    model.set_window(40, 8);
    model.transcript_mut().clear();

    apply_conversation_event(
        &mut model,
        None,
        ConversationEvent::Finished {
            response: ConversationResponse {
                content: "结论".to_string(),
                reasoning_content: Some("先分析".to_string()),
                reasoning_duration: Some(std::time::Duration::from_secs(3)),
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
fn conversation_reasoning_header_click_outside_label_does_not_toggle() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            show_reasoning_content: true,
            ..ModelOptions::default()
        },
    );
    model.set_palette(crate::theme::default_palette(), true);
    model.set_window(40, 8);
    model.transcript_mut().clear();

    apply_conversation_event(
        &mut model,
        None,
        ConversationEvent::Finished {
            response: ConversationResponse {
                content: "结论".to_string(),
                reasoning_content: Some("先分析".to_string()),
                reasoning_duration: Some(std::time::Duration::from_secs(3)),
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
fn conversation_completion_hides_reasoning_when_configured_off() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.transcript_mut().clear();
    model.show_stream_activity("qwen3");

    apply_conversation_event(
        &mut model,
        None,
        ConversationEvent::Finished {
            response: ConversationResponse {
                content: "结论".to_string(),
                reasoning_content: Some("先分析".to_string()),
                reasoning_duration: Some(std::time::Duration::from_secs(3)),
            },
            metrics: None,
        },
    );

    assert_eq!(model.transcript_plain_items(), vec!["结论".to_string()]);
}

#[test]
fn conversation_thinking_event_toggles_activity_segment() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(80, 6);
    model.transcript_mut().clear();
    model.show_stream_activity("qwen3");

    apply_conversation_event(
        &mut model,
        None,
        ConversationEvent::Thinking { is_thinking: true },
    );

    assert!(
        model
            .current_stream_activity_render_result()
            .plain_line
            .contains("thinking")
    );

    apply_conversation_event(
        &mut model,
        None,
        ConversationEvent::Thinking { is_thinking: false },
    );

    assert!(
        !model
            .current_stream_activity_render_result()
            .plain_line
            .contains("thinking")
    );
}

#[test]
fn conversation_failure_appends_system_message_in_transcript() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.transcript_mut().clear();
    model.show_stream_activity("qwen3");

    apply_conversation_event(
        &mut model,
        None,
        ConversationEvent::Failed {
            message: "request /v1/chat/completions: connection refused".to_string(),
        },
    );

    assert_eq!(
        model.transcript_plain_items(),
        vec!["■ request /v1/chat/completions: connection refused".to_string(),]
    );
    assert!(model.current_status_notice_text().is_empty());
    assert!(!model.current_stream_activity_render_result().has_content);
}

#[test]
fn conversation_failure_formats_provider_json_error() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.transcript_mut().clear();
    model.show_stream_activity("qwen3");

    apply_conversation_event(
        &mut model,
        None,
        ConversationEvent::Failed {
            message: "provider error HTTP 401: Invalid status code 401 Unauthorized with message:\n{\"type\":\"error\",\"error\":{\"type\":\"CreditsError\",\"message\":\"Insufficient balance...\"}}".to_string(),
        },
    );

    assert_eq!(
        model.transcript_plain_items(),
        vec![
            "■ provider error HTTP 401: Invalid status code 401 Unauthorized with message:\n  {\n    \"error\": {\n      \"message\": \"Insufficient balance...\",\n      \"type\": \"CreditsError\"\n    },\n    \"type\": \"error\"\n  }".to_string(),
        ]
    );
    assert!(model.current_status_notice_text().is_empty());
    assert!(!model.current_stream_activity_render_result().has_content);
}

#[test]
fn conversation_retry_event_shows_reconnecting_activity() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.transcript_mut().clear();
    model.show_stream_activity("qwen3");

    apply_conversation_event(
        &mut model,
        None,
        ConversationEvent::Retrying {
            message: "Reconnecting... 1/3".to_string(),
        },
    );

    let activity = model.current_stream_activity_render_result().plain_line;
    assert!(activity.contains("Reconnecting... 1/3"));
    assert!(model.transcript_plain_items().is_empty());
}

#[test]
fn conversation_progress_after_retry_restores_previous_activity_header() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(70, 6);
    model.transcript_mut().clear();
    model.show_stream_activity_with_header("Generating");

    apply_conversation_event(
        &mut model,
        None,
        ConversationEvent::Retrying {
            message: "Reconnecting... 1/3".to_string(),
        },
    );
    assert!(
        model
            .current_stream_activity_render_result()
            .plain_line
            .contains("Reconnecting... 1/3")
    );

    apply_conversation_event(
        &mut model,
        None,
        ConversationEvent::OutputTokenEstimate { total_tokens: 32 },
    );

    let activity = model
        .current_stream_activity_render_result_at(
            std::time::Instant::now() + std::time::Duration::from_millis(120),
        )
        .plain_line;
    assert!(activity.contains("Generating"));
    assert!(activity.contains("↓ 32 tokens"));
    assert!(!activity.contains("Reconnecting... 1/3"));
}

#[test]
fn conversation_retry_clears_failed_attempt_activity_progress() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(80, 6);
    model.transcript_mut().clear();
    model.show_stream_activity_with_header("Generating");

    apply_conversation_event(
        &mut model,
        None,
        ConversationEvent::OutputTokenEstimate { total_tokens: 80 },
    );
    apply_conversation_event(
        &mut model,
        None,
        ConversationEvent::Thinking { is_thinking: true },
    );

    apply_conversation_event(
        &mut model,
        None,
        ConversationEvent::Retrying {
            message: "Reconnecting... 1/3".to_string(),
        },
    );

    let retry_activity = model
        .current_stream_activity_render_result_at(
            std::time::Instant::now() + std::time::Duration::from_millis(120),
        )
        .plain_line;
    assert!(retry_activity.contains("Reconnecting... 1/3"));
    assert!(!retry_activity.contains("thinking"));
    assert!(!retry_activity.contains("tokens"));

    apply_conversation_event(
        &mut model,
        None,
        ConversationEvent::OutputTokenEstimate { total_tokens: 32 },
    );

    let resumed_activity = model
        .current_stream_activity_render_result_at(
            std::time::Instant::now() + std::time::Duration::from_millis(120),
        )
        .plain_line;
    assert!(resumed_activity.contains("Generating"));
    assert!(resumed_activity.contains("↓ 32 tokens"));
    assert!(!resumed_activity.contains("↓ 80 tokens"));
    assert!(!resumed_activity.contains("thinking"));
}

#[test]
fn conversation_retry_discards_streamed_expanded_reasoning_from_failed_attempt() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            show_reasoning_content: true,
            reasoning_display_mode: ReasoningDisplayMode::Expanded,
            ..ModelOptions::default()
        },
    );
    let target = RuntimeTarget::provider("local", "qwen3");
    model.transcript_mut().clear();
    model.show_stream_activity("qwen3");

    model.apply_runtime_event(RuntimeEvent::ReasoningDelta {
        target: target.clone(),
        content: "先分析".to_string(),
    });
    model.apply_runtime_event(RuntimeEvent::AssistantDelta {
        target,
        content: "先给结论".to_string(),
    });

    assert_eq!(model.transcript_plain_items(), vec!["先分析".to_string()]);

    apply_conversation_event(
        &mut model,
        None,
        ConversationEvent::Retrying {
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
fn conversation_token_estimate_updates_activity_without_finishing_request() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(70, 6);
    model.transcript_mut().clear();
    model.show_stream_activity("qwen3");

    apply_conversation_event(
        &mut model,
        None,
        ConversationEvent::OutputTokenEstimate { total_tokens: 32 },
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
fn conversation_token_estimate_starts_activity_for_tool_only_stream() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(70, 6);
    model.transcript_mut().clear();

    apply_conversation_event(
        &mut model,
        Some(RuntimeTarget::provider("local", "qwen3")),
        ConversationEvent::OutputTokenEstimate { total_tokens: 57 },
    );

    let activity = model
        .current_stream_activity_render_result_at(
            std::time::Instant::now() + std::time::Duration::from_millis(120),
        )
        .plain_line;
    assert!(activity.contains("↓ 57 tokens"));
    assert!(model.current_stream_activity_render_result().has_content);
    assert!(model.transcript_plain_items().is_empty());
}

#[test]
fn conversation_input_token_estimate_updates_activity_without_finishing_request() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(70, 6);
    model.transcript_mut().clear();
    model.show_stream_activity("qwen3");

    apply_conversation_event(
        &mut model,
        None,
        ConversationEvent::OutputTokenEstimate { total_tokens: 32 },
    );
    apply_conversation_event(
        &mut model,
        None,
        ConversationEvent::InputTokenEstimate { total_tokens: 12 },
    );

    let activity = model
        .current_stream_activity_render_result_at(
            std::time::Instant::now() + std::time::Duration::from_millis(120),
        )
        .plain_line;
    assert!(activity.contains("↑ 44 tokens"));
    assert!(model.current_stream_activity_render_result().has_content);
    assert!(model.transcript_plain_items().is_empty());
}

#[test]
fn conversation_tool_started_appends_runtime_tool_activity() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(70, 6);
    model.transcript_mut().clear();
    model.show_stream_activity("qwen3");

    apply_conversation_event(
        &mut model,
        Some(RuntimeTarget::provider("local", "qwen3")),
        ConversationEvent::ToolActivityStarted {
            activity: RuntimeToolActivity {
                activity_id: "call-1".to_string(),
                title: "Read Cargo.toml".to_string(),
                kind: RuntimeToolKind::Read,
                status: RuntimeToolActivityStatus::InProgress,
                content: vec![RuntimeToolActivityContent::Text(
                    r#"{ "path": "Cargo.toml" }"#.to_string(),
                )],
                locations: Vec::new(),
                raw_input: None,
                raw_output: None,
            },
        },
    );

    let transcript = model.transcript_plain_items().join("\n");
    assert!(transcript.contains("Read Cargo.toml"));
    assert!(model.transcript_mut().source_messages().is_empty());
}

#[test]
fn conversation_tool_finished_updates_runtime_tool_activity() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(70, 6);
    model.transcript_mut().clear();
    model.show_stream_activity("qwen3");

    apply_conversation_event(
        &mut model,
        Some(RuntimeTarget::provider("local", "qwen3")),
        ConversationEvent::ToolActivityStarted {
            activity: RuntimeToolActivity {
                activity_id: "call-1".to_string(),
                title: "Read Cargo.toml".to_string(),
                kind: RuntimeToolKind::Read,
                status: RuntimeToolActivityStatus::InProgress,
                content: vec![RuntimeToolActivityContent::Text(
                    r#"{ "path": "Cargo.toml" }"#.to_string(),
                )],
                locations: Vec::new(),
                raw_input: None,
                raw_output: None,
            },
        },
    );
    apply_conversation_event(
        &mut model,
        Some(RuntimeTarget::provider("local", "qwen3")),
        ConversationEvent::ToolActivityUpdated {
            update: RuntimeToolActivityUpdate {
                activity_id: "call-1".to_string(),
                status: Some(RuntimeToolActivityStatus::Completed),
                content: Some(vec![RuntimeToolActivityContent::Text(
                    "1\t[package]".to_string(),
                )]),
                ..RuntimeToolActivityUpdate::default()
            },
        },
    );

    let transcript = model.transcript_plain_items().join("\n");
    assert!(transcript.contains("Read Cargo.toml"));
    assert!(!transcript.contains("[package]"));
    assert!(
        model
            .runtime_tool_activity_item_index_from_runtime("call-1")
            .is_some()
    );
    assert!(model.transcript_mut().source_messages().is_empty());
}

#[test]
fn conversation_send_effect_starts_conversation_target() {
    let mut model = Model::new(StartupBannerOptions::default());
    let mut runtime_coordinator = TestRuntimeCoordinator::default();
    let request = ConversationTurnRequest::new(
        "local",
        ProviderKind::OpenAiCompatible,
        "qwen3",
        None,
        None,
        None,
        ChatMessage::user("hello".to_string()),
    );

    run_send_conversation_turn_effect(&mut model, &mut runtime_coordinator, request);

    assert_eq!(
        runtime_coordinator
            .conversation_request
            .as_ref()
            .map(ConversationTurnRequest::target),
        Some(RuntimeTarget::provider("local", "qwen3"))
    );
    assert!(model.current_stream_activity_render_result().has_content);
}

#[test]
fn truncate_conversation_command_records_retained_turns() {
    let mut runtime_coordinator = TestRuntimeCoordinator::default();

    runtime_coordinator
        .dispatch_runtime_command(RuntimeCommand::truncate_conversation(2))
        .expect("truncate command should be accepted");

    assert_eq!(
        runtime_coordinator.conversation_retained_user_turns,
        Some(2)
    );
}

#[test]
fn conversation_turn_request_keeps_runtime_target_in_core_dto() {
    let request = ConversationTurnRequest::new(
        "local",
        ProviderKind::OpenAiCompatible,
        "qwen3",
        None,
        None,
        None,
        ChatMessage::user("hello".to_string()),
    );

    assert_eq!(request.target(), RuntimeTarget::provider("local", "qwen3"));
}

#[test]
fn interrupt_conversation_clears_runtime_without_immediate_notice() {
    let mut runtime_coordinator = TestRuntimeCoordinator {
        conversation_running: true,
        ..TestRuntimeCoordinator::default()
    };
    let mut model = Model::new(StartupBannerOptions::default());
    model.transcript_mut().clear();
    model.show_stream_activity("qwen3");

    apply_effect_if_needed_for_test(
        &mut model,
        &mut runtime_coordinator,
        Some(AppEffect::InterruptCurrentTurn),
    );

    assert!(runtime_coordinator.conversation_interrupted);
    assert!(!model.current_stream_activity_render_result().has_content);
    assert!(model.transcript_plain_items().is_empty());
}

#[test]
fn interrupt_receipt_and_runtime_event_append_single_system_message() {
    let mut runtime_coordinator = TestRuntimeCoordinator {
        conversation_running: true,
        ..TestRuntimeCoordinator::default()
    };
    let target = RuntimeTarget::provider("local", "qwen3");
    let mut model = Model::new(StartupBannerOptions::default());
    model.transcript_mut().clear();
    model.show_stream_activity("qwen3");

    apply_effect_if_needed_for_test(
        &mut model,
        &mut runtime_coordinator,
        Some(AppEffect::InterruptCurrentTurn),
    );
    model.apply_runtime_event(RuntimeEvent::Interrupted {
        target: Some(target),
    });

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

#[test]
fn startup_probe_without_background_leaves_palette_for_startup_timeout() {
    assert!(
        startup_palette_detection(terminal_probe::TerminalBackgroundProbeResult::unavailable())
            .is_none()
    );
}

#[test]
fn startup_probe_timeout_does_not_request_event_level_late_response_cleanup() {
    assert_eq!(
        terminal_probe::TerminalBackgroundProbeResult::timed_out(),
        terminal_probe::TerminalBackgroundProbeResult::unavailable()
    );
}

fn apply_effect_if_needed_for_test(
    model: &mut Model,
    runtime_coordinator: &mut TestRuntimeCoordinator,
    effect: Option<AppEffect>,
) {
    if let Some(AppEffect::InterruptCurrentTurn) = effect {
        run_interrupt_current_turn_effect(model, runtime_coordinator);
    }
}
