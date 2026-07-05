use super::support::*;

macro_rules! scope_state {
    (scope: $scope:expr, $($field:ident $(: $value:expr)?),* $(,)?) => {{
        let mut state =
            runtime_domain::prompt_assembly::persistence::PromptAssemblyScopeState::new($scope);
        $(scope_state!(@assign state, $field $(: $value)?);)*
        state
    }};
    (@assign $state:ident, core_system_override : $value:expr) => {
        $state.set_core_system_override($value);
    };
    (@assign $state:ident, skill_discovery_override : $value:expr) => {
        $state.set_skill_discovery_override($value);
    };
    (@assign $state:ident, tool_guidelines_override : $value:expr) => {
        $state.set_tool_guidelines_override($value);
    };
    (@assign $state:ident, entries : $value:expr) => {
        $state.set_entries($value);
    };
    (@assign $state:ident, skill_discovery_skills : $value:expr) => {
        $state.set_skill_discovery_skills($value);
    };
    (@assign $state:ident, tool_selections : $value:expr) => {
        $state.set_tool_selections($value);
    };
    (@assign $state:ident, dynamic_environment_sources : $value:expr) => {
        $state.set_dynamic_environment_sources($value);
    };
    (@assign $state:ident, extra_prompts : $value:expr) => {
        $state.set_extra_prompts($value);
    };
    (@assign $state:ident, $field:ident) => {
        scope_state!(@assign $state, $field : $field);
    };
}

#[test]
fn conversation_target_must_match_running_worker() {
    let active_target = RuntimeTarget::provider("openai", "gpt-4o-mini");
    assert!(ensure_conversation_target(Some(&active_target), None).is_ok());
    assert!(ensure_conversation_target(Some(&active_target), Some(&active_target)).is_ok());

    let inactive_target = RuntimeTarget::provider("openai", "gpt-4.1-mini");
    let inactive_error = ensure_conversation_target(Some(&active_target), Some(&inactive_target))
        .expect_err("wrong conversation target should be rejected");
    assert!(inactive_error.contains("Conversation is not active"));

    let stopped_error = ensure_conversation_target(None, Some(&active_target))
        .expect_err("explicit conversation target should require a running worker");
    assert!(stopped_error.contains("Conversation is not running"));
}

#[test]
fn token_estimate_creates_render_barrier_before_permission_request() {
    let output_batch = vec![RuntimeEvent::OutputTokenEstimate {
        target: Some(RuntimeTarget::provider("local", "qwen3")),
        total_tokens: 57,
    }];
    let input_batch = vec![RuntimeEvent::InputTokenEstimate {
        target: Some(RuntimeTarget::provider("local", "qwen3")),
        total_tokens: 12,
    }];
    let permission_event = RuntimeEvent::PermissionRequested {
        target: RuntimeTarget::provider("local", "qwen3"),
        request: RuntimePermissionRequest::new(
            "permission-1",
            Some("Write temp.md".into()),
            vec![],
        ),
    };

    assert!(
        should_defer_runtime_event_for_render_barrier(&output_batch, &permission_event),
        "permission should wait for the output token estimate batch to render first"
    );
    assert!(
        should_defer_runtime_event_for_render_barrier(&input_batch, &permission_event),
        "permission should wait for the input token estimate batch to render first"
    );
    assert!(
        !should_defer_runtime_event_for_render_barrier(&[], &permission_event),
        "permission should not be deferred when there is no token estimate to render"
    );
}

#[test]
fn app_layer_persists_managed_search_tool_authorization() {
    let root = temp_test_dir("managed-search-authorization");
    let config_path = root.join("config.toml");
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        managed_search_authorization_config_path: Some(config_path.clone()),
        ..AppRuntimeOptions::default()
    });

    let event = coordinator.persist_managed_search_tool_authorization(ManagedSearchTool::Fd, None);

    assert_eq!(event, None);
    assert_eq!(
        coordinator.options.managed_search_tools.allow_managed_fd,
        Some(true)
    );
    let registry_definitions = coordinator
        .workspace_tools
        .definitions()
        .definitions()
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        coordinator.prompt_assembly_tool_definitions(),
        registry_definitions.as_slice(),
        "prompt assembly should use the refreshed workspace tool definitions"
    );
    let content = fs::read_to_string(&config_path).expect("config should be readable");
    assert!(content.contains("allow_managed_fd = true"));
    cleanup(&root);
}

#[test]
fn conversation_failure_before_provider_request_rolls_back_pending_user() {
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        runtime_request_policy: runtime_domain::request_policy::RuntimeRequestPolicy::new(
            0,
            Vec::new(),
            1,
        ),
        ..AppRuntimeOptions::default()
    });
    let request = ConversationTurnRequest::new(
        "openai",
        ProviderKind::OpenAi,
        "gpt-4o-mini",
        None,
        None,
        None,
        ConversationItem::text(Role::User, "hello"),
    );
    let target = request.target();

    coordinator
        .handle_runtime_command(RuntimeCommand::SubmitConversationTurn {
            target,
            request: Box::new(request),
        })
        .expect("conversation request should start");

    let mut events = Vec::new();
    for _ in 0..50 {
        events.extend(RuntimeCoordinator::drain_runtime_events(&mut coordinator));
        if events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::Failed { .. }))
        {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }

    assert!(
        events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::Failed { .. })),
        "preflight failure should be reported"
    );
    assert!(coordinator.provider_conversation.is_history_empty());

    let next_request = ConversationTurnRequest::new(
        "local",
        ProviderKind::OpenAiCompatible,
        "qwen3",
        Some("http://127.0.0.1:1234/v1".to_string()),
        None,
        None,
        ConversationItem::text(Role::User, "next"),
    );
    coordinator
        .provider_conversation
        .prepare_turn(&next_request)
        .expect("failed preflight turn should not leave stale pending state");
}

#[test]
fn startup_prompt_missing_source_check_emits_aggregated_runtime_event() {
    let root = temp_test_dir("prompt-missing-check");
    let work_dir = root.join("repo");
    fs::create_dir_all(&work_dir).expect("work dir should exist");
    let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build");
    runtime
        .block_on(store.save_global_prompt_assembly_state(&scope_state! {
            scope: runtime_domain::prompt_assembly::persistence::PromptAssemblyScope::Global,
            core_system_override: None,
            entries: vec![
                runtime_domain::prompt_assembly::persistence::PersistedPromptAssemblyEntry {
                    reference_id: "missing-skill".to_string(),
                    kind: runtime_domain::prompt_assembly::PromptSourceKind::LongLivedSkill,
                    title: "missing-skill".to_string(),
                    enabled: true,
                    requested_order: Some(10),
                },
            ],
            skill_discovery_override: None,
            skill_discovery_skills: Vec::new(),
            extra_prompts: Vec::new(),
            tool_guidelines_override: None,
            tool_selections: Vec::new(),
            dynamic_environment_sources: Vec::new(),
        }))
        .expect("global prompt state should save");

    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        session_store: Some(store),
        session_header_template: Some(SessionHeader {
            session_id: SessionId::new(),
            work_dir: work_dir.clone(),
            session_name: None,
            initial_model: "qwen3".to_string(),
            git_head: None,
            cli_version: None,
        }),
        ..AppRuntimeOptions::default()
    });

    coordinator
        .handle_runtime_command(RuntimeCommand::CheckPromptAssemblyMissingSources)
        .expect("startup prompt check should be accepted");

    let missing_count = wait_for_runtime_event(
        &mut coordinator,
        |event| match event {
            RuntimeEvent::PromptAssemblyMissingSourcesChecked { missing_count } => {
                Some(missing_count)
            }
            _ => None,
        },
        "prompt missing check result",
    );

    assert_eq!(missing_count, 1);
    cleanup(&root);
}

#[test]
fn startup_prompt_missing_source_check_reports_load_failure() {
    let root = temp_test_dir("prompt-missing-check-failure");
    let work_dir = root.join("repo");
    fs::create_dir_all(&work_dir).expect("work dir should exist");
    let store: Arc<dyn SessionStore> = Arc::new(FailingSessionStore::new(
        Arc::new(InMemorySessionStore::new()),
        FailingSessionStoreLoad::PromptAssemblyLoad,
    ));
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        session_store: Some(store),
        session_header_template: Some(SessionHeader {
            session_id: SessionId::new(),
            work_dir: work_dir.clone(),
            session_name: None,
            initial_model: "qwen3".to_string(),
            git_head: None,
            cli_version: None,
        }),
        ..AppRuntimeOptions::default()
    });

    coordinator
        .handle_runtime_command(RuntimeCommand::CheckPromptAssemblyMissingSources)
        .expect("startup prompt check should be accepted");

    let (kind, message) = wait_for_runtime_event(
        &mut coordinator,
        |event| match event {
            RuntimeEvent::PromptAssemblyUpdateFailed { kind, message } => Some((kind, message)),
            _ => None,
        },
        "prompt missing check failure",
    );

    assert_eq!(
        kind,
        runtime_domain::session::PromptAssemblyCommandFailureKind::CheckMissingSources
    );
    assert!(
        message.contains("load global prompt assembly state"),
        "failure should explain the failed check stage: {message}"
    );
    cleanup(&root);
}

#[test]
fn dynamic_environment_does_not_advance_observations_without_injected_snapshot() {
    let root = temp_test_dir("dynamic-environment-empty-snapshot");
    let work_dir = root.join("repo");
    fs::create_dir_all(&work_dir).expect("work dir should exist");
    let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
    let store_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build");
    let mut dynamic_environment_sources =
        runtime_domain::dynamic_environment::default_dynamic_environment_selections();
    for source in &mut dynamic_environment_sources {
        source.enabled = false;
    }
    store_runtime
        .block_on(store.save_global_prompt_assembly_state(&scope_state! {
            scope: runtime_domain::prompt_assembly::persistence::PromptAssemblyScope::Global,
            core_system_override: None,
            entries: Vec::new(),
            skill_discovery_override: None,
            skill_discovery_skills: Vec::new(),
            extra_prompts: Vec::new(),
            tool_guidelines_override: None,
            tool_selections: Vec::new(),
            dynamic_environment_sources,
        }))
        .expect("global prompt state should save");

    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        session_store: Some(store),
        session_header_template: Some(SessionHeader {
            session_id: SessionId::new(),
            work_dir: work_dir.clone(),
            session_name: None,
            initial_model: "qwen3".to_string(),
            git_head: None,
            cli_version: None,
        }),
        ..AppRuntimeOptions::default()
    });

    let injection = coordinator
        .dynamic_environment_prefix_items()
        .expect("dynamic environment assembly should succeed");

    assert!(
        injection.prefix_texts.is_empty(),
        "no dynamic source is selected, so no provider-visible snapshot should be injected"
    );
    assert_eq!(
        injection.next_observations, None,
        "unsent observations must not become the next comparison baseline"
    );
    cleanup(&root);
}

#[test]
fn conversation_submit_dispatches_without_waiting_for_dynamic_environment_observation() {
    let root = temp_test_dir("dynamic-environment-async-submit");
    let work_dir = root.join("repo");
    fs::create_dir_all(&work_dir).expect("work dir should exist");
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let observer = Arc::new(BlockingDynamicEnvironmentObserver {
        started: std::sync::Mutex::new(Some(started_tx)),
        release: std::sync::Mutex::new(Some(release_rx)),
        cancellation_observed: std::sync::Mutex::new(None),
    });
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        runtime_request_policy: runtime_domain::request_policy::RuntimeRequestPolicy::new(
            0,
            Vec::new(),
            1,
        ),
        dynamic_environment_observer: observer,
        session_header_template: Some(SessionHeader {
            session_id: SessionId::new(),
            work_dir: work_dir.clone(),
            session_name: None,
            initial_model: "gpt-4o-mini".to_string(),
            git_head: None,
            cli_version: None,
        }),
        initial_dynamic_environment_session_config: Some(
            runtime_domain::dynamic_environment::DynamicEnvironmentSessionConfig {
                baseline_enabled: true,
                changes_enabled: false,
                source_selections: vec![
                    runtime_domain::dynamic_environment::DynamicEnvironmentSourceSelection {
                        snapshot_kind:
                            runtime_domain::dynamic_environment::DynamicEnvironmentSnapshotKind::Baseline,
                        source_kind:
                            runtime_domain::dynamic_environment::DynamicEnvironmentSourceKind::Date,
                        enabled: true,
                    },
                ],
            },
        ),
        ..AppRuntimeOptions::default()
    });
    let request = ConversationTurnRequest::new(
        "openai",
        ProviderKind::OpenAi,
        "gpt-4o-mini",
        None,
        None,
        None,
        ConversationItem::text(Role::User, "hello"),
    );
    let target = request.target();

    let started = std::time::Instant::now();
    let receipt = coordinator
        .handle_runtime_command(RuntimeCommand::SubmitConversationTurn {
            target,
            request: Box::new(request),
        })
        .expect("conversation request should dispatch");

    assert_eq!(receipt, RuntimeCommandReceipt::Accepted);
    assert!(
        started.elapsed() < Duration::from_millis(100),
        "submit should not wait for dynamic environment observation"
    );
    started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("dynamic environment observation should run in background");

    release_tx
        .send(())
        .expect("test should release dynamic environment observation");
    let failure = wait_for_runtime_event(
        &mut coordinator,
        |event| match event {
            RuntimeEvent::Failed { message, .. } => Some(message),
            _ => None,
        },
        "provider failure after dynamic environment observation",
    );
    assert!(
        failure.contains("requires API key"),
        "conversation should continue through the normal provider path: {failure}"
    );
    cleanup(&root);
}

#[test]
fn interrupting_pending_dynamic_environment_cancels_observation() {
    let root = temp_test_dir("dynamic-environment-cancel");
    let work_dir = root.join("repo");
    fs::create_dir_all(&work_dir).expect("work dir should exist");
    let (started_tx, started_rx) = mpsc::channel();
    let (_release_tx, release_rx) = tokio::sync::oneshot::channel();
    let (cancelled_tx, cancelled_rx) = mpsc::channel();
    let observer = Arc::new(BlockingDynamicEnvironmentObserver {
        started: std::sync::Mutex::new(Some(started_tx)),
        release: std::sync::Mutex::new(Some(release_rx)),
        cancellation_observed: std::sync::Mutex::new(Some(cancelled_tx)),
    });
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        dynamic_environment_observer: observer,
        session_header_template: Some(SessionHeader {
            session_id: SessionId::new(),
            work_dir: work_dir.clone(),
            session_name: None,
            initial_model: "gpt-4o-mini".to_string(),
            git_head: None,
            cli_version: None,
        }),
        initial_dynamic_environment_session_config: Some(
            runtime_domain::dynamic_environment::DynamicEnvironmentSessionConfig {
                baseline_enabled: true,
                changes_enabled: false,
                source_selections: vec![
                    runtime_domain::dynamic_environment::DynamicEnvironmentSourceSelection {
                        snapshot_kind:
                            runtime_domain::dynamic_environment::DynamicEnvironmentSnapshotKind::Baseline,
                        source_kind:
                            runtime_domain::dynamic_environment::DynamicEnvironmentSourceKind::Date,
                        enabled: true,
                    },
                ],
            },
        ),
        ..AppRuntimeOptions::default()
    });
    let request = ConversationTurnRequest::new(
        "openai",
        ProviderKind::OpenAi,
        "gpt-4o-mini",
        None,
        None,
        None,
        ConversationItem::text(Role::User, "hello"),
    );
    let target = request.target();

    coordinator
        .handle_runtime_command(RuntimeCommand::SubmitConversationTurn {
            target,
            request: Box::new(request),
        })
        .expect("conversation request should dispatch");
    started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("dynamic environment observation should start");

    let receipt = coordinator
        .handle_runtime_command(RuntimeCommand::interrupt_current())
        .expect("interrupt should be accepted");

    assert!(matches!(
        receipt,
        RuntimeCommandReceipt::Interrupted { target: Some(_) }
    ));
    cancelled_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("dynamic environment observer should receive cancellation");
    cleanup(&root);
}

#[test]
fn manual_skill_mentions_emit_synthetic_skill_usage_events_before_worker_failure() {
    let root = temp_test_dir("manual-skill-events");
    let work_dir = root.join("repo");
    let skill_dir = work_dir.join(".agents/skills/code-review");
    fs::create_dir_all(&skill_dir).expect("skill dir should exist");
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: code-review\ndescription: Review code\n---\n# Code Review\n\nReview carefully.\n",
    )
    .expect("skill file should exist");

    let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
    let mut coordinator = runtime_coordinator(AppRuntimeOptions {
        runtime_request_policy: runtime_domain::request_policy::RuntimeRequestPolicy::new(
            0,
            Vec::new(),
            1,
        ),
        session_store: Some(store),
        session_header_template: Some(SessionHeader {
            session_id: SessionId::new(),
            work_dir: work_dir.clone(),
            session_name: None,
            initial_model: "gpt-4o-mini".to_string(),
            git_head: None,
            cli_version: None,
        }),
        ..AppRuntimeOptions::default()
    });
    let request = ConversationTurnRequest::new_user_source_message(
        "openai",
        ProviderKind::OpenAi,
        "gpt-4o-mini",
        None,
        None,
        None,
        runtime_domain::session::TranscriptUserMessage {
            content: "Please audit this diff with $code-review".to_string(),
            attachments: Vec::new(),
            skill_bindings: vec![runtime_domain::session::TranscriptSkillBinding {
                skill_name: "code-review".to_string(),
                origin: runtime_domain::prompt_assembly::PromptSourceOrigin::Project,
                skill_path: skill_dir.join("SKILL.md").display().to_string(),
                start_char: 28,
                end_char: 40,
            }],
            custom_prompt_bindings: Vec::new(),
        },
    );
    let target = request.target();

    coordinator
        .handle_runtime_command(RuntimeCommand::SubmitConversationTurn {
            target,
            request: Box::new(request),
        })
        .expect("conversation should start");

    let events = RuntimeCoordinator::drain_runtime_events(&mut coordinator);
    assert!(matches!(
        events.as_slice(),
        [RuntimeEvent::ToolActivityStarted { activity, .. }]
            if activity.title.ends_with(".agents/skills/code-review/SKILL.md")
                && activity.raw_input.as_ref().and_then(|raw| raw.string_field(&["hunea_skill_name"]))
                    == Some("code-review".to_string())
    ));

    let failure = wait_for_runtime_event(
        &mut coordinator,
        |event| match event {
            RuntimeEvent::Failed { message, .. } => Some(message),
            _ => None,
        },
        "worker failure after synthetic skill usage event",
    );
    assert!(
        failure.contains("requires API key"),
        "worker should still fail through the normal runtime path: {failure}"
    );
    cleanup(&root);
}

#[test]
fn custom_prompt_attachment_uses_cached_prompt_assembly_without_waiting_for_store_io() {
    let root = temp_test_dir("custom-prompt-attachment-cached");
    let work_dir = root.join("repo");
    fs::create_dir_all(&work_dir).expect("work dir should exist");
    runtime_domain::prompt_assembly::persistence::save_project_prompt_assembly_state(
        &work_dir,
        &scope_state! {
            scope: runtime_domain::prompt_assembly::persistence::PromptAssemblyScope::Project,
            core_system_override: None,
            entries: vec![
                runtime_domain::prompt_assembly::persistence::PersistedPromptAssemblyEntry {
                    reference_id: "review-rules".to_string(),
                    kind: runtime_domain::prompt_assembly::PromptSourceKind::ExtraPrompt,
                    title: "Review Rules".to_string(),
                    enabled: false,
                    requested_order: None,
                },
            ],
            skill_discovery_override: None,
            skill_discovery_skills: Vec::new(),
            extra_prompts: vec![
                runtime_domain::prompt_assembly::persistence::StoredPromptBody {
                    reference_id: "review-rules".to_string(),
                    title: "Review Rules".to_string(),
                    body: "# Review Rules\nCheck regressions before approving.".to_string(),
                },
            ],
            tool_guidelines_override: None,
            tool_selections: Vec::new(),
            dynamic_environment_sources: Vec::new(),
        },
    )
    .expect("project prompt assembly should save");

    let base_store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
    let store_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build");
    store_runtime
        .block_on(base_store.save_global_prompt_assembly_state(
            &runtime_domain::prompt_assembly::persistence::PromptAssemblyScopeState::new(
                runtime_domain::prompt_assembly::persistence::PromptAssemblyScope::Global,
            ),
        ))
        .expect("global prompt state should save");
    let cached_manager = crate::prompt_assembly::PromptAssemblyWorkspace::new(&work_dir, &[])
        .load_manager(Arc::clone(&base_store))
        .expect("cached prompt assembly manager should load");

    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let delayed_store: Arc<dyn SessionStore> =
        Arc::new(DelayedListSessionStore::new_with_prompt_assembly_delay(
            Arc::new(InMemorySessionStore::new()),
            started_tx,
            release_rx,
        ));
    let release_thread = thread::spawn(move || {
        if started_rx.recv_timeout(Duration::from_millis(250)).is_ok() {
            thread::sleep(Duration::from_millis(200));
            release_tx
                .send(())
                .expect("delayed prompt assembly load should still be waiting");
        }
    });
    let coordinator = runtime_coordinator(AppRuntimeOptions {
        session_store: Some(delayed_store),
        session_header_template: Some(SessionHeader {
            session_id: SessionId::new(),
            work_dir: work_dir.clone(),
            session_name: None,
            initial_model: "gpt-4o-mini".to_string(),
            git_head: None,
            cli_version: None,
        }),
        prompt_assembly_manager: Some(cached_manager),
        ..AppRuntimeOptions::default()
    });
    let user_message = runtime_domain::session::TranscriptUserMessage {
        content: "Before\n#review-rules\nAfter".to_string(),
        attachments: Vec::new(),
        skill_bindings: Vec::new(),
        custom_prompt_bindings: vec![runtime_domain::session::TranscriptCustomPromptBinding {
            reference_id: "review-rules".to_string(),
            origin: runtime_domain::prompt_assembly::PromptSourceOrigin::Project,
            start_char: 7,
            end_char: 20,
        }],
    };

    let started = std::time::Instant::now();
    let assembled = coordinator
        .attached_prompt_message_assembly(&user_message)
        .expect("custom prompt attachment should assemble from cached manager");

    assert!(
        started.elapsed() < Duration::from_millis(100),
        "custom prompt assembly should not wait for prompt assembly I/O"
    );
    assert_eq!(
        assembled.provider_visible_user_text,
        "Before\n\n# Review Rules\nCheck regressions before approving.\n\nAfter"
    );
    release_thread
        .join()
        .expect("release thread should finish cleanly");
    cleanup(&root);
}

struct BlockingDynamicEnvironmentObserver {
    started: std::sync::Mutex<Option<mpsc::Sender<()>>>,
    release: std::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    cancellation_observed: std::sync::Mutex<Option<mpsc::Sender<()>>>,
}

impl crate::dynamic_environment::DynamicEnvironmentObserver for BlockingDynamicEnvironmentObserver {
    fn observe<'a>(
        &'a self,
        _work_dir: &'a Path,
        sources: &'a [runtime_domain::dynamic_environment::DynamicEnvironmentSourceKind],
        cancellation: &'a tokio_util::sync::CancellationToken,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        Vec<runtime_domain::dynamic_environment::DynamicEnvironmentObservation>,
                        crate::dynamic_environment::DynamicEnvironmentObservationError,
                    >,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            if let Some(started) = self.started.lock().expect("started lock").take() {
                let _ = started.send(());
            }
            let release = self.release.lock().expect("release lock").take();
            if let Some(release) = release {
                tokio::select! {
                    _ = cancellation.cancelled() => {
                        if let Some(cancelled) = self
                            .cancellation_observed
                            .lock()
                            .expect("cancellation lock")
                            .take()
                        {
                            let _ = cancelled.send(());
                        }
                        return Err(crate::dynamic_environment::DynamicEnvironmentObservationError);
                    }
                    result = release => {
                        result.expect("test should release dynamic environment observer");
                    }
                }
            }
            Ok(sources
                .iter()
                .copied()
                .map(|source_kind| {
                    runtime_domain::dynamic_environment::DynamicEnvironmentObservation {
                        source_kind,
                        fingerprint: "observed".to_string(),
                        summary: "observed".to_string(),
                        details: None,
                    }
                })
                .collect())
        })
    }
}
