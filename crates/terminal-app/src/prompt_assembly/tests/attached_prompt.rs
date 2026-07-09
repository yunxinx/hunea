use super::*;

fn test_config_dir(work_dir: &std::path::Path) -> std::path::PathBuf {
    let dir = work_dir.join(".hunea");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

#[test]
fn assemble_attached_prompt_message_expands_unique_skill_mentions_in_first_use_order() {
    let work_dir = temp_dir("manual-skill-assembly");
    let repo_bootstrap_dir = work_dir.join(".agents/skills/repo-bootstrap");
    fs::create_dir_all(&repo_bootstrap_dir).expect("repo-bootstrap dir should exist");
    fs::write(
            repo_bootstrap_dir.join(SKILL_FILE_NAME),
            "---\nname: repo-bootstrap\ndescription: Bootstrap repo\n---\n# Repo Bootstrap\n\nBootstrap steps.\n",
        )
        .expect("repo-bootstrap skill should write");
    let code_review_dir = work_dir.join(".agents/skills/code-review");
    fs::create_dir_all(&code_review_dir).expect("code-review dir should exist");
    fs::write(
            code_review_dir.join(SKILL_FILE_NAME),
            "---\nname: code-review\ndescription: Review code\n---\n# Code Review\n\nReview carefully.\n",
        )
        .expect("code-review skill should write");

    let assembled = assemble_attached_prompt_message(
        None,
        &work_dir,
        &TranscriptUserMessage {
            content: "Please use $repo-bootstrap before $code-review and repeat $repo-bootstrap"
                .to_string(),
            attachments: Vec::new(),
            skill_bindings: vec![
                runtime_domain::session::TranscriptSkillBinding {
                    skill_name: "repo-bootstrap".to_string(),
                    origin: PromptSourceOrigin::Project,
                    skill_path: repo_bootstrap_dir
                        .join(SKILL_FILE_NAME)
                        .display()
                        .to_string(),
                    start_char: 11,
                    end_char: 26,
                },
                runtime_domain::session::TranscriptSkillBinding {
                    skill_name: "code-review".to_string(),
                    origin: PromptSourceOrigin::Project,
                    skill_path: code_review_dir.join(SKILL_FILE_NAME).display().to_string(),
                    start_char: 34,
                    end_char: 46,
                },
                runtime_domain::session::TranscriptSkillBinding {
                    skill_name: "repo-bootstrap".to_string(),
                    origin: PromptSourceOrigin::Project,
                    skill_path: repo_bootstrap_dir
                        .join(SKILL_FILE_NAME)
                        .display()
                        .to_string(),
                    start_char: 58,
                    end_char: 73,
                },
            ],
            custom_prompt_bindings: Vec::new(),
        },
    );

    assert_eq!(
        assembled
            .manual_skill_uses
            .iter()
            .map(|skill| skill.skill_name.as_str())
            .collect::<Vec<_>>(),
        vec!["repo-bootstrap", "code-review"]
    );
    assert_eq!(
        assembled.provider_visible_user_text,
        format!(
            "{}\n\n{}\n\nPlease use $repo-bootstrap before $code-review and repeat $repo-bootstrap",
            format_long_lived_skill_body(&DiscoveredSkill {
                name: "repo-bootstrap".to_string(),
                description: "Bootstrap repo".to_string(),
                skill_path: repo_bootstrap_dir.join(SKILL_FILE_NAME),
                body: "# Repo Bootstrap\n\nBootstrap steps.".to_string(),
                origin: PromptSourceOrigin::Project,
                disable_model_invocation: false,
            }),
            format_long_lived_skill_body(&DiscoveredSkill {
                name: "code-review".to_string(),
                description: "Review code".to_string(),
                skill_path: code_review_dir.join(SKILL_FILE_NAME),
                body: "# Code Review\n\nReview carefully.".to_string(),
                origin: PromptSourceOrigin::Project,
                disable_model_invocation: false,
            }),
        )
    );
}
#[test]
fn assemble_attached_prompt_message_ignores_plain_text_tokens_without_bindings() {
    let work_dir = temp_dir("manual-skill-without-bindings");
    let code_review_dir = work_dir.join(".agents/skills/code-review");
    fs::create_dir_all(&code_review_dir).expect("code-review dir should exist");
    fs::write(
            code_review_dir.join(SKILL_FILE_NAME),
            "---\nname: code-review\ndescription: Review code\n---\n# Code Review\n\nReview carefully.\n",
        )
        .expect("code-review skill should write");

    let assembled = assemble_attached_prompt_message(
        None,
        &work_dir,
        &TranscriptUserMessage {
            content: "Please use $code-review".to_string(),
            attachments: Vec::new(),
            skill_bindings: Vec::new(),
            custom_prompt_bindings: Vec::new(),
        },
    );

    assert!(assembled.manual_skill_uses.is_empty());
    assert!(assembled.custom_prompt_uses.is_empty());
    assert_eq!(
        assembled.provider_visible_user_text,
        "Please use $code-review"
    );
}
#[test]
fn assemble_attached_prompt_message_includes_custom_prompt_bodies_in_first_use_order() {
    let work_dir = temp_dir("custom-prompt-attachment");
    let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
    save_project_prompt_assembly_state(
        &work_dir,
        &scope_state! {
            scope: PromptAssemblyScope::Project,
            core_system_override: None,
            entries: vec![PersistedPromptAssemblyEntry {
                reference_id: "review-rules".to_string(),
                kind: PromptSourceKind::ExtraPrompt,
                title: "Review Rules".to_string(),
                enabled: false,
                requested_order: None,
            }],
            skill_discovery_override: None,
            skill_discovery_skills: Vec::new(),
            extra_prompts: vec![StoredPromptBody {
                reference_id: "review-rules".to_string(),
                title: "Review Rules".to_string(),
                body: "# Review Rules\nCheck regressions before approving.".to_string(),
            }],
            tool_guidelines_override: None,
            tool_selections: Vec::new(),
            dynamic_environment_sources: Vec::new(),
        },
    )
    .expect("project prompt state should save");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build");
    runtime
        .block_on(
            store.save_global_prompt_assembly_state(&PromptAssemblyScopeState::new(
                PromptAssemblyScope::Global,
            )),
        )
        .expect("global prompt state should save");

    let manager = PromptAssemblyWorkspace::new(&work_dir, &test_config_dir(&work_dir), &[])
        .load_manager(store)
        .expect("prompt assembly manager should load");

    let assembled = assemble_attached_prompt_message(
        Some(&manager),
        &work_dir,
        &TranscriptUserMessage {
            content: "Before\n#review-rules\nAfter".to_string(),
            attachments: Vec::new(),
            skill_bindings: Vec::new(),
            custom_prompt_bindings: vec![runtime_domain::session::TranscriptCustomPromptBinding {
                reference_id: "review-rules".to_string(),
                origin: PromptSourceOrigin::Project,
                start_char: 7,
                end_char: 20,
            }],
        },
    );

    assert!(assembled.manual_skill_uses.is_empty());
    assert_eq!(
        assembled
            .custom_prompt_uses
            .iter()
            .map(|prompt| prompt.reference_id.as_str())
            .collect::<Vec<_>>(),
        vec!["review-rules"]
    );
    assert_eq!(
        assembled.provider_visible_user_text,
        "Before\n\n# Review Rules\nCheck regressions before approving.\n\nAfter"
    );
}
