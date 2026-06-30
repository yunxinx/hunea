use super::support::*;
use runtime_domain::prompt_assembly::{
    PromptPreludeSection, PromptPreludeSnapshot, PromptSourceKind, PromptSourceOrigin,
};

#[test]
fn resume_session_emits_transcript_and_restored_model() {
    let work_dir = temp_test_dir("resume-session-work");
    let store = Arc::new(InMemorySessionStore::new());
    let store_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("session store runtime should start");
    let header = SessionHeader {
        session_id: SessionId::new(),
        work_dir: work_dir.clone(),
        session_name: None,
        initial_model: "qwen2".to_string(),
        git_head: None,
        cli_version: None,
    };
    let session_id = store_runtime
        .block_on(async {
            let session_id = store.create_session(header.clone()).await?;
            store
                .append(
                    &session_id,
                    ConversationItem::text(Role::User, "hello resume"),
                )
                .await?;
            store
                .append_transcript_replay(
                    &session_id,
                    TranscriptReplayItem::Message {
                        role: TranscriptReplayRole::User,
                        content: "hello resume".to_string(),
                    },
                )
                .await?;
            store
                .append(
                    &session_id,
                    ConversationItem::text(Role::Assistant, "resume answer"),
                )
                .await?;
            store
                .append_transcript_replay(
                    &session_id,
                    TranscriptReplayItem::Message {
                        role: TranscriptReplayRole::Assistant,
                        content: "resume answer".to_string(),
                    },
                )
                .await?;
            store
                .append_config_change(
                    &session_id,
                    ConfigSnapshot {
                        provider_id: "local".to_string(),
                        model: "qwen3".to_string(),
                        system_prompt: Some("historical prompt".to_string()),
                        prompt_prelude: None,
                    },
                )
                .await?;
            Ok::<SessionId, session_store::SessionStoreError>(session_id)
        })
        .expect("session fixture should persist");
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        session_store: Some(store),
        session_header_template: Some(header),
        ..AppRuntimeOptions::default()
    });

    coordinator
        .handle_runtime_command(RuntimeCommand::ResumeSession {
            session_id: session_id.to_string(),
        })
        .expect("resume session should succeed");

    let payload = wait_for_session_resumed(&mut coordinator);
    assert_eq!(payload.session_id, session_id.to_string());
    assert_eq!(
        payload.restored_model,
        Some(ModelSelection::new("local", "qwen3"))
    );
    assert_eq!(
        payload
            .transcript
            .iter()
            .map(TranscriptReplayItem::content_text)
            .collect::<Vec<_>>(),
        vec!["hello resume", "resume answer"]
    );
    assert_eq!(
        coordinator
            .provider_conversation
            .history()
            .map(ConversationItem::text_content)
            .collect::<Vec<_>>(),
        vec!["hello resume", "resume answer"]
    );
    assert_eq!(
        coordinator.provider_conversation.system_prompt(),
        Some("historical prompt")
    );
    cleanup(&work_dir);
}

#[test]
fn resume_session_payload_does_not_label_reasoning_as_system() {
    let work_dir = temp_test_dir("resume-session-reasoning-work");
    let store = Arc::new(InMemorySessionStore::new());
    let store_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("session store runtime should start");
    let header = SessionHeader {
        session_id: SessionId::new(),
        work_dir: work_dir.clone(),
        session_name: None,
        initial_model: "qwen3".to_string(),
        git_head: None,
        cli_version: None,
    };
    let session_id = store_runtime
        .block_on(async {
            let session_id = store.create_session(header.clone()).await?;
            store
                .append(&session_id, ConversationItem::text(Role::User, "hello"))
                .await?;
            store
                .append_transcript_replay(
                    &session_id,
                    TranscriptReplayItem::Message {
                        role: TranscriptReplayRole::User,
                        content: "hello".to_string(),
                    },
                )
                .await?;
            store
                .append(&session_id, ConversationItem::reasoning("private chain"))
                .await?;
            store
                .append_transcript_replay(
                    &session_id,
                    TranscriptReplayItem::Reasoning {
                        content: "private chain".to_string(),
                    },
                )
                .await?;
            store
                .append(&session_id, ConversationItem::text(Role::Assistant, "done"))
                .await?;
            store
                .append_transcript_replay(
                    &session_id,
                    TranscriptReplayItem::Message {
                        role: TranscriptReplayRole::Assistant,
                        content: "done".to_string(),
                    },
                )
                .await?;
            Ok::<SessionId, session_store::SessionStoreError>(session_id)
        })
        .expect("session fixture should persist");
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        session_store: Some(store),
        session_header_template: Some(header),
        ..AppRuntimeOptions::default()
    });

    coordinator
        .handle_runtime_command(RuntimeCommand::ResumeSession {
            session_id: session_id.to_string(),
        })
        .expect("resume session should succeed");

    let payload = wait_for_session_resumed(&mut coordinator);
    let reasoning = payload
            .transcript
            .iter()
            .find(|item| {
                matches!(item, TranscriptReplayItem::Reasoning { content } if content == "private chain")
            })
            .expect("reasoning replay item should be present");
    assert!(
        !matches!(reasoning, TranscriptReplayItem::System { .. }),
        "reasoning must not be replayed as a system message"
    );
    cleanup(&work_dir);
}

#[test]
fn resume_session_payload_does_not_reconstruct_transcript_from_provider_history() {
    let work_dir = temp_test_dir("resume-session-provider-only-work");
    let store = Arc::new(InMemorySessionStore::new());
    let store_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("session store runtime should start");
    let header = SessionHeader {
        session_id: SessionId::new(),
        work_dir: work_dir.clone(),
        session_name: None,
        initial_model: "qwen3".to_string(),
        git_head: None,
        cli_version: None,
    };
    let session_id = store_runtime
        .block_on(async {
            let session_id = store.create_session(header.clone()).await?;
            store
                .append(
                    &session_id,
                    ConversationItem::text(Role::User, "provider-only user"),
                )
                .await?;
            store
                .append(
                    &session_id,
                    ConversationItem::text(Role::Assistant, "provider-only answer"),
                )
                .await?;
            Ok::<SessionId, session_store::SessionStoreError>(session_id)
        })
        .expect("session fixture should persist");
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        session_store: Some(store),
        session_header_template: Some(header),
        ..AppRuntimeOptions::default()
    });

    coordinator
        .handle_runtime_command(RuntimeCommand::ResumeSession {
            session_id: session_id.to_string(),
        })
        .expect("resume session should succeed");

    let payload = wait_for_session_resumed(&mut coordinator);
    assert!(payload.transcript.is_empty());
    cleanup(&work_dir);
}

#[test]
fn reset_after_resume_restores_fresh_prompt_prelude_for_next_new_session() {
    let work_dir = temp_test_dir("reset-after-resume-prelude-work");
    let store = Arc::new(InMemorySessionStore::new());
    let store_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("session store runtime should start");
    let header = SessionHeader {
        session_id: SessionId::new(),
        work_dir: work_dir.clone(),
        session_name: None,
        initial_model: "qwen3".to_string(),
        git_head: None,
        cli_version: None,
    };
    let session_id = store_runtime
        .block_on(async {
            let session_id = store.create_session(header.clone()).await?;
            store
                .append_config_change(
                    &session_id,
                    ConfigSnapshot {
                        provider_id: "local".to_string(),
                        model: "qwen3".to_string(),
                        system_prompt: Some("historical prompt".to_string()),
                        prompt_prelude: Some(PromptPreludeSnapshot {
                            sections: vec![PromptPreludeSection {
                                reference_id: "core-system".to_string(),
                                kind: PromptSourceKind::CoreSystemPrompt,
                                title: "Core system prompt".to_string(),
                                origin: Some(PromptSourceOrigin::Project),
                                body: "historical prompt".to_string(),
                            }],
                        }),
                    },
                )
                .await?;
            Ok::<SessionId, session_store::SessionStoreError>(session_id)
        })
        .expect("session fixture should persist");
    let fresh_prelude = PromptPreludeSnapshot {
        sections: vec![
            PromptPreludeSection {
                reference_id: "core-system".to_string(),
                kind: PromptSourceKind::CoreSystemPrompt,
                title: "Core system prompt".to_string(),
                origin: Some(PromptSourceOrigin::Builtin),
                body: "fresh core".to_string(),
            },
            PromptPreludeSection {
                reference_id: "repo-rules".to_string(),
                kind: PromptSourceKind::ExtraPrompt,
                title: "repo-rules".to_string(),
                origin: Some(PromptSourceOrigin::Project),
                body: "fresh project rules".to_string(),
            },
        ],
    };
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        session_store: Some(store),
        session_header_template: Some(header),
        initial_prompt_prelude: Some(fresh_prelude),
        ..AppRuntimeOptions::default()
    });

    coordinator
        .handle_runtime_command(RuntimeCommand::ResumeSession {
            session_id: session_id.to_string(),
        })
        .expect("resume session should succeed");
    wait_for_session_resumed(&mut coordinator);

    assert_eq!(
        coordinator.provider_conversation.system_prompt(),
        Some("historical prompt")
    );

    coordinator
        .handle_runtime_command(RuntimeCommand::Reset)
        .expect("reset should succeed");

    let request = coordinator
        .provider_conversation
        .prepare_turn(&ConversationTurnRequest::new(
            "local",
            ProviderKind::OpenAiCompatible,
            "qwen3",
            Some("http://127.0.0.1:1234/v1".to_string()),
            None,
            None,
            ConversationItem::text(Role::User, "new session"),
        ))
        .expect("fresh new session turn should prepare");

    assert_eq!(request.items()[0].role(), Some(Role::System));
    assert_eq!(
        request.items()[0].text_content(),
        "fresh core\n\nfresh project rules"
    );
    cleanup(&work_dir);
}

#[test]
fn resume_session_payload_prefers_persisted_transcript_replay() {
    let work_dir = temp_test_dir("resume-session-explicit-replay-work");
    let store = Arc::new(InMemorySessionStore::new());
    let store_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("session store runtime should start");
    let header = SessionHeader {
        session_id: SessionId::new(),
        work_dir: work_dir.clone(),
        session_name: None,
        initial_model: "qwen3".to_string(),
        git_head: None,
        cli_version: None,
    };
    let replay_activity = RuntimeToolActivity {
        activity_id: "call-1".to_string(),
        title: "Write src/lib.rs".to_string(),
        kind: RuntimeToolKind::Write,
        status: RuntimeToolActivityStatus::Completed,
        content: vec![RuntimeToolActivityContent::Diff {
            path: "src/lib.rs".to_string(),
            old_text: Some("old".to_string()),
            new_text: "new".to_string(),
            is_truncated: false,
        }],
        locations: Vec::new(),
        raw_input: Some(RuntimeToolActivityRawValue::from(
            r#"{"path":"src/lib.rs"}"#,
        )),
        raw_output: Some(RuntimeToolActivityRawValue::tool_result(
            "plain provider output",
            None,
        )),
    };
    let session_id = store_runtime
        .block_on(async {
            let session_id = store.create_session(header.clone()).await?;
            store
                .append(
                    &session_id,
                    ConversationItem::assistant_with_tool_calls(
                        "editing".to_string(),
                        vec![ToolCall::new(
                            "call-1",
                            "write_file",
                            r#"{"path":"src/lib.rs"}"#,
                        )],
                    ),
                )
                .await?;
            store
                .append(
                    &session_id,
                    ConversationItem::tool_result(
                        "call-1",
                        vec![ContentBlock::Text("plain provider output".to_string())],
                        false,
                    ),
                )
                .await?;
            store
                .append_transcript_replay(
                    &session_id,
                    TranscriptReplayItem::Message {
                        role: TranscriptReplayRole::Assistant,
                        content: "editing".to_string(),
                    },
                )
                .await?;
            store
                .append_transcript_replay(
                    &session_id,
                    TranscriptReplayItem::ToolActivity {
                        activity: replay_activity.clone(),
                    },
                )
                .await?;
            Ok::<SessionId, session_store::SessionStoreError>(session_id)
        })
        .expect("session fixture should persist");
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        session_store: Some(store),
        session_header_template: Some(header),
        ..AppRuntimeOptions::default()
    });

    coordinator
        .handle_runtime_command(RuntimeCommand::ResumeSession {
            session_id: session_id.to_string(),
        })
        .expect("resume session should succeed");

    let payload = wait_for_session_resumed(&mut coordinator);
    assert_eq!(
        payload.transcript,
        vec![
            TranscriptReplayItem::Message {
                role: TranscriptReplayRole::Assistant,
                content: "editing".to_string(),
            },
            TranscriptReplayItem::ToolActivity {
                activity: replay_activity,
            },
        ],
        "explicit replay should preserve rich diff content instead of fallback text"
    );
    cleanup(&work_dir);
}

#[test]
fn load_session_preview_emits_transcript_without_resuming_runtime_session() {
    let work_dir = temp_test_dir("preview-session-work");
    let store = Arc::new(InMemorySessionStore::new());
    let store_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("session store runtime should start");
    let header = SessionHeader {
        session_id: SessionId::new(),
        work_dir: work_dir.clone(),
        session_name: None,
        initial_model: "qwen2".to_string(),
        git_head: None,
        cli_version: None,
    };
    let preview_session_id = store_runtime
        .block_on(async {
            let active_session_id = store.create_session(header.clone()).await?;
            store
                .append(
                    &active_session_id,
                    ConversationItem::text(Role::User, "active user"),
                )
                .await?;
            let preview_session_id = store
                .create_session(SessionHeader {
                    session_id: SessionId::new(),
                    session_name: Some("preview".to_string()),
                    ..header.clone()
                })
                .await?;
            store
                .append(
                    &preview_session_id,
                    ConversationItem::text(Role::User, "preview user"),
                )
                .await?;
            store
                .append_transcript_replay(
                    &preview_session_id,
                    TranscriptReplayItem::Message {
                        role: TranscriptReplayRole::User,
                        content: "preview user".to_string(),
                    },
                )
                .await?;
            store
                .append(
                    &preview_session_id,
                    ConversationItem::text(Role::Assistant, "preview answer"),
                )
                .await?;
            store
                .append_transcript_replay(
                    &preview_session_id,
                    TranscriptReplayItem::Message {
                        role: TranscriptReplayRole::Assistant,
                        content: "preview answer".to_string(),
                    },
                )
                .await?;
            Ok::<SessionId, session_store::SessionStoreError>(preview_session_id)
        })
        .expect("session fixture should persist");
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        session_store: Some(store),
        session_header_template: Some(header),
        ..AppRuntimeOptions::default()
    });

    coordinator
        .handle_runtime_command(RuntimeCommand::LoadSessionPreview {
            session_id: preview_session_id.to_string(),
        })
        .expect("load preview should succeed");

    let payload = wait_for_session_preview(&mut coordinator);
    assert_eq!(payload.session_id, preview_session_id.to_string());
    assert_eq!(
        payload
            .transcript
            .iter()
            .map(TranscriptReplayItem::content_text)
            .collect::<Vec<_>>(),
        vec!["preview user", "preview answer"]
    );
    assert!(
        coordinator.provider_conversation.is_history_empty(),
        "loading preview should not replace the active provider conversation"
    );
    cleanup(&work_dir);
}
