mod event_mapping;
mod managed_search_authorization;

use std::{future::Future, path::PathBuf, sync::Arc};

use conversation_runtime::{
    ConversationWorker, ModelRefreshWorker, ProviderConversation, ProviderConversationError,
    models as provider_models,
};
use runtime_domain::{
    model_catalog::{ModelProviderRefreshEvent, ModelSelection, ProviderSyncRequest},
    request_policy::RuntimeRequestPolicy,
    session::{
        ConversationEvent, ConversationTurnRequest, RuntimeCommand, RuntimeCommandReceipt,
        RuntimeEvent, RuntimeTarget, SessionBranchTreePayload, SessionPickerRow,
        SessionPreviewPayload, SessionResumePayload, SessionTreePayload, SessionTreeRow,
    },
};
use session_store::{
    ResolvedSessionState, SessionBranchTreeSnapshot, SessionHeader, SessionId, SessionMeta,
    SessionStore, SessionStoreError, SessionTreeSnapshot, SessionTreeSnapshotRow,
};
use terminal_ui::RuntimeCoordinator;
use tool_runtime::{ToolExecutorRegistry, builtin::ManagedSearchToolConfig};

use self::{
    event_mapping::{
        runtime_event_from_conversation_event, should_defer_runtime_event_for_render_barrier,
    },
    managed_search_authorization::{
        conversation_workspace_tools, persist_managed_search_tool_authorization,
    },
};

/// `AppRuntimeOptions` 保存 app 层对话运行时所需的配置。
#[derive(Clone, Default)]
pub(crate) struct AppRuntimeOptions {
    pub(crate) model_config_path: Option<PathBuf>,
    pub(crate) runtime_request_policy: RuntimeRequestPolicy,
    pub(crate) managed_search_tools: ManagedSearchToolConfig,
    pub(crate) managed_search_authorization_config_path: Option<PathBuf>,
    pub(crate) session_store: Option<Arc<dyn SessionStore>>,
    pub(crate) session_header_template: Option<SessionHeader>,
}

/// `AppRuntimeCoordinator` 负责把 TUI runtime command 连接到对话运行时。
pub(crate) struct AppRuntimeCoordinator {
    options: AppRuntimeOptions,
    conversation_worker: ConversationWorker,
    provider_conversation: ProviderConversation,
    model_refresh: ModelRefreshWorker,
    workspace_tools: ToolExecutorRegistry,
    session_store_runtime: SessionStoreRuntime,
    pending_runtime_events: Vec<RuntimeEvent>,
}

struct SessionStoreRuntime {
    runtime: tokio::runtime::Runtime,
}

struct PreparedSessionRestore {
    session_id: SessionId,
    conversation: ProviderConversation,
    restored_state: ResolvedSessionState,
}

#[derive(Debug)]
enum SessionRestoreError {
    Store(SessionStoreError),
    ProviderConversation(ProviderConversationError),
}

impl SessionStoreRuntime {
    fn new() -> Result<Self, SessionStoreError> {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map(|runtime| Self { runtime })
            .map_err(|source| SessionStoreError::IoError { source })
    }

    fn block_on<T>(
        &self,
        future: impl Future<Output = Result<T, SessionStoreError>>,
    ) -> Result<T, SessionStoreError> {
        self.runtime.block_on(future)
    }
}

impl From<SessionStoreError> for SessionRestoreError {
    fn from(error: SessionStoreError) -> Self {
        Self::Store(error)
    }
}

impl From<ProviderConversationError> for SessionRestoreError {
    fn from(error: ProviderConversationError) -> Self {
        Self::ProviderConversation(error)
    }
}

impl std::fmt::Display for SessionRestoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store(error) => error.fmt(formatter),
            Self::ProviderConversation(error) => error.fmt(formatter),
        }
    }
}

impl AppRuntimeCoordinator {
    pub(crate) fn new(options: AppRuntimeOptions) -> Self {
        let workspace_tools = conversation_workspace_tools(&options.managed_search_tools);
        let session_store_runtime =
            SessionStoreRuntime::new().expect("session store runtime should initialize");
        let provider_conversation = match (
            options.session_store.clone(),
            options.session_header_template.clone(),
        ) {
            (Some(store), Some(header_template)) => {
                ProviderConversation::with_session_store(store, header_template, None)
                    .expect("session store should initialize without a session id")
            }
            _ => ProviderConversation::default(),
        };
        Self {
            options,
            conversation_worker: ConversationWorker::default(),
            provider_conversation,
            model_refresh: ModelRefreshWorker::default(),
            workspace_tools,
            session_store_runtime,
            pending_runtime_events: Vec::new(),
        }
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
            } => {
                if self.conversation_worker.is_running() {
                    return Err(
                        "Cannot truncate provider conversation while a request is running"
                            .to_string(),
                    );
                }
                self.provider_conversation
                    .truncate_after_user_turns(retained_user_turns)
                    .map_err(|error| error.to_string())?;
                Ok(RuntimeCommandReceipt::Accepted)
            }
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
            RuntimeCommand::ListSessions => {
                let rows = self.session_picker_rows()?;
                self.pending_runtime_events
                    .push(RuntimeEvent::SessionListLoaded { rows });
                Ok(RuntimeCommandReceipt::Accepted)
            }
            RuntimeCommand::LoadSessionPreview { session_id } => {
                self.load_session_preview(&session_id)
            }
            RuntimeCommand::ResumeSession { session_id } => self.resume_session(&session_id),
            RuntimeCommand::LoadEntryTree => self.load_entry_tree(),
            RuntimeCommand::LoadBranchTree => self.load_branch_tree(),
            RuntimeCommand::LoadBranchPreview { branch_row_id } => {
                self.load_branch_preview(&branch_row_id)
            }
            RuntimeCommand::SwitchBranch { leaf_id } => self.switch_branch(&leaf_id),
            RuntimeCommand::SelectEntryRewind { entry_id } => self.select_entry_rewind(&entry_id),
            RuntimeCommand::Reset => {
                self.conversation_worker.reset_after_clear();
                self.provider_conversation.clear();
                self.model_refresh.reset_after_clear();
                self.workspace_tools =
                    conversation_workspace_tools(&self.options.managed_search_tools);
                self.pending_runtime_events.clear();
                Ok(RuntimeCommandReceipt::Accepted)
            }
        }
    }

    fn session_picker_rows(&self) -> Result<Vec<SessionPickerRow>, String> {
        let store = self
            .options
            .session_store
            .as_ref()
            .ok_or_else(|| "Session store is not available".to_string())?;
        let header = self
            .options
            .session_header_template
            .as_ref()
            .ok_or_else(|| "Session header template is not available".to_string())?;
        let project_dir = header.work_dir.to_string_lossy();
        let metas = self
            .session_store_runtime
            .block_on(store.list_sessions(project_dir.as_ref()))
            .map_err(|error| error.to_string())?;
        let active_session_id = self
            .provider_conversation
            .session_id()
            .or(Some(&header.session_id));
        Ok(metas
            .into_iter()
            .filter(|meta| {
                active_session_id
                    .map(|session_id| meta.session_id != *session_id)
                    .unwrap_or(true)
            })
            .map(session_picker_row_from_meta)
            .collect())
    }

    fn resume_session(&mut self, session_id: &str) -> Result<RuntimeCommandReceipt, String> {
        if self.conversation_worker.is_running() {
            return Err("Cannot resume session while a request is running".to_string());
        }

        let session_id = session_id
            .parse::<SessionId>()
            .map_err(|error| format!("Invalid session id: {error}"))?;
        let store = self
            .options
            .session_store
            .as_ref()
            .cloned()
            .ok_or_else(|| "Session store is not available".to_string())?;
        let header = self
            .options
            .session_header_template
            .as_ref()
            .cloned()
            .ok_or_else(|| "Session header template is not available".to_string())?;

        let restore = self
            .prepare_committed_session_restore(store, header, session_id)
            .map_err(|error| error.to_string())?;
        let payload = self.apply_prepared_session_leaf_restore(restore);

        self.conversation_worker.reset_after_clear();
        self.pending_runtime_events
            .push(RuntimeEvent::SessionResumed { payload });
        Ok(RuntimeCommandReceipt::Accepted)
    }

    fn prepare_committed_session_restore(
        &self,
        store: Arc<dyn SessionStore>,
        header: SessionHeader,
        session_id: SessionId,
    ) -> Result<PreparedSessionRestore, SessionRestoreError> {
        let restored_state = self
            .session_store_runtime
            .block_on(store.load_session(&session_id, None))?;
        let conversation = ProviderConversation::with_resolved_session_store(
            store,
            header,
            Some(session_id.clone()),
            &restored_state,
        )?;

        Ok(PreparedSessionRestore {
            session_id,
            conversation,
            restored_state,
        })
    }

    fn load_session_preview(&mut self, session_id: &str) -> Result<RuntimeCommandReceipt, String> {
        let session_id = session_id
            .parse::<SessionId>()
            .map_err(|error| format!("Invalid session id: {error}"))?;
        let store = self
            .options
            .session_store
            .as_ref()
            .cloned()
            .ok_or_else(|| "Session store is not available".to_string())?;
        let restored_state = self
            .session_store_runtime
            .block_on(store.load_session(&session_id, None))
            .map_err(|error| error.to_string())?;
        let payload = session_preview_payload(session_id, restored_state);

        self.pending_runtime_events
            .push(RuntimeEvent::SessionPreviewLoaded { payload });
        Ok(RuntimeCommandReceipt::Accepted)
    }

    fn load_entry_tree(&mut self) -> Result<RuntimeCommandReceipt, String> {
        let Some(session_id) = self.provider_conversation.session_id().cloned() else {
            if self.provider_conversation.history().is_empty() {
                self.pending_runtime_events
                    .push(RuntimeEvent::SessionTreeLoaded {
                        payload: SessionTreePayload {
                            rows: Vec::new(),
                            current_row_id: None,
                        },
                    });
                return Ok(RuntimeCommandReceipt::Accepted);
            }
            return Err("No active persisted session to show tree".to_string());
        };
        let store = self
            .options
            .session_store
            .as_ref()
            .cloned()
            .ok_or_else(|| "Session store is not available".to_string())?;
        let snapshot = self
            .session_store_runtime
            .block_on(store.load_session_tree(&session_id))
            .map_err(|error| error.to_string())?;
        self.pending_runtime_events
            .push(RuntimeEvent::SessionTreeLoaded {
                payload: session_tree_payload(snapshot),
            });
        Ok(RuntimeCommandReceipt::Accepted)
    }

    fn load_branch_preview(
        &mut self,
        branch_row_id: &str,
    ) -> Result<RuntimeCommandReceipt, String> {
        let session_id = self
            .provider_conversation
            .session_id()
            .cloned()
            .ok_or_else(|| "No active persisted session to preview".to_string())?;
        let store = self
            .options
            .session_store
            .as_ref()
            .cloned()
            .ok_or_else(|| "Session store is not available".to_string())?;
        let snapshot = self
            .session_store_runtime
            .block_on(store.load_session_branch_preview(&session_id, branch_row_id))
            .map_err(|error| error.to_string())?;
        self.pending_runtime_events
            .push(RuntimeEvent::SessionTreePreviewLoaded {
                payload: session_tree_payload(snapshot),
            });
        Ok(RuntimeCommandReceipt::Accepted)
    }

    fn load_branch_tree(&mut self) -> Result<RuntimeCommandReceipt, String> {
        let Some(session_id) = self.provider_conversation.session_id().cloned() else {
            if self.provider_conversation.history().is_empty() {
                self.pending_runtime_events
                    .push(RuntimeEvent::SessionBranchTreeLoaded {
                        payload: SessionBranchTreePayload {
                            nodes: Vec::new(),
                            current_branch_row_id: None,
                            total_message_count: 0,
                        },
                    });
                return Ok(RuntimeCommandReceipt::Accepted);
            }
            return Err("No active persisted session to show branch tree".to_string());
        };
        let store = self
            .options
            .session_store
            .as_ref()
            .cloned()
            .ok_or_else(|| "Session store is not available".to_string())?;
        let snapshot = self
            .session_store_runtime
            .block_on(store.load_session_branch_tree(&session_id))
            .map_err(|error| error.to_string())?;
        self.pending_runtime_events
            .push(RuntimeEvent::SessionBranchTreeLoaded {
                payload: session_branch_tree_payload(snapshot),
            });
        Ok(RuntimeCommandReceipt::Accepted)
    }

    fn switch_branch(&mut self, leaf_id: &str) -> Result<RuntimeCommandReceipt, String> {
        if self.conversation_worker.is_running() {
            return Err("Cannot switch branch while a request is running".to_string());
        }
        let session_id = self
            .provider_conversation
            .session_id()
            .cloned()
            .ok_or_else(|| "No active persisted session to switch branch".to_string())?;
        let store = self
            .options
            .session_store
            .as_ref()
            .cloned()
            .ok_or_else(|| "Session store is not available".to_string())?;
        let header = self
            .options
            .session_header_template
            .as_ref()
            .cloned()
            .ok_or_else(|| "Session header template is not available".to_string())?;

        let restore = self
            .prepare_session_leaf_restore(store.clone(), header, session_id.clone(), leaf_id)
            .map_err(|error| error.to_string())?;
        let snapshot = self
            .session_store_runtime
            .block_on(store.load_session_tree_for_leaf(&session_id, leaf_id))
            .map_err(|error| error.to_string())?;
        self.session_store_runtime
            .block_on(store.set_leaf(&session_id, Some(leaf_id)))
            .map_err(|error| error.to_string())?;

        let resume_payload = self.apply_prepared_session_leaf_restore(restore);
        let tree_payload = session_tree_payload(snapshot);
        self.pending_runtime_events
            .push(RuntimeEvent::SessionResumed {
                payload: resume_payload,
            });
        self.pending_runtime_events
            .push(RuntimeEvent::SessionTreeLoaded {
                payload: tree_payload,
            });
        Ok(RuntimeCommandReceipt::Accepted)
    }

    fn select_entry_rewind(&mut self, entry_id: &str) -> Result<RuntimeCommandReceipt, String> {
        if self.conversation_worker.is_running() {
            return Err("Cannot rewind session while a request is running".to_string());
        }
        let session_id = self
            .provider_conversation
            .session_id()
            .cloned()
            .ok_or_else(|| "No active persisted session to rewind".to_string())?;
        let store = self
            .options
            .session_store
            .as_ref()
            .cloned()
            .ok_or_else(|| "Session store is not available".to_string())?;
        let header = self
            .options
            .session_header_template
            .as_ref()
            .cloned()
            .ok_or_else(|| "Session header template is not available".to_string())?;
        let snapshot = self
            .session_store_runtime
            .block_on(store.load_session_tree(&session_id))
            .map_err(|error| error.to_string())?;
        let selected_row = snapshot
            .rows
            .iter()
            .find(|row| row.id == entry_id)
            .ok_or_else(|| format!("Tree row `{entry_id}` was not found"))?;
        let Some(rewind_target_id) = selected_row.rewind_target_id.as_deref() else {
            return Ok(RuntimeCommandReceipt::Accepted);
        };
        let rewind_target_id = rewind_target_id.to_string();
        let restore = self
            .prepare_session_leaf_restore(
                store.clone(),
                header,
                session_id.clone(),
                &rewind_target_id,
            )
            .map_err(|error| error.to_string())?;
        self.session_store_runtime
            .block_on(store.set_leaf(&session_id, Some(&rewind_target_id)))
            .map_err(|error| error.to_string())?;
        let payload = self.apply_prepared_session_leaf_restore(restore);
        self.pending_runtime_events
            .push(RuntimeEvent::SessionResumed { payload });
        Ok(RuntimeCommandReceipt::Accepted)
    }

    fn prepare_session_leaf_restore(
        &self,
        store: Arc<dyn SessionStore>,
        header: SessionHeader,
        session_id: SessionId,
        leaf_id: &str,
    ) -> Result<PreparedSessionRestore, SessionRestoreError> {
        let restored_state = self
            .session_store_runtime
            .block_on(store.load_session(&session_id, Some(leaf_id)))?;
        let conversation = ProviderConversation::with_resolved_session_store(
            store,
            header,
            Some(session_id.clone()),
            &restored_state,
        )?;

        Ok(PreparedSessionRestore {
            session_id,
            conversation,
            restored_state,
        })
    }

    fn apply_prepared_session_leaf_restore(
        &mut self,
        restore: PreparedSessionRestore,
    ) -> SessionResumePayload {
        self.provider_conversation = restore.conversation;
        session_resume_payload(restore.session_id, restore.restored_state)
    }

    fn respond_conversation_permission(
        &mut self,
        target: Option<&RuntimeTarget>,
        request_id: &str,
        option_id: Option<String>,
    ) -> Result<(), String> {
        ensure_conversation_target(self.conversation_worker.current_target(), target)?;
        self.conversation_worker
            .respond_permission(request_id, option_id)
    }

    fn respond_permission(
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

    fn start_conversation_turn(
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

        let activity_label = request.model_id().to_string();
        let prepared_request = self
            .provider_conversation
            .prepare_turn(&request)
            .map_err(|error| error.to_string())?;
        self.conversation_worker.start(
            prepared_request,
            self.workspace_tools.clone(),
            self.options.runtime_request_policy.clone(),
        );
        Ok(RuntimeCommandReceipt::ConversationStarted { activity_label })
    }

    fn interrupt_runtime(
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

    fn interrupt_conversation_worker(
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
}

fn session_picker_row_from_meta(meta: SessionMeta) -> SessionPickerRow {
    let size_bytes = std::fs::metadata(&meta.jsonl_path)
        .ok()
        .map(|metadata| metadata.len());
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
        work_dir: meta.work_dir.display().to_string(),
        size_bytes,
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
                self.provider_conversation.rollback_pending_user();
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
        self.conversation_worker.is_running() || self.model_refresh.is_running()
    }

    fn dispatch_runtime_command(
        &mut self,
        command: RuntimeCommand,
    ) -> Result<RuntimeCommandReceipt, String> {
        self.handle_runtime_command(command)
    }

    fn persist_selected_model(&mut self, selection: &ModelSelection) -> Result<(), String> {
        provider_models::write_default_model(self.options.model_config_path.as_deref(), selection)
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
    fn reconcile_conversation_updates(&mut self) {
        if let Some(entry_id) = self.conversation_worker.take_pending_user_entry_id() {
            self.provider_conversation
                .commit_pending_user(Some(entry_id));
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
