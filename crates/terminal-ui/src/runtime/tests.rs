use ratatui::{buffer::Buffer, style::Color};
use runtime_domain::{
    model_catalog::{ModelCatalog, ModelEntry, ModelProvider, ModelSelection, ModelSource},
    prompt_assembly::{
        PromptAssemblyDiscoveredSkill, PromptAssemblyInput, PromptAssemblyManagerSnapshot,
        PromptPreludeSnapshot, PromptSourceOrigin, resolve_prompt_assembly,
    },
    provider::ProviderKind,
    session::{
        RuntimeEvent, RuntimeIdentity, RuntimeTarget, SessionResumePayload, TranscriptReplayItem,
        TranscriptReplayRole, TranscriptSkillBinding, TranscriptUserMessage,
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
            requires_model_selection: true,
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
            requires_model_selection: true,
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
                    content: "$code-review please inspect".to_string(),
                    skill_bindings: vec![TranscriptSkillBinding {
                        skill_name: "code-review".to_string(),
                        origin: PromptSourceOrigin::Project,
                        skill_path: "/tmp/code-review/SKILL.md".to_string(),
                        start_char: 0,
                        end_char: 12,
                    }],
                },
            }],
            restored_model: None,
        },
    });

    let buffer = render_model_buffer(&mut model, 60, 10);
    assert_text_cells_use_color(&buffer, "$code-review", default_palette().command_accent);
}

#[test]
fn session_resumed_drops_missing_skill_binding_color() {
    let mut model = model_with_manual_skill("other-skill");

    model.apply_runtime_event(RuntimeEvent::SessionResumed {
        payload: SessionResumePayload {
            session_id: "session-1".to_string(),
            transcript: vec![TranscriptReplayItem::BoundUserMessage {
                message: TranscriptUserMessage {
                    content: "$code-review please inspect".to_string(),
                    skill_bindings: vec![TranscriptSkillBinding {
                        skill_name: "code-review".to_string(),
                        origin: PromptSourceOrigin::Project,
                        skill_path: "/tmp/code-review/SKILL.md".to_string(),
                        start_char: 0,
                        end_char: 12,
                    }],
                },
            }],
            restored_model: None,
        },
    });

    let buffer = render_model_buffer(&mut model, 60, 10);
    assert_text_cells_do_not_use_color(&buffer, "$code-review", default_palette().command_accent);
}

fn model_catalog() -> ModelCatalog {
    ModelCatalog::new(vec![ModelProvider::new(
        "local",
        ProviderKind::OpenAiCompatible,
        "Local",
        Some("http://127.0.0.1:1234/v1".to_string()),
        ModelSource::Configured,
        vec![
            ModelEntry::new("qwen2", None, ModelSource::Configured),
            ModelEntry::new("qwen3", None, ModelSource::Configured),
        ],
    )])
}

fn model_with_manual_skill(skill_name: &str) -> Model {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            prompt_assembly: Some(PromptAssemblyManagerSnapshot {
                snapshot: resolve_prompt_assembly(&PromptAssemblyInput::default()),
                prelude: PromptPreludeSnapshot::default(),
                sources: Vec::new(),
                discovered_skills: Vec::new(),
                manual_skills: vec![PromptAssemblyDiscoveredSkill {
                    skill_name: skill_name.to_string(),
                    title: skill_name.to_string(),
                    description: "Manual skill".to_string(),
                    origin: PromptSourceOrigin::Project,
                    skill_path: format!("/tmp/{skill_name}/SKILL.md"),
                    body: "# Manual Skill".to_string(),
                }],
                builtin_core_system_body: String::new(),
                global_core_system_override: None,
                project_core_system_override: None,
            }),
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
