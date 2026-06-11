use runtime_domain::{
    model_catalog::{ModelCatalog, ModelEntry, ModelProvider, ModelSelection, ModelSource},
    provider::ProviderKind,
    session::{RuntimeEvent, SessionResumePayload, TranscriptReplayItem, TranscriptReplayRole},
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
                TranscriptReplayItem {
                    role: TranscriptReplayRole::User,
                    content: "hello resume".to_string(),
                },
                TranscriptReplayItem {
                    role: TranscriptReplayRole::Assistant,
                    content: "resume answer".to_string(),
                },
            ],
            restored_model: Some("qwen3".to_string()),
            missing_model: None,
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
}

#[test]
fn session_resumed_requires_model_selection_when_historical_model_is_unavailable() {
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
            transcript: vec![TranscriptReplayItem {
                role: TranscriptReplayRole::User,
                content: "hello resume".to_string(),
            }],
            restored_model: Some("missing-model".to_string()),
            missing_model: None,
        },
    });

    assert_eq!(model.selected_model(), None);
    let transcript = model.transcript_plain_items().join("\n");
    assert!(transcript.contains("hello resume"));
    assert!(transcript.contains("Model from resumed session is unavailable"));
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
