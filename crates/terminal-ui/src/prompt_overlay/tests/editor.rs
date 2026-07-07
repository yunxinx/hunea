use super::*;

#[test]
fn unchanged_prompt_overlay_external_editor_exit_does_not_fall_through_to_composer() {
    let mut model = ready_model();
    model.open_prompt_overlay();
    model
        .prompt_overlay
        .as_mut()
        .expect("overlay should open")
        .pending_editor = Some(super::PromptOverlayPendingEditor {
        target: runtime_domain::prompt_assembly::PromptAssemblyEditorTarget::ExtraPrompt {
            scope: runtime_domain::prompt_assembly::persistence::PromptAssemblyScope::Project,
            reference_id: "repo-rules".to_string(),
        },
        original_draft: "# Repo rules\n".to_string(),
        cleanup_path_after_finish: true,
    });
    let draft_path = temp_test_file("overlay-editor-unchanged");
    fs::write(&draft_path, "# Repo rules\n").expect("draft file should exist");

    let effect = model.update(AppEvent::ExternalEditorFinished {
        draft_path: draft_path.clone(),
        original_draft: "# Repo rules\n".to_string(),
        failed: false,
    });

    assert_eq!(effect, None);
    assert_eq!(model.active_toast_text_for_test(), None);
    assert_eq!(
        model
            .prompt_overlay
            .as_ref()
            .and_then(|state| state.pending_editor.as_ref()),
        None
    );
}

#[test]
fn changed_prompt_overlay_external_editor_exit_returns_save_mutation() {
    let mut model = ready_model();
    model.open_prompt_overlay();
    model
        .prompt_overlay
        .as_mut()
        .expect("overlay should open")
        .pending_editor = Some(super::PromptOverlayPendingEditor {
        target: runtime_domain::prompt_assembly::PromptAssemblyEditorTarget::ExtraPrompt {
            scope: runtime_domain::prompt_assembly::persistence::PromptAssemblyScope::Project,
            reference_id: "repo-rules".to_string(),
        },
        original_draft: "# Repo rules\n".to_string(),
        cleanup_path_after_finish: true,
    });
    let draft_path = temp_test_file("overlay-editor-changed");
    fs::write(&draft_path, "# Repo rules\nUse cargo nextest run.\n")
        .expect("draft file should exist");

    let effect = model.update(AppEvent::ExternalEditorFinished {
        draft_path,
        original_draft: "# Repo rules\n".to_string(),
        failed: false,
    });

    assert_eq!(
        effect,
        Some(AppEffect::ApplyPromptAssemblyEditMutation {
            mutation: PromptAssemblyMutation::SaveEditorTarget {
                target: runtime_domain::prompt_assembly::PromptAssemblyEditorTarget::ExtraPrompt {
                    scope:
                        runtime_domain::prompt_assembly::persistence::PromptAssemblyScope::Project,
                    reference_id: "repo-rules".to_string(),
                },
                content: "# Repo rules\nUse cargo nextest run.\n".to_string(),
            },
        })
    );
}

#[test]
fn e_on_instruction_file_opens_real_file_in_external_editor() {
    let mut model = ready_model_with_external_editor();
    let instruction_path = temp_test_file("overlay-instructions-real-file");
    fs::write(&instruction_path, "project instructions\n").expect("instruction file should exist");
    model
        .prompt_assembly
        .resolution
        .assembly
        .active_sources
        .insert(
            1,
            prompt_source(
                "instructions:project:.",
                "AGENTS.md",
                PromptSourceKind::InstructionsFile,
                Some(PromptSourceOrigin::Project),
                PromptSourceStatus::Active { order: 1 },
            ),
        );
    model.prompt_assembly.sources.managed.insert(
        1,
        PromptAssemblyManagedSource {
            reference_id: "instructions:project:.".to_string(),
            kind: PromptSourceKind::InstructionsFile,
            title: "AGENTS.md".to_string(),
            origin: Some(PromptSourceOrigin::Project),
            scope: Some(PromptAssemblyScope::Project),
            enabled: true,
            order: 2,
        },
    );
    model
        .prompt_assembly
        .sources
        .preview
        .push(PromptAssemblyManagerSource {
            reference_id: "instructions:project:.".to_string(),
            kind: PromptSourceKind::InstructionsFile,
            title: "AGENTS.md".to_string(),
            origin: Some(PromptSourceOrigin::Project),
            resolved_body_origin: Some(PromptSourceOrigin::Project),
            backing_file_path: Some(instruction_path.clone()),
            body: Some("project instructions\n".to_string()),
        });
    model.open_prompt_overlay();
    let _ = model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Down));

    let super::OverlayInputResult::Effect(AppEffect::LaunchExternalEditor(effect)) =
        model.handle_prompt_overlay_key(KeyEvent::from(KeyCode::Char('e')))
    else {
        panic!("editing an instruction file should launch the external editor");
    };

    assert_eq!(effect.draft_path, instruction_path);
    assert_eq!(
        effect.command.last().map(String::as_str),
        Some(effect.draft_path.to_string_lossy().as_ref())
    );
}
