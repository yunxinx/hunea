mod event_mapping;
mod managed_search_authorization;

use std::{path::PathBuf, sync::Arc};

use conversation_runtime::{
    ConversationItem, ConversationWorker, ModelRefreshWorker, ProviderConversation, Role,
    models as provider_models,
};
use runtime_domain::{
    model_catalog::{ModelProviderRefreshEvent, ModelSelection, ProviderSyncRequest},
    request_policy::RuntimeRequestPolicy,
    session::{
        ConversationEvent, ConversationTurnRequest, RuntimeCommand, RuntimeCommandReceipt,
        RuntimeEvent, RuntimeTarget, SessionPickerRow, SessionPreviewPayload, SessionResumePayload,
        SessionTreeEntry, SessionTreeEntryKind, SessionTreePayload, TranscriptReplayItem,
        TranscriptReplayRole,
    },
};
use session_store::{
    ResolvedSessionState, SessionHeader, SessionId, SessionMeta, SessionStore, SessionStoreError,
    SessionTreeSnapshot, SessionTreeSnapshotEntry, SessionTreeSnapshotEntryKind,
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
    pending_runtime_events: Vec<RuntimeEvent>,
}

impl AppRuntimeCoordinator {
    pub(crate) fn new(options: AppRuntimeOptions) -> Self {
        let workspace_tools = conversation_workspace_tools(&options.managed_search_tools);
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
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("Failed to start session list runtime: {error}"))?;
        let metas = runtime
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

        let restored_conversation = ProviderConversation::with_session_store(
            store.clone(),
            header,
            Some(session_id.clone()),
        )
        .map_err(|error| error.to_string())?;
        let restored_state = block_on_session_store(store.load_session(&session_id, None))
            .map_err(|error| error.to_string())?;
        let payload = session_resume_payload(session_id, restored_state);

        self.provider_conversation = restored_conversation;
        self.conversation_worker.reset_after_clear();
        self.pending_runtime_events
            .push(RuntimeEvent::SessionResumed { payload });
        Ok(RuntimeCommandReceipt::Accepted)
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
        let restored_state = block_on_session_store(store.load_session(&session_id, None))
            .map_err(|error| error.to_string())?;
        let payload = session_preview_payload(session_id, restored_state);

        self.pending_runtime_events
            .push(RuntimeEvent::SessionPreviewLoaded { payload });
        Ok(RuntimeCommandReceipt::Accepted)
    }

    fn load_entry_tree(&mut self) -> Result<RuntimeCommandReceipt, String> {
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
        let snapshot = block_on_session_store(store.load_session_tree(&session_id))
            .map_err(|error| error.to_string())?;
        self.pending_runtime_events
            .push(RuntimeEvent::SessionTreeLoaded {
                payload: session_tree_payload(snapshot),
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
        let snapshot = block_on_session_store(store.load_session_tree(&session_id))
            .map_err(|error| error.to_string())?;
        let selected_entry = snapshot
            .entries
            .iter()
            .find(|entry| entry.id == entry_id)
            .ok_or_else(|| format!("Entry `{entry_id}` was not found"))?;
        let target_id = selected_entry.rewind_target_id.as_deref();
        block_on_session_store(store.set_leaf(&session_id, target_id))
            .map_err(|error| error.to_string())?;
        self.provider_conversation = ProviderConversation::with_session_store(
            store.clone(),
            header,
            Some(session_id.clone()),
        )
        .map_err(|error| error.to_string())?;
        let restored_state = block_on_session_store(store.load_session(&session_id, None))
            .map_err(|error| error.to_string())?;
        let payload = session_resume_payload(session_id, restored_state);
        self.pending_runtime_events
            .push(RuntimeEvent::SessionResumed { payload });
        Ok(RuntimeCommandReceipt::Accepted)
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
    let restored_model = restored_state
        .latest_config
        .as_ref()
        .map(|config| config.model.clone());
    SessionResumePayload {
        session_id: session_id.to_string(),
        transcript: restored_state
            .items
            .into_iter()
            .filter_map(|item| transcript_replay_item_from_conversation_item(item.item))
            .collect(),
        restored_model,
        missing_model: None,
    }
}

fn session_preview_payload(
    session_id: SessionId,
    restored_state: ResolvedSessionState,
) -> SessionPreviewPayload {
    SessionPreviewPayload {
        session_id: session_id.to_string(),
        transcript: restored_state
            .items
            .into_iter()
            .filter_map(|item| transcript_replay_item_from_conversation_item(item.item))
            .collect(),
    }
}

fn session_tree_payload(snapshot: SessionTreeSnapshot) -> SessionTreePayload {
    let current_leaf_id = snapshot.current_leaf_id;
    let active_path_ids = snapshot.active_path_ids;
    SessionTreePayload {
        entries: snapshot
            .entries
            .into_iter()
            .map(|entry| session_tree_entry(entry, current_leaf_id.as_deref(), &active_path_ids))
            .collect(),
    }
}

fn session_tree_entry(
    entry: SessionTreeSnapshotEntry,
    current_leaf_id: Option<&str>,
    active_path_ids: &std::collections::HashSet<String>,
) -> SessionTreeEntry {
    let is_current_leaf = current_leaf_id == Some(entry.id.as_str());
    let is_active_path = active_path_ids.contains(&entry.id);
    SessionTreeEntry {
        entry_id: entry.id,
        parent_id: entry.parent_id,
        depth: entry.depth,
        kind: session_tree_entry_kind(entry.kind),
        label: entry.label,
        content: entry.content,
        rewind_target_id: entry.rewind_target_id,
        rewind_prefill: entry.rewind_prefill,
        is_active_path,
        is_current_leaf,
    }
}

fn session_tree_entry_kind(kind: SessionTreeSnapshotEntryKind) -> SessionTreeEntryKind {
    match kind {
        SessionTreeSnapshotEntryKind::Header => SessionTreeEntryKind::Header,
        SessionTreeSnapshotEntryKind::User => SessionTreeEntryKind::User,
        SessionTreeSnapshotEntryKind::Assistant => SessionTreeEntryKind::Assistant,
        SessionTreeSnapshotEntryKind::Tool => SessionTreeEntryKind::Tool,
        SessionTreeSnapshotEntryKind::Reasoning => SessionTreeEntryKind::Reasoning,
        SessionTreeSnapshotEntryKind::Config => SessionTreeEntryKind::Config,
        SessionTreeSnapshotEntryKind::Leaf => SessionTreeEntryKind::Leaf,
        SessionTreeSnapshotEntryKind::Other => SessionTreeEntryKind::Other,
    }
}

fn transcript_replay_item_from_conversation_item(
    item: ConversationItem,
) -> Option<TranscriptReplayItem> {
    match item {
        ConversationItem::Message { role, content } => {
            let content = ConversationItem::Message { role, content }.text_content();
            (!content.trim().is_empty()).then_some(TranscriptReplayItem {
                role: transcript_role_from_message_role(role),
                content,
            })
        }
        ConversationItem::ToolResult {
            call_id, content, ..
        } => {
            let content = ConversationItem::ToolResult {
                call_id,
                content,
                is_error: false,
            }
            .text_content();
            (!content.trim().is_empty()).then_some(TranscriptReplayItem {
                role: TranscriptReplayRole::Tool,
                content,
            })
        }
        ConversationItem::Reasoning { content, .. } => {
            (!content.trim().is_empty()).then_some(TranscriptReplayItem {
                role: TranscriptReplayRole::System,
                content,
            })
        }
    }
}

fn transcript_role_from_message_role(role: Role) -> TranscriptReplayRole {
    match role {
        Role::System => TranscriptReplayRole::System,
        Role::User => TranscriptReplayRole::User,
        Role::Assistant => TranscriptReplayRole::Assistant,
    }
}

fn block_on_session_store<T>(
    future: impl std::future::Future<Output = Result<T, SessionStoreError>>,
) -> Result<T, SessionStoreError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|source| SessionStoreError::IoError { source })?
        .block_on(future)
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
mod tests {
    use std::{
        fs,
        future::Future,
        path::{Path, PathBuf},
        pin::Pin,
        sync::Arc,
        sync::atomic::{AtomicUsize, Ordering},
        thread,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    use super::{
        AppRuntimeCoordinator, AppRuntimeOptions, ensure_conversation_target,
        should_defer_runtime_event_for_render_barrier,
    };
    use provider_protocol::{ConversationItem, Role};
    use runtime_domain::{
        provider::ProviderKind,
        session::{
            ConversationTurnRequest, ManagedSearchTool, RuntimeCommand, RuntimeEvent,
            RuntimePermissionRequest, RuntimeTarget, SessionTreeEntryKind,
        },
    };
    use session_store::{
        ConfigSnapshot, InMemorySessionStore, ResolvedSessionState, SessionHeader, SessionId,
        SessionMeta, SessionStore, SessionStoreError, SessionTreeSnapshot,
    };
    use terminal_ui::RuntimeCoordinator;

    struct LoadCountingSessionStore {
        inner: Arc<InMemorySessionStore>,
        load_session_calls: AtomicUsize,
    }

    impl LoadCountingSessionStore {
        fn load_session_calls(&self) -> usize {
            self.load_session_calls.load(Ordering::SeqCst)
        }
    }

    impl SessionStore for LoadCountingSessionStore {
        fn create_session<'a>(
            &'a self,
            header: SessionHeader,
        ) -> Pin<Box<dyn Future<Output = Result<SessionId, SessionStoreError>> + Send + 'a>>
        {
            self.inner.create_session(header)
        }

        fn append<'a>(
            &'a self,
            session_id: &'a SessionId,
            item: ConversationItem,
        ) -> Pin<Box<dyn Future<Output = Result<String, SessionStoreError>> + Send + 'a>> {
            self.inner.append(session_id, item)
        }

        fn append_config_change<'a>(
            &'a self,
            session_id: &'a SessionId,
            snapshot: ConfigSnapshot,
        ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>> {
            self.inner.append_config_change(session_id, snapshot)
        }

        fn set_leaf<'a>(
            &'a self,
            session_id: &'a SessionId,
            leaf_id: Option<&'a str>,
        ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>> {
            self.inner.set_leaf(session_id, leaf_id)
        }

        fn resolve<'a>(
            &'a self,
            session_id: &'a SessionId,
            leaf_id: Option<&'a str>,
        ) -> Pin<
            Box<dyn Future<Output = Result<Vec<ConversationItem>, SessionStoreError>> + Send + 'a>,
        > {
            self.inner.resolve(session_id, leaf_id)
        }

        fn load_session<'a>(
            &'a self,
            session_id: &'a SessionId,
            leaf_id: Option<&'a str>,
        ) -> Pin<
            Box<dyn Future<Output = Result<ResolvedSessionState, SessionStoreError>> + Send + 'a>,
        > {
            self.load_session_calls.fetch_add(1, Ordering::SeqCst);
            self.inner.load_session(session_id, leaf_id)
        }

        fn load_session_tree<'a>(
            &'a self,
            session_id: &'a SessionId,
        ) -> Pin<Box<dyn Future<Output = Result<SessionTreeSnapshot, SessionStoreError>> + Send + 'a>>
        {
            self.inner.load_session_tree(session_id)
        }

        fn list_sessions<'a>(
            &'a self,
            project_dir: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<SessionMeta>, SessionStoreError>> + Send + 'a>>
        {
            self.inner.list_sessions(project_dir)
        }

        fn get_session_meta<'a>(
            &'a self,
            session_id: &'a SessionId,
        ) -> Pin<Box<dyn Future<Output = Result<SessionMeta, SessionStoreError>> + Send + 'a>>
        {
            self.inner.get_session_meta(session_id)
        }

        fn flush<'a>(
            &'a self,
            session_id: &'a SessionId,
        ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>> {
            self.inner.flush(session_id)
        }
    }

    #[test]
    fn conversation_target_must_match_running_worker() {
        let active_target = RuntimeTarget::provider("openai", "gpt-4o-mini");
        assert!(ensure_conversation_target(Some(&active_target), None).is_ok());
        assert!(ensure_conversation_target(Some(&active_target), Some(&active_target)).is_ok());

        let inactive_target = RuntimeTarget::provider("openai", "gpt-4.1-mini");
        let inactive_error =
            ensure_conversation_target(Some(&active_target), Some(&inactive_target))
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
        let mut coordinator = AppRuntimeCoordinator::new(AppRuntimeOptions {
            managed_search_authorization_config_path: Some(config_path.clone()),
            ..AppRuntimeOptions::default()
        });

        let event =
            coordinator.persist_managed_search_tool_authorization(ManagedSearchTool::Fd, None);

        assert_eq!(event, None);
        assert_eq!(
            coordinator.options.managed_search_tools.allow_managed_fd,
            Some(true)
        );
        let content = fs::read_to_string(&config_path).expect("config should be readable");
        assert!(content.contains("allow_managed_fd = true"));
        cleanup(&root);
    }

    #[test]
    fn list_sessions_emits_session_picker_rows_for_current_project() {
        let work_dir = temp_test_dir("list-sessions-work");
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
        store_runtime
            .block_on(async {
                let session_id = store.create_session(header.clone()).await?;
                let named_header = SessionHeader {
                    session_id: SessionId::new(),
                    session_name: Some("Named session should not replace first user".to_string()),
                    ..header.clone()
                };
                let named_session_id = store.create_session(named_header).await?;
                store
                    .append(
                        &named_session_id,
                        ConversationItem::text(Role::User, "first named user"),
                    )
                    .await?;
                store
                    .append(
                        &named_session_id,
                        ConversationItem::text(Role::Assistant, "last named assistant"),
                    )
                    .await?;
                store
                    .append(
                        &session_id,
                        ConversationItem::text(Role::User, "hello resume"),
                    )
                    .await?;
                store
                    .append(
                        &session_id,
                        ConversationItem::text(Role::Assistant, "resume preview answer"),
                    )
                    .await?;
                Ok::<(), session_store::SessionStoreError>(())
            })
            .expect("session fixture should persist");
        let mut coordinator = AppRuntimeCoordinator::new(AppRuntimeOptions {
            session_store: Some(store),
            session_header_template: Some(header),
            ..AppRuntimeOptions::default()
        });

        coordinator
            .handle_runtime_command(RuntimeCommand::ListSessions)
            .expect("list sessions should succeed");

        let events = RuntimeCoordinator::drain_runtime_events(&mut coordinator);
        let Some(RuntimeEvent::SessionListLoaded { rows }) = events.into_iter().next() else {
            panic!("expected session list event");
        };
        assert_eq!(rows.len(), 2);
        let row = rows
            .iter()
            .find(|row| row.first_user_message == "hello resume")
            .expect("ordinary session row should use first user message");
        assert_eq!(row.last_assistant_message, "resume preview answer");
        let named_row = rows
            .iter()
            .find(|row| row.title == "Named session should not replace first user")
            .expect("named session row should be present");
        assert_eq!(named_row.first_user_message, "first named user");
        assert_eq!(named_row.last_assistant_message, "last named assistant");
        cleanup(&work_dir);
    }

    #[test]
    fn list_sessions_builds_rows_from_metadata_without_loading_full_sessions() {
        let work_dir = temp_test_dir("list-sessions-metadata-only-work");
        let inner_store = Arc::new(InMemorySessionStore::new());
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
        store_runtime
            .block_on(async {
                let session_id = inner_store.create_session(header.clone()).await?;
                inner_store
                    .append(
                        &session_id,
                        ConversationItem::text(Role::User, "metadata first user"),
                    )
                    .await?;
                inner_store
                    .append(
                        &session_id,
                        ConversationItem::text(Role::Assistant, "metadata assistant answer"),
                    )
                    .await?;
                Ok::<(), SessionStoreError>(())
            })
            .expect("session fixture should persist");
        let store = Arc::new(LoadCountingSessionStore {
            inner: inner_store,
            load_session_calls: AtomicUsize::new(0),
        });
        let mut coordinator = AppRuntimeCoordinator::new(AppRuntimeOptions {
            session_store: Some(store.clone()),
            session_header_template: Some(header),
            ..AppRuntimeOptions::default()
        });

        coordinator
            .handle_runtime_command(RuntimeCommand::ListSessions)
            .expect("list sessions should succeed");

        let events = RuntimeCoordinator::drain_runtime_events(&mut coordinator);
        let Some(RuntimeEvent::SessionListLoaded { rows }) = events.into_iter().next() else {
            panic!("expected session list event");
        };
        assert_eq!(store.load_session_calls(), 0);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].first_user_message, "metadata first user");
        assert_eq!(rows[0].last_assistant_message, "metadata assistant answer");
        cleanup(&work_dir);
    }

    #[test]
    fn list_sessions_excludes_active_session() {
        let work_dir = temp_test_dir("list-sessions-excludes-active-work");
        let store = Arc::new(InMemorySessionStore::new());
        let store_runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("session store runtime should start");
        let active_header = SessionHeader {
            session_id: SessionId::new(),
            work_dir: work_dir.clone(),
            session_name: Some("active session".to_string()),
            initial_model: "qwen3".to_string(),
            git_head: None,
            cli_version: None,
        };
        let other_header = SessionHeader {
            session_id: SessionId::new(),
            session_name: Some("other session".to_string()),
            ..active_header.clone()
        };
        let (active_session_id, other_session_id) = store_runtime
            .block_on(async {
                let active_session_id = store.create_session(active_header.clone()).await?;
                let other_session_id = store.create_session(other_header).await?;
                Ok::<(SessionId, SessionId), session_store::SessionStoreError>((
                    active_session_id,
                    other_session_id,
                ))
            })
            .expect("session fixture should persist");
        let mut coordinator = AppRuntimeCoordinator::new(AppRuntimeOptions {
            session_store: Some(store),
            session_header_template: Some(SessionHeader {
                session_id: active_session_id.clone(),
                ..active_header
            }),
            ..AppRuntimeOptions::default()
        });

        coordinator
            .handle_runtime_command(RuntimeCommand::ListSessions)
            .expect("list sessions should succeed");

        let events = RuntimeCoordinator::drain_runtime_events(&mut coordinator);
        let Some(RuntimeEvent::SessionListLoaded { rows }) = events.into_iter().next() else {
            panic!("expected session list event");
        };
        assert_eq!(
            rows.iter()
                .map(|row| row.session_id.as_str())
                .collect::<Vec<_>>(),
            vec![other_session_id.to_string()]
        );
        cleanup(&work_dir);
    }

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
                    .append(
                        &session_id,
                        ConversationItem::text(Role::Assistant, "resume answer"),
                    )
                    .await?;
                store
                    .append_config_change(
                        &session_id,
                        ConfigSnapshot {
                            model: "qwen3".to_string(),
                            system_prompt: Some("historical prompt".to_string()),
                        },
                    )
                    .await?;
                Ok::<SessionId, session_store::SessionStoreError>(session_id)
            })
            .expect("session fixture should persist");
        let mut coordinator = AppRuntimeCoordinator::new(AppRuntimeOptions {
            session_store: Some(store),
            session_header_template: Some(header),
            ..AppRuntimeOptions::default()
        });

        coordinator
            .handle_runtime_command(RuntimeCommand::ResumeSession {
                session_id: session_id.to_string(),
            })
            .expect("resume session should succeed");

        let events = RuntimeCoordinator::drain_runtime_events(&mut coordinator);
        let Some(RuntimeEvent::SessionResumed { payload }) = events.into_iter().next() else {
            panic!("expected session resumed event");
        };
        assert_eq!(payload.session_id, session_id.to_string());
        assert_eq!(payload.restored_model.as_deref(), Some("qwen3"));
        assert_eq!(payload.missing_model, None);
        assert_eq!(
            payload
                .transcript
                .iter()
                .map(|item| item.content.as_str())
                .collect::<Vec<_>>(),
            vec!["hello resume", "resume answer"]
        );
        assert_eq!(
            coordinator
                .provider_conversation
                .history()
                .iter()
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
                    .append(
                        &preview_session_id,
                        ConversationItem::text(Role::Assistant, "preview answer"),
                    )
                    .await?;
                Ok::<SessionId, session_store::SessionStoreError>(preview_session_id)
            })
            .expect("session fixture should persist");
        let mut coordinator = AppRuntimeCoordinator::new(AppRuntimeOptions {
            session_store: Some(store),
            session_header_template: Some(header),
            ..AppRuntimeOptions::default()
        });

        coordinator
            .handle_runtime_command(RuntimeCommand::LoadSessionPreview {
                session_id: preview_session_id.to_string(),
            })
            .expect("load preview should succeed");

        let events = RuntimeCoordinator::drain_runtime_events(&mut coordinator);
        let Some(RuntimeEvent::SessionPreviewLoaded { payload }) = events.into_iter().next() else {
            panic!("expected session preview event");
        };
        assert_eq!(payload.session_id, preview_session_id.to_string());
        assert_eq!(
            payload
                .transcript
                .iter()
                .map(|item| item.content.as_str())
                .collect::<Vec<_>>(),
            vec!["preview user", "preview answer"]
        );
        assert!(
            coordinator.provider_conversation.history().is_empty(),
            "loading preview should not replace the active provider conversation"
        );
        cleanup(&work_dir);
    }

    #[test]
    fn load_entry_tree_emits_rewind_targets_for_active_session() {
        let work_dir = temp_test_dir("load-entry-tree-work");
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
                    .append(&session_id, ConversationItem::text(Role::User, "first"))
                    .await?;
                store
                    .append(
                        &session_id,
                        ConversationItem::text(Role::Assistant, "answer"),
                    )
                    .await?;
                store
                    .append(&session_id, ConversationItem::text(Role::User, "second"))
                    .await?;
                Ok::<SessionId, session_store::SessionStoreError>(session_id)
            })
            .expect("session fixture should persist");
        let mut coordinator = AppRuntimeCoordinator::new(AppRuntimeOptions {
            session_store: Some(store),
            session_header_template: Some(header),
            ..AppRuntimeOptions::default()
        });
        coordinator
            .handle_runtime_command(RuntimeCommand::ResumeSession {
                session_id: session_id.to_string(),
            })
            .expect("resume session should succeed");
        RuntimeCoordinator::drain_runtime_events(&mut coordinator);

        coordinator
            .handle_runtime_command(RuntimeCommand::LoadEntryTree)
            .expect("load entry tree should succeed");

        let events = RuntimeCoordinator::drain_runtime_events(&mut coordinator);
        let Some(RuntimeEvent::SessionTreeLoaded { payload }) = events.into_iter().next() else {
            panic!("expected session tree loaded event");
        };
        let second_user = payload
            .entries
            .iter()
            .find(|entry| entry.content == "second")
            .expect("second user entry should be present");
        assert_eq!(second_user.kind, SessionTreeEntryKind::User);
        assert_eq!(second_user.rewind_prefill.as_deref(), Some("second"));
        let assistant = payload
            .entries
            .iter()
            .find(|entry| entry.content == "answer")
            .expect("assistant entry should be present");
        assert_eq!(
            second_user.rewind_target_id.as_deref(),
            Some(assistant.entry_id.as_str())
        );
        assert!(payload.entries.iter().any(|entry| entry.is_current_leaf));
        cleanup(&work_dir);
    }

    #[test]
    fn select_entry_rewind_rebuilds_provider_history_to_selected_entry() {
        let work_dir = temp_test_dir("select-entry-rewind-work");
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
        let (session_id, assistant_entry_id) = store_runtime
            .block_on(async {
                let session_id = store.create_session(header.clone()).await?;
                store
                    .append(&session_id, ConversationItem::text(Role::User, "first"))
                    .await?;
                let assistant_entry_id = store
                    .append(
                        &session_id,
                        ConversationItem::text(Role::Assistant, "answer"),
                    )
                    .await?;
                store
                    .append(&session_id, ConversationItem::text(Role::User, "second"))
                    .await?;
                Ok::<(SessionId, String), session_store::SessionStoreError>((
                    session_id,
                    assistant_entry_id,
                ))
            })
            .expect("session fixture should persist");
        let mut coordinator = AppRuntimeCoordinator::new(AppRuntimeOptions {
            session_store: Some(store),
            session_header_template: Some(header),
            ..AppRuntimeOptions::default()
        });
        coordinator
            .handle_runtime_command(RuntimeCommand::ResumeSession {
                session_id: session_id.to_string(),
            })
            .expect("resume session should succeed");
        RuntimeCoordinator::drain_runtime_events(&mut coordinator);

        coordinator
            .handle_runtime_command(RuntimeCommand::SelectEntryRewind {
                entry_id: assistant_entry_id,
            })
            .expect("select entry rewind should succeed");

        assert_eq!(
            coordinator
                .provider_conversation
                .history()
                .iter()
                .map(ConversationItem::text_content)
                .collect::<Vec<_>>(),
            vec!["first", "answer"]
        );
        let events = RuntimeCoordinator::drain_runtime_events(&mut coordinator);
        let Some(RuntimeEvent::SessionResumed { payload }) = events.into_iter().next() else {
            panic!("expected resumed payload after entry rewind");
        };
        assert_eq!(
            payload
                .transcript
                .iter()
                .map(|item| item.content.as_str())
                .collect::<Vec<_>>(),
            vec!["first", "answer"]
        );
        cleanup(&work_dir);
    }

    #[test]
    fn conversation_failure_before_provider_request_rolls_back_pending_user() {
        let mut coordinator = AppRuntimeCoordinator::new(AppRuntimeOptions {
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
            .handle_runtime_command(RuntimeCommand::SubmitConversationTurn { target, request })
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
        assert!(coordinator.provider_conversation.history().is_empty());

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

    fn temp_test_dir(prefix: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("hunea-{prefix}-{}-{stamp}", std::process::id()));
        fs::create_dir_all(&root).expect("create temp root");
        root
    }

    fn cleanup(path: &Path) {
        let _ = fs::remove_dir_all(path);
    }
}
