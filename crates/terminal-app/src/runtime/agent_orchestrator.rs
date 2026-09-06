//! Runtime-owned Agent tree、identity routing 与 lifecycle ownership。

use session_store::SessionPort;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};
use tokio::sync::oneshot;

use crate::session_store_bridge::run_session_store_future;
use runtime_domain::agent::{
    AgentActivitySummary, AgentChildCompletion, AgentCommand, AgentCommandReceipt, AgentEvent,
    AgentEventKind, AgentGroupCompletion, AgentId, AgentLaunchBatch, AgentLaunchChildSnapshot,
    AgentLaunchGroupId, AgentLaunchReceipt, AgentObjectiveSummary, AgentObservationId,
    AgentOutcome, AgentOutcomeSummary, AgentOverviewRow, AgentOverviewSnapshot,
    AgentProjectionRevision, AgentProjectionStatus, AgentRuntimeError, AgentRuntimeGeneration,
    AgentTitle, AgentTurnId, AgentTurnRequest,
};
use runtime_domain::session::RuntimeTarget;
use runtime_domain::session::{ConversationTurnRequest, TranscriptReplayItem};

use super::agent::{
    AgentChildRuntimeLeases, AgentChildRuntimeStaticGrants, AgentRuntimeActivationGrants,
    AgentRuntimeActivity, AgentRuntimePort, AgentSessionCapability, ChildAgentFactory,
    SpawnAgentsFailure, SpawnAgentsRequest,
};
use super::agent_capability_context::{
    AgentCapabilityContext, AgentChildCapabilityGrants, AgentContextOwner,
    AgentRootCapabilityGrants, AgentScopedEffectKind,
};
use super::context::{CapabilityLease, PromptAssemblyCapability, ToolCatalogCapability};
use super::effect_scope::EffectScope;

#[allow(dead_code)]
const MAX_ACTIVE_CHILD_AGENTS: usize = 32;

/// Child adapter 由 context effect 和 registry record 共同引用，但 runtime owner 始终唯一。
/// effect inverse 成功后取走 boxed adapter；失败则原位保留，供同一 owner 重试。
#[derive(Clone)]
struct ChildRuntimeHandle {
    runtime: Arc<Mutex<Option<Box<dyn AgentRuntimePort>>>>,
}

struct PendingChildCleanup {
    context: AgentCapabilityContext,
    runtime: ChildRuntimeHandle,
}

impl ChildRuntimeHandle {
    fn new(runtime: Box<dyn AgentRuntimePort>) -> Self {
        Self {
            runtime: Arc::new(Mutex::new(Some(runtime))),
        }
    }

    fn register(
        context: &AgentCapabilityContext,
        runtime: Box<dyn AgentRuntimePort>,
    ) -> Result<Self, (AgentRuntimeError, Self)> {
        let handle = Self::new(runtime);
        let inverse = handle.clone();
        let registration =
            context.register_effect(&context.token(), AgentScopedEffectKind::Worker, move || {
                Ok::<_, ()>(move || {
                    inverse
                        .shutdown()
                        .map_err(|_| "Agent child runtime cleanup is pending".to_string())
                })
            });
        if registration.is_err() {
            return Err((
                AgentRuntimeError::Shutdown(
                    "Agent child runtime ownership is unavailable".to_string(),
                ),
                handle,
            ));
        }
        Ok(handle)
    }

    fn dispatch(&self, command: AgentCommand) -> Result<AgentCommandReceipt, AgentRuntimeError> {
        let mut runtime = self.lock();
        runtime
            .as_mut()
            .ok_or(AgentRuntimeError::Disposed)?
            .dispatch(command)
    }

    fn drain_events(&self) -> Vec<AgentEvent> {
        self.lock()
            .as_mut()
            .map_or_else(Vec::new, |runtime| runtime.drain_events())
    }

    fn shutdown(&self) -> Result<(), AgentRuntimeError> {
        let mut runtime = self.lock();
        let Some(adapter) = runtime.as_mut() else {
            return Ok(());
        };
        adapter.shutdown()?;
        runtime.take();
        Ok(())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Box<dyn AgentRuntimePort>>> {
        self.runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// 一个 child 的 runtime、context 与 projection 必须由同一个 record 持有。
///
/// 该结构不实现 `Clone`，避免把 adapter 或 cleanup owner 隐式复制到 registry 之外。
#[allow(dead_code)]
struct ChildAgentRecord {
    parent_agent_id: AgentId,
    parent_turn_id: Option<AgentTurnId>,
    turn_id: AgentTurnId,
    generation: AgentRuntimeGeneration,
    title: AgentTitle,
    launch_group_id: Option<AgentLaunchGroupId>,
    launch_objective: Option<AgentObjectiveSummary>,
    target: Option<RuntimeTarget>,
    context: Option<AgentCapabilityContext>,
    runtime: ChildRuntimeHandle,
    status: AgentProjectionStatus,
    latest_activity: AgentActivitySummary,
    latest_committed_answer: Option<String>,
    terminal_outcome_seen: bool,
    outcome_persisted: bool,
    pending_outcome: Option<runtime_domain::agent::AgentOutcomeSnapshot>,
    terminal_status: Option<AgentProjectionStatus>,
    pending_terminal_event: Option<AgentEvent>,
    started_at_ms: i64,
    tool_uses: usize,
    token_usage: usize,
}

#[allow(dead_code)]
impl ChildAgentRecord {
    fn new(
        parent_agent_id: AgentId,
        turn_id: AgentTurnId,
        generation: AgentRuntimeGeneration,
        title: AgentTitle,
        target: Option<RuntimeTarget>,
        context: AgentCapabilityContext,
        runtime: ChildRuntimeHandle,
    ) -> Self {
        Self {
            parent_agent_id,
            parent_turn_id: None,
            turn_id,
            generation,
            title,
            launch_group_id: None,
            launch_objective: None,
            target,
            context: Some(context),
            runtime,
            status: AgentProjectionStatus::Pending,
            latest_activity: AgentActivitySummary::Preparing,
            latest_committed_answer: None,
            terminal_outcome_seen: false,
            outcome_persisted: false,
            pending_outcome: None,
            terminal_status: None,
            pending_terminal_event: None,
            started_at_ms: 0,
            tool_uses: 0,
            token_usage: 0,
        }
    }

    fn is_terminal(&self) -> bool {
        self.terminal_status.is_some()
    }

    fn admission_open(&self) -> bool {
        matches!(
            self.status,
            AgentProjectionStatus::Pending
                | AgentProjectionStatus::Working
                | AgentProjectionStatus::WaitingPermission
        )
    }
}

/// `AgentOrchestrator` 是 active Agent plugin generation 与全部 logical Agent 的唯一 owner。
///
/// 当前 slice 先把既有 main adapter 收进该边界；child records、factory 与 scope tree 后续只会
/// 在此 module 内增加，不再向 coordinator 或 `RuntimeComponents` 扩散第二份 registry。
pub(super) struct AgentOrchestrator {
    generation: AgentRuntimeGeneration,
    main_runtime: Box<dyn AgentRuntimePort>,
    child_factory: Option<ChildAgentFactory>,
    root_context: Option<AgentCapabilityContext>,
    child_leases: Option<AgentChildRuntimeLeases>,
    child_static_grants: Option<AgentChildRuntimeStaticGrants>,
    is_main_quiescent: bool,
    children: BTreeMap<AgentId, ChildAgentRecord>,
    children_by_parent: BTreeMap<AgentId, BTreeSet<AgentId>>,
    pending_context_cleanups: Vec<AgentCapabilityContext>,
    pending_child_cleanups: Vec<PendingChildCleanup>,
    session_port: Option<Arc<dyn SessionPort>>,
    next_agent_id: u64,
    next_observation_id: u64,
    projection_revision: u64,
    next_launch_group_id: u64,
    group_waiters: BTreeMap<AgentLaunchGroupId, GroupWaiter>,
    main_turn_id: Option<AgentTurnId>,
}

struct GroupWaiter {
    parent_agent_id: AgentId,
    child_ids: Vec<AgentId>,
    response: oneshot::Sender<Result<AgentGroupCompletion, SpawnAgentsFailure>>,
}

type StagedChild = (AgentId, ChildAgentRecord, AgentTurnRequest);

impl AgentOrchestrator {
    pub(super) fn new(
        main_runtime: Box<dyn AgentRuntimePort>,
        child_factory: Option<ChildAgentFactory>,
        child_static_grants: Option<AgentChildRuntimeStaticGrants>,
    ) -> Self {
        Self {
            generation: AgentRuntimeGeneration::new(1),
            main_runtime,
            child_factory,
            root_context: None,
            child_leases: None,
            child_static_grants,
            is_main_quiescent: true,
            children: BTreeMap::new(),
            children_by_parent: BTreeMap::new(),
            pending_context_cleanups: Vec::new(),
            pending_child_cleanups: Vec::new(),
            session_port: None,
            next_agent_id: AgentId::MAIN.get().saturating_add(1),
            next_observation_id: 1,
            projection_revision: 0,
            next_launch_group_id: 1,
            group_waiters: BTreeMap::new(),
            main_turn_id: None,
        }
    }

    pub(super) fn bind_session_port(&mut self, session_port: Option<Arc<dyn SessionPort>>) {
        self.session_port = session_port;
    }

    pub(super) fn rebind_main_tools(
        &mut self,
        registry: tool_runtime::ToolExecutorRegistry,
    ) -> Result<(), String> {
        let Some(root_context) = self.root_context.as_ref() else {
            return Ok(());
        };
        let tools = root_context
            .tools_with_registry(registry)
            .map_err(|error| error.to_string())?;
        self.main_runtime.bind_tools(tools)
    }

    #[cfg(test)]
    pub(super) fn generation(&self) -> AgentRuntimeGeneration {
        self.generation
    }

    /// 只在 committed plugin replacement boundary 安装已构造的 fresh main generation。
    pub(super) fn replace_main(
        &mut self,
        main_runtime: Box<dyn AgentRuntimePort>,
        child_factory: Option<ChildAgentFactory>,
        child_static_grants: Option<AgentChildRuntimeStaticGrants>,
    ) -> Result<(), AgentRuntimeError> {
        let next_generation = self.prepare_main_replacement_generation()?;
        self.commit_prepared_main_replacement(
            main_runtime,
            child_factory,
            child_static_grants,
            next_generation,
        );
        Ok(())
    }

    pub(super) fn prepare_main_replacement_generation(
        &self,
    ) -> Result<AgentRuntimeGeneration, AgentRuntimeError> {
        self.validate_replace_main()?;
        self.generation
            .get()
            .checked_add(1)
            .map(AgentRuntimeGeneration::new)
            .ok_or_else(|| {
                AgentRuntimeError::CommandRejected(
                    "Agent runtime generation identity exhausted".to_string(),
                )
            })
    }

    pub(super) fn commit_prepared_main_replacement(
        &mut self,
        main_runtime: Box<dyn AgentRuntimePort>,
        child_factory: Option<ChildAgentFactory>,
        child_static_grants: Option<AgentChildRuntimeStaticGrants>,
        next_generation: AgentRuntimeGeneration,
    ) {
        debug_assert!(self.validate_replace_main().is_ok());
        debug_assert_eq!(next_generation.get(), self.generation.get() + 1);
        let mut main_runtime = main_runtime;
        main_runtime.bind_runtime_generation(next_generation.get());
        self.main_runtime = main_runtime;
        self.child_factory = child_factory;
        self.child_static_grants = child_static_grants;
        self.is_main_quiescent = true;
        self.root_context = None;
        self.child_leases = None;
        self.fail_group_waiters(SpawnAgentsFailure::Unavailable);
        self.children.clear();
        self.children_by_parent.clear();
        self.projection_revision = 0;
        self.generation = next_generation;
        self.main_turn_id = None;
    }

    fn validate_replace_main(&self) -> Result<(), AgentRuntimeError> {
        if self
            .children
            .values()
            .any(|record| record.context.is_some())
        {
            return Err(AgentRuntimeError::Shutdown(
                "Agent child runtime cleanup is pending".to_string(),
            ));
        }
        if self.root_context.is_some() || self.child_leases.is_some() {
            return Err(AgentRuntimeError::Shutdown(
                "Agent child authority cleanup is pending".to_string(),
            ));
        }
        if !self.pending_context_cleanups.is_empty() || !self.pending_child_cleanups.is_empty() {
            return Err(AgentRuntimeError::Shutdown(
                "Agent child capability cleanup is pending".to_string(),
            ));
        }
        if !self.group_waiters.is_empty() {
            return Err(AgentRuntimeError::Shutdown(
                "Agent launch completion is pending".to_string(),
            ));
        }
        if !self.is_main_quiescent {
            return Err(AgentRuntimeError::Shutdown(
                "Agent main runtime cleanup is pending".to_string(),
            ));
        }
        Ok(())
    }

    /// Main product command 必须显式携带 `AgentId::MAIN`，避免 coordinator 的隐式 current worker。
    pub(super) fn dispatch_main(
        &mut self,
        command: AgentCommand,
    ) -> Result<AgentCommandReceipt, AgentRuntimeError> {
        if command.agent_id() != AgentId::MAIN {
            return Err(AgentRuntimeError::UnknownAgent);
        }
        let submitted_turn_id = match &command {
            AgentCommand::SubmitTurn { turn_id, .. } => Some(*turn_id),
            _ => None,
        };
        let receipt = self.main_runtime.dispatch(command)?;
        if let (Some(submitted_turn_id), AgentCommandReceipt::TurnStarted { turn_id, .. }) =
            (submitted_turn_id, &receipt)
            && *turn_id == submitted_turn_id
        {
            self.main_turn_id = Some(submitted_turn_id);
        }
        Ok(receipt)
    }

    /// Main adapter facts 在丢失 identity 前先经过 fail-closed ownership validation。
    pub(super) fn drain_main_events(&mut self) -> Vec<AgentEvent> {
        self.main_runtime
            .drain_events()
            .into_iter()
            .filter(|event| {
                let is_main = event.agent_id == AgentId::MAIN;
                debug_assert!(is_main, "main adapter emitted a non-main Agent fact");
                is_main
            })
            .collect()
    }

    /// 分发一个已注册 child 的命令；main command 必须继续经过 `dispatch_main`。
    #[allow(dead_code)]
    pub(super) fn dispatch_child(
        &mut self,
        command: AgentCommand,
    ) -> Result<AgentCommandReceipt, AgentRuntimeError> {
        self.reconcile_revoked_children();
        let agent_id = command.agent_id();
        let record = self
            .children
            .get_mut(&agent_id)
            .ok_or(AgentRuntimeError::UnknownAgent)?;
        if record.generation != self.generation || !record.admission_open() {
            return Err(AgentRuntimeError::UnknownAgent);
        }
        if record
            .context
            .as_ref()
            .is_none_or(|context| !context.is_current())
        {
            record.status = AgentProjectionStatus::CleanupBlocked;
            return Err(AgentRuntimeError::Disposed);
        }
        record.runtime.dispatch(command)
    }

    /// 取出 child adapter 的 facts，并在 identity/turn/terminal 边界更新 authoritative record。
    ///
    /// 未知 Agent、错误 turn、旧 generation 或 terminal 后的 late event 都被丢弃；它们不能
    /// 进入 main mapper，也不能改变任何 child projection。
    #[allow(dead_code)]
    pub(super) fn drain_child_events(&mut self) -> Vec<AgentEvent> {
        self.reconcile_revoked_children();
        let child_ids = self.children.keys().copied().collect::<Vec<_>>();
        let mut accepted = Vec::new();
        for agent_id in child_ids {
            let Some(record) = self.children.get_mut(&agent_id) else {
                continue;
            };
            let events = record.runtime.drain_events();
            for event in events {
                if event.agent_id != agent_id
                    || event.turn_id != record.turn_id
                    || record.generation != self.generation
                    || record.terminal_outcome_seen
                    || !record.admission_open()
                    || record
                        .target
                        .as_ref()
                        .is_some_and(|target| target != &event.target)
                    || record
                        .context
                        .as_ref()
                        .is_none_or(|context| !context.is_current())
                {
                    continue;
                }
                let is_terminal = event.kind.is_terminal();
                apply_child_projection(record, &event.kind);
                if is_terminal {
                    record.terminal_outcome_seen = true;
                    record.pending_terminal_event = Some(safe_child_terminal_event(event));
                    freeze_pending_outcome(agent_id, record);
                } else {
                    accepted.push(event);
                }
                self.projection_revision = self.projection_revision.saturating_add(1);
            }
        }
        self.release_terminal_authority();
        self.persist_terminal_outcomes();
        for record in self.children.values_mut() {
            if record.context.is_none()
                && record.outcome_persisted
                && let Some(event) = record.pending_terminal_event.take()
            {
                accepted.push(event);
            }
        }
        self.try_complete_group_waiters();
        accepted
    }

    pub(super) fn activate_main(
        &mut self,
        grants: AgentRuntimeActivationGrants,
        mut root_context: Option<AgentCapabilityContext>,
        child_leases: Option<AgentChildRuntimeLeases>,
    ) -> Result<(), String> {
        let expects_child_authority = self.child_factory.is_some();
        if expects_child_authority
            != (root_context.is_some()
                && child_leases.is_some()
                && self.child_static_grants.is_some())
        {
            return Err("Agent child authority grants are incomplete".to_string());
        }
        if self.root_context.is_some() || self.child_leases.is_some() {
            return Err("Agent child authority cleanup is pending".to_string());
        }
        if !self.pending_context_cleanups.is_empty() {
            return Err("Agent child capability cleanup is pending".to_string());
        }
        if let (Some(context), Some(leases)) = (root_context.as_ref(), child_leases.as_ref()) {
            for guard in leases.generation_guards() {
                if let Err(error) = context.retain_generation(guard) {
                    let context = root_context
                        .take()
                        .expect("root context must remain available during activation");
                    self.rollback_staged_context(context);
                    return Err(error.to_string());
                }
            }
        }
        self.main_runtime
            .bind_runtime_generation(self.generation.get());
        if let Err(error) = self.main_runtime.activate(grants) {
            if let Some(context) = root_context.take() {
                self.rollback_staged_context(context);
            }
            return Err(error);
        }
        self.is_main_quiescent = false;
        self.root_context = root_context;
        self.child_leases = child_leases;
        Ok(())
    }

    pub(super) fn has_child_factory(&self) -> bool {
        self.child_factory.is_some()
    }

    fn rollback_staged_context(&mut self, context: AgentCapabilityContext) -> bool {
        context.begin_disposal();
        let report = context.dispose();
        if report.is_success() {
            true
        } else {
            self.pending_context_cleanups.push(context);
            false
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn rollback_staged_child(
        &mut self,
        agent_id: AgentId,
        parent_agent_id: AgentId,
        turn_id: AgentTurnId,
        title: AgentTitle,
        target: Option<RuntimeTarget>,
        context: AgentCapabilityContext,
        runtime: ChildRuntimeHandle,
        failure: AgentRuntimeError,
    ) -> AgentRuntimeError {
        context.begin_disposal();
        let runtime_result = runtime.shutdown();
        let context_result = context.dispose();
        if runtime_result.is_ok() && context_result.is_success() {
            return failure;
        }

        let mut record = ChildAgentRecord::new(
            parent_agent_id,
            turn_id,
            self.generation,
            title,
            target,
            context,
            runtime,
        );
        record.status = AgentProjectionStatus::CleanupBlocked;
        self.insert_child_record(agent_id, record);
        AgentRuntimeError::Shutdown("Agent child cleanup is pending".to_string())
    }

    fn retry_pending_context_cleanups(&mut self) -> Result<(), AgentRuntimeError> {
        if self.pending_context_cleanups.is_empty() {
            return Ok(());
        }
        let pending = std::mem::take(&mut self.pending_context_cleanups);
        for context in pending {
            if !self.rollback_staged_context(context) {
                return Err(AgentRuntimeError::Shutdown(
                    "Agent child capability cleanup is pending".to_string(),
                ));
            }
        }
        Ok(())
    }

    fn retry_pending_child_cleanups(&mut self) -> Result<(), AgentRuntimeError> {
        if self.pending_child_cleanups.is_empty() {
            return Ok(());
        }
        let pending = std::mem::take(&mut self.pending_child_cleanups);
        let mut retained = Vec::new();
        for cleanup in pending {
            cleanup.context.begin_disposal();
            let runtime_ok = cleanup.runtime.shutdown().is_ok();
            let context_ok = cleanup.context.dispose().is_success();
            if !(runtime_ok && context_ok) {
                retained.push(cleanup);
            }
        }
        self.pending_child_cleanups = retained;
        if self.pending_child_cleanups.is_empty() {
            Ok(())
        } else {
            Err(AgentRuntimeError::Shutdown(
                "Agent child cleanup is pending".to_string(),
            ))
        }
    }

    /// 为 immediate parent 创建并注册一个 child record。
    ///
    /// 这是后续 typed spawn provider 的唯一 runtime seam。方法先完成身份分配、scoped
    /// context 与 adapter construction，再提交 record；任何失败都不会留下 registry row。
    fn stage_child_record(
        &mut self,
        parent_agent_id: AgentId,
        turn_id: AgentTurnId,
        title: AgentTitle,
        grants: AgentChildCapabilityGrants,
        request: &AgentTurnRequest,
    ) -> Result<(AgentId, ChildAgentRecord), AgentRuntimeError> {
        self.retry_pending_context_cleanups()?;
        self.retry_pending_child_cleanups()?;
        if self.active_child_count() >= MAX_ACTIVE_CHILD_AGENTS {
            return Err(AgentRuntimeError::CommandRejected(
                "Active child Agent limit reached".to_string(),
            ));
        }
        let parent_context = self.parent_context(parent_agent_id)?;
        let agent_id = self.allocate_agent_id()?;
        let owner = AgentContextOwner::try_new(format!("child-agent-{}", agent_id.get())).map_err(
            |_| AgentRuntimeError::CommandRejected("Child owner is unavailable".to_string()),
        )?;
        let child_context = parent_context.child(owner, grants).map_err(|_| {
            AgentRuntimeError::CommandRejected(
                "Child capability context is unavailable".to_string(),
            )
        })?;
        let target = request.target();
        let runtime = match self.construct_child(agent_id, child_context.clone()) {
            Ok(runtime) => runtime,
            Err(_) => {
                if !self.rollback_staged_context(child_context) {
                    return Err(AgentRuntimeError::Shutdown(
                        "Agent child capability cleanup is pending".to_string(),
                    ));
                }
                return Err(AgentRuntimeError::CommandRejected(
                    "Child Agent construction failed".to_string(),
                ));
            }
        };
        let Some(leases) = self.child_leases.as_ref() else {
            if !self.rollback_staged_context(child_context) {
                return Err(AgentRuntimeError::Shutdown(
                    "Agent child capability cleanup is pending".to_string(),
                ));
            }
            return Err(AgentRuntimeError::CommandRejected(
                "Agent child capability leases are unavailable".to_string(),
            ));
        };
        let mut runtime = runtime;
        if runtime.activate(leases.activation_grants()).is_err() {
            let staged = PendingChildCleanup {
                context: child_context,
                runtime: ChildRuntimeHandle::new(runtime),
            };
            staged.context.begin_disposal();
            let runtime_ok = staged.runtime.shutdown().is_ok();
            let context_ok = staged.context.dispose().is_success();
            if !(runtime_ok && context_ok) {
                self.pending_child_cleanups.push(staged);
                return Err(AgentRuntimeError::Shutdown(
                    "Agent child cleanup is pending".to_string(),
                ));
            }
            return Err(AgentRuntimeError::CommandRejected(
                "Child Agent activation failed".to_string(),
            ));
        }
        let runtime = match ChildRuntimeHandle::register(&child_context, runtime) {
            Ok(runtime) => runtime,
            Err((error, handle)) => {
                let staged = PendingChildCleanup {
                    context: child_context,
                    runtime: handle,
                };
                staged.context.begin_disposal();
                let runtime_ok = staged.runtime.shutdown().is_ok();
                let context_ok = staged.context.dispose().is_success();
                if !(runtime_ok && context_ok) {
                    self.pending_child_cleanups.push(staged);
                    return Err(AgentRuntimeError::Shutdown(
                        "Agent child cleanup is pending".to_string(),
                    ));
                }
                return Err(error);
            }
        };
        let record = ChildAgentRecord::new(
            parent_agent_id,
            turn_id,
            self.generation,
            title,
            Some(target),
            child_context,
            runtime,
        );
        Ok((agent_id, record))
    }

    #[allow(dead_code)]
    pub(super) fn spawn_child(
        &mut self,
        parent_agent_id: AgentId,
        turn_id: AgentTurnId,
        title: AgentTitle,
        grants: AgentChildCapabilityGrants,
        request: AgentTurnRequest,
    ) -> Result<(AgentId, AgentCommandReceipt), AgentRuntimeError> {
        let (agent_id, mut record) =
            self.stage_child_record(parent_agent_id, turn_id, title, grants, &request)?;
        let receipt = match record.runtime.dispatch(AgentCommand::SubmitTurn {
            agent_id,
            turn_id,
            request: Box::new(request),
        }) {
            Ok(receipt) => receipt,
            Err(error) => {
                let context = record
                    .context
                    .take()
                    .expect("staged child record must retain its context");
                return Err(self.rollback_staged_child(
                    agent_id,
                    parent_agent_id,
                    turn_id,
                    record.title,
                    record.target,
                    context,
                    record.runtime,
                    error,
                ));
            }
        };
        record.started_at_ms = runtime_domain::time::unix_timestamp_ms().unwrap_or(0);
        self.insert_child_record(agent_id, record);
        Ok((agent_id, receipt))
    }

    /// 处理 host-owned `spawn_agents` request；tool bridge 不直接持有 lifecycle authority。
    pub(super) fn handle_spawn_agents_request(&mut self, request: SpawnAgentsRequest) {
        let SpawnAgentsRequest {
            identity,
            batch,
            response,
        } = request;
        match self.launch_batch(identity, batch) {
            Ok((receipt, child_ids)) => {
                self.group_waiters.insert(
                    receipt.group_id,
                    GroupWaiter {
                        parent_agent_id: receipt.parent_agent_id,
                        child_ids,
                        response,
                    },
                );
                self.try_complete_group_waiters();
            }
            Err(error) => {
                let _ = response.send(Err(safe_launch_error(&error)));
            }
        }
    }

    fn launch_batch(
        &mut self,
        identity: tool_runtime::ToolInvocationIdentity,
        batch: AgentLaunchBatch,
    ) -> Result<(AgentLaunchReceipt, Vec<AgentId>), AgentRuntimeError> {
        let parent_agent_id = AgentId::new(identity.agent_id());
        if identity.runtime_generation() != self.generation.get()
            || parent_agent_id.get() == 0
            || identity.context_epoch() == 0
        {
            return Err(AgentRuntimeError::UnknownAgent);
        }
        let parent_turn_id = AgentTurnId::new(identity.turn_id());
        if !self.parent_turn_matches(parent_agent_id, parent_turn_id) {
            return Err(AgentRuntimeError::UnknownAgent);
        }
        let parent_context = self.parent_context(parent_agent_id)?;
        if parent_context.epoch() != identity.context_epoch() {
            return Err(AgentRuntimeError::UnknownAgent);
        }
        if self.child_factory.is_none() || self.child_static_grants.is_none() {
            return Err(AgentRuntimeError::CommandRejected(
                "Agent plugin does not provide child Agent capability".to_string(),
            ));
        }
        if self
            .active_child_count()
            .saturating_add(batch.requests().len())
            > MAX_ACTIVE_CHILD_AGENTS
        {
            return Err(AgentRuntimeError::CommandRejected(
                "Active child Agent limit reached".to_string(),
            ));
        }
        let target = self
            .parent_target(parent_agent_id)
            .ok_or(AgentRuntimeError::UnknownAgent)?;
        let group_id = self.allocate_launch_group_id()?;
        let mut staged: Vec<StagedChild> = Vec::with_capacity(batch.requests().len());
        for request in batch.into_requests() {
            let child_id = AgentId::new(self.next_agent_id);
            let child_turn_id = AgentTurnId::new(child_id.get());
            let turn_request = child_turn_request(&target, &request);
            let objective_summary = AgentObjectiveSummary::from_objective(request.objective())
                .map_err(|_| {
                    AgentRuntimeError::CommandRejected(
                        "Agent objective summary is unavailable".to_string(),
                    )
                })?;
            let (child_id, mut record) = match self.stage_child_record(
                parent_agent_id,
                child_turn_id,
                request.title().clone(),
                AgentChildCapabilityGrants::empty()
                    .inherit_tools()
                    .inherit_prompt(),
                &turn_request,
            ) {
                Ok(staged) => staged,
                Err(error) => {
                    self.cleanup_unpublished_children(staged);
                    return Err(error);
                }
            };
            record.launch_group_id = Some(group_id);
            record.parent_turn_id = Some(parent_turn_id);
            record.launch_objective = Some(objective_summary);
            staged.push((child_id, record, turn_request));
        }

        let children = staged
            .iter()
            .map(|(child_id, record, _)| AgentLaunchChildSnapshot {
                agent_id: *child_id,
                title: record.title.clone(),
                objective: record
                    .launch_objective
                    .clone()
                    .expect("staged objective must be available"),
            })
            .collect::<Vec<_>>();
        let snapshot = runtime_domain::agent::AgentLaunchSnapshot {
            group_id,
            parent_agent_id,
            parent_turn_id,
            children: children.clone(),
            occurred_at_ms: runtime_domain::time::unix_timestamp_ms().unwrap_or(0),
        };
        if let Err(error) = self.append_replay_fact(TranscriptReplayItem::AgentLaunch(snapshot)) {
            self.cleanup_unpublished_children(staged);
            return Err(error);
        }

        let mut dispatches = Vec::with_capacity(staged.len());
        for (child_id, record, request) in staged {
            let turn_id = record.turn_id;
            dispatches.push((child_id, turn_id, request));
            let mut record = record;
            record.started_at_ms = runtime_domain::time::unix_timestamp_ms().unwrap_or(0);
            self.insert_child_record(child_id, record);
        }
        for (child_id, turn_id, request) in dispatches {
            let Some(record) = self.children.get_mut(&child_id) else {
                continue;
            };
            if let Err(_error) = record.runtime.dispatch(AgentCommand::SubmitTurn {
                agent_id: child_id,
                turn_id,
                request: Box::new(request),
            }) {
                apply_child_projection(
                    record,
                    &AgentEventKind::TurnFailed {
                        message: "Child Agent failed to start".to_string(),
                    },
                );
                record.terminal_outcome_seen = true;
                record.pending_terminal_event = Some(AgentEvent {
                    agent_id: child_id,
                    turn_id,
                    target: record.target.clone().expect("child target must exist"),
                    kind: AgentEventKind::TurnFailed {
                        message: "Child Agent failed to start".to_string(),
                    },
                });
                freeze_pending_outcome(child_id, record);
            }
        }

        let child_ids = children.iter().map(|child| child.agent_id).collect();
        Ok((
            AgentLaunchReceipt {
                group_id,
                parent_agent_id,
                children,
            },
            child_ids,
        ))
    }

    fn parent_turn_matches(&self, parent_agent_id: AgentId, turn_id: AgentTurnId) -> bool {
        if parent_agent_id == AgentId::MAIN {
            return self.main_turn_id == Some(turn_id)
                && self.main_runtime.current_target().is_some();
        }
        self.children
            .get(&parent_agent_id)
            .is_some_and(|record| record.turn_id == turn_id && record.admission_open())
    }

    fn cleanup_unpublished_children(
        &mut self,
        staged: Vec<(AgentId, ChildAgentRecord, AgentTurnRequest)>,
    ) {
        for (_, mut record, _) in staged.into_iter().rev() {
            let Some(context) = record.context.take() else {
                continue;
            };
            context.begin_disposal();
            let runtime = record.runtime;
            let runtime_ok = runtime.shutdown().is_ok();
            let context_ok = context.dispose().is_success();
            if !(runtime_ok && context_ok) {
                self.pending_child_cleanups
                    .push(PendingChildCleanup { context, runtime });
            }
        }
    }

    fn persist_terminal_outcomes(&mut self) {
        let outcome_ids = self
            .children
            .iter()
            .filter_map(|(agent_id, record)| {
                (record.context.is_none()
                    && record.terminal_status.is_some()
                    && !record.outcome_persisted)
                    .then_some(*agent_id)
            })
            .collect::<Vec<_>>();
        for agent_id in outcome_ids {
            let Some(record) = self.children.get(&agent_id) else {
                continue;
            };
            let Some(snapshot) = record.pending_outcome.clone() else {
                continue;
            };
            if self
                .append_replay_fact(TranscriptReplayItem::AgentOutcome(snapshot))
                .is_ok()
                && let Some(record) = self.children.get_mut(&agent_id)
            {
                record.outcome_persisted = true;
                record.pending_outcome = None;
            }
        }
    }

    fn append_replay_fact(&self, item: TranscriptReplayItem) -> Result<(), AgentRuntimeError> {
        let Some(session_port) = self.session_port.as_ref() else {
            return Ok(());
        };
        let Some(session_id) = self
            .main_runtime
            .session()
            .and_then(|session| session.snapshot().session_id)
        else {
            return Ok(());
        };
        let session_port = Arc::clone(session_port);
        run_session_store_future(
            move || async move {
                session_port
                    .append_transcript_replay(&session_id, item)
                    .await
            },
            "Agent replay fact",
        )
        .map_err(|_| {
            AgentRuntimeError::CommandRejected("Agent replay fact unavailable".to_string())
        })?
        .map(|_| ())
        .map_err(|_| {
            AgentRuntimeError::CommandRejected("Agent replay fact unavailable".to_string())
        })
    }

    fn parent_target(&self, parent_agent_id: AgentId) -> Option<RuntimeTarget> {
        if parent_agent_id == AgentId::MAIN {
            self.main_runtime.current_target()
        } else {
            self.children
                .get(&parent_agent_id)
                .and_then(|record| record.target.clone())
        }
    }

    fn allocate_launch_group_id(&mut self) -> Result<AgentLaunchGroupId, AgentRuntimeError> {
        let value = self.next_launch_group_id;
        self.next_launch_group_id = self.next_launch_group_id.checked_add(1).ok_or_else(|| {
            AgentRuntimeError::CommandRejected("Agent launch group identity exhausted".to_string())
        })?;
        Ok(AgentLaunchGroupId::new(value))
    }

    fn try_complete_group_waiters(&mut self) {
        let completed = self
            .group_waiters
            .iter()
            .filter_map(|(group_id, waiter)| {
                let children = waiter
                    .child_ids
                    .iter()
                    .map(|agent_id| self.children.get(agent_id))
                    .collect::<Option<Vec<_>>>()?;
                children
                    .iter()
                    .all(|record| {
                        record.is_terminal() && record.context.is_none() && record.outcome_persisted
                    })
                    .then_some((*group_id, waiter.parent_agent_id, waiter.child_ids.clone()))
            })
            .collect::<Vec<_>>();
        for (group_id, parent_agent_id, child_ids) in completed {
            let Some(waiter) = self.group_waiters.remove(&group_id) else {
                continue;
            };
            let children = child_ids
                .into_iter()
                .filter_map(|agent_id| {
                    self.children
                        .get(&agent_id)
                        .map(|record| AgentChildCompletion {
                            agent_id,
                            title: record.title.clone(),
                            outcome: outcome_for_status(record.terminal_status),
                            summary: safe_outcome_summary(record.terminal_status),
                        })
                })
                .collect::<Vec<_>>();
            let completion = AgentGroupCompletion {
                group_id,
                parent_agent_id,
                children,
                occurred_at_ms: runtime_domain::time::unix_timestamp_ms().unwrap_or(0),
            };
            let _ = waiter.response.send(Ok(completion));
        }
    }

    fn insert_child_record(&mut self, agent_id: AgentId, record: ChildAgentRecord) {
        self.next_agent_id = self.next_agent_id.max(agent_id.get().saturating_add(1));
        self.children_by_parent
            .entry(record.parent_agent_id)
            .or_default()
            .insert(agent_id);
        self.children.insert(agent_id, record);
        self.projection_revision = self.projection_revision.saturating_add(1);
    }

    fn active_child_count(&self) -> usize {
        self.children
            .values()
            .filter(|record| record.context.is_some())
            .count()
    }

    /// 创建一个与当前 generation 绑定的 overview snapshot。
    ///
    /// 该方法只投影 delivery-safe record fields；observer 增量与 disposal lease 属于后续
    /// runtime command slice，不能让 UI 直接读取 registry。
    #[allow(dead_code)]
    pub(super) fn overview_snapshot(&mut self) -> AgentOverviewSnapshot {
        self.reconcile_revoked_children();
        let observation_id = AgentObservationId::new(self.next_observation_id);
        self.next_observation_id = self.next_observation_id.saturating_add(1);
        self.overview_snapshot_for(
            observation_id,
            runtime_domain::time::unix_timestamp_ms().unwrap_or(0),
        )
    }

    #[allow(dead_code)]
    fn overview_snapshot_for(
        &self,
        observation_id: AgentObservationId,
        now_ms: i64,
    ) -> AgentOverviewSnapshot {
        let rows = self
            .children
            .iter()
            .map(|(agent_id, record)| AgentOverviewRow {
                agent_id: *agent_id,
                title: record.title.clone(),
                status: record.status,
                latest_activity: record.latest_activity.clone(),
                elapsed_ms: (record.started_at_ms > 0 && now_ms >= record.started_at_ms)
                    .then_some((now_ms - record.started_at_ms) as u64),
                tool_uses: (record.tool_uses > 0).then_some(record.tool_uses),
                token_usage: (record.token_usage > 0).then_some(record.token_usage),
            })
            .collect();
        AgentOverviewSnapshot {
            observation_id,
            generation: self.generation,
            revision: AgentProjectionRevision::new(self.projection_revision),
            rows,
        }
    }

    /// Stop 一个 child subtree；该操作不会影响 parent 或 sibling。
    #[allow(dead_code)]
    pub(super) fn stop_child(&mut self, agent_id: AgentId) -> Result<(), AgentRuntimeError> {
        if !self.children.contains_key(&agent_id) {
            return Err(AgentRuntimeError::UnknownAgent);
        }
        let retain_terminal_projection = self
            .children
            .get(&agent_id)
            .is_some_and(|record| record.launch_group_id.is_some());
        let result = self.dispose_child_ids(self.subtree_ids(agent_id), retain_terminal_projection);
        self.persist_terminal_outcomes();
        result
    }

    /// Session identity 切换时只撤销 runtime-owned child tree，main adapter 由 restore
    /// transaction 继续拥有；cleanup 未收敛时不得安装 fresh session state。
    pub(super) fn dispose_children_for_session_transition(
        &mut self,
    ) -> Result<(), AgentRuntimeError> {
        let result = self.dispose_children();
        self.persist_terminal_outcomes();
        result
    }

    fn subtree_ids(&self, root: AgentId) -> Vec<AgentId> {
        let mut ids = self.descendant_ids_postorder(root);
        ids.push(root);
        ids
    }

    fn descendant_ids_postorder(&self, parent: AgentId) -> Vec<AgentId> {
        fn visit(
            parent: AgentId,
            children_by_parent: &BTreeMap<AgentId, BTreeSet<AgentId>>,
            visited: &mut BTreeSet<AgentId>,
            ids: &mut Vec<AgentId>,
        ) {
            let Some(children) = children_by_parent.get(&parent) else {
                return;
            };
            for child in children {
                if !visited.insert(*child) {
                    continue;
                }
                visit(*child, children_by_parent, visited, ids);
                ids.push(*child);
            }
        }

        let mut ids = Vec::new();
        visit(
            parent,
            &self.children_by_parent,
            &mut BTreeSet::new(),
            &mut ids,
        );
        ids
    }

    #[allow(dead_code)]
    fn parent_context(
        &self,
        parent_agent_id: AgentId,
    ) -> Result<AgentCapabilityContext, AgentRuntimeError> {
        if parent_agent_id == AgentId::MAIN {
            return self
                .root_context
                .as_ref()
                .filter(|context| context.is_current())
                .cloned()
                .ok_or(AgentRuntimeError::UnknownAgent);
        }
        self.children
            .get(&parent_agent_id)
            .filter(|record| {
                record.generation == self.generation
                    && record.admission_open()
                    && record
                        .context
                        .as_ref()
                        .is_some_and(|context| context.is_current())
            })
            .and_then(|record| record.context.clone())
            .ok_or(AgentRuntimeError::UnknownAgent)
    }

    /// Provider/parent scope revoke may run outside the orchestrator call stack. Reconcile such
    /// records before accepting commands, events or snapshots so disposed contexts cannot remain
    /// an admission path for a generic child adapter.
    fn reconcile_revoked_children(&mut self) {
        let revoked = self
            .children
            .iter()
            .filter_map(|(agent_id, record)| {
                record
                    .context
                    .as_ref()
                    .filter(|context| !context.is_current())
                    .and_then(|_| record.terminal_status.is_none().then_some(*agent_id))
            })
            .collect::<Vec<_>>();
        if !revoked.is_empty() {
            let _ = self.dispose_child_ids(revoked, false);
        }
        self.release_terminal_authority();
    }

    #[allow(dead_code)]
    fn allocate_agent_id(&mut self) -> Result<AgentId, AgentRuntimeError> {
        let value = self.next_agent_id;
        self.next_agent_id = self.next_agent_id.checked_add(1).ok_or_else(|| {
            AgentRuntimeError::CommandRejected("Agent identity exhausted".to_string())
        })?;
        Ok(AgentId::new(value))
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(super) fn child_count(&self) -> usize {
        self.children
            .values()
            .filter(|record| {
                record.context.is_some()
                    || matches!(
                        record.terminal_status,
                        Some(AgentProjectionStatus::Completed)
                            | Some(AgentProjectionStatus::Failed)
                            | Some(AgentProjectionStatus::Cancelled)
                    )
            })
            .count()
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(super) fn children_of(&self, parent_agent_id: AgentId) -> Vec<AgentId> {
        self.children_by_parent
            .get(&parent_agent_id)
            .map(|children| children.iter().copied().collect())
            .unwrap_or_default()
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(super) fn child_status(&self, agent_id: AgentId) -> Option<AgentProjectionStatus> {
        self.children.get(&agent_id).map(|record| record.status)
    }

    #[cfg(test)]
    pub(super) fn pending_outcome_for_test(
        &self,
        agent_id: AgentId,
    ) -> Option<runtime_domain::agent::AgentOutcomeSnapshot> {
        self.children
            .get(&agent_id)
            .and_then(|record| record.pending_outcome.clone())
    }

    #[cfg(test)]
    pub(super) fn child_has_authority(&self, agent_id: AgentId) -> bool {
        self.children
            .get(&agent_id)
            .is_some_and(|record| record.context.is_some())
    }

    #[cfg(test)]
    pub(super) fn register_child_for_test(
        &mut self,
        agent_id: AgentId,
        parent_agent_id: AgentId,
        turn_id: AgentTurnId,
        title: AgentTitle,
        context: AgentCapabilityContext,
        runtime: Box<dyn AgentRuntimePort>,
    ) {
        let runtime = match ChildRuntimeHandle::register(&context, runtime) {
            Ok(runtime) => runtime,
            Err((error, _handle)) => panic!("test child runtime should attach: {error:?}"),
        };
        self.insert_child_record(
            agent_id,
            ChildAgentRecord::new(
                parent_agent_id,
                turn_id,
                self.generation,
                title,
                None,
                context,
                runtime,
            ),
        );
    }

    pub(super) fn suspend(&mut self) -> Result<(), AgentRuntimeError> {
        if let Some(context) = &self.root_context {
            context.begin_disposal();
        }
        self.fail_group_waiters(SpawnAgentsFailure::Unavailable);
        let child_result = self.dispose_children();
        let runtime_result = self.main_runtime.suspend();
        if runtime_result.is_ok() {
            self.is_main_quiescent = true;
        }
        let cleanup_succeeded = child_result.is_ok()
            && runtime_result.is_ok()
            && self
                .root_context
                .as_ref()
                .is_none_or(|context| context.dispose().is_success());
        if cleanup_succeeded {
            self.root_context = None;
            self.child_leases = None;
            self.main_turn_id = None;
        }
        match (runtime_result, child_result, cleanup_succeeded) {
            (Err(error), _, _) => Err(error),
            (Ok(()), Ok(()), true) => Ok(()),
            (Ok(()), Err(error), _) => Err(error),
            (Ok(()), Ok(()), false) => Err(AgentRuntimeError::Shutdown(
                "Agent root capability cleanup is pending".to_string(),
            )),
        }
    }

    pub(super) fn shutdown(&mut self) -> Result<(), AgentRuntimeError> {
        if let Some(context) = &self.root_context {
            context.begin_disposal();
        }
        self.fail_group_waiters(SpawnAgentsFailure::Unavailable);
        let child_result = self.dispose_children();
        let runtime_result = self.main_runtime.shutdown();
        if runtime_result.is_ok() {
            self.is_main_quiescent = true;
        }
        let cleanup_succeeded = child_result.is_ok()
            && runtime_result.is_ok()
            && self
                .root_context
                .as_ref()
                .is_none_or(|context| context.dispose().is_success());
        if cleanup_succeeded {
            self.root_context = None;
            self.child_leases = None;
            self.main_turn_id = None;
        }
        match (runtime_result, child_result, cleanup_succeeded) {
            (Err(error), _, _) => Err(error),
            (Ok(()), Ok(()), true) => Ok(()),
            (Ok(()), Err(error), _) => Err(error),
            (Ok(()), Ok(()), false) => Err(AgentRuntimeError::Shutdown(
                "Agent root capability cleanup is pending".to_string(),
            )),
        }
    }

    fn dispose_children(&mut self) -> Result<(), AgentRuntimeError> {
        let mut child_ids = self.descendant_ids_postorder(AgentId::MAIN);
        for agent_id in self.children.keys().copied().collect::<Vec<_>>() {
            if !child_ids.contains(&agent_id) {
                child_ids.push(agent_id);
            }
        }
        self.dispose_child_ids(child_ids, false)
    }

    fn fail_group_waiters(&mut self, failure: SpawnAgentsFailure) {
        for (_, waiter) in std::mem::take(&mut self.group_waiters) {
            let _ = waiter.response.send(Err(failure));
        }
    }

    fn dispose_child_ids(
        &mut self,
        child_ids: Vec<AgentId>,
        retain_terminal_projection: bool,
    ) -> Result<(), AgentRuntimeError> {
        for agent_id in &child_ids {
            if let Some(record) = self.children.get_mut(agent_id) {
                record.status = AgentProjectionStatus::Stopping;
                if record.terminal_status.is_none() {
                    record.terminal_outcome_seen = true;
                    record.terminal_status = Some(AgentProjectionStatus::Cancelled);
                    record.latest_activity = AgentActivitySummary::Idle;
                    freeze_pending_outcome(*agent_id, record);
                    if retain_terminal_projection {
                        record.pending_terminal_event =
                            record.target.clone().map(|target| AgentEvent {
                                agent_id: *agent_id,
                                turn_id: record.turn_id,
                                target,
                                kind: AgentEventKind::TurnInterrupted,
                            });
                    }
                }
                if let Some(context) = &record.context {
                    context.begin_disposal();
                }
            }
        }
        let mut first_error = None;
        let mut ready_to_remove = Vec::new();
        for agent_id in child_ids {
            let has_owned_descendant =
                self.children_by_parent
                    .get(&agent_id)
                    .is_some_and(|children| {
                        children
                            .iter()
                            .any(|child| self.children.contains_key(child))
                    });
            let Some(record) = self.children.get_mut(&agent_id) else {
                continue;
            };
            if has_owned_descendant {
                record.status = AgentProjectionStatus::CleanupBlocked;
                first_error.get_or_insert(AgentRuntimeError::Shutdown(
                    "Agent descendant cleanup is pending".to_string(),
                ));
                continue;
            }
            let runtime_result = record.runtime.shutdown();
            if runtime_result.is_err() {
                record.status = AgentProjectionStatus::CleanupBlocked;
                first_error.get_or_insert(AgentRuntimeError::Shutdown(
                    "Agent child runtime cleanup is pending".to_string(),
                ));
                continue;
            }
            let context_succeeded = record
                .context
                .as_ref()
                .is_none_or(|context| context.dispose().is_success());
            if !context_succeeded {
                record.status = AgentProjectionStatus::CleanupBlocked;
                first_error.get_or_insert(AgentRuntimeError::Shutdown(
                    "Agent child capability cleanup is pending".to_string(),
                ));
                continue;
            }
            let parent_agent_id = record.parent_agent_id;
            record.context = None;
            record.status = record
                .terminal_status
                .unwrap_or(AgentProjectionStatus::Cancelled);
            let _ = record;
            if !retain_terminal_projection {
                ready_to_remove.push(agent_id);
            }
            self.children_by_parent.remove(&agent_id);
            self.projection_revision = self.projection_revision.saturating_add(1);
            if let Some(children) = self.children_by_parent.get_mut(&parent_agent_id) {
                children.remove(&agent_id);
                if children.is_empty() {
                    self.children_by_parent.remove(&parent_agent_id);
                }
            }
        }
        self.persist_terminal_outcomes();
        for agent_id in ready_to_remove {
            if self
                .children
                .get(&agent_id)
                .is_some_and(|record| record.launch_group_id.is_none() || record.outcome_persisted)
            {
                self.children.remove(&agent_id);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    /// Terminal projection 与 runtime authority 分离：row 留在 overview，worker/context 在
    /// descendants 收敛后立即撤销。cleanup 失败时 terminal fact 暂不向 parent 发布。
    fn release_terminal_authority(&mut self) {
        let child_ids = self.descendant_ids_postorder(AgentId::MAIN);
        for agent_id in child_ids {
            let has_owned_descendant =
                self.children_by_parent
                    .get(&agent_id)
                    .is_some_and(|children| {
                        children.iter().any(|child| {
                            self.children
                                .get(child)
                                .is_some_and(|record| record.context.is_some())
                        })
                    });
            let Some(record) = self.children.get_mut(&agent_id) else {
                continue;
            };
            if record.terminal_status.is_none() || record.context.is_none() || has_owned_descendant
            {
                continue;
            }
            let context = record
                .context
                .as_ref()
                .expect("terminal authority check must retain context");
            context.begin_disposal();
            if record.runtime.shutdown().is_err() || !context.dispose().is_success() {
                if record.status != AgentProjectionStatus::CleanupBlocked {
                    self.projection_revision = self.projection_revision.saturating_add(1);
                }
                record.status = AgentProjectionStatus::CleanupBlocked;
                continue;
            }
            record.context = None;
            record.status = record
                .terminal_status
                .expect("terminal cleanup must retain settled projection");
            self.projection_revision = self.projection_revision.saturating_add(1);
        }
    }

    /// 在 Agent component activation boundary 创建唯一 root capability context。
    pub(super) fn build_root_context(
        &self,
        parent_scope: &EffectScope,
        tool_lease: &CapabilityLease<ToolCatalogCapability>,
        prompt_lease: &CapabilityLease<PromptAssemblyCapability>,
        allowed_tool_names: impl IntoIterator<Item = String>,
    ) -> Result<AgentCapabilityContext, String> {
        let owner = AgentContextOwner::try_new("main-agent").map_err(|error| error.to_string())?;
        AgentCapabilityContext::root(
            owner,
            parent_scope,
            AgentRootCapabilityGrants::empty()
                .with_tools(tool_lease, allowed_tool_names)
                .with_prompt(prompt_lease),
        )
        .map_err(|error| error.to_string())
    }

    #[allow(dead_code)]
    pub(super) fn construct_child(
        &self,
        owned_agent_id: AgentId,
        capability_context: AgentCapabilityContext,
    ) -> Result<Box<dyn AgentRuntimePort>, String> {
        let factory = self
            .child_factory
            .as_ref()
            .ok_or_else(|| "Agent plugin does not provide child Agent capability".to_string())?;
        let leases = self
            .child_leases
            .as_ref()
            .ok_or_else(|| "Agent child capability leases are unavailable".to_string())?;
        let static_grants = self
            .child_static_grants
            .as_ref()
            .ok_or_else(|| "Agent child static grants are unavailable".to_string())?;
        let runtime = factory.construct(leases.construction_grants(
            owned_agent_id,
            capability_context,
            static_grants,
        ))?;
        let mut runtime = runtime;
        runtime.bind_runtime_generation(self.generation.get());
        Ok(runtime)
    }

    pub(super) fn activity(&self) -> AgentRuntimeActivity {
        self.main_runtime.activity()
    }

    pub(super) fn main_session(&self) -> Option<&dyn AgentSessionCapability> {
        self.main_runtime.session()
    }

    pub(super) fn main_session_mut(&mut self) -> Option<&mut dyn AgentSessionCapability> {
        self.main_runtime.session_mut()
    }

    #[cfg(test)]
    pub(super) fn main_port(&self) -> &dyn AgentRuntimePort {
        &*self.main_runtime
    }

    #[cfg(test)]
    pub(super) fn root_context(&self) -> Option<AgentCapabilityContext> {
        self.root_context.clone()
    }

    #[cfg(test)]
    pub(super) fn main_port_mut(&mut self) -> &mut dyn AgentRuntimePort {
        &mut *self.main_runtime
    }

    #[cfg(test)]
    pub(super) fn has_pending_work(&self) -> bool {
        self.main_runtime.has_pending_work()
    }
}

fn child_turn_request(
    target: &RuntimeTarget,
    request: &runtime_domain::agent::AgentLaunchRequest,
) -> AgentTurnRequest {
    let RuntimeTarget::Provider(target) = target;
    AgentTurnRequest::from_conversation_request(ConversationTurnRequest::new_user_text(
        target.provider_id.clone(),
        target.model_id.clone(),
        request.objective().as_str(),
    ))
    .with_direct_instructions(request.instructions().clone())
}

fn outcome_for_status(status: Option<AgentProjectionStatus>) -> AgentOutcome {
    match status {
        Some(AgentProjectionStatus::Completed) => AgentOutcome::Completed,
        Some(AgentProjectionStatus::Cancelled) => AgentOutcome::Cancelled,
        _ => AgentOutcome::Failed,
    }
}

fn safe_launch_error(error: &AgentRuntimeError) -> SpawnAgentsFailure {
    match error {
        AgentRuntimeError::Busy => SpawnAgentsFailure::ParentBusy,
        AgentRuntimeError::Disposed => SpawnAgentsFailure::Unavailable,
        AgentRuntimeError::UnknownAgent => SpawnAgentsFailure::ParentUnavailable,
        AgentRuntimeError::CommandRejected(_) => SpawnAgentsFailure::RequestRejected,
        AgentRuntimeError::Shutdown(_) => SpawnAgentsFailure::CleanupPending,
    }
}

fn safe_outcome_summary(status: Option<AgentProjectionStatus>) -> Option<AgentOutcomeSummary> {
    let text = match status {
        Some(AgentProjectionStatus::Completed) => "Child Agent completed",
        Some(AgentProjectionStatus::Cancelled) => "Child Agent cancelled",
        _ => "Child Agent failed",
    };
    AgentOutcomeSummary::new(text).ok()
}

fn freeze_pending_outcome(agent_id: AgentId, record: &mut ChildAgentRecord) {
    if record.pending_outcome.is_some() {
        return;
    }
    let Some(terminal_status) = record.terminal_status else {
        return;
    };
    record.pending_outcome = Some(runtime_domain::agent::AgentOutcomeSnapshot {
        agent_id,
        title: record.title.clone(),
        group_id: record.launch_group_id,
        parent_agent_id: Some(record.parent_agent_id),
        parent_turn_id: record.parent_turn_id,
        outcome: outcome_for_status(Some(terminal_status)),
        occurred_at_ms: runtime_domain::time::unix_timestamp_ms().unwrap_or(0),
        summary: safe_outcome_summary(Some(terminal_status)),
    });
}

fn safe_child_terminal_event(event: AgentEvent) -> AgentEvent {
    let AgentEvent {
        agent_id,
        turn_id,
        target,
        kind,
    } = event;
    let kind = match kind {
        AgentEventKind::TurnFailed { .. } => AgentEventKind::TurnFailed {
            message: "Child Agent failed".to_string(),
        },
        kind => kind,
    };
    AgentEvent {
        agent_id,
        turn_id,
        target,
        kind,
    }
}

fn apply_child_projection(record: &mut ChildAgentRecord, kind: &AgentEventKind) {
    match kind {
        AgentEventKind::Thinking { is_thinking } => {
            record.status = AgentProjectionStatus::Working;
            record.latest_activity = if *is_thinking {
                AgentActivitySummary::Thinking
            } else {
                AgentActivitySummary::Idle
            };
        }
        AgentEventKind::AssistantDelta { .. }
        | AgentEventKind::ReasoningDelta { .. }
        | AgentEventKind::TerminalUpdated { .. }
        | AgentEventKind::SystemMessage { .. }
        | AgentEventKind::PreparationWarning { .. } => {
            record.status = AgentProjectionStatus::Working;
        }
        AgentEventKind::OutputTokenEstimate { total_tokens }
        | AgentEventKind::InputTokenEstimate { total_tokens } => {
            record.status = AgentProjectionStatus::Working;
            record.token_usage = record.token_usage.max(*total_tokens);
        }
        AgentEventKind::Retrying { .. } => {
            record.status = AgentProjectionStatus::Working;
            // Provider messages are control/provider content at this boundary. The overview only
            // receives a fixed safe activity label, never the raw retry diagnostic.
            record.latest_activity = AgentActivitySummary::Retrying {
                summary: "Retrying".to_string(),
            };
        }
        AgentEventKind::ToolActivityStarted { .. } => {
            record.status = AgentProjectionStatus::Working;
            record.tool_uses = record.tool_uses.saturating_add(1);
            record.latest_activity = AgentActivitySummary::UsingTool {
                title: "Using tool".to_string(),
            };
        }
        AgentEventKind::ToolActivityUpdated { .. } => {
            record.status = AgentProjectionStatus::Working;
        }
        AgentEventKind::PermissionRequested { .. } => {
            record.status = AgentProjectionStatus::WaitingPermission;
            record.latest_activity = AgentActivitySummary::WaitingPermission {
                summary: "Waiting for approval".to_string(),
            };
        }
        AgentEventKind::TurnFinished { response, .. } => {
            record.status = AgentProjectionStatus::Completed;
            record.terminal_status = Some(AgentProjectionStatus::Completed);
            record.latest_activity = AgentActivitySummary::Idle;
            record.latest_committed_answer = Some(response.text_content());
        }
        AgentEventKind::TurnFailed { .. } => {
            record.status = AgentProjectionStatus::Failed;
            record.terminal_status = Some(AgentProjectionStatus::Failed);
            record.latest_activity = AgentActivitySummary::Idle;
        }
        AgentEventKind::TurnInterrupted => {
            record.status = AgentProjectionStatus::Cancelled;
            record.terminal_status = Some(AgentProjectionStatus::Cancelled);
            record.latest_activity = AgentActivitySummary::Idle;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use runtime_domain::{
        agent::{
            AgentCommand, AgentCommandReceipt, AgentEvent, AgentEventKind, AgentId, AgentObjective,
            AgentRuntime, AgentRuntimeError, AgentTitle, AgentTurnId, AgentTurnRequest,
        },
        session::{ConversationTurnRequest, RuntimeTarget},
    };

    use super::*;

    #[derive(Default)]
    struct StubMainRuntime {
        events: Vec<AgentEvent>,
    }

    impl AgentRuntime for StubMainRuntime {
        fn dispatch(
            &mut self,
            _command: AgentCommand,
        ) -> Result<AgentCommandReceipt, AgentRuntimeError> {
            Ok(AgentCommandReceipt::Accepted)
        }

        fn drain_events(&mut self) -> Vec<AgentEvent> {
            std::mem::take(&mut self.events)
        }

        fn shutdown(&mut self) -> Result<(), AgentRuntimeError> {
            Ok(())
        }
    }

    impl AgentRuntimePort for StubMainRuntime {
        fn activate(&mut self, _grants: AgentRuntimeActivationGrants) -> Result<(), String> {
            Ok(())
        }

        fn suspend(&mut self) -> Result<(), AgentRuntimeError> {
            Ok(())
        }

        fn activity(&self) -> AgentRuntimeActivity {
            AgentRuntimeActivity::Idle
        }

        fn session(&self) -> Option<&dyn AgentSessionCapability> {
            None
        }

        fn session_mut(&mut self) -> Option<&mut dyn AgentSessionCapability> {
            None
        }

        fn has_pending_work(&self) -> bool {
            false
        }
    }

    #[derive(Default)]
    struct AdmittingThenBusyMainRuntime {
        accepted_target: Option<RuntimeTarget>,
    }

    impl AgentRuntime for AdmittingThenBusyMainRuntime {
        fn dispatch(
            &mut self,
            command: AgentCommand,
        ) -> Result<AgentCommandReceipt, AgentRuntimeError> {
            match command {
                AgentCommand::SubmitTurn {
                    agent_id,
                    turn_id,
                    request,
                } if agent_id == AgentId::MAIN && self.accepted_target.is_none() => {
                    let target = request.target();
                    self.accepted_target = Some(target.clone());
                    Ok(AgentCommandReceipt::TurnStarted {
                        turn_id,
                        target,
                        activity_label: request.activity_label().to_string(),
                    })
                }
                AgentCommand::SubmitTurn { .. } => Err(AgentRuntimeError::Busy),
                _ => Ok(AgentCommandReceipt::Accepted),
            }
        }

        fn drain_events(&mut self) -> Vec<AgentEvent> {
            Vec::new()
        }

        fn shutdown(&mut self) -> Result<(), AgentRuntimeError> {
            Ok(())
        }
    }

    impl AgentRuntimePort for AdmittingThenBusyMainRuntime {
        fn activate(&mut self, _grants: AgentRuntimeActivationGrants) -> Result<(), String> {
            Ok(())
        }

        fn suspend(&mut self) -> Result<(), AgentRuntimeError> {
            Ok(())
        }

        fn activity(&self) -> AgentRuntimeActivity {
            AgentRuntimeActivity::Idle
        }

        fn session(&self) -> Option<&dyn AgentSessionCapability> {
            None
        }

        fn session_mut(&mut self) -> Option<&mut dyn AgentSessionCapability> {
            None
        }

        fn current_target(&self) -> Option<RuntimeTarget> {
            self.accepted_target.clone()
        }

        fn has_pending_work(&self) -> bool {
            self.accepted_target.is_some()
        }
    }

    #[allow(dead_code)]
    struct FailingActivationRuntime;

    impl AgentRuntime for FailingActivationRuntime {
        fn dispatch(
            &mut self,
            _command: AgentCommand,
        ) -> Result<AgentCommandReceipt, AgentRuntimeError> {
            Ok(AgentCommandReceipt::Accepted)
        }

        fn drain_events(&mut self) -> Vec<AgentEvent> {
            Vec::new()
        }

        fn shutdown(&mut self) -> Result<(), AgentRuntimeError> {
            Ok(())
        }
    }

    impl AgentRuntimePort for FailingActivationRuntime {
        fn activate(&mut self, _grants: AgentRuntimeActivationGrants) -> Result<(), String> {
            Err("closed test activation failure".to_string())
        }

        fn suspend(&mut self) -> Result<(), AgentRuntimeError> {
            Ok(())
        }

        fn activity(&self) -> AgentRuntimeActivity {
            AgentRuntimeActivity::Idle
        }

        fn session(&self) -> Option<&dyn AgentSessionCapability> {
            None
        }

        fn session_mut(&mut self) -> Option<&mut dyn AgentSessionCapability> {
            None
        }

        fn has_pending_work(&self) -> bool {
            false
        }
    }

    struct FailingShutdownRuntime {
        failures_remaining: usize,
        events: Vec<AgentEvent>,
    }

    impl AgentRuntime for FailingShutdownRuntime {
        fn dispatch(
            &mut self,
            _command: AgentCommand,
        ) -> Result<AgentCommandReceipt, AgentRuntimeError> {
            Ok(AgentCommandReceipt::Accepted)
        }

        fn drain_events(&mut self) -> Vec<AgentEvent> {
            std::mem::take(&mut self.events)
        }

        fn shutdown(&mut self) -> Result<(), AgentRuntimeError> {
            if self.failures_remaining > 0 {
                self.failures_remaining -= 1;
                return Err(AgentRuntimeError::Shutdown(
                    "closed test failure".to_string(),
                ));
            }
            Ok(())
        }
    }

    impl AgentRuntimePort for FailingShutdownRuntime {
        fn activate(&mut self, _grants: AgentRuntimeActivationGrants) -> Result<(), String> {
            Ok(())
        }

        fn suspend(&mut self) -> Result<(), AgentRuntimeError> {
            Ok(())
        }

        fn activity(&self) -> AgentRuntimeActivity {
            AgentRuntimeActivity::Idle
        }

        fn session(&self) -> Option<&dyn AgentSessionCapability> {
            None
        }

        fn session_mut(&mut self) -> Option<&mut dyn AgentSessionCapability> {
            None
        }

        fn has_pending_work(&self) -> bool {
            false
        }
    }

    struct RecordingShutdownRuntime {
        label: &'static str,
        context: AgentCapabilityContext,
        order: Arc<Mutex<Vec<&'static str>>>,
    }

    impl AgentRuntime for RecordingShutdownRuntime {
        fn dispatch(
            &mut self,
            _command: AgentCommand,
        ) -> Result<AgentCommandReceipt, AgentRuntimeError> {
            Ok(AgentCommandReceipt::Accepted)
        }

        fn drain_events(&mut self) -> Vec<AgentEvent> {
            Vec::new()
        }

        fn shutdown(&mut self) -> Result<(), AgentRuntimeError> {
            assert!(
                !self.context.is_current(),
                "child cancellation must close authority before adapter shutdown"
            );
            self.order
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(self.label);
            Ok(())
        }
    }

    impl AgentRuntimePort for RecordingShutdownRuntime {
        fn activate(&mut self, _grants: AgentRuntimeActivationGrants) -> Result<(), String> {
            Ok(())
        }

        fn suspend(&mut self) -> Result<(), AgentRuntimeError> {
            Ok(())
        }

        fn activity(&self) -> AgentRuntimeActivity {
            AgentRuntimeActivity::Idle
        }

        fn session(&self) -> Option<&dyn AgentSessionCapability> {
            None
        }

        fn session_mut(&mut self) -> Option<&mut dyn AgentSessionCapability> {
            None
        }

        fn has_pending_work(&self) -> bool {
            false
        }
    }

    #[test]
    fn dispatch_main_rejects_non_main_identity_before_adapter_dispatch() {
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);

        let error = orchestrator
            .dispatch_main(AgentCommand::Interrupt {
                agent_id: AgentId::new(2),
                target: None,
            })
            .expect_err("child identity must not reach main adapter");

        assert!(matches!(error, AgentRuntimeError::UnknownAgent));
    }

    #[test]
    fn rejected_main_submit_does_not_replace_the_admitted_parent_turn() {
        let mut orchestrator = AgentOrchestrator::new(
            Box::new(AdmittingThenBusyMainRuntime::default()),
            None,
            None,
        );
        let admitted_turn = AgentTurnId::new(7);
        orchestrator
            .dispatch_main(AgentCommand::SubmitTurn {
                agent_id: AgentId::MAIN,
                turn_id: admitted_turn,
                request: Box::new(AgentTurnRequest::from_conversation_request(
                    ConversationTurnRequest::new_user_text("local", "qwen3", "admitted"),
                )),
            })
            .expect("first turn should be admitted");

        let rejected_turn = AgentTurnId::new(8);
        assert!(matches!(
            orchestrator.dispatch_main(AgentCommand::SubmitTurn {
                agent_id: AgentId::MAIN,
                turn_id: rejected_turn,
                request: Box::new(AgentTurnRequest::from_conversation_request(
                    ConversationTurnRequest::new_user_text("local", "qwen3", "rejected"),
                )),
            }),
            Err(AgentRuntimeError::Busy)
        ));

        assert!(orchestrator.parent_turn_matches(AgentId::MAIN, admitted_turn));
        assert!(!orchestrator.parent_turn_matches(AgentId::MAIN, rejected_turn));
    }

    #[test]
    fn main_facts_keep_identity_until_orchestrator_gate() {
        let event = AgentEvent {
            agent_id: AgentId::MAIN,
            turn_id: AgentTurnId::new(7),
            target: RuntimeTarget::provider("local", "qwen3"),
            kind: AgentEventKind::TurnInterrupted,
        };
        let mut orchestrator = AgentOrchestrator::new(
            Box::new(StubMainRuntime {
                events: vec![event.clone()],
            }),
            None,
            None,
        );

        assert_eq!(orchestrator.drain_main_events(), vec![event]);
    }

    #[test]
    fn committed_main_replacement_advances_generation_once() {
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        let first = orchestrator.generation();

        orchestrator
            .replace_main(Box::new(StubMainRuntime::default()), None, None)
            .expect("clean orchestrator should accept replacement");

        assert_eq!(orchestrator.generation().get(), first.get() + 1);
    }

    #[test]
    fn main_replacement_drops_settled_child_projection() {
        let agent_id = AgentId::new(2);
        let turn_id = AgentTurnId::new(1);
        let target = RuntimeTarget::provider("local", "qwen3");
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        orchestrator.register_child_for_test(
            agent_id,
            AgentId::MAIN,
            turn_id,
            test_title("settled child"),
            test_context("settled-child"),
            Box::new(StubMainRuntime {
                events: vec![AgentEvent {
                    agent_id,
                    turn_id,
                    target,
                    kind: AgentEventKind::TurnFinished {
                        response: runtime_domain::session::ConversationResponse::assistant_text(
                            "answer",
                        ),
                        metrics: None,
                        context_usage: None,
                    },
                }],
            }),
        );

        assert_eq!(orchestrator.drain_child_events().len(), 1);
        assert_eq!(orchestrator.child_count(), 1);

        orchestrator
            .replace_main(Box::new(StubMainRuntime::default()), None, None)
            .expect("settled child projection must not block replacement");

        assert_eq!(orchestrator.child_count(), 0);
        assert!(orchestrator.children_of(AgentId::MAIN).is_empty());
    }

    #[test]
    fn suspending_runtime_releases_pending_group_waiters() {
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        let (response_sender, response_receiver) = oneshot::channel();
        let group_id = AgentLaunchGroupId::new(1);
        orchestrator.group_waiters.insert(
            group_id,
            GroupWaiter {
                parent_agent_id: AgentId::MAIN,
                child_ids: vec![AgentId::new(2)],
                response: response_sender,
            },
        );

        orchestrator
            .suspend()
            .expect("clean suspend should release pending waiters");

        assert!(orchestrator.group_waiters.is_empty());
        assert_eq!(
            response_receiver
                .blocking_recv()
                .expect("waiter should receive a terminal response"),
            Err(SpawnAgentsFailure::Unavailable)
        );
    }

    fn test_context(owner: &str) -> AgentCapabilityContext {
        let parent_scope = Box::leak(Box::new(EffectScope::default()));
        AgentCapabilityContext::root(
            AgentContextOwner::try_new(owner).expect("test owner should be valid"),
            parent_scope,
            AgentRootCapabilityGrants::empty(),
        )
        .expect("test context should be constructible")
    }

    fn test_title(value: &str) -> AgentTitle {
        AgentTitle::resolve(
            &AgentObjective::new(value).expect("test objective should be valid"),
            None,
        )
        .expect("test title should resolve")
    }

    #[test]
    fn child_events_are_identity_gated_and_terminal_is_exactly_once() {
        let agent_id = AgentId::new(2);
        let turn_id = AgentTurnId::new(7);
        let target = RuntimeTarget::provider("local", "qwen3");
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        let context = test_context("child-test");
        orchestrator.register_child_for_test(
            agent_id,
            AgentId::MAIN,
            turn_id,
            test_title("child task"),
            context,
            Box::new(StubMainRuntime {
                events: vec![
                    AgentEvent {
                        agent_id,
                        turn_id,
                        target: target.clone(),
                        kind: AgentEventKind::AssistantDelta {
                            content: "partial".to_string(),
                        },
                    },
                    AgentEvent {
                        agent_id,
                        turn_id,
                        target: target.clone(),
                        kind: AgentEventKind::TurnFinished {
                            response: runtime_domain::session::ConversationResponse::assistant_text(
                                "committed",
                            ),
                            metrics: None,
                            context_usage: None,
                        },
                    },
                    AgentEvent {
                        agent_id,
                        turn_id,
                        target: target.clone(),
                        kind: AgentEventKind::AssistantDelta {
                            content: "late partial".to_string(),
                        },
                    },
                    AgentEvent {
                        agent_id: AgentId::new(99),
                        turn_id,
                        target,
                        kind: AgentEventKind::TurnInterrupted,
                    },
                ],
            }),
        );

        let accepted = orchestrator.drain_child_events();
        assert_eq!(accepted.len(), 2);
        assert_eq!(accepted[0].agent_id, agent_id);
        assert_eq!(accepted[1].agent_id, agent_id);
        assert_eq!(
            orchestrator.child_status(agent_id),
            Some(AgentProjectionStatus::Completed)
        );
        assert_eq!(orchestrator.child_count(), 1);
        assert_eq!(
            orchestrator.active_child_count(),
            0,
            "settled projections must not consume the live child resource limit"
        );
        assert!(matches!(
            orchestrator.dispatch_child(AgentCommand::Interrupt {
                agent_id: AgentId::MAIN,
                target: None,
            }),
            Err(AgentRuntimeError::UnknownAgent)
        ));
        let overview = orchestrator.overview_snapshot();
        assert_eq!(overview.generation, orchestrator.generation());
        assert_eq!(overview.rows.len(), 1);
        assert_eq!(overview.rows[0].agent_id, agent_id);
        assert_eq!(overview.rows[0].status, AgentProjectionStatus::Completed);
    }

    #[test]
    fn child_failure_event_is_redacted_before_delivery() {
        let agent_id = AgentId::new(2);
        let turn_id = AgentTurnId::new(8);
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        orchestrator.register_child_for_test(
            agent_id,
            AgentId::MAIN,
            turn_id,
            test_title("failed child"),
            test_context("failed-child"),
            Box::new(StubMainRuntime {
                events: vec![AgentEvent {
                    agent_id,
                    turn_id,
                    target: RuntimeTarget::provider("local", "qwen3"),
                    kind: AgentEventKind::TurnFailed {
                        message: "PRIVATE_PROVIDER_ERROR".to_string(),
                    },
                }],
            }),
        );

        let events = orchestrator.drain_child_events();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0].kind,
            AgentEventKind::TurnFailed { message } if message == "Child Agent failed"
        ));
        assert!(!format!("{events:?}").contains("PRIVATE_PROVIDER_ERROR"));
    }

    #[test]
    fn child_index_and_disposal_are_descendants_first() {
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        let root_context = test_context("tree-test");
        let child_context = root_context
            .child(
                AgentContextOwner::try_new("child-two").expect("owner should be valid"),
                AgentChildCapabilityGrants::empty(),
            )
            .expect("child context should be valid");
        let grandchild_context = child_context
            .child(
                AgentContextOwner::try_new("child-three").expect("owner should be valid"),
                AgentChildCapabilityGrants::empty(),
            )
            .expect("grandchild context should be valid");
        orchestrator.register_child_for_test(
            AgentId::new(2),
            AgentId::MAIN,
            AgentTurnId::new(1),
            test_title("parent child"),
            child_context,
            Box::new(StubMainRuntime::default()),
        );
        orchestrator.register_child_for_test(
            AgentId::new(3),
            AgentId::new(2),
            AgentTurnId::new(2),
            test_title("grandchild"),
            grandchild_context,
            Box::new(StubMainRuntime::default()),
        );
        assert_eq!(
            orchestrator.children_of(AgentId::MAIN),
            vec![AgentId::new(2)]
        );
        assert_eq!(
            orchestrator.children_of(AgentId::new(2)),
            vec![AgentId::new(3)]
        );

        orchestrator
            .stop_child(AgentId::new(2))
            .expect("subtree disposal should converge");
        assert_eq!(orchestrator.child_count(), 0);
        assert!(orchestrator.children_of(AgentId::MAIN).is_empty());
        assert!(root_context.dispose().is_success());
    }

    #[test]
    fn failed_child_cleanup_retains_record_and_allows_retry() {
        let agent_id = AgentId::new(2);
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        orchestrator.register_child_for_test(
            agent_id,
            AgentId::MAIN,
            AgentTurnId::new(1),
            test_title("cleanup retry"),
            test_context("cleanup-test"),
            Box::new(FailingShutdownRuntime {
                failures_remaining: 1,
                events: Vec::new(),
            }),
        );

        assert!(orchestrator.stop_child(agent_id).is_err());
        assert_eq!(
            orchestrator.child_status(agent_id),
            Some(AgentProjectionStatus::CleanupBlocked)
        );
        assert_eq!(orchestrator.child_count(), 1);

        orchestrator
            .stop_child(agent_id)
            .expect("retry should reuse the retained runtime/context owner");
        assert_eq!(orchestrator.child_count(), 0);
    }

    #[test]
    fn terminal_fact_waits_for_authority_cleanup_and_retries_the_same_owner() {
        let agent_id = AgentId::new(2);
        let turn_id = AgentTurnId::new(7);
        let terminal = AgentEvent {
            agent_id,
            turn_id,
            target: RuntimeTarget::provider("local", "qwen3"),
            kind: AgentEventKind::TurnFinished {
                response: runtime_domain::session::ConversationResponse::assistant_text(
                    "committed",
                ),
                metrics: None,
                context_usage: None,
            },
        };
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        orchestrator.register_child_for_test(
            agent_id,
            AgentId::MAIN,
            turn_id,
            test_title("terminal cleanup"),
            test_context("terminal-cleanup"),
            Box::new(FailingShutdownRuntime {
                failures_remaining: 1,
                events: vec![terminal.clone()],
            }),
        );

        assert!(orchestrator.drain_child_events().is_empty());
        assert_eq!(
            orchestrator.child_status(agent_id),
            Some(AgentProjectionStatus::CleanupBlocked)
        );
        assert!(orchestrator.child_has_authority(agent_id));

        assert_eq!(orchestrator.drain_child_events(), vec![terminal]);
        assert_eq!(
            orchestrator.child_status(agent_id),
            Some(AgentProjectionStatus::Completed)
        );
        assert!(!orchestrator.child_has_authority(agent_id));
        assert_eq!(orchestrator.child_count(), 1);
    }

    #[test]
    fn child_disposal_closes_admission_then_quiesces_runtime_before_other_inverses() {
        let agent_id = AgentId::new(2);
        let context = test_context("ordered-cleanup");
        let order = Arc::new(Mutex::new(Vec::new()));
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        orchestrator.register_child_for_test(
            agent_id,
            AgentId::MAIN,
            AgentTurnId::new(1),
            test_title("ordered cleanup"),
            context.clone(),
            Box::new(RecordingShutdownRuntime {
                label: "runtime",
                context: context.clone(),
                order: Arc::clone(&order),
            }),
        );
        let listener_order = Arc::clone(&order);
        context
            .register_effect(
                &context.token(),
                AgentScopedEffectKind::Listener,
                move || {
                    Ok::<_, ()>(move || {
                        listener_order
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .push("listener");
                        Ok(())
                    })
                },
            )
            .expect("listener effect should attach");

        orchestrator
            .stop_child(agent_id)
            .expect("ordered cleanup should converge");

        assert_eq!(
            *order
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec!["runtime", "listener"]
        );
    }

    #[test]
    fn context_inverse_failure_retains_closed_child_until_retry() {
        let agent_id = AgentId::new(2);
        let context = test_context("inverse-retry");
        let attempts = Arc::new(AtomicUsize::new(0));
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        orchestrator.register_child_for_test(
            agent_id,
            AgentId::MAIN,
            AgentTurnId::new(1),
            test_title("inverse retry"),
            context.clone(),
            Box::new(StubMainRuntime::default()),
        );
        let cleanup_attempts = Arc::clone(&attempts);
        context
            .register_effect(
                &context.token(),
                AgentScopedEffectKind::Listener,
                move || {
                    Ok::<_, ()>(move || {
                        let attempt = cleanup_attempts.fetch_add(1, Ordering::SeqCst);
                        if attempt == 0 {
                            Err("PRIVATE_LISTENER_FAILURE".to_string())
                        } else {
                            Ok(())
                        }
                    })
                },
            )
            .expect("listener effect should attach");

        assert!(orchestrator.stop_child(agent_id).is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert_eq!(
            orchestrator.child_status(agent_id),
            Some(AgentProjectionStatus::CleanupBlocked)
        );
        assert!(matches!(
            orchestrator.dispatch_child(AgentCommand::Interrupt {
                agent_id,
                target: None,
            }),
            Err(AgentRuntimeError::UnknownAgent)
        ));

        orchestrator
            .stop_child(agent_id)
            .expect("retained context inverse should retry");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert_eq!(orchestrator.child_count(), 0);
    }

    #[test]
    fn subtree_cleanup_uses_ownership_order_instead_of_identity_order() {
        let parent_id = AgentId::new(10);
        let child_id = AgentId::new(2);
        let root_context = test_context("identity-order-root");
        let parent_context = root_context
            .child(
                AgentContextOwner::try_new("identity-order-parent").unwrap(),
                AgentChildCapabilityGrants::empty(),
            )
            .unwrap();
        let child_context = parent_context
            .child(
                AgentContextOwner::try_new("identity-order-child").unwrap(),
                AgentChildCapabilityGrants::empty(),
            )
            .unwrap();
        let order = Arc::new(Mutex::new(Vec::new()));
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        orchestrator.register_child_for_test(
            parent_id,
            AgentId::MAIN,
            AgentTurnId::new(1),
            test_title("parent"),
            parent_context.clone(),
            Box::new(RecordingShutdownRuntime {
                label: "parent",
                context: parent_context,
                order: Arc::clone(&order),
            }),
        );
        orchestrator.register_child_for_test(
            child_id,
            parent_id,
            AgentTurnId::new(2),
            test_title("child"),
            child_context.clone(),
            Box::new(RecordingShutdownRuntime {
                label: "child",
                context: child_context,
                order: Arc::clone(&order),
            }),
        );

        orchestrator.stop_child(parent_id).unwrap();

        assert_eq!(
            *order
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec!["child", "parent"]
        );
    }

    #[test]
    fn owning_scope_revocation_quiesces_registered_child_runtime_once() {
        let host_scope = EffectScope::default();
        let root = AgentCapabilityContext::root(
            AgentContextOwner::try_new("reactive-root").unwrap(),
            &host_scope,
            AgentRootCapabilityGrants::empty(),
        )
        .unwrap();
        let child = root
            .child(
                AgentContextOwner::try_new("reactive-child").unwrap(),
                AgentChildCapabilityGrants::empty(),
            )
            .unwrap();
        let order = Arc::new(Mutex::new(Vec::new()));
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        let agent_id = AgentId::new(2);
        orchestrator.register_child_for_test(
            agent_id,
            AgentId::MAIN,
            AgentTurnId::new(1),
            test_title("reactive child"),
            child.clone(),
            Box::new(RecordingShutdownRuntime {
                label: "runtime",
                context: child,
                order: Arc::clone(&order),
            }),
        );

        assert!(host_scope.dispose().is_success());
        orchestrator
            .stop_child(agent_id)
            .expect("registry removal should observe completed reactive cleanup");

        assert_eq!(
            *order
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec!["runtime"]
        );
    }

    #[test]
    fn replacement_rejects_live_child_owner_in_release_behavior() {
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        let agent_id = AgentId::new(2);
        orchestrator.register_child_for_test(
            agent_id,
            AgentId::MAIN,
            AgentTurnId::new(1),
            test_title("replacement barrier"),
            test_context("replacement-barrier"),
            Box::new(StubMainRuntime::default()),
        );
        let generation = orchestrator.generation();

        assert!(
            orchestrator
                .replace_main(Box::new(StubMainRuntime::default()), None, None)
                .is_err()
        );
        assert_eq!(orchestrator.generation(), generation);
        assert_eq!(orchestrator.child_count(), 1);

        orchestrator.stop_child(agent_id).unwrap();
        orchestrator
            .replace_main(Box::new(StubMainRuntime::default()), None, None)
            .expect("replacement should proceed after retained owner converges");
    }
}
