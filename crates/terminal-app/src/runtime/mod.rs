mod agent;
mod components;
mod context;
mod context_budget;
mod context_budget_command;
mod context_budget_worker;
mod conversation_commands;
mod dynamic_environment_worker;
mod effect_scope;
mod event_mapping;
mod inspection;
mod lifecycle;
mod lifecycle_executor;
mod llm_port;
mod permission_policy;
mod plugin;
mod prompt_assembly;
mod prompt_assembly_commands;
mod session_commands;
mod session_port;
mod session_tree_load;
mod session_worker;
mod tool_catalog;
mod workspace_tools;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use conversation_runtime::models as provider_models;
use runtime_domain::{
    model_catalog::{ModelProviderRefreshEvent, ModelSelection, ProviderSyncRequest},
    prompt_assembly::PromptAssemblyManagerSnapshot,
    request_policy::RuntimeRequestPolicy,
    session::{
        RuntimeCommand, RuntimeCommandReceipt, RuntimeEvent, SessionBranchTreePayload,
        SessionPickerRow, SessionPreviewPayload, SessionResumePayload, SessionTreePayload,
        SessionTreeRow,
    },
};
use session_store::{
    ResolvedSessionState, SessionBranchTreeSnapshot, SessionHeader, SessionId, SessionMeta,
    SessionStore, SessionTreeSnapshot, SessionTreeSnapshotRow,
};
use terminal_ui::{RuntimeWake, UiRuntimePort};
use tool_runtime::{ToolDefinition, ToolExecutorRegistry, builtin::ManagedRipgrepConfig};

use self::{
    components::RuntimeComponents,
    context::{LlmPortCapability, SessionPersistenceCapability, ToolCatalogCapability},
    event_mapping::{
        runtime_event_from_agent_event, should_defer_runtime_event_for_render_barrier,
    },
    session_port::SessionBackendViews,
    session_worker::SessionStoreWorkerEvent,
    tool_catalog::ToolCatalog,
    workspace_tools::conversation_workspace_tool_catalog,
};
use crate::prompt_assembly::PromptAssemblyEditSession;

/// `tool_definitions_for_managed_ripgrep` 在 coordinator 创建前收集内置工具定义，
/// 供初始 prompt assembly 加载使用。
pub(crate) fn tool_definitions_for_managed_ripgrep(
    managed_ripgrep: &ManagedRipgrepConfig,
    managed_root: &Path,
) -> Vec<ToolDefinition> {
    let (catalog, _registration) =
        conversation_workspace_tool_catalog(managed_ripgrep, managed_root)
            .expect("builtin workspace tools must have unique names");
    catalog.definitions()
}

/// `manager_disabled_tool_names` 投影管理快照中被禁用的工具名集合；无快照视为无禁用。
fn manager_disabled_tool_names(
    manager: Option<&PromptAssemblyManagerSnapshot>,
) -> std::collections::HashSet<String> {
    manager
        .map(|manager| {
            manager
                .candidates
                .tools
                .iter()
                .filter(|tool| !tool.tool_enabled)
                .map(|tool| tool.name.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// `session_tools_for_manager` 依据 prompt assembly 管理快照过滤出本 session 生效的工具集。
///
/// 与 prelude 的生效时机一致：coordinator 构造、`Reset`（新 session）时计算；
/// `/prompt` commit 命中当前空会话时同步重算，否则等待下一次新会话。
/// 始终返回独立快照（`filtered`），避免与全量 registry 共享内部状态导致
/// 后续注册的工具在"有无禁用记录"两种路径下可见性不一致。
fn session_tools_for_manager(
    tool_catalog: &ToolCatalog,
    manager: Option<&PromptAssemblyManagerSnapshot>,
) -> ToolExecutorRegistry {
    let disabled_tools = manager_disabled_tool_names(manager);
    tool_catalog.filtered(|tool_name| !disabled_tools.contains(tool_name))
}

/// `AppRuntimeOptions` 保存 app 层对话运行时所需的配置。
#[derive(Clone)]
pub(crate) struct AppRuntimeOptions {
    pub(crate) loaded_models: provider_models::LoadedModelCatalog,
    pub(crate) runtime_request_policy: RuntimeRequestPolicy,
    pub(crate) managed_ripgrep: ManagedRipgrepConfig,
    /// 数据目录（全局或便携 `.hunea/`），用于 AGENTS.md 等用户级文件。
    ///
    /// 由预检 `DataDirResolution` 注入；测试 Default 用 `.hunea` 占位，生产路径必须显式设置。
    pub(crate) hunea_config_dir: PathBuf,
    pub(crate) session_store: Option<Arc<dyn SessionStore>>,
    pub(crate) session_header_template: Option<SessionHeader>,
    pub(crate) initial_prompt_assembly: Option<PromptAssemblyManagerSnapshot>,
    pub(crate) dynamic_environment_observer:
        Arc<dyn crate::dynamic_environment::DynamicEnvironmentObserver>,
}

/// `AppRuntimeCoordinator` 负责把 TUI runtime command 连接到对话运行时。
pub(crate) struct AppRuntimeCoordinator {
    options: AppRuntimeOptions,
    components: RuntimeComponents,
    pending_runtime_events: Vec<RuntimeEvent>,
    next_agent_turn_id: u64,
    prompt_assembly_edit_session: Option<PromptAssemblyEditSession>,
}

impl Default for AppRuntimeOptions {
    fn default() -> Self {
        Self {
            loaded_models: provider_models::LoadedModelCatalog::default(),
            runtime_request_policy: RuntimeRequestPolicy::default(),
            managed_ripgrep: ManagedRipgrepConfig::default(),
            hunea_config_dir: PathBuf::from(".hunea"),
            session_store: None,
            session_header_template: None,
            initial_prompt_assembly: None,
            dynamic_environment_observer:
                crate::dynamic_environment::default_dynamic_environment_observer(),
        }
    }
}

impl AppRuntimeCoordinator {
    pub(crate) fn new(mut options: AppRuntimeOptions) -> Result<Self, String> {
        let components = RuntimeComponents::new(&mut options)?;
        let coordinator = Self {
            options,
            components,
            pending_runtime_events: Vec::new(),
            next_agent_turn_id: 1,
            prompt_assembly_edit_session: None,
        };
        coordinator.components.validate_context_alignment()?;
        coordinator
            .inspect_composition()
            .validate()
            .map_err(|error| format!("invalid runtime composition: {error}"))?;
        Ok(coordinator)
    }

    fn handle_runtime_command(
        &mut self,
        command: RuntimeCommand,
    ) -> Result<RuntimeCommandReceipt, String> {
        match command {
            RuntimeCommand::SubmitConversationTurn { target, request } => {
                self.start_conversation_turn(target, *request)
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
            RuntimeCommand::Reset => {
                self.components.reset_after_clear(&self.options)?;
                self.pending_runtime_events.clear();
                self.prompt_assembly_edit_session = None;
                Ok(RuntimeCommandReceipt::Accepted)
            }
        }
    }

    fn session_views(&self) -> Result<SessionBackendViews, String> {
        self.components
            .require::<SessionPersistenceCapability>()
            .map(|views| (*views).clone())
            .map_err(|_| "Session store is not available".to_string())
    }

    fn session_header(&self) -> Result<SessionHeader, String> {
        self.options
            .session_header_template
            .as_ref()
            .cloned()
            .ok_or_else(|| "Session header template is not available".to_string())
    }

    fn ensure_session_mutation_available(&self, action: &str) -> Result<(), String> {
        if self.components.session_store_worker.has_pending_mutation() {
            return Err(format!(
                "Cannot {action} while a session mutation is running"
            ));
        }
        Ok(())
    }

    fn prompt_assembly_tool_definitions(&self) -> Result<Vec<ToolDefinition>, String> {
        self.components
            .require::<ToolCatalogCapability>()
            .map(|catalog| catalog.definitions())
            .map_err(|error| error.to_string())
    }

    #[cfg(test)]
    fn defer_runtime_event_until_next_render(&mut self, event: RuntimeEvent) {
        self.defer_runtime_events_until_next_render(event, std::iter::empty());
    }

    fn defer_runtime_events_until_next_render(
        &mut self,
        event: RuntimeEvent,
        remaining: impl IntoIterator<Item = RuntimeEvent>,
    ) {
        self.pending_runtime_events.push(event);
        self.pending_runtime_events.extend(remaining);
        self.components.notify_runtime_event();
    }

    pub(crate) fn shutdown(&mut self) -> Result<(), String> {
        self.pending_runtime_events.clear();
        self.components.shutdown()
    }

    #[cfg(test)]
    pub(crate) fn has_pending_work_for_test(&self) -> bool {
        self.components.agent_port().has_pending_work()
            || self.components.model_refresh.is_running()
            || self.components.session_store_worker.has_pending_work()
            || self.components.context_budget_worker.has_pending_work()
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

impl UiRuntimePort for AppRuntimeCoordinator {
    fn bind_runtime_wake(&mut self, wake: RuntimeWake) -> Result<(), String> {
        self.components.bind_runtime_wake(wake)
    }

    fn drain_runtime_events(&mut self) -> Vec<RuntimeEvent> {
        // 消费一次 wake 后必须观测所有 producer。deferred event 只建立跨 render 的
        // 交付边界，不能让已经就绪且其 wake 可能被合并的 worker payload 留在 receiver。
        let mut events = std::mem::take(&mut self.pending_runtime_events);
        self.drain_context_budget_events_into(&mut events);
        self.drain_session_store_events_into(&mut events);
        let agent_events = self.components.agent_port_mut().drain_events();
        let mut agent_events = agent_events.into_iter().map(runtime_event_from_agent_event);
        while let Some(runtime_event) = agent_events.next() {
            if should_defer_runtime_event_for_render_barrier(&events, &runtime_event) {
                self.defer_runtime_events_until_next_render(runtime_event, agent_events);
                return events;
            }
            events.push(runtime_event);
        }
        events
    }

    fn drain_model_provider_refresh_events(&mut self) -> Vec<ModelProviderRefreshEvent> {
        let mut events = Vec::new();
        while let Some(event) = self.components.model_refresh.try_recv_event() {
            events.push(event);
        }
        events
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
        if self.components.model_refresh.is_running() {
            return Err("Model refresh is already running".to_string());
        }

        let provider_lease = self
            .components
            .require::<LlmPortCapability>()
            .map_err(|error| error.to_string())?
            .resolve_model_listing(&request.provider_id)
            .map_err(|error| error.to_string())?;
        self.components.model_refresh.start(request, provider_lease)
    }

    fn begin_prompt_assembly_edit(
        &mut self,
    ) -> Result<runtime_domain::prompt_assembly::PromptAssemblyManagerSnapshot, String> {
        self.begin_prompt_assembly_edit_impl()
    }

    fn apply_prompt_assembly_edit_mutation(
        &mut self,
        mutation: runtime_domain::prompt_assembly::PromptAssemblyMutation,
    ) -> Result<runtime_domain::prompt_assembly::PromptAssemblyManagerSnapshot, String> {
        self.apply_prompt_assembly_edit_mutation_impl(mutation)
    }

    fn commit_prompt_assembly_edit(&mut self) -> Result<(), String> {
        self.commit_prompt_assembly_edit_impl()
    }
}

impl AppRuntimeCoordinator {
    fn drain_session_store_events_into(&mut self, events: &mut Vec<RuntimeEvent>) {
        for event in self.components.session_store_worker.drain_events() {
            match event {
                SessionStoreWorkerEvent::Runtime { event, .. } => events.push(event),
                SessionStoreWorkerEvent::Restored {
                    conversation,
                    payload,
                } => {
                    if let Err(message) = self
                        .components
                        .agent_port_mut()
                        .replace_conversation(conversation)
                    {
                        events.push(RuntimeEvent::Failed {
                            target: None,
                            message,
                        });
                    } else {
                        events.push(RuntimeEvent::SessionResumed { payload });
                    }
                }
                SessionStoreWorkerEvent::RestoredWithTree {
                    conversation,
                    resume_payload,
                    tree_request_id,
                    tree_payload,
                } => {
                    if let Err(message) = self
                        .components
                        .agent_port_mut()
                        .replace_conversation(conversation)
                    {
                        events.push(RuntimeEvent::Failed {
                            target: None,
                            message,
                        });
                    } else {
                        events.push(RuntimeEvent::SessionResumed {
                            payload: resume_payload,
                        });
                        events.push(RuntimeEvent::SessionTreeLoaded {
                            request_id: tree_request_id,
                            payload: tree_payload,
                        });
                    }
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
        events.extend(self.components.context_budget_worker.drain_events());
    }
}

#[cfg(test)]
mod tests;
