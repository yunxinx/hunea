use runtime_domain::{
    model_catalog::{ModelCatalog, ModelEntry, ModelProvider, ModelSelection, ModelSource},
    provider::ProviderKind,
    session::{
        RuntimeEvent, RuntimeIdentity, RuntimeTarget, SessionResumePayload, TranscriptReplayItem,
        TranscriptReplayRole,
    },
    session::{
        RuntimeTerminalSnapshot, RuntimeToolActivity, RuntimeToolActivityContent,
        RuntimeToolActivityStatus, RuntimeToolKind,
    },
};

use crate::{Model, ModelOptions, StartupBannerOptions, runtime::event_apply::RuntimeEventApply};

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
