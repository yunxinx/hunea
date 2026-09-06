use ratatui::{buffer::Buffer, style::Color};
use runtime_domain::{
    context_budget::{ContextTokenLimit, ContextWindowUsage},
    model_catalog::{ModelCatalog, ModelEntry, ModelProvider, ModelSelection, ModelSource},
    prompt_assembly::persistence::PromptAssemblyScope,
    prompt_assembly::{
        PromptAssemblyDiscoveredSkill, PromptAssemblyManagerSnapshot, PromptAssemblySelectionState,
        PromptSourceOrigin,
    },
    provider::ProviderKind,
    session::{
        ConversationResponse, RuntimeEvent, RuntimeIdentity, RuntimeTarget, SessionResumePayload,
        TranscriptReplayItem, TranscriptReplayRole, TranscriptSkillBinding,
        TranscriptUserAttachment, TranscriptUserMessage,
    },
    session::{
        RuntimeTerminalSnapshot, RuntimeToolActivity, RuntimeToolActivityContent,
        RuntimeToolActivityStatus, RuntimeToolKind,
    },
};

use crate::{
    Model, ModelOptions, StartupBannerOptions, runtime::event_apply::RuntimeEventApply,
    test_helpers::render_model_buffer, theme::default_palette,
};

#[test]
fn session_resumed_rebuilds_visible_transcript_and_restores_model() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            model_catalog: model_catalog(),
            selected_model: Some(ModelSelection::new("local", "qwen2")),
            ..ModelOptions::default()
        },
    );
    model.append_assistant_message_from_runtime("current session should be replaced");

    model.apply_runtime_event(RuntimeEvent::SessionResumed {
        payload: SessionResumePayload {
            session_id: "session-1".to_string(),
            transcript: vec![
                TranscriptReplayItem::Message {
                    role: TranscriptReplayRole::User,
                    content: "hello resume".to_string(),
                },
                TranscriptReplayItem::Message {
                    role: TranscriptReplayRole::Assistant,
                    content: "resume answer".to_string(),
                },
            ],
            restored_model: Some(ModelSelection::new("local", "qwen3")),
        },
    });

    let transcript = model.transcript_plain_items().join("\n");
    assert!(transcript.contains("hello resume"));
    assert!(transcript.contains("resume answer"));
    assert!(!transcript.contains("current session should be replaced"));
    assert_eq!(
        model.selected_model(),
        Some(ModelSelection::new("local", "qwen3"))
    );
    assert_eq!(model.current_status_notice_text(), "");
    assert_eq!(
        model.active_toast_text_for_test(),
        Some("Resumed session session-1")
    );
}

#[test]
fn runtime_start_events_use_toasts_not_status_notice() {
    let mut model = Model::new(StartupBannerOptions::default());

    model.apply_runtime_event(RuntimeEvent::Started {
        target: RuntimeTarget::provider("local", "qwen3"),
        identity: RuntimeIdentity::new("Qwen Runtime"),
    });

    assert_eq!(model.current_status_notice_text(), "");
    assert_eq!(
        model.active_toast_text_for_test(),
        Some("Runtime ready: Qwen Runtime")
    );

    let mut model = Model::new(StartupBannerOptions::default());
    model.apply_runtime_event(RuntimeEvent::StartFailed {
        target: Some(RuntimeTarget::provider("local", "qwen3")),
        message: "connection refused".to_string(),
    });

    assert_eq!(model.current_status_notice_text(), "");
    assert_eq!(
        model.active_toast_text_for_test(),
        Some("Runtime start failed: connection refused")
    );
}

#[test]
fn prompt_missing_source_check_uses_single_aggregated_toast() {
    let mut model = Model::new(StartupBannerOptions::default());

    model.apply_runtime_event(RuntimeEvent::PromptAssemblyMissingSourcesChecked {
        missing_count: 2,
    });

    assert_eq!(model.current_status_notice_text(), "");
    assert_eq!(
        model.active_toast_text_for_test(),
        Some("2 prompt sources are missing; open /prompt to repair them")
    );
}

#[test]
fn prompt_missing_source_check_skips_toast_when_nothing_is_missing() {
    let mut model = Model::new(StartupBannerOptions::default());

    model.apply_runtime_event(RuntimeEvent::PromptAssemblyMissingSourcesChecked {
        missing_count: 0,
    });

    assert_eq!(model.active_toast_text_for_test(), None);
}

#[test]
fn session_resumed_trusts_historical_model_selection_without_catalog_check() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            model_catalog: model_catalog(),
            selected_model: Some(ModelSelection::new("local", "qwen2")),
            ..ModelOptions::default()
        },
    );

    model.apply_runtime_event(RuntimeEvent::SessionResumed {
        payload: SessionResumePayload {
            session_id: "session-1".to_string(),
            transcript: vec![TranscriptReplayItem::Message {
                role: TranscriptReplayRole::User,
                content: "hello resume".to_string(),
            }],
            restored_model: Some(ModelSelection::new("local", "missing-model")),
        },
    });

    assert_eq!(
        model.selected_model(),
        Some(ModelSelection::new("local", "missing-model"))
    );
    let transcript = model.transcript_plain_items().join("\n");
    assert!(transcript.contains("hello resume"));
}

#[test]
fn session_resumed_replays_tool_items_as_tool_results() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            selected_model: Some(ModelSelection::new("local", "qwen2")),
            ..ModelOptions::default()
        },
    );

    model.apply_runtime_event(RuntimeEvent::SessionResumed {
        payload: SessionResumePayload {
            session_id: "session-1".to_string(),
            transcript: vec![TranscriptReplayItem::ToolResult {
                content: "workspace output".to_string(),
            }],
            restored_model: None,
        },
    });

    let transcript = model.transcript_plain_items().join("\n");
    assert!(
        transcript.contains("● workspace output"),
        "tool replay should use the native tool transcript item: {transcript:?}"
    );
    assert!(
        !transcript.contains("■ workspace output"),
        "tool replay must not be rendered as a system message: {transcript:?}"
    );
}

#[test]
fn session_resumed_replays_terminal_snapshot_for_tool_activity() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            selected_model: Some(ModelSelection::new("local", "qwen2")),
            ..ModelOptions::default()
        },
    );

    model.apply_runtime_event(RuntimeEvent::SessionResumed {
        payload: SessionResumePayload {
            session_id: "session-1".to_string(),
            transcript: vec![
                TranscriptReplayItem::ToolActivity {
                    activity: RuntimeToolActivity {
                        activity_id: "call-terminal".to_string(),
                        title: "Run tests".to_string(),
                        kind: RuntimeToolKind::Execute,
                        status: RuntimeToolActivityStatus::Completed,
                        content: vec![RuntimeToolActivityContent::Terminal {
                            terminal_id: "term-1".to_string(),
                        }],
                        locations: Vec::new(),
                        raw_input: None,
                        raw_output: None,
                    },
                },
                TranscriptReplayItem::TerminalSnapshot {
                    snapshot: RuntimeTerminalSnapshot {
                        terminal_id: "term-1".to_string(),
                        command: Some("cargo check".to_string()),
                        cwd: None,
                        output: "Checking hunea\nFinished".to_string(),
                        truncated: false,
                        exit_status: None,
                        released: true,
                    },
                },
            ],
            restored_model: None,
        },
    });

    let transcript = model.transcript_plain_items().join("\n");
    assert!(transcript.contains("Checking hunea"));
    assert!(transcript.contains("Finished"));
    assert!(!transcript.contains("runtime terminal unavailable"));
}

#[test]
fn session_resumed_keeps_valid_skill_binding_colored() {
    let mut model = model_with_manual_skill("code-review");

    model.apply_runtime_event(RuntimeEvent::SessionResumed {
        payload: SessionResumePayload {
            session_id: "session-1".to_string(),
            transcript: vec![TranscriptReplayItem::BoundUserMessage {
                message: TranscriptUserMessage {
                    content: "@code-review please inspect".to_string(),
                    attachments: Vec::new(),
                    skill_bindings: vec![TranscriptSkillBinding {
                        skill_name: "code-review".to_string(),
                        origin: PromptSourceOrigin::Project,
                        skill_path: "/tmp/code-review/SKILL.md".to_string(),
                        start_char: 0,
                        end_char: 12,
                    }],
                    custom_prompt_bindings: Vec::new(),
                },
            }],
            restored_model: None,
        },
    });

    let buffer = render_model_buffer(&mut model, 60, 10);
    assert_text_cells_use_color(&buffer, "@code-review", default_palette().command_accent);
}

#[test]
fn session_resumed_keeps_labeled_image_attachment_colored_without_duplicate_summary() {
    let mut model = model_with_manual_skill("unused");

    model.apply_runtime_event(RuntimeEvent::SessionResumed {
        payload: SessionResumePayload {
            session_id: "session-1".to_string(),
            transcript: vec![TranscriptReplayItem::BoundUserMessage {
                message: TranscriptUserMessage {
                    content: "啊\n[Image #1] inspect".to_string(),
                    attachments: vec![TranscriptUserAttachment::local_image(
                        "iVBORw0KGgo=",
                        "image/png",
                        Some("assets/a.png".to_string()),
                    )],
                    skill_bindings: Vec::new(),
                    custom_prompt_bindings: Vec::new(),
                },
            }],
            restored_model: None,
        },
    });

    let transcript = model.transcript_plain_items().join("\n");
    assert!(transcript.contains("[Image #1] inspect"));
    assert!(!transcript.contains("Attached image"));

    let buffer = render_model_buffer(&mut model, 60, 10);
    assert_text_cells_use_color(&buffer, "[Image #1]", default_palette().command_accent);
}

#[test]
fn session_resumed_drops_missing_skill_binding_color() {
    let mut model = model_with_manual_skill("other-skill");

    model.apply_runtime_event(RuntimeEvent::SessionResumed {
        payload: SessionResumePayload {
            session_id: "session-1".to_string(),
            transcript: vec![TranscriptReplayItem::BoundUserMessage {
                message: TranscriptUserMessage {
                    content: "@code-review please inspect".to_string(),
                    attachments: Vec::new(),
                    skill_bindings: vec![TranscriptSkillBinding {
                        skill_name: "code-review".to_string(),
                        origin: PromptSourceOrigin::Project,
                        skill_path: "/tmp/code-review/SKILL.md".to_string(),
                        start_char: 0,
                        end_char: 12,
                    }],
                    custom_prompt_bindings: Vec::new(),
                },
            }],
            restored_model: None,
        },
    });

    let buffer = render_model_buffer(&mut model, 60, 10);
    assert_text_cells_do_not_use_color(&buffer, "@code-review", default_palette().command_accent);
}

#[test]
fn message_finished_updates_last_context_usage_and_retains_it_without_new_usage() {
    let mut model = Model::new(StartupBannerOptions::default());
    let usage = ContextWindowUsage {
        limit: ContextTokenLimit::new(128_000).expect("test limit should be non-zero"),
        used: 32_000,
    };

    model.apply_runtime_event(RuntimeEvent::MessageFinished {
        target: Some(RuntimeTarget::provider("local", "qwen3")),
        response: ConversationResponse::assistant_text("done"),
        finish_reason: None,
        metrics: None,
        context_usage: Some(usage),
    });
    assert_eq!(model.last_context_usage(), Some(usage));

    // usage 缺失的后续完成事件保留上次数值,与 metrics 的"最近一次"语义一致。
    model.apply_runtime_event(RuntimeEvent::MessageFinished {
        target: Some(RuntimeTarget::provider("local", "qwen3")),
        response: ConversationResponse::assistant_text("again"),
        finish_reason: None,
        metrics: None,
        context_usage: None,
    });
    assert_eq!(model.last_context_usage(), Some(usage));
}

#[test]
fn session_resumed_resets_last_context_usage() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_last_context_usage(Some(ContextWindowUsage {
        limit: ContextTokenLimit::new(128_000).expect("test limit should be non-zero"),
        used: 32_000,
    }));

    model.apply_runtime_event(RuntimeEvent::SessionResumed {
        payload: SessionResumePayload {
            session_id: "session-1".to_string(),
            transcript: Vec::new(),
            restored_model: None,
        },
    });

    // v1 不在 resume 路径恢复占用数据,切换会话后应隐藏等待下一次请求完成。
    assert_eq!(model.last_context_usage(), None);
}

fn model_catalog() -> ModelCatalog {
    ModelCatalog::new(vec![ModelProvider::new(
        "local",
        ProviderKind::OpenAiCompatible,
        "Local",
        true,
        ModelSource::Configured,
        vec![
            ModelEntry::new("qwen2", None, ModelSource::Configured),
            ModelEntry::new("qwen3", None, ModelSource::Configured),
        ],
    )])
}

fn model_with_manual_skill(skill_name: &str) -> Model {
    let mut prompt_assembly = PromptAssemblyManagerSnapshot::default();
    prompt_assembly.candidates.manual_skills = vec![PromptAssemblyDiscoveredSkill {
        skill_name: skill_name.to_string(),
        title: skill_name.to_string(),
        description: "Manual skill".to_string(),
        origin: PromptSourceOrigin::Project,
        selection_scope: PromptAssemblyScope::Project,
        skill_path: format!("/tmp/{skill_name}/SKILL.md").into(),
        body: "# Manual Skill".to_string(),
        selection: PromptAssemblySelectionState::from_parts(false, false, None),
    }];
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            prompt_assembly: Some(prompt_assembly),
            ..ModelOptions::default()
        },
    );
    model.set_window(60, 10);
    model.set_palette(default_palette(), true);
    model
}

fn assert_text_cells_use_color(buffer: &Buffer, text: &str, expected: Color) {
    let (row, column) = find_text(buffer, text).expect("text should render in buffer");
    for offset in 0..text.chars().count() {
        assert_eq!(buffer[(column + offset as u16, row)].fg, expected);
    }
}

fn assert_text_cells_do_not_use_color(buffer: &Buffer, text: &str, unexpected: Color) {
    let (row, column) = find_text(buffer, text).expect("text should render in buffer");
    for offset in 0..text.chars().count() {
        assert_ne!(buffer[(column + offset as u16, row)].fg, unexpected);
    }
}

fn find_text(buffer: &Buffer, needle: &str) -> Option<(u16, u16)> {
    for row in 0..buffer.area.height {
        let rendered = (0..buffer.area.width)
            .map(|column| buffer[(column, row)].symbol())
            .collect::<String>();
        if let Some(byte_index) = rendered.find(needle) {
            let column = rendered[..byte_index].chars().count();
            return Some((row, column as u16));
        }
    }
    None
}

fn message_finished_event() -> RuntimeEvent {
    RuntimeEvent::MessageFinished {
        target: Some(RuntimeTarget::provider("local", "qwen3")),
        response: ConversationResponse::assistant_text("final answer"),
        finish_reason: None,
        metrics: None,
        context_usage: None,
    }
}

fn scrollable_model() -> Model {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(40, 6);
    model.set_palette(default_palette(), true);
    for index in 0..20 {
        model.append_assistant_message_from_runtime(format!("history message {index}"));
    }
    model
}

#[test]
fn message_finished_while_pinned_shows_no_pill_and_follows_bottom() {
    let mut model = scrollable_model();
    assert!(model.document_pinned_to_bottom());

    model.apply_runtime_event(message_finished_event());

    assert_eq!(model.attention_pill_new_message_count_for_test(), None);
    assert!(model.document_pinned_to_bottom());
}

#[test]
fn message_finished_while_scrolled_up_shows_pill_and_keeps_viewport() {
    let mut model = scrollable_model();
    model.scroll_document_by(-4);
    assert!(!model.document_pinned_to_bottom());
    let viewport_y = model.document_runtime.viewport_y;

    model.apply_runtime_event(message_finished_event());

    assert_eq!(model.attention_pill_new_message_count_for_test(), Some(1));
    assert!(!model.document_pinned_to_bottom());
    assert_eq!(
        model.document_runtime.viewport_y, viewport_y,
        "final message must not move a manually scrolled viewport"
    );
}

#[test]
fn message_finished_behind_fullscreen_modal_shows_pill_and_keeps_layer() {
    let mut model = scrollable_model();
    model.open_session_picker_loading();
    assert!(model.top_modal_layer().is_some());

    model.apply_runtime_event(message_finished_event());

    assert_eq!(model.attention_pill_new_message_count_for_test(), Some(1));
    assert!(
        model.top_modal_layer().is_some(),
        "final message must not close the active fullscreen modal layer"
    );
}

#[test]
fn assistant_delta_flush_keeps_viewport_during_manual_scroll() {
    let mut model = scrollable_model();
    model.scroll_document_by(-4);
    let viewport_y = model.document_runtime.viewport_y;

    model.apply_runtime_event(RuntimeEvent::AssistantDelta {
        target: RuntimeTarget::provider("local", "qwen3"),
        content: "streamed content".to_string(),
    });
    model.flush_runtime_response_buffer();

    assert_eq!(
        model.document_runtime.viewport_y, viewport_y,
        "flushed streamed content must not move a manually scrolled viewport"
    );
    assert!(!model.document_pinned_to_bottom());
}

#[test]
fn session_resume_replay_restores_bottom_follow() {
    let mut model = scrollable_model();
    model.scroll_document_by(-4);
    assert!(!model.document_pinned_to_bottom());

    model.apply_runtime_event(RuntimeEvent::SessionResumed {
        payload: SessionResumePayload {
            session_id: "session-replay".to_string(),
            transcript: vec![TranscriptReplayItem::Message {
                role: TranscriptReplayRole::Assistant,
                content: "replayed".to_string(),
            }],
            restored_model: None,
        },
    });

    assert!(
        model.document_runtime.follow_bottom,
        "session replay must rebuild the transcript pinned to bottom"
    );
}

#[test]
fn agent_observation_projection_events_are_absorbed_without_touching_session_state() {
    use runtime_domain::agent::{
        AgentObservationId, AgentObservationRejection, AgentObservationRequestId,
        AgentOverviewSnapshot, AgentProjectionEvent, AgentProjectionRevision,
        AgentRuntimeGeneration,
    };

    let mut model = scrollable_model();
    let transcript_before = model.transcript_plain_items().join("\n");
    let viewport_y = model.document_runtime.viewport_y;

    let projection_events = [
        AgentProjectionEvent::AgentsOverviewSnapshotLoaded {
            request_id: AgentObservationRequestId::new(1),
            snapshot: AgentOverviewSnapshot {
                observation_id: AgentObservationId::new(1),
                generation: AgentRuntimeGeneration::new(1),
                revision: AgentProjectionRevision::new(1),
                rows: Vec::new(),
            },
        },
        AgentProjectionEvent::AgentObservationRejected {
            request_id: AgentObservationRequestId::new(2),
            reason: AgentObservationRejection::UnknownAgent,
        },
    ];

    for projection_event in projection_events {
        model.apply_runtime_event(RuntimeEvent::AgentProjection(Box::new(projection_event)));
    }

    // observation 类 TUI surface 尚未接入：事件被无害吸收，transcript/viewport 状态不变。
    assert_eq!(model.transcript_plain_items().join("\n"), transcript_before);
    assert_eq!(model.document_runtime.viewport_y, viewport_y);
}

fn agent_launch_child_fixture(
    agent_id: u64,
    title: &str,
) -> runtime_domain::agent::AgentLaunchChildSnapshot {
    use runtime_domain::agent::{AgentObjective, AgentObjectiveSummary, AgentTitle};
    runtime_domain::agent::AgentLaunchChildSnapshot {
        agent_id: runtime_domain::agent::AgentId::new(agent_id),
        title: AgentTitle::resolve(
            &AgentObjective::new("fallback objective").expect("objective should be valid"),
            Some(title),
        )
        .expect("title should resolve"),
        objective: AgentObjectiveSummary::from_objective(
            &AgentObjective::new("objective body").expect("objective should be valid"),
        )
        .expect("objective summary should resolve"),
    }
}

fn agent_launch_snapshot_fixture(
    children: Vec<runtime_domain::agent::AgentLaunchChildSnapshot>,
) -> runtime_domain::agent::AgentLaunchSnapshot {
    runtime_domain::agent::AgentLaunchSnapshot {
        group_id: runtime_domain::agent::AgentLaunchGroupId::new(7),
        parent_agent_id: runtime_domain::agent::AgentId::MAIN,
        parent_turn_id: runtime_domain::agent::AgentTurnId::new(9),
        children,
        occurred_at_ms: 42,
    }
}

fn agent_outcome_snapshot_fixture(
    outcome: runtime_domain::agent::AgentOutcome,
) -> runtime_domain::agent::AgentOutcomeSnapshot {
    use runtime_domain::agent::{AgentObjective, AgentOutcomeSummary, AgentTitle};
    runtime_domain::agent::AgentOutcomeSnapshot {
        agent_id: runtime_domain::agent::AgentId::new(2),
        title: AgentTitle::resolve(
            &AgentObjective::new("fallback objective").expect("objective should be valid"),
            Some("research task"),
        )
        .expect("title should resolve"),
        group_id: Some(runtime_domain::agent::AgentLaunchGroupId::new(7)),
        parent_agent_id: Some(runtime_domain::agent::AgentId::MAIN),
        parent_turn_id: Some(runtime_domain::agent::AgentTurnId::new(9)),
        outcome,
        occurred_at_ms: 43,
        summary: Some(
            AgentOutcomeSummary::new("Child Agent completed").expect("summary should resolve"),
        ),
    }
}

#[test]
fn agent_document_facts_append_semantic_transcript_items() {
    use runtime_domain::agent::AgentProjectionEvent;

    let mut model = scrollable_model();

    model.apply_runtime_event(RuntimeEvent::AgentProjection(Box::new(
        AgentProjectionEvent::AgentLaunchFact {
            snapshot: agent_launch_snapshot_fixture(vec![agent_launch_child_fixture(
                2,
                "research task",
            )]),
        },
    )));
    model.apply_runtime_event(RuntimeEvent::AgentProjection(Box::new(
        AgentProjectionEvent::AgentLaunchFact {
            snapshot: agent_launch_snapshot_fixture(vec![
                agent_launch_child_fixture(3, "first task"),
                agent_launch_child_fixture(4, "second task"),
            ]),
        },
    )));
    model.apply_runtime_event(RuntimeEvent::AgentProjection(Box::new(
        AgentProjectionEvent::AgentOutcomeFact {
            snapshot: agent_outcome_snapshot_fixture(
                runtime_domain::agent::AgentOutcome::Completed,
            ),
        },
    )));

    let transcript = model.transcript_plain_items().join("\n");
    assert!(transcript.contains("● Launched research task"));
    assert!(transcript.contains("● Launched 2 agents"));
    assert!(transcript.contains("  ├ first task"));
    assert!(transcript.contains("  └ second task"));
    assert!(transcript.contains("● Completed research task"));
    assert!(transcript.contains("  └ Child Agent completed"));
    // Agent 事实不降级成 error-styled system message。
    assert!(!transcript.contains('■'));
    // 追加事实后仍保持贴底跟随（非手动滚动）。
    assert!(model.document_pinned_to_bottom());
}

#[test]
fn agent_document_fact_appends_keep_manual_scroll_viewport() {
    use runtime_domain::agent::AgentProjectionEvent;

    let mut model = scrollable_model();
    model.scroll_document_by(-4);
    let viewport_y = model.document_runtime.viewport_y;

    model.apply_runtime_event(RuntimeEvent::AgentProjection(Box::new(
        AgentProjectionEvent::AgentLaunchFact {
            snapshot: agent_launch_snapshot_fixture(vec![agent_launch_child_fixture(
                2,
                "research task",
            )]),
        },
    )));

    assert_eq!(
        model.document_runtime.viewport_y, viewport_y,
        "appended agent facts must not move a manually scrolled viewport"
    );
}

#[test]
fn agent_projection_deltas_do_not_rewrite_appended_document_facts() {
    use runtime_domain::agent::{
        AgentId, AgentObservationId, AgentOverviewDelta, AgentOverviewDeltaKind, AgentOverviewRow,
        AgentProjectionEvent, AgentProjectionRevision, AgentRuntimeGeneration, AgentViewSnapshot,
    };

    let mut model = scrollable_model();
    model.apply_runtime_event(RuntimeEvent::AgentProjection(Box::new(
        AgentProjectionEvent::AgentLaunchFact {
            snapshot: agent_launch_snapshot_fixture(vec![agent_launch_child_fixture(
                2,
                "research task",
            )]),
        },
    )));
    let fact_item = model
        .transcript
        .item(model.transcript.len() - 1)
        .expect("launch fact item should exist")
        .clone();

    // status/permission/metrics delta 类投影事件不得触碰 transcript。
    let revision = AgentProjectionRevision::new(2);
    let delta = AgentProjectionEvent::AgentsOverviewUpdated {
        delta: AgentOverviewDelta {
            observation_id: AgentObservationId::new(1),
            generation: AgentRuntimeGeneration::new(1),
            revision,
            kind: AgentOverviewDeltaKind::Upsert(AgentOverviewRow {
                agent_id: AgentId::new(2),
                title: runtime_domain::agent::AgentTitle::resolve(
                    &runtime_domain::agent::AgentObjective::new("fallback")
                        .expect("objective should be valid"),
                    Some("renamed title"),
                )
                .expect("title should resolve"),
                status: runtime_domain::agent::AgentProjectionStatus::Working,
                latest_activity: runtime_domain::agent::AgentActivitySummary::Thinking,
                elapsed_ms: Some(1200),
                tool_uses: Some(3),
                token_usage: Some(2048),
            }),
        },
    };
    model.apply_runtime_event(RuntimeEvent::AgentProjection(Box::new(delta)));
    model.apply_runtime_event(RuntimeEvent::AgentProjection(Box::new(
        AgentProjectionEvent::AgentViewUpdated {
            snapshot: AgentViewSnapshot {
                observation_id: AgentObservationId::new(2),
                generation: AgentRuntimeGeneration::new(1),
                revision,
                transcript: runtime_domain::agent::AgentTranscriptSnapshot {
                    observation_id: AgentObservationId::new(2),
                    generation: AgentRuntimeGeneration::new(1),
                    revision,
                    agent_id: AgentId::new(2),
                    title: runtime_domain::agent::AgentTitle::resolve(
                        &runtime_domain::agent::AgentObjective::new("fallback")
                            .expect("objective should be valid"),
                        Some("renamed title"),
                    )
                    .expect("title should resolve"),
                    status: runtime_domain::agent::AgentProjectionStatus::Working,
                    items: Vec::new(),
                },
                preview: runtime_domain::agent::AgentPreviewSnapshot {
                    generation: AgentRuntimeGeneration::new(1),
                    revision,
                    agent_id: AgentId::new(2),
                    title: runtime_domain::agent::AgentTitle::resolve(
                        &runtime_domain::agent::AgentObjective::new("fallback")
                            .expect("objective should be valid"),
                        Some("renamed title"),
                    )
                    .expect("title should resolve"),
                    status: runtime_domain::agent::AgentProjectionStatus::Working,
                    latest_activity: runtime_domain::agent::AgentActivitySummary::Thinking,
                    elapsed_ms: Some(1200),
                    latest_committed_answer: None,
                    permission: None,
                },
            },
        },
    )));

    let transcript = model.transcript_plain_items().join("\n");
    assert!(
        transcript.contains("● Launched research task"),
        "launch fact must keep the frozen title"
    );
    assert!(!transcript.contains("renamed title"));
    // permission request/decision 只属于后续 panel/pill surface，不进 document timeline。
    let item_count_after_deltas = model.transcript.len();
    model.apply_runtime_event(RuntimeEvent::AgentProjection(Box::new(
        AgentProjectionEvent::AgentPermissionUpdated {
            update: runtime_domain::agent::AgentPermissionUpdate {
                agent_id: AgentId::new(2),
                generation: AgentRuntimeGeneration::new(1),
                request: None,
            },
        },
    )));
    assert_eq!(
        model.transcript.len(),
        item_count_after_deltas,
        "permission projections must not append document items"
    );
    assert_eq!(
        model
            .transcript
            .item(model.transcript.len() - 1)
            .expect("launch fact item should still exist"),
        &fact_item,
        "delta projections must not rewrite the appended fact item"
    );
}

#[test]
fn agent_document_facts_resume_replay_matches_live_items() {
    use runtime_domain::agent::AgentProjectionEvent;

    let launch_snapshot =
        agent_launch_snapshot_fixture(vec![agent_launch_child_fixture(2, "research task")]);
    let outcome_snapshot =
        agent_outcome_snapshot_fixture(runtime_domain::agent::AgentOutcome::Failed);

    // live 路径：document fact 事件直接追加。
    let mut live_model = Model::new(StartupBannerOptions::default());
    live_model.apply_runtime_event(RuntimeEvent::AgentProjection(Box::new(
        AgentProjectionEvent::AgentLaunchFact {
            snapshot: launch_snapshot.clone(),
        },
    )));
    live_model.apply_runtime_event(RuntimeEvent::AgentProjection(Box::new(
        AgentProjectionEvent::AgentOutcomeFact {
            snapshot: outcome_snapshot.clone(),
        },
    )));

    // resume 路径：同一 typed snapshots 从 replay facts 重建。
    let mut resumed_model = Model::new(StartupBannerOptions::default());
    resumed_model.apply_runtime_event(RuntimeEvent::SessionResumed {
        payload: SessionResumePayload {
            session_id: "agent-facts-session".to_string(),
            transcript: vec![
                TranscriptReplayItem::AgentLaunch(launch_snapshot),
                TranscriptReplayItem::AgentOutcome(outcome_snapshot),
            ],
            restored_model: None,
        },
    });

    // 相同 facts 产生相同的 item 序列与渲染语义。live 路径的 model 带启动欢迎块，
    // resume 重建的 transcript 只含 replay facts，比较时跳过 banner 项。
    assert!(live_model.transcript.starts_with_startup_banner());
    let mut live_plain_items = live_model.transcript_plain_items();
    live_plain_items.remove(0);
    assert_eq!(live_plain_items, resumed_model.transcript_plain_items());
    for index in 0..resumed_model.transcript.len() {
        assert_eq!(
            live_model.transcript.item(index + 1),
            resumed_model.transcript.item(index),
            "agent fact items must be identical between live and resume paths"
        );
    }
    let transcript = resumed_model.transcript_plain_items().join("\n");
    assert!(transcript.contains("● Launched research task"));
    assert!(transcript.contains("● Failed research task"));
    assert!(transcript.contains("  └ Child Agent completed"));
}

#[test]
fn agent_outcome_replay_with_legacy_defaults_renders_without_group_identity() {
    // 旧 JSONL 没有新 metadata 字段：serde 安全默认值（group/parent 为 None）必须直接成立。
    let replay_json = r#"[
        {"type": "agent_launch", "payload": {
            "group_id": 7,
            "parent_agent_id": 1,
            "parent_turn_id": 9,
            "children": [
                {"agent_id": 2, "title": "legacy task", "objective": "legacy objective"}
            ],
            "occurred_at_ms": 42
        }},
        {"type": "agent_outcome", "payload": {
            "agent_id": 2,
            "title": "legacy task",
            "outcome": "cancelled",
            "occurred_at_ms": 43
        }}
    ]"#;
    let transcript: Vec<TranscriptReplayItem> = serde_json::from_str(replay_json)
        .expect("legacy agent replay facts should deserialize with safe defaults");

    let mut model = scrollable_model();
    model.apply_runtime_event(RuntimeEvent::SessionResumed {
        payload: SessionResumePayload {
            session_id: "legacy-agent-facts".to_string(),
            transcript,
            restored_model: None,
        },
    });

    let text = model.transcript_plain_items().join("\n");
    assert!(text.contains("● Launched legacy task"));
    assert!(text.contains("● Cancelled legacy task"));
}
