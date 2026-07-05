use conversation_runtime::PreparedTurnOptions;
use runtime_domain::{
    dynamic_environment::{
        DynamicEnvironmentSessionConfig, enabled_dynamic_environment_sources_for_session_config,
    },
    session::{
        ConversationTurnRequest, RuntimeCommandReceipt, RuntimeEvent, RuntimeTarget,
        RuntimeToolActivity, RuntimeToolActivityRawValue, RuntimeToolActivityStatus,
        RuntimeToolKind, TranscriptReplayItem, TranscriptUserMessage,
    },
};

#[cfg(test)]
use super::dynamic_environment_worker::build_dynamic_environment_injection;
use super::{
    AppRuntimeCoordinator, PendingConversationTurn,
    dynamic_environment_worker::{
        DynamicEnvironmentInjection, DynamicEnvironmentRequest,
        dynamic_environment_snapshot_for_turn,
    },
    ensure_conversation_target,
};
use crate::prompt_assembly::{
    AttachedPromptMessageAssembly, ManualSkillPromptUse, PromptAssemblyWorkspace,
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
        if self.pending_conversation_turn.is_some() {
            return Err("Conversation request is already preparing".to_string());
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
        let activity_label = request.model_id().to_string();
        let pending_turn = PendingConversationTurn {
            target: target.clone(),
            activity_label,
            provider_request,
            transcript_user_message,
            manual_skill_activities,
        };
        if let Some(dynamic_environment_request) = self.dynamic_environment_request()? {
            self.dynamic_environment_worker
                .load(dynamic_environment_request)?;
            self.pending_conversation_turn = Some(pending_turn);
            return Ok(RuntimeCommandReceipt::Accepted);
        }

        self.start_pending_conversation_turn(pending_turn, DynamicEnvironmentInjection::default())
    }

    pub(super) fn drain_dynamic_environment_events_into(&mut self, events: &mut Vec<RuntimeEvent>) {
        let Some(result) = self.dynamic_environment_worker.try_recv_injection() else {
            return;
        };
        let Some(pending_turn) = self.pending_conversation_turn.take() else {
            return;
        };
        let target = pending_turn.target.clone();
        let dynamic_environment = match result {
            Ok(dynamic_environment) => dynamic_environment,
            Err(message) => {
                events.push(RuntimeEvent::Failed {
                    target: Some(target.clone()),
                    message,
                });
                DynamicEnvironmentInjection::default()
            }
        };
        if let Err(message) =
            self.start_pending_conversation_turn(pending_turn, dynamic_environment)
        {
            events.push(RuntimeEvent::Failed {
                target: Some(target),
                message,
            });
        }
    }

    fn start_pending_conversation_turn(
        &mut self,
        pending_turn: PendingConversationTurn,
        dynamic_environment: DynamicEnvironmentInjection,
    ) -> Result<RuntimeCommandReceipt, String> {
        let PendingConversationTurn {
            target,
            activity_label,
            provider_request,
            transcript_user_message,
            manual_skill_activities,
        } = pending_turn;
        let mut turn_options = PreparedTurnOptions::default()
            .with_provider_prefix_texts(dynamic_environment.prefix_texts)
            .with_transcript_user_message(transcript_user_message)
            .with_transcript_replay_after_user(
                self.manual_skill_replay_items(&manual_skill_activities),
            );
        if let Some(observations) = dynamic_environment.next_observations {
            turn_options = turn_options.with_dynamic_environment_observations(observations);
        }
        let prepared_request = self
            .provider_conversation
            .prepare_turn_with_options(&provider_request, turn_options)
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

    fn dynamic_environment_request(&mut self) -> Result<Option<DynamicEnvironmentRequest>, String> {
        let Some(work_dir) = self
            .options
            .session_header_template
            .as_ref()
            .map(|header| header.work_dir.clone())
        else {
            return Ok(None);
        };
        let session_config = self.resolve_dynamic_environment_session_config(work_dir.as_path())?;
        let is_first_turn = self.provider_conversation.is_history_empty();
        let Some(snapshot_kind) =
            dynamic_environment_snapshot_for_turn(&session_config, is_first_turn)
        else {
            return Ok(None);
        };
        let sources =
            enabled_dynamic_environment_sources_for_session_config(&session_config, snapshot_kind);
        if sources.is_empty() {
            return Ok(None);
        }

        Ok(Some(DynamicEnvironmentRequest {
            work_dir,
            session_config,
            is_first_turn,
            previous_observations: self
                .provider_conversation
                .dynamic_environment_observations()
                .to_vec(),
        }))
    }

    #[cfg(test)]
    pub(super) fn dynamic_environment_prefix_items(
        &mut self,
    ) -> Result<DynamicEnvironmentInjection, String> {
        let Some(work_dir) = self
            .options
            .session_header_template
            .as_ref()
            .map(|header| header.work_dir.clone())
        else {
            return Ok(DynamicEnvironmentInjection::default());
        };
        let dynamic_environment_session_config =
            self.resolve_dynamic_environment_session_config(work_dir.as_path())?;
        let is_first_turn = self.provider_conversation.is_history_empty();
        let cancellation = tokio_util::sync::CancellationToken::new();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| error.to_string())?;
        runtime.block_on(build_dynamic_environment_injection(
            self.options.dynamic_environment_observer.clone(),
            DynamicEnvironmentRequest {
                work_dir,
                session_config: dynamic_environment_session_config,
                is_first_turn,
                previous_observations: self
                    .provider_conversation
                    .dynamic_environment_observations()
                    .to_vec(),
            },
            &cancellation,
        ))
    }

    fn resolve_dynamic_environment_session_config(
        &mut self,
        _work_dir: &std::path::Path,
    ) -> Result<DynamicEnvironmentSessionConfig, String> {
        if let Some(config) = self
            .provider_conversation
            .dynamic_environment_session_config()
            .cloned()
        {
            return Ok(config);
        }

        let config = self
            .options
            .initial_dynamic_environment_session_config
            .clone()
            .unwrap_or_default();
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
                if let Some(receipt) = self.interrupt_pending_conversation_turn(None)? {
                    return Ok(receipt);
                }
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
        if let Some(receipt) = self.interrupt_pending_conversation_turn(command_target)? {
            return Ok(receipt);
        }
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

    fn interrupt_pending_conversation_turn(
        &mut self,
        command_target: Option<&RuntimeTarget>,
    ) -> Result<Option<RuntimeCommandReceipt>, String> {
        let Some(active_target) = self
            .pending_conversation_turn
            .as_ref()
            .map(|pending_turn| pending_turn.target.clone())
        else {
            return Ok(None);
        };
        ensure_conversation_target(Some(&active_target), command_target)?;
        self.dynamic_environment_worker.cancel_pending();
        self.pending_conversation_turn = None;
        Ok(Some(RuntimeCommandReceipt::Interrupted {
            target: Some(active_target),
        }))
    }

    pub(super) fn attached_prompt_message_assembly(
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
        Ok(
            PromptAssemblyWorkspace::new(work_dir, self.prompt_assembly_tool_definitions())
                .assemble_attached_prompt_message(
                    self.options.prompt_assembly_manager.as_ref(),
                    user_message,
                ),
        )
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
                "hunea_skill_origin": skill_use.origin.as_str(),
            }))),
            raw_output: None,
        }
    }
}
