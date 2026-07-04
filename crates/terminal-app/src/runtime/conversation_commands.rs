use runtime_domain::{
    dynamic_environment::{
        DynamicEnvironmentObservation, DynamicEnvironmentSessionConfig,
        DynamicEnvironmentSnapshotKind, build_dynamic_environment_snapshot,
        dynamic_environment_changes, enabled_dynamic_environment_sources_for_session_config,
    },
    prompt_assembly::PromptSourceOrigin,
    session::{
        ConversationTurnRequest, RuntimeCommandReceipt, RuntimeEvent, RuntimeTarget,
        RuntimeToolActivity, RuntimeToolActivityRawValue, RuntimeToolActivityStatus,
        RuntimeToolKind, TranscriptReplayItem, TranscriptUserMessage,
    },
};

use super::{AppRuntimeCoordinator, ensure_conversation_target};
use crate::prompt_assembly::{
    AttachedPromptMessageAssembly, ManualSkillPromptUse,
    dynamic_environment_session_config_from_manager, load_prompt_assembly_manager_snapshot,
};

impl AppRuntimeCoordinator {
    pub(super) fn truncate_conversation(
        &mut self,
        retained_user_turns: usize,
    ) -> Result<RuntimeCommandReceipt, String> {
        if self.conversation_worker.is_running() {
            return Err(
                "Cannot truncate provider conversation while a request is running".to_string(),
            );
        }
        self.ensure_session_mutation_available("truncate conversation")?;
        if let Some((session_id, leaf_id)) = self
            .provider_conversation
            .truncate_after_user_turns(retained_user_turns)
            .map_err(|error| error.to_string())?
        {
            let store = self.session_store()?;
            self.session_store_worker
                .set_leaf(store, session_id, leaf_id)?;
        }
        Ok(RuntimeCommandReceipt::Accepted)
    }

    pub(super) fn respond_conversation_permission(
        &mut self,
        target: Option<&RuntimeTarget>,
        request_id: &str,
        option_id: Option<String>,
    ) -> Result<(), String> {
        ensure_conversation_target(self.conversation_worker.current_target(), target)?;
        self.conversation_worker
            .respond_permission(request_id, option_id)
    }

    pub(super) fn respond_permission(
        &mut self,
        target: Option<&RuntimeTarget>,
        request_id: &str,
        option_id: Option<String>,
    ) -> Result<(), String> {
        match target {
            Some(RuntimeTarget::Provider(_)) => {
                self.respond_conversation_permission(target, request_id, option_id)
            }
            None if self.conversation_worker.is_running() => {
                self.respond_conversation_permission(None, request_id, option_id)
            }
            None => Err("Conversation worker is not running".to_string()),
        }
    }

    pub(super) fn start_conversation_turn(
        &mut self,
        target: RuntimeTarget,
        request: ConversationTurnRequest,
    ) -> Result<RuntimeCommandReceipt, String> {
        let request_target = request.target();
        if target != request_target {
            return Err(format!(
                "Conversation target does not match request: {}",
                target.display_label()
            ));
        }
        if self.conversation_worker.is_running() {
            return Err("Conversation request is already running".to_string());
        }

        let transcript_user_message =
            request
                .transcript_user_message()
                .cloned()
                .unwrap_or_else(|| TranscriptUserMessage {
                    content: request.message_text(),
                    attachments: Vec::new(),
                    skill_bindings: Vec::new(),
                    custom_prompt_bindings: Vec::new(),
                });
        let attached_prompt_assembly =
            self.attached_prompt_message_assembly(&transcript_user_message)?;
        let provider_request = if attached_prompt_assembly.manual_skill_uses.is_empty()
            && attached_prompt_assembly.custom_prompt_uses.is_empty()
        {
            request.clone()
        } else {
            ConversationTurnRequest::new_user_content(
                request.provider_id(),
                request.provider_kind(),
                request.model_id(),
                request.base_url().map(str::to_string),
                request.api_key().cloned(),
                request.api_key_env().map(str::to_string),
                transcript_user_message.provider_content_with_text(
                    attached_prompt_assembly.provider_visible_user_text.clone(),
                ),
            )
        };
        let manual_skill_activities =
            self.manual_skill_activities(&attached_prompt_assembly.manual_skill_uses);
        let dynamic_environment =
            self.dynamic_environment_prefix_items()
                .unwrap_or_else(|message| {
                    self.pending_runtime_events.push(RuntimeEvent::Failed {
                        target: Some(target.clone()),
                        message,
                    });
                    DynamicEnvironmentTurnInjection::default()
                });
        let activity_label = request.model_id().to_string();
        let prepared_request = self
            .provider_conversation
            .prepare_turn_with_transcript_prefix_texts_and_dynamic_environment(
                &provider_request,
                dynamic_environment.prefix_texts,
                Some(transcript_user_message),
                self.manual_skill_replay_items(&manual_skill_activities),
                dynamic_environment.next_observations,
            )
            .map_err(|error| error.to_string())?;
        self.pending_runtime_events
            .extend(self.manual_skill_runtime_events(target.clone(), &manual_skill_activities));
        self.conversation_worker.start(
            prepared_request,
            self.workspace_tools.clone(),
            self.options.runtime_request_policy.clone(),
        );
        Ok(RuntimeCommandReceipt::ConversationStarted { activity_label })
    }

    pub(super) fn dynamic_environment_prefix_items(
        &mut self,
    ) -> Result<DynamicEnvironmentTurnInjection, String> {
        let Some(work_dir) = self
            .options
            .session_header_template
            .as_ref()
            .map(|header| header.work_dir.clone())
        else {
            return Ok(DynamicEnvironmentTurnInjection::default());
        };
        let dynamic_environment_session_config =
            self.resolve_dynamic_environment_session_config(work_dir.as_path())?;
        let is_first_turn = self.provider_conversation.is_history_empty();

        let mut prefix_texts = Vec::new();
        let mut next_observations = None;
        if is_first_turn
            && dynamic_environment_session_config
                .snapshot_enabled(DynamicEnvironmentSnapshotKind::Baseline)
        {
            let sources = enabled_dynamic_environment_sources_for_session_config(
                &dynamic_environment_session_config,
                DynamicEnvironmentSnapshotKind::Baseline,
            );
            let observations = crate::dynamic_environment::observe_dynamic_environment_sources(
                work_dir.as_path(),
                &sources,
            );
            if let Some(snapshot) = build_dynamic_environment_snapshot(
                DynamicEnvironmentSnapshotKind::Baseline,
                observations.clone(),
            ) {
                prefix_texts.push(snapshot.body);
                next_observations = Some(observations);
            }
        } else if !is_first_turn
            && dynamic_environment_session_config
                .snapshot_enabled(DynamicEnvironmentSnapshotKind::Changes)
        {
            let sources = enabled_dynamic_environment_sources_for_session_config(
                &dynamic_environment_session_config,
                DynamicEnvironmentSnapshotKind::Changes,
            );
            let observations = crate::dynamic_environment::observe_dynamic_environment_sources(
                work_dir.as_path(),
                &sources,
            );
            let changed = dynamic_environment_changes(
                self.provider_conversation
                    .dynamic_environment_observations(),
                &observations,
            );
            if let Some(snapshot) =
                build_dynamic_environment_snapshot(DynamicEnvironmentSnapshotKind::Changes, changed)
            {
                prefix_texts.push(snapshot.body);
                next_observations = Some(observations);
            }
        }

        Ok(DynamicEnvironmentTurnInjection {
            prefix_texts,
            next_observations,
        })
    }

    fn resolve_dynamic_environment_session_config(
        &mut self,
        work_dir: &std::path::Path,
    ) -> Result<DynamicEnvironmentSessionConfig, String> {
        if let Some(config) = self
            .provider_conversation
            .dynamic_environment_session_config()
            .cloned()
        {
            return Ok(config);
        }

        let Some(store) = self.options.session_store.clone() else {
            return Ok(DynamicEnvironmentSessionConfig::default());
        };
        let manager =
            load_prompt_assembly_manager_snapshot(store, work_dir, &self.tool_definitions())
                .map_err(|error| error.to_string())?;
        let config = dynamic_environment_session_config_from_manager(&manager);
        self.provider_conversation
            .set_dynamic_environment_session_config(Some(config.clone()));
        Ok(config)
    }

    pub(super) fn interrupt_runtime(
        &mut self,
        target: Option<RuntimeTarget>,
    ) -> Result<RuntimeCommandReceipt, String> {
        match target {
            Some(target @ RuntimeTarget::Provider(_)) => {
                self.interrupt_conversation_worker(Some(&target))
            }
            None => {
                if self.conversation_worker.is_running() {
                    return self.interrupt_conversation_worker(None);
                }
                Ok(RuntimeCommandReceipt::Accepted)
            }
        }
    }

    pub(super) fn interrupt_conversation_worker(
        &mut self,
        command_target: Option<&RuntimeTarget>,
    ) -> Result<RuntimeCommandReceipt, String> {
        let active_target = self.conversation_worker.current_target().cloned();
        ensure_conversation_target(active_target.as_ref(), command_target)?;
        if self.conversation_worker.interrupt() {
            Ok(RuntimeCommandReceipt::Interrupted {
                target: active_target,
            })
        } else {
            Ok(RuntimeCommandReceipt::Accepted)
        }
    }

    fn attached_prompt_message_assembly(
        &self,
        user_message: &TranscriptUserMessage,
    ) -> Result<AttachedPromptMessageAssembly, String> {
        let Some(work_dir) = self
            .options
            .session_header_template
            .as_ref()
            .map(|header| header.work_dir.as_path())
        else {
            return Ok(AttachedPromptMessageAssembly {
                provider_visible_user_text: user_message.content.clone(),
                manual_skill_uses: Vec::new(),
                custom_prompt_uses: Vec::new(),
            });
        };
        crate::prompt_assembly::assemble_attached_prompt_message(
            self.options.session_store.clone(),
            work_dir,
            user_message,
            &self.tool_definitions(),
        )
        .map_err(|error| error.to_string())
    }

    fn manual_skill_activities(
        &mut self,
        uses: &[ManualSkillPromptUse],
    ) -> Vec<RuntimeToolActivity> {
        uses.iter()
            .map(|skill_use| self.synthetic_manual_skill_activity(skill_use))
            .collect()
    }

    fn manual_skill_runtime_events(
        &self,
        target: RuntimeTarget,
        activities: &[RuntimeToolActivity],
    ) -> Vec<RuntimeEvent> {
        activities
            .iter()
            .cloned()
            .map(|activity| RuntimeEvent::ToolActivityStarted {
                target: target.clone(),
                activity,
            })
            .collect()
    }

    fn manual_skill_replay_items(
        &self,
        activities: &[RuntimeToolActivity],
    ) -> Vec<TranscriptReplayItem> {
        activities
            .iter()
            .map(|skill_use| TranscriptReplayItem::ToolActivity {
                activity: skill_use.clone(),
            })
            .collect()
    }

    fn synthetic_manual_skill_activity(
        &mut self,
        skill_use: &ManualSkillPromptUse,
    ) -> RuntimeToolActivity {
        self.manual_skill_activity_sequence = self.manual_skill_activity_sequence.saturating_add(1);
        RuntimeToolActivity {
            activity_id: format!(
                "manual-skill-{}-{}",
                self.manual_skill_activity_sequence, skill_use.skill_name
            ),
            title: format!("Read {}", skill_use.skill_path.display()),
            kind: RuntimeToolKind::Read,
            status: RuntimeToolActivityStatus::Completed,
            content: Vec::new(),
            locations: Vec::new(),
            raw_input: Some(RuntimeToolActivityRawValue::from(serde_json::json!({
                "path": skill_use.skill_path.display().to_string(),
                "hunea_skill_name": skill_use.skill_name,
                "hunea_skill_origin": manual_skill_origin_label(skill_use.origin),
            }))),
            raw_output: None,
        }
    }
}

#[derive(Default)]
pub(super) struct DynamicEnvironmentTurnInjection {
    pub(super) prefix_texts: Vec<String>,
    pub(super) next_observations: Option<Vec<DynamicEnvironmentObservation>>,
}

fn manual_skill_origin_label(origin: PromptSourceOrigin) -> &'static str {
    match origin {
        PromptSourceOrigin::Builtin => "builtin",
        PromptSourceOrigin::Global => "global",
        PromptSourceOrigin::Project => "project",
    }
}
