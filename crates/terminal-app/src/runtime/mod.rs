mod context_budget_command;
mod context_budget_worker;
mod conversation_commands;
mod event_mapping;
mod managed_search_authorization;
mod prompt_assembly_commands;
mod session_commands;
mod session_tree_load;
mod session_worker;

use std::{path::PathBuf, sync::Arc};

use conversation_runtime::{
    ConversationWorker, ModelRefreshWorker, ProviderConversation, models as provider_models,
};
use runtime_domain::{
    model_catalog::{ModelProviderRefreshEvent, ModelSelection, ProviderSyncRequest},
    prompt_assembly::PromptPreludeSnapshot,
    request_policy::RuntimeRequestPolicy,
    session::{
        ConversationEvent, RuntimeCommand, RuntimeCommandReceipt, RuntimeEvent, RuntimeTarget,
        SessionBranchTreePayload, SessionPickerRow, SessionPreviewPayload, SessionResumePayload,
        SessionTreePayload, SessionTreeRow,
    },
};
use session_store::{
    ResolvedSessionState, SessionBranchTreeSnapshot, SessionHeader, SessionId, SessionMeta,
    SessionStore, SessionTreeSnapshot, SessionTreeSnapshotRow,
};
use terminal_ui::RuntimeCoordinator;
use tool_runtime::{ToolExecutorRegistry, builtin::ManagedSearchToolConfig};

use self::{
    context_budget_worker::ContextBudgetWorker,
    event_mapping::{
        runtime_event_from_conversation_event, should_defer_runtime_event_for_render_barrier,
    },
    managed_search_authorization::{
        conversation_workspace_tools, persist_managed_search_tool_authorization,
    },
    session_worker::{SessionStoreWorker, SessionStoreWorkerEvent},
};

/// `AppRuntimeOptions` 保存 app 层对话运行时所需的配置。
#[derive(Clone, Default)]
pub(crate) struct AppRuntimeOptions {
    pub(crate) loaded_models: provider_models::LoadedModelCatalog,
    pub(crate) runtime_request_policy: RuntimeRequestPolicy,
    pub(crate) managed_search_tools: ManagedSearchToolConfig,
    pub(crate) managed_search_authorization_config_path: Option<PathBuf>,
    pub(crate) session_store: Option<Arc<dyn SessionStore>>,
    pub(crate) session_header_template: Option<SessionHeader>,
    pub(crate) initial_prompt_prelude: Option<PromptPreludeSnapshot>,
}

/// `AppRuntimeCoordinator` 负责把 TUI runtime command 连接到对话运行时。
pub(crate) struct AppRuntimeCoordinator {
    options: AppRuntimeOptions,
    conversation_worker: ConversationWorker,
    provider_conversation: ProviderConversation,
    model_refresh: ModelRefreshWorker,
    workspace_tools: ToolExecutorRegistry,
    session_store_worker: SessionStoreWorker,
    context_budget_worker: ContextBudgetWorker,
    pending_runtime_events: Vec<RuntimeEvent>,
}

impl AppRuntimeCoordinator {
    pub(crate) fn new(options: AppRuntimeOptions) -> Result<Self, String> {
        let workspace_tools = conversation_workspace_tools(&options.managed_search_tools);
        let provider_conversation = fresh_provider_conversation(&options)?;
        Ok(Self {
            options,
            conversation_worker: ConversationWorker::default(),
            provider_conversation,
            model_refresh: ModelRefreshWorker::default(),
            workspace_tools,
            session_store_worker: SessionStoreWorker::new(),
            context_budget_worker: ContextBudgetWorker::new().map_err(|error| error.to_string())?,
            pending_runtime_events: Vec::new(),
        })
    }

    fn handle_runtime_command(
        &mut self,
        command: RuntimeCommand,
    ) -> Result<RuntimeCommandReceipt, String> {
        match command {
            RuntimeCommand::SubmitConversationTurn { target, request } => {
                self.start_conversation_turn(target, request)
            }
            RuntimeCommand::TruncateConversation {
                retained_user_turns,
            } => self.truncate_conversation(retained_user_turns),
            RuntimeCommand::Interrupt { target } => self.interrupt_runtime(target),
            RuntimeCommand::RespondPermission {
                target,
                request_id,
                option_id,
                ..
            } => {
                self.respond_permission(target.as_ref(), &request_id, option_id)?;
                Ok(RuntimeCommandReceipt::Accepted)
            }
            RuntimeCommand::ListSessions => self.list_sessions(),
            RuntimeCommand::LoadSessionPreview { session_id } => {
                self.load_session_preview(&session_id)
            }
            RuntimeCommand::ResumeSession { session_id } => self.resume_session(&session_id),
            RuntimeCommand::LoadEntryTree { request_id } => self.load_entry_tree(request_id),
            RuntimeCommand::LoadCopyPickerTree { request_id } => {
                self.load_copy_picker_tree(request_id)
            }
            RuntimeCommand::LoadContextBudgetSnapshot {
                request_id,
                selection,
            } => self.load_context_budget_snapshot_command(request_id, &selection),
            RuntimeCommand::CancelContextBudgetSnapshot => {
                Ok(self.cancel_context_budget_snapshot_command())
            }
            RuntimeCommand::LoadBranchTree { request_id } => self.load_branch_tree(request_id),
            RuntimeCommand::LoadBranchPreview {
                request_id,
                branch_row_id,
            } => self.load_branch_preview(request_id, &branch_row_id),
            RuntimeCommand::SwitchBranch {
                request_id,
                leaf_id,
            } => self.switch_branch(request_id, &leaf_id),
            RuntimeCommand::SelectEntryRewind { entry_id } => self.select_entry_rewind(&entry_id),
            RuntimeCommand::LoadMessageHistoryStartupCache => {
                self.load_message_history_startup_cache()
            }
            RuntimeCommand::CheckPromptAssemblyMissingSources => {
                self.check_prompt_assembly_missing_sources()
            }
            RuntimeCommand::LoadMessageHistoryPickerRows { request_id } => {
                self.load_message_history_picker_rows(request_id)
            }
            RuntimeCommand::RecordMessageHistory {
                entry_id,
                text,
                limit,
            } => self.record_message_history(entry_id, text, limit),
            RuntimeCommand::MutatePromptAssembly { mutation } => {
                self.mutate_prompt_assembly(mutation)
            }
            RuntimeCommand::Reset => {
                self.conversation_worker.reset_after_clear();
                self.provider_conversation = fresh_provider_conversation(&self.options)?;
                self.model_refresh.reset_after_clear();
                self.context_budget_worker.cancel_pending();
                self.workspace_tools =
                    conversation_workspace_tools(&self.options.managed_search_tools);
                self.pending_runtime_events.clear();
                Ok(RuntimeCommandReceipt::Accepted)
            }
        }
    }

    fn session_store(&self) -> Result<Arc<dyn SessionStore>, String> {
        self.options
            .session_store
            .as_ref()
            .cloned()
            .ok_or_else(|| "Session store is not available".to_string())
    }

    fn session_header(&self) -> Result<SessionHeader, String> {
        self.options
            .session_header_template
            .as_ref()
            .cloned()
            .ok_or_else(|| "Session header template is not available".to_string())
    }

    fn ensure_session_mutation_available(&self, action: &str) -> Result<(), String> {
        if self.session_store_worker.has_pending_mutation() {
            return Err(format!(
                "Cannot {action} while a session mutation is running"
            ));
        }
        Ok(())
    }

    pub(crate) fn shutdown(&mut self) -> Result<(), String> {
        self.conversation_worker.reset_after_clear();
        self.context_budget_worker.shutdown()?;
        if let Some(store) = self.options.session_store.as_ref() {
            self.session_store_worker.flush_all(store.clone())?;
        }
        Ok(())
    }
}

fn session_picker_row_from_meta(meta: SessionMeta) -> SessionPickerRow {
    let first_user_message = meta
        .first_user_preview
        .as_deref()
        .filter(|message| !message.trim().is_empty())
        .unwrap_or(&meta.title)
        .to_string();
    let last_assistant_message = meta
        .last_assistant_preview
        .or_else(|| meta.preview.clone())
        .unwrap_or_default();
    SessionPickerRow {
        session_id: meta.session_id.to_string(),
        title: meta.title.clone(),
        first_user_message,
        last_assistant_message,
        updated_at_ms: meta.updated_at,
        work_dir: meta.project_dir.display().to_string(),
        size_bytes: meta.size_bytes,
        model: meta.model,
    }
}

fn session_resume_payload(
    session_id: SessionId,
    restored_state: ResolvedSessionState,
) -> SessionResumePayload {
    let ResolvedSessionState {
        transcript,
        latest_config,
        ..
    } = restored_state;
    let restored_model = restored_model_selection(latest_config.as_ref());
    SessionResumePayload {
        session_id: session_id.to_string(),
        transcript,
        restored_model,
    }
}

fn session_preview_payload(
    session_id: SessionId,
    restored_state: ResolvedSessionState,
) -> SessionPreviewPayload {
    let ResolvedSessionState { transcript, .. } = restored_state;
    SessionPreviewPayload {
        session_id: session_id.to_string(),
        transcript,
    }
}

fn session_tree_payload(snapshot: SessionTreeSnapshot) -> SessionTreePayload {
    let current_row_id = snapshot.current_row_id.clone();
    let active_row_ids = snapshot.active_row_ids;
    SessionTreePayload {
        rows: snapshot
            .rows
            .into_iter()
            .map(|row| session_tree_row(row, current_row_id.as_deref(), &active_row_ids))
            .collect(),
        current_row_id,
    }
}

fn session_branch_tree_payload(snapshot: SessionBranchTreeSnapshot) -> SessionBranchTreePayload {
    SessionBranchTreePayload {
        nodes: snapshot.nodes,
        current_branch_row_id: snapshot.current_branch_row_id,
        total_message_count: snapshot.total_message_count,
    }
}

fn session_tree_row(
    row: SessionTreeSnapshotRow,
    current_row_id: Option<&str>,
    active_row_ids: &std::collections::HashSet<String>,
) -> SessionTreeRow {
    let is_current = current_row_id == Some(row.id.as_str());
    let is_active_path = active_row_ids.contains(&row.id);
    SessionTreeRow {
        row_id: row.id,
        parent_id: row.parent_id,
        display_depth: row.display_depth,
        kind: row.kind,
        display_text: row.display_text,
        summary: row.summary,
        preview_content: row.preview_content,
        preview_replay_items: row.preview_replay_items,
        rewind_target_id: row.rewind_target_id,
        rewind_prefill: row.rewind_prefill,
        is_active_path,
        is_current,
        branch_choices: row.branch_choices,
    }
}

fn restored_model_selection(
    config: Option<&session_store::ConfigSnapshot>,
) -> Option<ModelSelection> {
    let model_id = config
        .map(|config| config.model.trim())
        .filter(|model| !model.is_empty())?;
    let provider_id = config
        .map(|config| config.provider_id.trim())
        .filter(|provider_id| !provider_id.trim().is_empty())?;

    Some(ModelSelection::new(
        provider_id.to_string(),
        model_id.to_string(),
    ))
}

impl RuntimeCoordinator for AppRuntimeCoordinator {
    fn drain_runtime_events(&mut self) -> Vec<RuntimeEvent> {
        if !self.pending_runtime_events.is_empty() {
            return std::mem::take(&mut self.pending_runtime_events);
        }

        let mut events = Vec::new();
        self.drain_context_budget_events_into(&mut events);
        self.drain_session_store_events_into(&mut events);
        loop {
            let target = self.conversation_worker.current_target().cloned();
            let Some(event) = self.conversation_worker.try_recv_event() else {
                self.reconcile_conversation_updates();
                break;
            };
            self.reconcile_conversation_updates();
            if let ConversationEvent::ManagedSearchToolAuthorization { tool } = event {
                if let Some(event) = persist_managed_search_tool_authorization(
                    &mut self.options,
                    &mut self.workspace_tools,
                    tool,
                    target.clone(),
                ) {
                    events.push(event);
                }
                continue;
            }
            if event.is_terminal() {
                let _ = self.provider_conversation.rollback_pending_user();
            }
            let runtime_event = runtime_event_from_conversation_event(target, event);
            if should_defer_runtime_event_for_render_barrier(&events, &runtime_event) {
                self.pending_runtime_events.push(runtime_event);
                break;
            }
            events.push(runtime_event);
        }
        events
    }

    fn drain_model_provider_refresh_events(&mut self) -> Vec<ModelProviderRefreshEvent> {
        let mut events = Vec::new();
        while let Some(event) = self.model_refresh.try_recv_event() {
            events.push(event);
        }
        events
    }

    fn has_background_runtime(&self) -> bool {
        self.conversation_worker.is_running()
            || self.model_refresh.is_running()
            || self.session_store_worker.has_pending_work()
            || self.context_budget_worker.has_pending_work()
    }

    fn dispatch_runtime_command(
        &mut self,
        command: RuntimeCommand,
    ) -> Result<RuntimeCommandReceipt, String> {
        self.handle_runtime_command(command)
    }

    fn persist_selected_model(&mut self, selection: &ModelSelection) -> Result<(), String> {
        provider_models::write_default_model(
            self.options.loaded_models.source_path.as_deref(),
            selection,
        )
        .map(|_| ())
        .map_err(|error| format!("Failed to save default model: {error}"))
    }

    fn refresh_model_provider(&mut self, request: ProviderSyncRequest) -> Result<(), String> {
        if self.model_refresh.is_running() {
            return Err("Model refresh is already running".to_string());
        }

        self.model_refresh.start(request);
        Ok(())
    }
}

impl AppRuntimeCoordinator {
    fn drain_session_store_events_into(&mut self, events: &mut Vec<RuntimeEvent>) {
        for event in self.session_store_worker.drain_events() {
            match event {
                SessionStoreWorkerEvent::Runtime { event, .. } => events.push(event),
                SessionStoreWorkerEvent::Restored {
                    conversation,
                    payload,
                } => {
                    self.provider_conversation = conversation;
                    self.conversation_worker.reset_after_clear();
                    events.push(RuntimeEvent::SessionResumed { payload });
                }
                SessionStoreWorkerEvent::RestoredWithTree {
                    conversation,
                    resume_payload,
                    tree_request_id,
                    tree_payload,
                } => {
                    self.provider_conversation = conversation;
                    self.conversation_worker.reset_after_clear();
                    events.push(RuntimeEvent::SessionResumed {
                        payload: resume_payload,
                    });
                    events.push(RuntimeEvent::SessionTreeLoaded {
                        request_id: tree_request_id,
                        payload: tree_payload,
                    });
                }
                SessionStoreWorkerEvent::Noop => {}
                SessionStoreWorkerEvent::Failed { message, .. } => {
                    events.push(RuntimeEvent::Failed {
                        target: None,
                        message,
                    });
                }
            }
        }
    }

    fn drain_context_budget_events_into(&mut self, events: &mut Vec<RuntimeEvent>) {
        events.extend(self.context_budget_worker.drain_events());
    }

    fn reconcile_conversation_updates(&mut self) {
        let session_id = self.conversation_worker.take_pending_session_id();
        if let Some(entry_id) = self.conversation_worker.take_pending_user_entry_id() {
            let _ = self
                .provider_conversation
                .commit_pending_user(Some(entry_id), session_id);
        } else if let Some(session_id) = session_id {
            self.provider_conversation.set_session_id(session_id);
        }

        let items = self.conversation_worker.take_session_items();
        if items.is_empty() {
            return;
        }

        self.provider_conversation.commit_turn_items(items);
    }

    #[cfg(test)]
    fn persist_managed_search_tool_authorization(
        &mut self,
        tool: runtime_domain::session::ManagedSearchTool,
        target: Option<RuntimeTarget>,
    ) -> Option<RuntimeEvent> {
        persist_managed_search_tool_authorization(
            &mut self.options,
            &mut self.workspace_tools,
            tool,
            target,
        )
    }
}

fn fresh_provider_conversation(
    options: &AppRuntimeOptions,
) -> Result<ProviderConversation, String> {
    let mut provider_conversation = match (
        options.session_store.clone(),
        options.session_header_template.clone(),
    ) {
        (Some(store), Some(header_template)) => {
            ProviderConversation::with_session_store(store, header_template)
                .map_err(|error| error.to_string())?
        }
        _ => ProviderConversation::default(),
    };
    provider_conversation.set_prompt_prelude(options.initial_prompt_prelude.clone());
    Ok(provider_conversation)
}

fn ensure_conversation_target(
    active_target: Option<&RuntimeTarget>,
    command_target: Option<&RuntimeTarget>,
) -> Result<(), String> {
    match command_target {
        Some(target @ RuntimeTarget::Provider(_)) => match active_target {
            Some(active_target) if active_target == target => Ok(()),
            Some(_) => Err(format!(
                "Conversation is not active: {}",
                target.display_label()
            )),
            None => Err(format!(
                "Conversation is not running: {}",
                target.display_label()
            )),
        },
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests;
