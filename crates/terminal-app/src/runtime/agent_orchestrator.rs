//! Runtime-owned Agent tree、identity routing 与 lifecycle ownership。

use session_store::SessionPort;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, Mutex},
};
use tokio::sync::oneshot;

use crate::session_store_bridge::run_session_store_future;
use runtime_domain::agent::{
    AgentActivitySummary, AgentChildCompletion, AgentChildMessage, AgentCommand,
    AgentCommandReceipt, AgentEvent, AgentEventKind, AgentGroupCompletion, AgentId,
    AgentInstructions, AgentLaunchBatch, AgentLaunchChildSnapshot, AgentLaunchGroupId,
    AgentLaunchReceipt, AgentObjectiveSummary, AgentObservationId, AgentObservationRejection,
    AgentObservationRequestId, AgentOutcome, AgentOutcomeSummary, AgentOverviewDelta,
    AgentOverviewDeltaKind, AgentOverviewRow, AgentOverviewSnapshot, AgentPermissionRequest,
    AgentPermissionState, AgentPermissionTarget, AgentPermissionUpdate, AgentPreviewSnapshot,
    AgentProjectionEvent, AgentProjectionRevision, AgentProjectionStatus, AgentRuntimeError,
    AgentRuntimeGeneration, AgentTitle, AgentTranscriptItem, AgentTranscriptSnapshot, AgentTurnId,
    AgentTurnRequest, AgentViewSnapshot, SETTLED_CHILD_AUTO_DESTROY_AFTER_MS,
};
use runtime_domain::session::RuntimeTarget;
use runtime_domain::session::{
    ConversationTurnRequest, RuntimeToolActivityContent, TranscriptReplayItem,
};

use super::agent::{
    AgentChildRuntimeLeases, AgentChildRuntimeStaticGrants, AgentMessageDelivery,
    AgentReportEnvelope, AgentRuntimeActivationGrants, AgentRuntimeActivity, AgentRuntimePort,
    AgentSessionCapability, AgentStopReceipt, ChildAgentFactory, SendAgentMessageFailure,
    SendAgentMessageRequest, SpawnAgentsFailure, SpawnAgentsRequest, StopAgentsFailure,
    StopAgentsRequest,
};
use super::agent_capability_context::{
    AgentCapabilityContext, AgentChildCapabilityGrants, AgentContextOwner,
    AgentRootCapabilityGrants, AgentScopedEffectKind,
};
use super::context::{CapabilityLease, PromptAssemblyCapability, ToolCatalogCapability};
use super::effect_scope::EffectScope;

const MAX_ACTIVE_CHILD_AGENTS: usize = 32;
/// settled child 常驻 runtime/context/transcript 是支持后续 followup turn 的内存代价；
/// 上限兜底防止已完成 child 无限累积，超限按最旧淘汰并走完整清理路径。
const MAX_SETTLED_CHILD_AGENTS: usize = 16;
const CHILD_COMPLETED_WITHOUT_REPORT_TEXT: &str = "Child Agent completed without a report";
const CHILD_CANCELLED_TEXT: &str = "Child Agent cancelled";
/// 显式 stop 定格的 Cancelled 摘要；帮助等待方区分"被停止"与自然取消/失败。
const CHILD_STOPPED_BY_REQUEST_TEXT: &str = "Child Agent stopped";

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

/// 单个 child 在 disposal 收敛循环中的结果；`projection_changed` 避免重试时重复发布。
enum ChildDisposal {
    Blocked {
        projection_changed: bool,
        reason: &'static str,
    },
    Converged(AgentId),
}

/// 一次 child disposal 的发起意图。
///
/// `retain_terminal_projection` 决定 registry 移除后是否保留 terminal 投影行；
/// `stopped_by_request` 标记显式 stop（用户/模型请求停止 subtree），使 Cancelled
/// 定格的 outcome summary 与消息 waiter 结算携带"被停止"语义，区别于 revoke、
/// session 切换等生命周期收敛。清理发起到收敛期间意图保持不变，重试复用同一意图。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ChildDisposalIntent {
    retain_terminal_projection: bool,
    stopped_by_request: bool,
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

/// Child authority 的 lifecycle 阶段；与投影 `status` 正交。
///
/// `Settled` 与 `Disposed` 是 terminal 后的两个稳定态：前者保留 runtime/context 作
/// followup 宿主，后者已完全释放、只剩投影行。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChildLifecycle {
    /// turn 运行或等待 permission；占并发额度。
    Active,
    /// terminal 已定格；runtime/context 保留，不占并发额度。
    Settled,
    /// 显式清理（stop/session 切换/淘汰/替换）已发起但未收敛；owner 保留供重试。
    Disposing,
    /// authority 已完全释放；行仅作为 terminal 投影保留，直到显式 tree 清理移除。
    Disposed,
}

/// 一个 child 的 runtime、context 与 projection 必须由同一个 record 持有。
///
/// 该结构不实现 `Clone`，避免把 adapter 或 cleanup owner 隐式复制到 registry 之外。
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
    lifecycle: ChildLifecycle,
    /// 清理发起到收敛期间的 disposal 意图；重试复用同一意图。
    disposal_intent: ChildDisposalIntent,
    /// 该 Cancelled terminal 由显式 stop 请求定格（区别于 adapter interrupt、revoke
    /// 等自然取消）；durable outcome 仍三态，来源只影响 summary 取值。
    terminal_stopped_by_request: bool,
    status: AgentProjectionStatus,
    latest_activity: AgentActivitySummary,
    /// committed-only transcript projection；streaming partial 与 raw tool payload 永不进入。
    transcript: Vec<AgentTranscriptItem>,
    /// tool activity id -> transcript item index，用于把 Started/Updated 折叠到同一 item。
    transcript_tool_items: BTreeMap<String, usize>,
    /// authoritative permission FIFO；head 是唯一可交互的 unresolved request。
    pending_permissions: VecDeque<AgentPermissionRequest>,
    /// Active/未持久化 settled 期间到达的 `SendMessage`；turn 边界后按序转为 followup turn。
    queued_messages: VecDeque<AgentCommand>,
    /// 当前 turn 是否由 followup 消息触发；followup 的 outcome fact 不关联 launch group。
    current_turn_is_followup: bool,
    terminal_outcome_seen: bool,
    outcome_persisted: bool,
    pending_outcome: Option<runtime_domain::agent::AgentOutcomeSnapshot>,
    terminal_status: Option<AgentProjectionStatus>,
    pending_terminal_event: Option<AgentEvent>,
    started_at_ms: i64,
    /// elapsed 累计值：计时暂停（等待 permission）或进入终态时定格的部分。
    /// 不持久化——resume 恢复的 settled 投影没有计时起点，elapsed 显示 `None`。
    elapsed_accumulated_ms: u64,
    /// 计时运行区段起点；`None` 即计时暂停。
    elapsed_running_since_ms: Option<i64>,
    /// 当前 terminal 周期的定格时刻；followup 新 turn 起算时清除。
    settled_at_ms: Option<i64>,
    tool_uses: usize,
    token_usage: usize,
}

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
            lifecycle: ChildLifecycle::Active,
            disposal_intent: ChildDisposalIntent::default(),
            terminal_stopped_by_request: false,
            status: AgentProjectionStatus::Pending,
            latest_activity: AgentActivitySummary::Preparing,
            transcript: Vec::new(),
            transcript_tool_items: BTreeMap::new(),
            pending_permissions: VecDeque::new(),
            queued_messages: VecDeque::new(),
            current_turn_is_followup: false,
            terminal_outcome_seen: false,
            outcome_persisted: false,
            pending_outcome: None,
            terminal_status: None,
            pending_terminal_event: None,
            started_at_ms: 0,
            elapsed_accumulated_ms: 0,
            elapsed_running_since_ms: None,
            settled_at_ms: None,
            tool_uses: 0,
            token_usage: 0,
        }
    }

    /// 暂停计时并运行区段并入累计值；已暂停时是幂等 no-op。
    fn pause_elapsed_at(&mut self, now_ms: i64) {
        if let Some(since) = self.elapsed_running_since_ms.take()
            && now_ms > since
        {
            self.elapsed_accumulated_ms += u64::try_from(now_ms - since).unwrap_or(u64::MAX);
        }
    }

    /// 恢复计时：从当前时刻重新起算运行区段；已在运行时是幂等 no-op。
    fn resume_elapsed_at(&mut self, now_ms: i64) {
        if self.elapsed_running_since_ms.is_none() {
            self.elapsed_running_since_ms = Some(now_ms);
        }
    }

    /// 重置计时（新 turn 重新起算）：累计清零、运行区段从当前时刻开始，并结束上一个
    /// terminal 周期（settled 时刻随之失效）。
    fn restart_elapsed_at(&mut self, now_ms: i64) {
        self.elapsed_accumulated_ms = 0;
        self.elapsed_running_since_ms = Some(now_ms);
        self.settled_at_ms = None;
    }

    /// terminal 定格：暂停 elapsed 计时并记录定格时刻。与等待 permission 的暂停不同，
    /// 该定格只在终态处调用，写入的 `settled_at_ms` 归属当前 terminal 周期。
    fn freeze_terminal_at(&mut self, now_ms: i64) {
        self.pause_elapsed_at(now_ms);
        self.settled_at_ms = Some(now_ms);
    }

    /// elapsed 投影值：运行中为累计值 + 当前区段实时差，暂停/终态为定格累计值。
    /// `started_at_ms` 为 0（test 注册或 resume 恢复的投影）时没有计时语义。
    fn elapsed_ms_at(&self, now_ms: i64) -> Option<u64> {
        (self.started_at_ms > 0).then(|| match self.elapsed_running_since_ms {
            Some(since) if now_ms > since => {
                self.elapsed_accumulated_ms + u64::try_from(now_ms - since).unwrap_or(u64::MAX)
            }
            _ => self.elapsed_accumulated_ms,
        })
    }

    fn is_terminal(&self) -> bool {
        self.terminal_status.is_some()
    }

    /// terminal fact 的交付 gate：`Settled`（authority 保留、投影已定格）与 `Disposed`
    /// （authority 已完全释放）都可交付；`Disposing` 收敛前持有 owner，暂不交付。
    fn terminal_fact_deliverable(&self) -> bool {
        matches!(
            self.lifecycle,
            ChildLifecycle::Settled | ChildLifecycle::Disposed
        )
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

/// observation 是纯 projection state：打开/关闭都不改变 child authority、permission FIFO
/// 或 replay facts，失效只需移除注册，不需要 EffectScope inverse。
struct Observation {
    generation: AgentRuntimeGeneration,
    kind: ObservationKind,
    /// 该 observation 已交付的最高 revision；新 revision 必须严格大于它才发布。
    delivered_revision: AgentProjectionRevision,
}

enum ObservationKind {
    Overview,
    AgentView { agent_id: AgentId },
}

/// typed Agent product command 的 closed 拒绝分类。
///
/// 错误正文是固定分类文案；provider error、cleanup source 等不跨越 command boundary。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AgentProductCommandRejection {
    UnknownAgent,
    StaleGeneration,
    UnknownRequest,
    InvalidOption,
    AlreadySubmitted,
    CleanupPending,
    ReportPending,
}

impl AgentProductCommandRejection {
    pub(super) fn closed_message(self) -> &'static str {
        match self {
            Self::UnknownAgent => "Unknown child Agent",
            Self::StaleGeneration => "Child Agent generation is stale",
            Self::UnknownRequest => "Unknown child Agent permission request",
            Self::InvalidOption => "Invalid child Agent permission option",
            Self::AlreadySubmitted => "Child Agent permission response is already submitted",
            Self::CleanupPending => "Child Agent cleanup is pending",
            Self::ReportPending => "Child Agent group report is pending",
        }
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
    /// 存活的 projection observer；generation mismatch 的 entry 不再发布任何 delta。
    observations: BTreeMap<AgentObservationId, Observation>,
    /// 待 coordinator 在 runtime event consumer 边界 flush 的 projection facts。
    projection_events: Vec<AgentProjectionEvent>,
    pending_context_cleanups: Vec<AgentCapabilityContext>,
    pending_child_cleanups: Vec<PendingChildCleanup>,
    session_port: Option<Arc<dyn SessionPort>>,
    next_agent_id: u64,
    next_observation_id: u64,
    projection_revision: u64,
    next_launch_group_id: u64,
    group_waiters: BTreeMap<AgentLaunchGroupId, GroupWaiter>,
    /// per-child FIFO 的消息回执等待方；与 `queued_messages` 的消费顺序一一对齐。
    message_waiters: BTreeMap<AgentId, VecDeque<MessageWaiter>>,
    main_turn_id: Option<AgentTurnId>,
}

struct GroupWaiter {
    parent_agent_id: AgentId,
    child_ids: Vec<AgentId>,
    response: oneshot::Sender<Result<AgentGroupCompletion, SpawnAgentsFailure>>,
}

/// `send_agent_message` 的同步等待回执：在消息触发的 turn terminal fact 交付点结算。
struct MessageWaiter {
    response: oneshot::Sender<Result<AgentMessageDelivery, SendAgentMessageFailure>>,
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
            observations: BTreeMap::new(),
            projection_events: Vec::new(),
            pending_context_cleanups: Vec::new(),
            pending_child_cleanups: Vec::new(),
            session_port: None,
            next_agent_id: AgentId::MAIN.get().saturating_add(1),
            next_observation_id: 1,
            projection_revision: 0,
            next_launch_group_id: 1,
            group_waiters: BTreeMap::new(),
            message_waiters: BTreeMap::new(),
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
        &mut self,
    ) -> Result<AgentRuntimeGeneration, AgentRuntimeError> {
        // replacement 边界不保留 settled child：先完整清理（连同 active 后代），
        // 清理失败即拒绝切换；剩余 live work 仍由 validate 拒绝。
        self.dispose_settled_children()?;
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
        self.fail_all_message_waiters();
        // generation 切换使全部 observation 失效；旧 observation id 不再收到任何 delta。
        self.observations.clear();
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

    /// Parent→child 消息的 orchestrator 路由入口。
    ///
    /// Active child 只入队（turn 边界后自动开始下一 turn）；Settled child 且上一个
    /// turn 的 outcome 已持久化时立即开始 followup turn；Settled 但 outcome 持久化
    /// 未收敛时同样入队，由 drain 侧 gate 在持久化完成后投递。Disposing/Disposed、
    /// unknown 与 stale generation 一律 closed 拒绝。
    pub(super) fn send_child_message(
        &mut self,
        agent_id: AgentId,
        message: AgentChildMessage,
    ) -> Result<AgentCommandReceipt, AgentRuntimeError> {
        self.reconcile_revoked_children();
        // child turn id 沿用 launch 分配规则（由 agent id 派生）；followup turn 复用
        // 同一 id，permission target 与 parent-turn 匹配语义保持稳定。
        let turn_id = AgentTurnId::new(agent_id.get());
        {
            let Some(record) = self.children.get_mut(&agent_id) else {
                return Err(AgentRuntimeError::UnknownAgent);
            };
            if record.generation != self.generation {
                return Err(AgentRuntimeError::UnknownAgent);
            }
            // Disposing/Disposed 的 child 不是可寻址的消息目标；拒绝路径不产生投影副作用。
            if !matches!(
                record.lifecycle,
                ChildLifecycle::Active | ChildLifecycle::Settled
            ) {
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
            record.queued_messages.push_back(AgentCommand::SendMessage {
                agent_id,
                turn_id,
                message,
            });
        }
        let starts_followup_now = self.children.get(&agent_id).is_some_and(|record| {
            matches!(record.lifecycle, ChildLifecycle::Settled) && record.outcome_persisted
        });
        if starts_followup_now {
            self.start_child_followup_turn(agent_id);
            Ok(AgentCommandReceipt::MessageStarted { turn_id })
        } else {
            Ok(AgentCommandReceipt::MessageQueued { turn_id })
        }
    }

    /// 处理 host-owned `send_agent_message` request；tool bridge 不直接持有 child authority。
    ///
    /// 受理成功后登记 per-child FIFO waiter，由消息触发的 followup turn terminal fact
    /// 交付点（或 disposal 边界）结算；拒绝路径同步返回 closed failure。
    pub(super) fn handle_send_agent_message_request(&mut self, request: SendAgentMessageRequest) {
        let SendAgentMessageRequest {
            identity,
            agent_id,
            message,
            response,
        } = request;
        match self.deliver_child_message(identity, agent_id, message) {
            Ok(target_id) => {
                self.message_waiters
                    .entry(target_id)
                    .or_default()
                    .push_back(MessageWaiter { response });
            }
            Err(failure) => {
                let _ = response.send(Err(failure));
            }
        }
    }

    /// `send_agent_message` 的 identity/target 校验与消息派发。
    ///
    /// caller 校验与 `launch_batch` 同源（generation、parent turn、context epoch），
    /// 但不要求消息 turn 等于 launch turn——followup 允许跨 main turn 追加。
    fn deliver_child_message(
        &mut self,
        identity: tool_runtime::ToolInvocationIdentity,
        agent_id: AgentId,
        message: AgentChildMessage,
    ) -> Result<AgentId, SendAgentMessageFailure> {
        let caller = AgentId::new(identity.agent_id());
        if identity.runtime_generation() != self.generation.get()
            || caller.get() == 0
            || identity.turn_id() == 0
            || identity.context_epoch() == 0
        {
            return Err(SendAgentMessageFailure::StaleGeneration);
        }
        if !self.parent_turn_matches(caller, AgentTurnId::new(identity.turn_id())) {
            return Err(SendAgentMessageFailure::ParentUnavailable);
        }
        let caller_context = self
            .parent_context(caller)
            .map_err(|_| SendAgentMessageFailure::ParentUnavailable)?;
        if caller_context.epoch() != identity.context_epoch() {
            return Err(SendAgentMessageFailure::ParentUnavailable);
        }
        if !self
            .children
            .get(&agent_id)
            .is_some_and(|record| self.child_is_message_target(caller, record))
        {
            return Err(SendAgentMessageFailure::NotFound {
                available_agent_ids: self.addressable_message_targets(caller),
            });
        }
        // send_child_message 只产生 UnknownAgent/Disposed 两类 closed 拒绝，均折叠为
        // “目标不可寻址”；available 列表帮助模型纠错，不透传 raw 错误。
        self.send_child_message(agent_id, message).map_err(|_| {
            SendAgentMessageFailure::NotFound {
                available_agent_ids: self.addressable_message_targets(caller),
            }
        })?;
        Ok(agent_id)
    }

    /// 消息目标必须是 caller 的 direct child 且 authority 仍可寻址。
    fn child_is_message_target(&self, caller: AgentId, record: &ChildAgentRecord) -> bool {
        record.generation == self.generation
            && matches!(
                record.lifecycle,
                ChildLifecycle::Active | ChildLifecycle::Settled
            )
            && record.parent_agent_id == caller
    }

    /// caller 当前可寻址的消息目标列表；只进入 closed NotFound 文案。
    fn addressable_message_targets(&self, caller: AgentId) -> Vec<AgentId> {
        self.children
            .iter()
            .filter(|(_, record)| self.child_is_message_target(caller, record))
            .map(|(agent_id, _)| *agent_id)
            .collect()
    }

    /// 处理 host-owned `stop_agents` request；tool bridge 不直接持有 lifecycle authority。
    ///
    /// 回执同步返回，不等清理收敛（blocked-retry 与 waiter fail-closed 兜底）；Settled
    /// child 幂等返回 already-settled 说明，不触碰 settled 保留语义。
    pub(super) fn handle_stop_agents_request(&mut self, request: StopAgentsRequest) {
        let StopAgentsRequest {
            identity,
            agent_id,
            response,
        } = request;
        let _ = response.send(self.deliver_stop_request(identity, agent_id));
    }

    /// `stop_agents` 的 identity/target 校验与停止路由；只复用 `stop_child`，不复制
    /// lifecycle 路径。
    fn deliver_stop_request(
        &mut self,
        identity: tool_runtime::ToolInvocationIdentity,
        agent_id: AgentId,
    ) -> Result<AgentStopReceipt, StopAgentsFailure> {
        let caller = AgentId::new(identity.agent_id());
        if identity.runtime_generation() != self.generation.get()
            || caller.get() == 0
            || identity.turn_id() == 0
            || identity.context_epoch() == 0
        {
            return Err(StopAgentsFailure::StaleGeneration);
        }
        if !self.parent_turn_matches(caller, AgentTurnId::new(identity.turn_id())) {
            return Err(StopAgentsFailure::ParentUnavailable);
        }
        let caller_context = self
            .parent_context(caller)
            .map_err(|_| StopAgentsFailure::ParentUnavailable)?;
        if caller_context.epoch() != identity.context_epoch() {
            return Err(StopAgentsFailure::ParentUnavailable);
        }
        let Some(record) = self
            .children
            .get(&agent_id)
            .filter(|record| self.child_is_stop_target(caller, record))
        else {
            return Err(StopAgentsFailure::NotFound {
                available_agent_ids: self.addressable_stop_targets(caller),
            });
        };
        match record.lifecycle {
            // 已 terminal 的 child：报告已按既有 outcome 交付，停止是幂等 no-op。
            ChildLifecycle::Settled | ChildLifecycle::Disposed => {
                return Ok(AgentStopReceipt::AlreadySettled {
                    agent_id,
                    title: record.title.clone(),
                    outcome: outcome_for_status(record.terminal_status),
                    summary: safe_outcome_summary(record),
                });
            }
            // Active 走完整 subtree stop；Disposing 是同一 owner 的幂等重试。
            ChildLifecycle::Active | ChildLifecycle::Disposing => {}
        }
        let title = record.title.clone();
        self.stop_child(agent_id)
            .map_err(|_| StopAgentsFailure::CleanupPending)?;
        Ok(AgentStopReceipt::Stopped { agent_id, title })
    }

    /// stop 目标必须是 caller 的 direct child；Disposing/Disposed 的 registry 行保持
    /// 可寻址（幂等重试/已完成说明），与消息目标的 Active/Settled 面互补。
    fn child_is_stop_target(&self, caller: AgentId, record: &ChildAgentRecord) -> bool {
        record.generation == self.generation && record.parent_agent_id == caller
    }

    /// caller 当前可寻址的 stop 目标列表；只进入 closed NotFound 文案。
    fn addressable_stop_targets(&self, caller: AgentId) -> Vec<AgentId> {
        self.children
            .iter()
            .filter(|(_, record)| self.child_is_stop_target(caller, record))
            .map(|(agent_id, _)| *agent_id)
            .collect()
    }

    /// followup turn terminal fact 交付后结算该 child 的队头消息 waiter。
    ///
    /// pending terminal event 每个 turn 恰好交付一次，且每条消息恰好触发一个 turn，
    /// 因此按 child FIFO 弹出一个 waiter 与消息消费顺序严格对齐。
    fn settle_message_waiter(&mut self, agent_id: AgentId) {
        let Some(waiter) = self
            .message_waiters
            .get_mut(&agent_id)
            .and_then(VecDeque::pop_front)
        else {
            return;
        };
        let delivery = self.children.get(&agent_id).map(|record| {
            AgentMessageDelivery::new(
                agent_id,
                record.title.clone(),
                outcome_for_status(record.terminal_status),
                child_report_envelope(
                    record,
                    runtime_domain::time::unix_timestamp_ms().unwrap_or(0),
                ),
            )
        });
        let _ = match delivery {
            Some(delivery) => waiter.response.send(Ok(delivery)),
            None => waiter
                .response
                .send(Err(SendAgentMessageFailure::TargetUnavailable)),
        };
        if self
            .message_waiters
            .get(&agent_id)
            .is_some_and(VecDeque::is_empty)
        {
            self.message_waiters.remove(&agent_id);
        }
    }

    /// disposal 发起即结算该 child 的全部消息 waiter：Disposing child 不再执行任何
    /// turn，排队消息与其等待回执一并 closed 失败；显式 stop 与生命周期收敛由
    /// caller 传入可区分的分类。
    fn fail_child_message_waiters(&mut self, agent_id: AgentId, failure: SendAgentMessageFailure) {
        if let Some(waiters) = self.message_waiters.remove(&agent_id) {
            for waiter in waiters {
                let _ = waiter.response.send(Err(failure.clone()));
            }
        }
    }

    fn fail_all_message_waiters(&mut self) {
        for (_, waiters) in std::mem::take(&mut self.message_waiters) {
            for waiter in waiters {
                let _ = waiter
                    .response
                    .send(Err(SendAgentMessageFailure::TargetUnavailable));
            }
        }
    }

    /// 把队列头部的 `SendMessage` 转为 followup turn 并提交 child runtime。
    ///
    /// 前提：record 处于 Settled 且上一个 turn 的 outcome 已持久化、terminal 事实已
    /// 交付（drain 侧 gate 保证），因此这里可以安全重置 terminal 状态。每个 turn 只
    /// 消费一条消息，保持 `SubmitTurn` 的单 user 消息语义。
    fn start_child_followup_turn(&mut self, agent_id: AgentId) {
        {
            let Some(record) = self.children.get_mut(&agent_id) else {
                return;
            };
            // 无 provider target 的 record 无法构造 followup request；消息留在队列，
            // 由下一次 drain 重试（launch 提交的 record 恒有 target）。
            let Some(target) = record.target.clone() else {
                return;
            };
            let Some(AgentCommand::SendMessage {
                turn_id, message, ..
            }) = record.queued_messages.pop_front()
            else {
                return;
            };
            record.terminal_outcome_seen = false;
            record.terminal_status = None;
            record.outcome_persisted = false;
            record.pending_outcome = None;
            record.pending_terminal_event = None;
            record.lifecycle = ChildLifecycle::Active;
            record.current_turn_is_followup = true;
            record.status = AgentProjectionStatus::Pending;
            record.latest_activity = AgentActivitySummary::Preparing;
            let now_ms = runtime_domain::time::unix_timestamp_ms().unwrap_or(0);
            record.started_at_ms = now_ms;
            record.restart_elapsed_at(now_ms);
            record.transcript.push(AgentTranscriptItem::User {
                content: message.as_str().to_string(),
            });
            let request = child_followup_turn_request(&target, &message);
            if record
                .runtime
                .dispatch(AgentCommand::SubmitTurn {
                    agent_id,
                    turn_id,
                    request: Box::new(request),
                })
                .is_err()
            {
                // 与 launch 的 initial dispatch 失败同构：followup turn 直接定格为
                // Failed terminal，由下一次 drain 交付 terminal 事实与 outcome。
                apply_child_projection(
                    record,
                    &AgentEventKind::TurnFailed {
                        message: "Child Agent failed to start".to_string(),
                    },
                    now_ms,
                );
                record.terminal_outcome_seen = true;
                record.pending_terminal_event = Some(AgentEvent {
                    agent_id,
                    turn_id,
                    target,
                    kind: AgentEventKind::TurnFailed {
                        message: "Child Agent failed to start".to_string(),
                    },
                });
                freeze_pending_outcome(agent_id, record, now_ms);
            }
        }
        self.projection_revision = self.projection_revision.saturating_add(1);
        self.publish_child_facts(agent_id, false);
    }

    /// settle/waiter 收敛后，把已可投递的排队消息转为 followup turn（每 child 一条）。
    fn dispatch_queued_child_messages(&mut self) {
        let ready_ids = self
            .children
            .iter()
            .filter(|(_, record)| {
                matches!(record.lifecycle, ChildLifecycle::Settled)
                    && record.outcome_persisted
                    && !record.queued_messages.is_empty()
            })
            .map(|(agent_id, _)| *agent_id)
            .collect::<Vec<_>>();
        for agent_id in ready_ids {
            self.start_child_followup_turn(agent_id);
        }
    }

    /// 取出 child adapter 的 facts，并在 identity/turn/terminal 边界更新 authoritative record。
    ///
    /// 未知 Agent、错误 turn、旧 generation 或 terminal 后的 late event 都被丢弃；它们不能
    /// 进入 main mapper，也不能改变任何 child projection。
    pub(super) fn drain_child_events(&mut self) -> Vec<AgentEvent> {
        self.reconcile_revoked_children();
        let child_ids = self.children.keys().copied().collect::<Vec<_>>();
        let mut accepted = Vec::new();
        for agent_id in child_ids {
            let events = match self.children.get_mut(&agent_id) {
                Some(record) => record.runtime.drain_events(),
                None => continue,
            };
            for event in events {
                self.accept_child_event(agent_id, event, &mut accepted);
            }
        }
        self.settle_terminal_children();
        self.persist_terminal_outcomes();
        // followup turn 的 terminal fact 交付点即消息 waiter 的结算点：pending event
        // 每个 turn 恰好交付一次，waiter 与消息触发的 turn 一一对齐。
        let mut followup_terminal_child_ids = Vec::new();
        for (agent_id, record) in self.children.iter_mut() {
            if record.outcome_persisted
                && record.terminal_fact_deliverable()
                && let Some(event) = record.pending_terminal_event.take()
            {
                if record.current_turn_is_followup {
                    followup_terminal_child_ids.push(*agent_id);
                }
                accepted.push(event);
            }
        }
        for agent_id in followup_terminal_child_ids {
            self.settle_message_waiter(agent_id);
        }
        self.try_complete_group_waiters();
        // waiter/terminal 事实收敛后才允许 followup：followup 会重置 terminal 投影，
        // 先结算才能保证 group completion 不被推迟。
        // 过期清扫夹在 waiter 结算与 followup 派发之间：已过期的 settled child 不再
        // 作为 followup 宿主消费排队消息，其残留 waiter 随清理 fail closed。
        self.evict_expired_settled_children();
        self.dispatch_queued_child_messages();
        accepted
    }

    /// 通过 identity/turn/generation/admission/terminal gate 接受单个 child fact，并推进
    /// authoritative record、transcript、permission FIFO 与 observation 投影。
    fn accept_child_event(
        &mut self,
        agent_id: AgentId,
        event: AgentEvent,
        accepted: &mut Vec<AgentEvent>,
    ) {
        let is_terminal = event.kind.is_terminal();
        let permission_changed;
        {
            let Some(record) = self.children.get_mut(&agent_id) else {
                return;
            };
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
                return;
            }
            let now_ms = runtime_domain::time::unix_timestamp_ms().unwrap_or(0);
            apply_child_projection(record, &event.kind, now_ms);
            permission_changed = apply_child_permission_fact(agent_id, record, &event);
            apply_child_transcript_fact(record, &event.kind);
            if is_terminal {
                record.terminal_outcome_seen = true;
                record.pending_terminal_event = Some(safe_child_terminal_event(event));
                freeze_pending_outcome(agent_id, record, now_ms);
            } else {
                accepted.push(event);
            }
        }
        self.projection_revision = self.projection_revision.saturating_add(1);
        self.publish_child_facts(agent_id, permission_changed);
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
    /// 这是 `launch_batch` 的唯一 staging seam：先完成身份分配、scoped context 与
    /// adapter construction，再提交 record；任何失败都不会留下 registry row。
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
            // launch 边界冻结 delivery-safe user objective；instructions 不进入 transcript。
            record.transcript.push(AgentTranscriptItem::User {
                content: request.objective().as_str().to_string(),
            });
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
        if let Err(error) =
            self.append_replay_fact(TranscriptReplayItem::AgentLaunch(snapshot.clone()))
        {
            self.cleanup_unpublished_children(staged);
            return Err(error);
        }
        // durable launch fact 已提交（或确认无需持久化）才进入 document 投影；
        // 事件与 replay fact 携带同一 typed snapshot，live/resume 渲染同源。
        self.projection_events
            .push(AgentProjectionEvent::AgentLaunchFact { snapshot });

        let mut dispatches = Vec::with_capacity(staged.len());
        for (child_id, record, request) in staged {
            let turn_id = record.turn_id;
            dispatches.push((child_id, turn_id, request));
            let mut record = record;
            let now_ms = runtime_domain::time::unix_timestamp_ms().unwrap_or(0);
            record.started_at_ms = now_ms;
            record.restart_elapsed_at(now_ms);
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
                let now_ms = runtime_domain::time::unix_timestamp_ms().unwrap_or(0);
                apply_child_projection(
                    record,
                    &AgentEventKind::TurnFailed {
                        message: "Child Agent failed to start".to_string(),
                    },
                    now_ms,
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
                freeze_pending_outcome(child_id, record, now_ms);
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
        // terminal 事实一旦冻结即可持久化；authority 保留（settled）与清理收敛都不
        // 阻塞 durable fact 与报告回传，失败的清理由 settle pass 以同一 owner 重试。
        let outcome_ids = self
            .children
            .iter()
            .filter_map(|(agent_id, record)| {
                (record.terminal_status.is_some() && !record.outcome_persisted).then_some(*agent_id)
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
                .append_replay_fact(TranscriptReplayItem::AgentOutcome(snapshot.clone()))
                .is_ok()
                && let Some(record) = self.children.get_mut(&agent_id)
            {
                record.outcome_persisted = true;
                record.pending_outcome = None;
                // 与 launch fact 同理：先持久化（或确认无需持久化）再交付 document 投影，
                // 重试成功时交付的是同一 frozen snapshot。
                self.projection_events
                    .push(AgentProjectionEvent::AgentOutcomeFact { snapshot });
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

    /// Group completion 的送达条件是全部 child 的 terminal 事实与 durable outcome；
    /// authority 清理收敛不是前置条件，清理 blocked 不得阻塞报告回传。
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
                    .all(|record| record.is_terminal() && record.outcome_persisted)
                    .then_some((*group_id, waiter.parent_agent_id, waiter.child_ids.clone()))
            })
            .collect::<Vec<_>>();
        for (group_id, parent_agent_id, child_ids) in completed {
            let Some(waiter) = self.group_waiters.remove(&group_id) else {
                continue;
            };
            let occurred_at_ms = runtime_domain::time::unix_timestamp_ms().unwrap_or(0);
            let children = child_ids
                .into_iter()
                .filter_map(|agent_id| {
                    self.children.get(&agent_id).map(|record| {
                        let envelope = child_report_envelope(record, occurred_at_ms);
                        AgentChildCompletion {
                            agent_id,
                            title: record.title.clone(),
                            outcome: outcome_for_status(record.terminal_status),
                            report: envelope.report,
                            tokens: envelope.tokens,
                            tool_uses: envelope.tool_uses,
                            duration: envelope.duration,
                            truncated: envelope.truncated,
                        }
                    })
                })
                .collect::<Vec<_>>();
            let completion = AgentGroupCompletion {
                group_id,
                parent_agent_id,
                children,
                occurred_at_ms,
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
        self.publish_child_facts(agent_id, false);
    }

    /// 并发额度只统计 Active child；settled 不占额度，Disposing/Disposed 的 authority
    /// 分别由 cleanup 重试与显式 tree 清理兜底。
    fn active_child_count(&self) -> usize {
        self.children
            .values()
            .filter(|record| matches!(record.lifecycle, ChildLifecycle::Active))
            .count()
    }

    /// 建立一个 overview observation：立即生成 snapshot 并 queue 对应 projection event。
    ///
    /// observation 是纯 projection state；打开它不改变 child authority、permission FIFO
    /// 或 replay facts。
    pub(super) fn observe_agents(&mut self, request_id: AgentObservationRequestId) {
        self.reconcile_revoked_children();
        let observation_id = AgentObservationId::new(self.next_observation_id);
        self.next_observation_id = self.next_observation_id.saturating_add(1);
        let snapshot = self.overview_snapshot_for(
            observation_id,
            runtime_domain::time::unix_timestamp_ms().unwrap_or(0),
        );
        self.observations.insert(
            observation_id,
            Observation {
                generation: self.generation,
                kind: ObservationKind::Overview,
                delivered_revision: snapshot.revision,
            },
        );
        self.projection_events
            .push(AgentProjectionEvent::AgentsOverviewSnapshotLoaded {
                request_id,
                snapshot,
            });
    }

    /// 建立一个 per-agent observation：transcript 与 preview 共用同一 observation 与 revision。
    ///
    /// 未知 Agent 或 stale generation 一律 fail closed，queue closed rejection。
    pub(super) fn observe_agent_transcript(
        &mut self,
        request_id: AgentObservationRequestId,
        agent_id: AgentId,
    ) {
        self.reconcile_revoked_children();
        let record = self
            .children
            .get(&agent_id)
            .filter(|record| record.generation == self.generation);
        let Some(record) = record else {
            self.projection_events
                .push(AgentProjectionEvent::AgentObservationRejected {
                    request_id,
                    reason: AgentObservationRejection::UnknownAgent,
                });
            return;
        };
        let observation_id = AgentObservationId::new(self.next_observation_id);
        self.next_observation_id = self.next_observation_id.saturating_add(1);
        let snapshot = agent_view_snapshot_for_child(
            observation_id,
            agent_id,
            record,
            self.generation,
            AgentProjectionRevision::new(self.projection_revision),
            runtime_domain::time::unix_timestamp_ms().unwrap_or(0),
        );
        self.observations.insert(
            observation_id,
            Observation {
                generation: self.generation,
                kind: ObservationKind::AgentView { agent_id },
                delivered_revision: snapshot.revision,
            },
        );
        self.projection_events
            .push(AgentProjectionEvent::AgentViewSnapshotLoaded {
                request_id,
                snapshot,
            });
    }

    /// 撤销一个 observation；id/generation mismatch 静默丢弃（幂等），无副作用需要撤销。
    pub(super) fn stop_observation(
        &mut self,
        observation_id: AgentObservationId,
        generation: AgentRuntimeGeneration,
    ) {
        if self
            .observations
            .get(&observation_id)
            .is_some_and(|observation| observation.generation == generation)
        {
            self.observations.remove(&observation_id);
        }
    }

    /// 取出已 queue 的 projection facts；由 coordinator 在 runtime event consumer 边界 flush。
    pub(super) fn drain_projection_events(&mut self) -> Vec<AgentProjectionEvent> {
        std::mem::take(&mut self.projection_events)
    }

    /// typed child permission response 的完整 identity/option 校验与路由。
    ///
    /// 校验或 dispatch 失败返回 closed rejection，entry 状态保持不变；成功 receipt 才把
    /// entry 置为 `Submitted` 并投影 FIFO head。
    pub(super) fn respond_agent_permission(
        &mut self,
        target: AgentPermissionTarget,
        option_id: Option<String>,
    ) -> Result<(), AgentProductCommandRejection> {
        if target.generation != self.generation {
            return Err(AgentProductCommandRejection::StaleGeneration);
        }
        self.reconcile_revoked_children();
        {
            let Some(record) = self.children.get(&target.agent_id) else {
                return Err(AgentProductCommandRejection::UnknownAgent);
            };
            if record.generation != self.generation
                || record.turn_id != target.turn_id
                || !record.admission_open()
                || record
                    .target
                    .as_ref()
                    .is_some_and(|record_target| *record_target != target.runtime_target)
            {
                return Err(AgentProductCommandRejection::UnknownAgent);
            }
            // child preview 侧永远显式提交 runtime-issued option；`None` 是封闭语义，不是 reject。
            let Some(option_id) = option_id.as_deref() else {
                return Err(AgentProductCommandRejection::InvalidOption);
            };
            let Some(entry) = record
                .pending_permissions
                .iter()
                .find(|entry| entry.target.request_id == target.request_id)
            else {
                return Err(AgentProductCommandRejection::UnknownRequest);
            };
            if entry.state == AgentPermissionState::Submitted {
                return Err(AgentProductCommandRejection::AlreadySubmitted);
            }
            if !entry
                .request
                .options
                .iter()
                .any(|option| option.option_id == option_id)
            {
                return Err(AgentProductCommandRejection::InvalidOption);
            }
        }
        let receipt = self.dispatch_child(AgentCommand::RespondPermission {
            agent_id: target.agent_id,
            target: Some(target.runtime_target.clone()),
            request_id: target.request_id.clone(),
            option_id,
        });
        if let Err(error) = receipt {
            return Err(match error {
                AgentRuntimeError::UnknownAgent => AgentProductCommandRejection::UnknownAgent,
                AgentRuntimeError::Busy => AgentProductCommandRejection::AlreadySubmitted,
                _ => AgentProductCommandRejection::CleanupPending,
            });
        }
        let head = {
            let Some(record) = self.children.get_mut(&target.agent_id) else {
                return Err(AgentProductCommandRejection::UnknownAgent);
            };
            if let Some(entry) = record
                .pending_permissions
                .iter_mut()
                .find(|entry| entry.target.request_id == target.request_id)
            {
                entry.state = AgentPermissionState::Submitted;
            }
            record.pending_permissions.front().cloned()
        };
        self.projection_events
            .push(AgentProjectionEvent::AgentPermissionUpdated {
                update: AgentPermissionUpdate {
                    agent_id: target.agent_id,
                    generation: self.generation,
                    request: head,
                },
            });
        Ok(())
    }

    /// typed subtree stop：generation 校验后复用既有 descendants-first `stop_child`。
    ///
    /// main `Interrupt` 语义保持分离；`AgentId::MAIN` 一律 closed 拒绝。
    /// settled/disposed 行的 stop 是删除请求：disposal 不保留 terminal 投影行，
    /// registry 移除并发布 Remove delta；running 行保持 stop 的投影保留语义。
    /// 所属 launch group 的 report 仍未交付时删除让位（`ReportPending`）——
    /// completion 需要 registry 内的全部 staged 行，交付后该行恢复可删除。
    pub(super) fn stop_agent(
        &mut self,
        agent_id: AgentId,
        generation: AgentRuntimeGeneration,
    ) -> Result<(), AgentProductCommandRejection> {
        if generation != self.generation {
            return Err(AgentProductCommandRejection::StaleGeneration);
        }
        if agent_id == AgentId::MAIN || !self.children.contains_key(&agent_id) {
            return Err(AgentProductCommandRejection::UnknownAgent);
        }
        let deleting_settled_row = self.children.get(&agent_id).is_some_and(|record| {
            matches!(
                record.lifecycle,
                ChildLifecycle::Settled | ChildLifecycle::Disposed
            )
        });
        // group completion 读取 registry 内全部 staged child 行：所属 launch group 的
        // waiter 仍在等待时删除该行会让 completion 永远无法凑齐（等待方挂死）。
        // 与过期清扫共用同一让位谓词，report 交付后该行恢复可删除。
        if deleting_settled_row
            && self
                .children
                .get(&agent_id)
                .is_some_and(|record| self.launch_group_completion_pending(record))
        {
            return Err(AgentProductCommandRejection::ReportPending);
        }
        let result = if deleting_settled_row {
            self.dispose_child_ids(
                self.subtree_ids(agent_id),
                ChildDisposalIntent {
                    retain_terminal_projection: false,
                    stopped_by_request: true,
                },
            )
        } else {
            self.stop_child(agent_id)
        };
        result.map_err(|_| AgentProductCommandRejection::CleanupPending)
    }

    /// 把某个 child 的最新投影发布给存活的 observation；permission 变化独立于 observation 交付。
    fn publish_child_facts(&mut self, agent_id: AgentId, permission_changed: bool) {
        let revision = AgentProjectionRevision::new(self.projection_revision);
        let now_ms = runtime_domain::time::unix_timestamp_ms().unwrap_or(0);
        let Some(record) = self.children.get(&agent_id) else {
            return;
        };
        let row = overview_row_for_child(&agent_id, record, now_ms);
        for (observation_id, observation) in self.observations.iter_mut() {
            if observation.generation != self.generation
                || observation.delivered_revision >= revision
            {
                continue;
            }
            match observation.kind {
                ObservationKind::Overview => {
                    self.projection_events
                        .push(AgentProjectionEvent::AgentsOverviewUpdated {
                            delta: AgentOverviewDelta {
                                observation_id: *observation_id,
                                generation: self.generation,
                                revision,
                                kind: AgentOverviewDeltaKind::Upsert(row.clone()),
                            },
                        });
                    observation.delivered_revision = revision;
                }
                ObservationKind::AgentView {
                    agent_id: observed_agent_id,
                } if observed_agent_id == agent_id => {
                    let snapshot = agent_view_snapshot_for_child(
                        *observation_id,
                        agent_id,
                        record,
                        self.generation,
                        revision,
                        now_ms,
                    );
                    self.projection_events
                        .push(AgentProjectionEvent::AgentViewUpdated { snapshot });
                    observation.delivered_revision = revision;
                }
                _ => {}
            }
        }
        if permission_changed {
            let update = AgentPermissionUpdate {
                agent_id,
                generation: self.generation,
                request: record.pending_permissions.front().cloned(),
            };
            self.projection_events
                .push(AgentProjectionEvent::AgentPermissionUpdated { update });
        }
    }

    /// child record 从 registry 移除后发布 Remove delta，并让绑定该 child 的
    /// per-agent observation 一并失效（fail closed，不再有后续 snapshot）。
    fn publish_overview_remove(&mut self, agent_id: AgentId) {
        let revision = AgentProjectionRevision::new(self.projection_revision);
        for (observation_id, observation) in self.observations.iter_mut() {
            if observation.generation != self.generation
                || observation.delivered_revision >= revision
                || !matches!(observation.kind, ObservationKind::Overview)
            {
                continue;
            }
            self.projection_events
                .push(AgentProjectionEvent::AgentsOverviewUpdated {
                    delta: AgentOverviewDelta {
                        observation_id: *observation_id,
                        generation: self.generation,
                        revision,
                        kind: AgentOverviewDeltaKind::Remove { agent_id },
                    },
                });
            observation.delivered_revision = revision;
        }
        self.observations.retain(|_, observation| {
            !matches!(
                observation.kind,
                ObservationKind::AgentView {
                    agent_id: observed_agent_id
                } if observed_agent_id == agent_id
            )
        });
    }

    /// permission queue 被清空时投影 `AgentPermissionUpdated(None)`；与 observation 无关。
    fn queue_permission_cleared(&mut self, agent_id: AgentId) {
        self.projection_events
            .push(AgentProjectionEvent::AgentPermissionUpdated {
                update: AgentPermissionUpdate {
                    agent_id,
                    generation: self.generation,
                    request: None,
                },
            });
    }

    fn overview_snapshot_for(
        &self,
        observation_id: AgentObservationId,
        now_ms: i64,
    ) -> AgentOverviewSnapshot {
        let rows = self
            .children
            .iter()
            .map(|(agent_id, record)| overview_row_for_child(agent_id, record, now_ms))
            .collect();
        AgentOverviewSnapshot {
            observation_id,
            generation: self.generation,
            revision: AgentProjectionRevision::new(self.projection_revision),
            rows,
        }
    }

    /// Stop 一个 child subtree；该操作不会影响 parent 或 sibling。
    pub(super) fn stop_child(&mut self, agent_id: AgentId) -> Result<(), AgentRuntimeError> {
        if !self.children.contains_key(&agent_id) {
            return Err(AgentRuntimeError::UnknownAgent);
        }
        let retain_terminal_projection = self
            .children
            .get(&agent_id)
            .is_some_and(|record| record.launch_group_id.is_some());
        let result = self.dispose_child_ids(
            self.subtree_ids(agent_id),
            ChildDisposalIntent {
                retain_terminal_projection,
                stopped_by_request: true,
            },
        );
        self.persist_terminal_outcomes();
        result
    }

    /// Session identity 切换时只撤销 runtime-owned child tree，main adapter 由 restore
    /// transaction 继续拥有；cleanup 未收敛时不得安装 fresh session state。
    pub(super) fn dispose_children_for_session_transition(
        &mut self,
    ) -> Result<(), AgentRuntimeError> {
        // observation 是 session-bound projection；session 切换后一律失效。
        self.observations.clear();
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
    /// an admission path for a generic child adapter. Revocation covers settled children too:
    /// their authority is gone with the context tree, so they take the full disposal path.
    /// In-flight `Disposing` children are excluded here—their own `begin_disposal` already closed
    /// the context, and their retry must keep the original disposal intent.
    fn reconcile_revoked_children(&mut self) {
        let revoked = self
            .children
            .iter()
            .filter_map(|(agent_id, record)| {
                record
                    .context
                    .as_ref()
                    .is_some_and(|context| !context.is_current())
                    .then_some(*agent_id)
                    .filter(|_| !matches!(record.lifecycle, ChildLifecycle::Disposing))
            })
            .collect::<Vec<_>>();
        if !revoked.is_empty() {
            let _ = self.dispose_child_ids(revoked, ChildDisposalIntent::default());
        }
        self.settle_terminal_children();
    }

    fn allocate_agent_id(&mut self) -> Result<AgentId, AgentRuntimeError> {
        let value = self.next_agent_id;
        self.next_agent_id = self.next_agent_id.checked_add(1).ok_or_else(|| {
            AgentRuntimeError::CommandRejected("Agent identity exhausted".to_string())
        })?;
        Ok(AgentId::new(value))
    }

    #[cfg(test)]
    pub(super) fn child_count(&self) -> usize {
        // 每个注册行都持有 authority 或已定格 terminal 投影，registry 大小即 child 数。
        self.children.len()
    }

    #[cfg(test)]
    pub(super) fn children_of(&self, parent_agent_id: AgentId) -> Vec<AgentId> {
        self.children_by_parent
            .get(&parent_agent_id)
            .map(|children| children.iter().copied().collect())
            .unwrap_or_default()
    }

    #[cfg(test)]
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
    pub(super) fn observation_count(&self) -> usize {
        self.observations.len()
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

    /// 为 test-registered child 标注 provider target；生产路径在 staging 时设置。
    #[cfg(test)]
    pub(super) fn mark_child_target_for_test(&mut self, agent_id: AgentId, target: RuntimeTarget) {
        if let Some(record) = self.children.get_mut(&agent_id) {
            record.target = Some(target);
        }
    }

    /// 为 test-registered child 启动 elapsed 计时；生产路径在 launch/followup
    /// 提交时由 `restart_elapsed_at` 完成。
    #[cfg(test)]
    pub(super) fn mark_child_elapsed_started_for_test(&mut self, agent_id: AgentId) {
        let now_ms = runtime_domain::time::unix_timestamp_ms().unwrap_or(0);
        if let Some(record) = self.children.get_mut(&agent_id) {
            record.started_at_ms = now_ms;
            record.restart_elapsed_at(now_ms);
        }
    }

    /// 为 test-registered child 标注 launch group；生产路径只有 `launch_batch` 会设置。
    #[cfg(test)]
    pub(super) fn mark_child_launch_group_for_test(
        &mut self,
        agent_id: AgentId,
        group_id: AgentLaunchGroupId,
    ) {
        if let Some(record) = self.children.get_mut(&agent_id) {
            record.launch_group_id = Some(group_id);
        }
    }

    pub(super) fn suspend(&mut self) -> Result<(), AgentRuntimeError> {
        if let Some(context) = &self.root_context {
            context.begin_disposal();
        }
        self.fail_group_waiters(SpawnAgentsFailure::Unavailable);
        self.fail_all_message_waiters();
        self.observations.clear();
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
        self.fail_all_message_waiters();
        self.observations.clear();
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
        self.dispose_child_ids(
            self.registry_disposal_order(),
            ChildDisposalIntent::default(),
        )
    }

    /// 全 registry 的 disposal 顺序：MAIN 子树按 descendants-first，游离 record 追加在后。
    fn registry_disposal_order(&self) -> Vec<AgentId> {
        let mut ids = self.descendant_ids_postorder(AgentId::MAIN);
        for agent_id in self.children.keys().copied().collect::<Vec<_>>() {
            if !ids.contains(&agent_id) {
                ids.push(agent_id);
            }
        }
        ids
    }

    fn fail_group_waiters(&mut self, failure: SpawnAgentsFailure) {
        for (_, waiter) in std::mem::take(&mut self.group_waiters) {
            let _ = waiter.response.send(Err(failure));
        }
    }

    fn dispose_child_ids(
        &mut self,
        child_ids: Vec<AgentId>,
        intent: ChildDisposalIntent,
    ) -> Result<(), AgentRuntimeError> {
        let waiter_failure = if intent.stopped_by_request {
            SendAgentMessageFailure::TargetStopped
        } else {
            SendAgentMessageFailure::TargetUnavailable
        };
        for agent_id in &child_ids {
            let mut stopping_started = false;
            let mut permission_cleared = false;
            if let Some(record) = self.children.get_mut(agent_id) {
                record.lifecycle = ChildLifecycle::Disposing;
                record.disposal_intent = intent;
                // Disposing child 不再执行任何 turn：排队消息随 authority 一并失效。
                record.queued_messages.clear();
                if record.status != AgentProjectionStatus::Stopping {
                    stopping_started = true;
                }
                record.status = AgentProjectionStatus::Stopping;
                if record.terminal_status.is_none() {
                    record.terminal_outcome_seen = true;
                    record.terminal_status = Some(AgentProjectionStatus::Cancelled);
                    // summary 来源必须在 freeze 前落位：pending outcome 与后续 group
                    // completion 读取同一取值，自然取消的既有摘要不被覆盖。
                    record.terminal_stopped_by_request = intent.stopped_by_request;
                    record.latest_activity = AgentActivitySummary::Idle;
                    // 显式 stop 定格 terminal 的同时定格 elapsed 与 settled 时刻。
                    let now_ms = runtime_domain::time::unix_timestamp_ms().unwrap_or(0);
                    record.freeze_terminal_at(now_ms);
                    freeze_pending_outcome(*agent_id, record, now_ms);
                    if intent.retain_terminal_projection {
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
                if !record.pending_permissions.is_empty() {
                    // stop/dispose 直接撤销 pending permission authority 并投影清空。
                    record.pending_permissions.clear();
                    permission_cleared = true;
                }
            }
            if stopping_started {
                self.projection_revision = self.projection_revision.saturating_add(1);
                self.publish_child_facts(*agent_id, false);
            }
            if permission_cleared {
                self.queue_permission_cleared(*agent_id);
            }
            // 消息等待方不随清理收敛挂起：disposal 发起即 closed 结算，显式 stop 与
            // 生命周期收敛使用可区分的分类。
            self.fail_child_message_waiters(*agent_id, waiter_failure.clone());
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
            // record 借用限制在作用域块内；registry/index 更新与投影发布在借用结束后进行。
            let disposal = {
                let Some(record) = self.children.get_mut(&agent_id) else {
                    continue;
                };
                let was_cleanup_blocked = record.status == AgentProjectionStatus::CleanupBlocked;
                if has_owned_descendant {
                    record.status = AgentProjectionStatus::CleanupBlocked;
                    ChildDisposal::Blocked {
                        projection_changed: !was_cleanup_blocked,
                        reason: "Agent descendant cleanup is pending",
                    }
                } else if record.runtime.shutdown().is_err() {
                    record.status = AgentProjectionStatus::CleanupBlocked;
                    ChildDisposal::Blocked {
                        projection_changed: !was_cleanup_blocked,
                        reason: "Agent child runtime cleanup is pending",
                    }
                } else if !record
                    .context
                    .as_ref()
                    .is_none_or(|context| context.dispose().is_success())
                {
                    record.status = AgentProjectionStatus::CleanupBlocked;
                    ChildDisposal::Blocked {
                        projection_changed: !was_cleanup_blocked,
                        reason: "Agent child capability cleanup is pending",
                    }
                } else {
                    let parent_agent_id = record.parent_agent_id;
                    record.context = None;
                    record.lifecycle = ChildLifecycle::Disposed;
                    record.status = record
                        .terminal_status
                        .unwrap_or(AgentProjectionStatus::Cancelled);
                    ChildDisposal::Converged(parent_agent_id)
                }
            };
            match disposal {
                ChildDisposal::Blocked {
                    projection_changed,
                    reason,
                } => {
                    first_error.get_or_insert(AgentRuntimeError::Shutdown(reason.to_string()));
                    if projection_changed {
                        self.projection_revision = self.projection_revision.saturating_add(1);
                        self.publish_child_facts(agent_id, false);
                    }
                }
                ChildDisposal::Converged(parent_agent_id) => {
                    if !intent.retain_terminal_projection {
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
                    self.publish_child_facts(agent_id, false);
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
                self.projection_revision = self.projection_revision.saturating_add(1);
                self.publish_overview_remove(agent_id);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    /// Terminal 定格后的 settle：runtime/context 不再销毁，作为后续 followup turn 的
    /// 宿主保留；投影定格为 terminal_status，并发额度随之释放。settled 超限按最旧
    /// 淘汰，未收敛的显式清理在此以同一 owner 重试。
    fn settle_terminal_children(&mut self) {
        self.retry_disposing_children();
        self.freeze_settled_children();
        self.evict_settled_children_over_limit();
    }

    /// 把已成立 terminal 事实的 Active child 转入 Settled。
    ///
    /// settle 是纯投影定格，不触碰 runtime/context，因此不产生新的 revision。
    fn freeze_settled_children(&mut self) {
        let settled_ids = self
            .children
            .iter()
            .filter(|(_, record)| {
                matches!(record.lifecycle, ChildLifecycle::Active)
                    && record.terminal_status.is_some()
            })
            .map(|(agent_id, _)| *agent_id)
            .collect::<Vec<_>>();
        for agent_id in settled_ids {
            if let Some(record) = self.children.get_mut(&agent_id) {
                record.lifecycle = ChildLifecycle::Settled;
                record.status = record
                    .terminal_status
                    .expect("settled child must carry a terminal status");
            }
        }
    }

    /// 显式清理未收敛的 child 以同一 owner 幂等重试；descendants-first 让被后代
    /// 阻塞的 parent 在同一 pass 内收敛。
    fn retry_disposing_children(&mut self) {
        if !self
            .children
            .values()
            .any(|record| matches!(record.lifecycle, ChildLifecycle::Disposing))
        {
            return;
        }
        let pending = self
            .registry_disposal_order()
            .into_iter()
            .filter_map(|agent_id| {
                self.children.get(&agent_id).and_then(|record| {
                    matches!(record.lifecycle, ChildLifecycle::Disposing)
                        .then_some((agent_id, record.disposal_intent))
                })
            })
            .collect::<Vec<_>>();
        for (agent_id, intent) in pending {
            let _ = self.dispose_child_ids(vec![agent_id], intent);
        }
    }

    /// settled 数量超过上限时按最旧淘汰。agent id 按分配单调递增，BTreeMap 顺序即
    /// 创建顺序；淘汰复用显式 stop 的完整清理路径（launch-group 投影行按既有语义保留）。
    fn evict_settled_children_over_limit(&mut self) {
        while self.settled_child_count() > MAX_SETTLED_CHILD_AGENTS {
            let Some(oldest) = self
                .children
                .iter()
                .find(|(_, record)| matches!(record.lifecycle, ChildLifecycle::Settled))
                .map(|(agent_id, _)| *agent_id)
            else {
                break;
            };
            if self.stop_child(oldest).is_err() {
                // CleanupBlocked：owner 保留，本 pass 停止淘汰，下轮 settle 重试。
                break;
            }
        }
    }

    fn settled_child_count(&self) -> usize {
        self.children
            .values()
            .filter(|record| matches!(record.lifecycle, ChildLifecycle::Settled))
            .count()
    }

    /// 该 child 所属 launch group 的 completion 是否仍在等待。group completion 读取
    /// registry 内全部 staged child 行，等待期间任一成员行都不可销毁——删除会让
    /// completion 永远无法凑齐（等待方挂死）。过期清扫与手动删除共用本谓词。
    fn launch_group_completion_pending(&self, record: &ChildAgentRecord) -> bool {
        record
            .launch_group_id
            .is_some_and(|group_id| self.group_waiters.contains_key(&group_id))
    }

    /// settled child 的过期清扫：终态定格超过 [`SETTLED_CHILD_AUTO_DESTROY_AFTER_MS`]
    /// 的 child 走完整 delete 路径（`dispose_child_ids` + Remove delta，与用户删除
    /// settled 投影行同路）。生命周期收敛语义下 waiter 以 `TargetUnavailable` 结算。
    ///
    /// 顺序约束：只在 group waiter 结算之后执行——所属 launch group 仍在等待的
    /// child 先不清扫，待 waiter 结算后的下一次 drain 回收。幂等：CleanupBlocked
    /// 的 owner 保留，由既有 settle pass 重试收敛。
    fn evict_expired_settled_children(&mut self) {
        let now_ms = runtime_domain::time::unix_timestamp_ms().unwrap_or(0);
        let expired_ids = self
            .children
            .iter()
            .filter(|(_, record)| {
                matches!(record.lifecycle, ChildLifecycle::Settled)
                    && record.settled_at_ms.is_some_and(|settled_at| {
                        now_ms - settled_at >= SETTLED_CHILD_AUTO_DESTROY_AFTER_MS
                    })
                    && !self.launch_group_completion_pending(record)
            })
            .map(|(agent_id, _)| *agent_id)
            .collect::<Vec<_>>();
        if expired_ids.is_empty() {
            return;
        }
        let _ = self.dispose_child_ids(expired_ids, ChildDisposalIntent::default());
    }

    /// generation replacement 边界对 settled child 立即完整清理（连同 active 后代，
    /// 保持 descendants-first 顺序）；失败则拒绝切换，旧 generation 保持 authority。
    fn dispose_settled_children(&mut self) -> Result<(), AgentRuntimeError> {
        let settled_roots = self
            .children
            .iter()
            .filter(|(_, record)| matches!(record.lifecycle, ChildLifecycle::Settled))
            .map(|(agent_id, _)| *agent_id)
            .collect::<Vec<_>>();
        if settled_roots.is_empty() {
            return Ok(());
        }
        let mut seen = BTreeSet::new();
        let mut subtree_ids = Vec::new();
        for root in settled_roots {
            for agent_id in self.subtree_ids(root) {
                if seen.insert(agent_id) {
                    subtree_ids.push(agent_id);
                }
            }
        }
        self.dispose_child_ids(subtree_ids, ChildDisposalIntent::default())
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

/// host 在 launch 边界恒注入的 child worker 身份指令。
///
/// sessionless child 没有 system prompt，报告与收尾语义只能由 provider request 内的
/// 守则建立：最终 assistant 消息会被单独摘取交付，child 必须自知任务边界并如实汇报。
pub(super) const CHILD_AGENT_IDENTITY_INSTRUCTIONS: &str = "You are a child agent dispatched for one specific task.

- Complete exactly the task in the message above. Do not broaden its scope.
- When the task is done, finish your turn. Do not ask for follow-up instructions or wait for further direction.
- Your final message is your report to the dispatching agent. Make it self-contained: state conclusions, findings, and deliverables directly — it is extracted and read on its own, outside this conversation.
- Report honestly. Mark unverified conclusions as unverified; if you cannot complete the task, say so and explain why instead of guessing.";

fn child_turn_request(
    target: &RuntimeTarget,
    request: &runtime_domain::agent::AgentLaunchRequest,
) -> AgentTurnRequest {
    let RuntimeTarget::Provider(target) = target;
    // 身份指令是 host 在 launch 边界恒注入的 child 语义，caller 没有覆盖通道；
    // objective 是唯一的 caller 任务输入。
    AgentTurnRequest::from_conversation_request(ConversationTurnRequest::new_user_text(
        target.provider_id.clone(),
        target.model_id.clone(),
        request.objective().as_str(),
    ))
    .with_direct_instructions(AgentInstructions::new(CHILD_AGENT_IDENTITY_INSTRUCTIONS))
}

/// followup turn 的 user 消息即消息正文；不带 direct instructions——launch 的
/// control 指令只属于 launch 边界，followup 依赖 child 保留的 conversation 上下文。
fn child_followup_turn_request(
    target: &RuntimeTarget,
    message: &AgentChildMessage,
) -> AgentTurnRequest {
    let RuntimeTarget::Provider(target) = target;
    AgentTurnRequest::from_conversation_request(ConversationTurnRequest::new_user_text(
        target.provider_id.clone(),
        target.model_id.clone(),
        message.as_str(),
    ))
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

/// transcript 中最近一条有非空正文的 committed assistant 内容；streaming partial 永不
/// 进入，因此它是 child 产出的唯一 committed 来源。空正文收尾（纯 tool_use turn、
/// reasoning-only 收尾）不代表报告，继续回溯更早的非空 item；全部为空时返回 `None`。
fn latest_committed_assistant_content(record: &ChildAgentRecord) -> Option<&str> {
    record.transcript.iter().rev().find_map(|item| match item {
        AgentTranscriptItem::Assistant { content } => {
            (!content.trim().is_empty()).then_some(content.as_str())
        }
        _ => None,
    })
}

/// 父 Agent tool result 携带的完整报告字符上限；超出时截断并追加固定 note。
const AGENT_REPORT_MAX_CHARS: usize = 16 * 1024;

/// child completion/delivery 信封的耗时档位格式。
///
/// 不足一分钟只输出整秒（`16s`）；不足一小时输出分 + 两位秒（`2m 05s`）；一小时及
/// 以上只保留时 + 两位分（`1h 05m`）。毫秒向下取整到秒；输出是固定档位文本，
/// 不携带计时原始数值。
fn format_child_duration(duration_ms: u64) -> String {
    let total_secs = duration_ms / 1_000;
    if total_secs < 60 {
        format!("{total_secs}s")
    } else if total_secs < 3_600 {
        format!("{}m {:02}s", total_secs / 60, total_secs % 60)
    } else {
        format!("{}h {:02}m", total_secs / 3_600, total_secs % 3_600 / 60)
    }
}

/// completion/delivery tool result 的报告信封取值。
///
/// `summary`（240 列单行）与完整报告是两个数据面：摘要服务 TUI 面板与 preview，
/// 报告只进入父 Agent 可见的 tool result。reasoning-only 收尾没有可回传的正文，
/// `report` 为 `None`，由 summary 保留空占位语义。
fn child_report_envelope(record: &ChildAgentRecord, now_ms: i64) -> AgentReportEnvelope {
    let (report, truncated) = match latest_committed_assistant_content(record) {
        Some(content) => {
            let char_count = content.chars().count();
            if char_count <= AGENT_REPORT_MAX_CHARS {
                (Some(content.to_string()), false)
            } else {
                let truncated_body: String = content.chars().take(AGENT_REPORT_MAX_CHARS).collect();
                (
                    Some(format!(
                        "{truncated_body}\n\n[report truncated: full length {char_count} chars]"
                    )),
                    true,
                )
            }
        }
        None => (None, false),
    };
    AgentReportEnvelope {
        report,
        truncated,
        tokens: (record.token_usage > 0).then_some(record.token_usage),
        tool_uses: (record.tool_uses > 0).then_some(record.tool_uses),
        duration: record.elapsed_ms_at(now_ms).map(format_child_duration),
    }
}

/// child terminal outcome 的 delivery-safe 摘要：Completed 取最近一条有非空正文的
/// committed assistant 内容，Cancelled 按定格来源区分显式停止与自然取消，其余 status
/// 只有固定占位文本。
fn safe_outcome_summary(record: &ChildAgentRecord) -> Option<AgentOutcomeSummary> {
    match record.terminal_status {
        Some(AgentProjectionStatus::Completed) => latest_committed_assistant_content(record)
            .and_then(|content| AgentOutcomeSummary::new(content).ok())
            .or_else(|| AgentOutcomeSummary::new(CHILD_COMPLETED_WITHOUT_REPORT_TEXT).ok()),
        Some(AgentProjectionStatus::Cancelled) => {
            let text = if record.terminal_stopped_by_request {
                CHILD_STOPPED_BY_REQUEST_TEXT
            } else {
                CHILD_CANCELLED_TEXT
            };
            AgentOutcomeSummary::new(text).ok()
        }
        _ => AgentOutcomeSummary::new("Child Agent failed").ok(),
    }
}

fn freeze_pending_outcome(agent_id: AgentId, record: &mut ChildAgentRecord, now_ms: i64) {
    if record.pending_outcome.is_some() {
        return;
    }
    let Some(terminal_status) = record.terminal_status else {
        return;
    };
    record.pending_outcome = Some(runtime_domain::agent::AgentOutcomeSnapshot {
        agent_id,
        title: record.title.clone(),
        // group_id 回答"该 outcome 完成哪个 launch group"：followup turn 的 outcome
        // 是消息触发的独立 durable fact（group completion 已随首个 terminal 交付），
        // 不再归属 launch group；parent 关联保持不变。
        group_id: record
            .launch_group_id
            .filter(|_| !record.current_turn_is_followup),
        parent_agent_id: Some(record.parent_agent_id),
        parent_turn_id: record.parent_turn_id,
        outcome: outcome_for_status(Some(terminal_status)),
        occurred_at_ms: runtime_domain::time::unix_timestamp_ms().unwrap_or(0),
        // elapsed 已在 terminal 定格处暂停，这里读到的是定格累计值；无计时起点的
        // 投影（resume 恢复）保持 None。
        duration_ms: record.elapsed_ms_at(now_ms),
        summary: safe_outcome_summary(record),
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

fn apply_child_projection(record: &mut ChildAgentRecord, kind: &AgentEventKind, now_ms: i64) {
    match kind {
        AgentEventKind::Thinking { is_thinking } => {
            record.status = AgentProjectionStatus::Working;
            record.resume_elapsed_at(now_ms);
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
            record.resume_elapsed_at(now_ms);
        }
        AgentEventKind::OutputTokenEstimate { total_tokens }
        | AgentEventKind::InputTokenEstimate { total_tokens } => {
            record.status = AgentProjectionStatus::Working;
            record.resume_elapsed_at(now_ms);
            record.token_usage = record.token_usage.max(*total_tokens);
        }
        AgentEventKind::Retrying { .. } => {
            record.status = AgentProjectionStatus::Working;
            record.resume_elapsed_at(now_ms);
            // Provider messages are control/provider content at this boundary. The overview only
            // receives a fixed safe activity label, never the raw retry diagnostic.
            record.latest_activity = AgentActivitySummary::Retrying {
                summary: "Retrying".to_string(),
            };
        }
        AgentEventKind::ToolActivityStarted { activity } => {
            record.status = AgentProjectionStatus::Working;
            record.resume_elapsed_at(now_ms);
            record.tool_uses = record.tool_uses.saturating_add(1);
            // overview 直接展示 definition-owned 的归一化 label（如
            // "Read Cargo.toml"）；事件缺失 label 时退回固定占位。
            record.latest_activity = AgentActivitySummary::UsingTool {
                title: tool_activity_display_title(&activity.title),
            };
        }
        AgentEventKind::ToolActivityUpdated { .. } => {
            record.status = AgentProjectionStatus::Working;
            record.resume_elapsed_at(now_ms);
        }
        AgentEventKind::PermissionRequested { .. } => {
            record.status = AgentProjectionStatus::WaitingPermission;
            // 等待人为审批期间不计入 elapsed：冻结在进入等待前的值，
            // 恢复 Working 后从当前时刻继续累计。
            record.pause_elapsed_at(now_ms);
            record.latest_activity = AgentActivitySummary::WaitingPermission {
                summary: "Waiting for approval".to_string(),
            };
        }
        AgentEventKind::TurnFinished { .. } => {
            record.status = AgentProjectionStatus::Completed;
            record.terminal_status = Some(AgentProjectionStatus::Completed);
            // 终态后 elapsed 定格为终态时刻的值，不再随时间推进。
            record.freeze_terminal_at(now_ms);
            record.latest_activity = AgentActivitySummary::Idle;
        }
        AgentEventKind::TurnFailed { .. } => {
            record.status = AgentProjectionStatus::Failed;
            record.terminal_status = Some(AgentProjectionStatus::Failed);
            record.freeze_terminal_at(now_ms);
            record.latest_activity = AgentActivitySummary::Idle;
        }
        AgentEventKind::TurnInterrupted => {
            record.status = AgentProjectionStatus::Cancelled;
            record.terminal_status = Some(AgentProjectionStatus::Cancelled);
            record.freeze_terminal_at(now_ms);
            record.latest_activity = AgentActivitySummary::Idle;
        }
    }
}

/// child 的 delivery-safe overview row 投影；只读取 record 的安全字段。
/// elapsed 在运行中为实时差，等待 permission 与终态为定格的累计值。
fn overview_row_for_child(
    agent_id: &AgentId,
    record: &ChildAgentRecord,
    now_ms: i64,
) -> AgentOverviewRow {
    AgentOverviewRow {
        agent_id: *agent_id,
        title: record.title.clone(),
        status: record.status,
        latest_activity: record.latest_activity.clone(),
        elapsed_ms: record.elapsed_ms_at(now_ms),
        tool_uses: (record.tool_uses > 0).then_some(record.tool_uses),
        token_usage: (record.token_usage > 0).then_some(record.token_usage),
        settled_at_ms: record.settled_at_ms,
    }
}

/// 一次 per-agent observation 的聚合 delivery 视图：transcript 与 preview 共用 revision。
fn agent_view_snapshot_for_child(
    observation_id: AgentObservationId,
    agent_id: AgentId,
    record: &ChildAgentRecord,
    generation: AgentRuntimeGeneration,
    revision: AgentProjectionRevision,
    now_ms: i64,
) -> AgentViewSnapshot {
    let transcript = AgentTranscriptSnapshot {
        observation_id,
        generation,
        revision,
        agent_id,
        title: record.title.clone(),
        status: record.status,
        items: record.transcript.clone(),
    };
    let preview = AgentPreviewSnapshot {
        generation,
        revision,
        agent_id,
        title: record.title.clone(),
        status: record.status,
        latest_activity: record.latest_activity.clone(),
        elapsed_ms: record.elapsed_ms_at(now_ms),
        latest_committed_answer: latest_committed_assistant_content(record).map(str::to_string),
        permission: record.pending_permissions.front().cloned(),
    };
    AgentViewSnapshot {
        observation_id,
        generation,
        revision,
        transcript,
        preview,
    }
}

/// 把已通过 identity gate 的 permission fact 并入 authoritative FIFO。
///
/// duplicate request id 同 turn 内直接忽略；新的非 permission fact（tool activity /
/// 新 permission / terminal / interrupt）收敛已 Submitted 的 head。返回 queue 是否变化。
fn apply_child_permission_fact(
    agent_id: AgentId,
    record: &mut ChildAgentRecord,
    event: &AgentEvent,
) -> bool {
    match &event.kind {
        AgentEventKind::PermissionRequested { request } => {
            if record
                .pending_permissions
                .iter()
                .any(|entry| entry.target.request_id == request.request_id)
            {
                return false;
            }
            drop_submitted_head(record);
            let target = AgentPermissionTarget {
                agent_id,
                turn_id: record.turn_id,
                generation: record.generation,
                runtime_target: record
                    .target
                    .clone()
                    .unwrap_or_else(|| event.target.clone()),
                request_id: request.request_id.clone(),
            };
            record
                .pending_permissions
                .push_back(AgentPermissionRequest {
                    target,
                    request: request.clone(),
                    state: AgentPermissionState::Pending,
                    occurred_at_ms: runtime_domain::time::unix_timestamp_ms().unwrap_or(0),
                });
            true
        }
        AgentEventKind::ToolActivityStarted { .. } | AgentEventKind::ToolActivityUpdated { .. } => {
            drop_submitted_head(record)
        }
        AgentEventKind::TurnFinished { .. }
        | AgentEventKind::TurnFailed { .. }
        | AgentEventKind::TurnInterrupted => {
            let was_empty = record.pending_permissions.is_empty();
            record.pending_permissions.clear();
            !was_empty
        }
        _ => false,
    }
}

/// 收敛 FIFO head 的 Submitted entry；只有 head 是 Submitted 时才移除。
fn drop_submitted_head(record: &mut ChildAgentRecord) -> bool {
    if record
        .pending_permissions
        .front()
        .is_some_and(|entry| entry.state == AgentPermissionState::Submitted)
    {
        record.pending_permissions.pop_front();
        true
    } else {
        false
    }
}

/// 只累积 committed transcript 事实：tool activity 折叠与 terminal committed answer。
///
/// 未提交的 AssistantDelta/ReasoningDelta 永不进入；terminal 后的 late event 已被
/// identity/terminal gate 拒绝，因此 transcript 是 exactly-once 的。
fn apply_child_transcript_fact(record: &mut ChildAgentRecord, kind: &AgentEventKind) {
    match kind {
        AgentEventKind::ToolActivityStarted { activity } => {
            upsert_transcript_tool_item(
                record,
                &activity.activity_id,
                Some(&activity.title),
                Some(delivery_safe_tool_content(&activity.content)),
            );
        }
        AgentEventKind::ToolActivityUpdated { update } => {
            upsert_transcript_tool_item(
                record,
                &update.activity_id,
                update.title.as_deref(),
                update.content.as_deref().map(delivery_safe_tool_content),
            );
        }
        AgentEventKind::TurnFinished { response, .. } => {
            record.transcript.push(AgentTranscriptItem::Assistant {
                content: response.text_content(),
            });
        }
        _ => {}
    }
}

/// tool activity label 的投影取值：label 交付面只取 trim 后的非空文本，
/// 缺失时退回固定占位（不读取 raw_input/raw_output）。
fn tool_activity_display_title(title: &str) -> String {
    let title = title.trim();
    if title.is_empty() {
        "Using tool".to_string()
    } else {
        title.to_string()
    }
}

/// 把 tool activity 的 Started/Updated 折叠为同一 transcript item。
fn upsert_transcript_tool_item(
    record: &mut ChildAgentRecord,
    activity_id: &str,
    title: Option<&str>,
    content: Option<String>,
) {
    if let Some(&index) = record.transcript_tool_items.get(activity_id)
        && let Some(AgentTranscriptItem::Tool {
            title: item_title,
            content: item_content,
        }) = record.transcript.get_mut(index)
    {
        if let Some(title) = title {
            *item_title = title.to_string();
        }
        if let Some(content) = content {
            *item_content = content;
        }
        return;
    }
    let index = record.transcript.len();
    record.transcript.push(AgentTranscriptItem::Tool {
        title: title.unwrap_or("Using tool").to_string(),
        content: content.unwrap_or_default(),
    });
    record
        .transcript_tool_items
        .insert(activity_id.to_string(), index);
}

/// tool activity content 的 delivery-safe 文本投影；`raw_input/raw_output` 永不读取。
fn delivery_safe_tool_content(content: &[RuntimeToolActivityContent]) -> String {
    content
        .iter()
        .filter_map(|item| match item {
            RuntimeToolActivityContent::Text(text) => Some(text.clone()),
            RuntimeToolActivityContent::Diff { path, new_text, .. } => {
                Some(format!("{path}\n{new_text}"))
            }
            RuntimeToolActivityContent::Resource {
                text: Some(text), ..
            } => Some(text.clone()),
            RuntimeToolActivityContent::ResourceLink { name, .. } => Some(name.clone()),
            _ => None,
        })
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
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

    /// 携带预置 events 并记录 shutdown 次数的 child runtime fixture。
    struct ShutdownCountingRuntime {
        events: Vec<AgentEvent>,
        shutdown_calls: Arc<AtomicUsize>,
    }

    impl AgentRuntime for ShutdownCountingRuntime {
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
            self.shutdown_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    impl AgentRuntimePort for ShutdownCountingRuntime {
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

    fn registered_child_with_terminal_event(kind: AgentEventKind) -> (AgentOrchestrator, AgentId) {
        let agent_id = AgentId::new(2);
        let turn_id = AgentTurnId::new(7);
        let target = RuntimeTarget::provider("local", "qwen3");
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        orchestrator.register_child_for_test(
            agent_id,
            AgentId::MAIN,
            turn_id,
            test_title("group child"),
            test_context("group-child"),
            Box::new(StubMainRuntime {
                events: vec![child_event(agent_id, turn_id, &target, kind)],
            }),
        );
        (orchestrator, agent_id)
    }

    fn completed_group_child(kind: AgentEventKind) -> AgentChildCompletion {
        let (mut orchestrator, agent_id) = registered_child_with_terminal_event(kind);
        let (response_sender, response_receiver) = oneshot::channel();
        orchestrator.group_waiters.insert(
            AgentLaunchGroupId::new(1),
            GroupWaiter {
                parent_agent_id: AgentId::MAIN,
                child_ids: vec![agent_id],
                response: response_sender,
            },
        );

        let _ = orchestrator.drain_child_events();

        let completion = response_receiver
            .blocking_recv()
            .expect("group waiter should receive a completion")
            .expect("group completion should succeed");
        assert_eq!(completion.children.len(), 1);
        assert_eq!(completion.children[0].agent_id, agent_id);
        completion.children[0].clone()
    }

    fn finished_turn_event(answer_text: &str) -> AgentEventKind {
        AgentEventKind::TurnFinished {
            response: runtime_domain::session::ConversationResponse::assistant_text(answer_text),
            metrics: None,
            context_usage: None,
        }
    }

    #[test]
    fn group_completion_carries_child_committed_answer() {
        let child = completed_group_child(finished_turn_event("  final researched   answer  "));
        // 报告保留 committed 原文（尾部空白随 text_content 归一）；单行摘要只进入
        // outcome snapshot，不在 completion 信封里。
        assert_eq!(child.report.as_deref(), Some("  final researched   answer"));
    }

    #[test]
    fn completed_outcome_snapshot_keeps_the_delivery_safe_summary() {
        let (mut orchestrator, agent_id) = registered_child_with_terminal_event(
            finished_turn_event("  final researched   answer  "),
        );
        let _ = orchestrator.drain_child_events();
        let snapshot = outcome_facts(&orchestrator.drain_projection_events())
            .into_iter()
            .find(|snapshot| snapshot.agent_id == agent_id)
            .expect("completed child should project an outcome fact");
        assert_eq!(snapshot.outcome, AgentOutcome::Completed);
        assert_eq!(
            snapshot.summary.as_ref().map(AgentOutcomeSummary::as_str),
            Some("final researched answer")
        );
    }

    #[test]
    fn tool_activity_started_projects_the_normalized_activity_title() {
        // overview 的活动提示直接展示事件携带的归一化 label（如
        // "Read Cargo.toml"）；事件缺失 label 时退回固定占位。
        for (event_title, projected_title) in
            [("Read Cargo.toml", "Read Cargo.toml"), ("  ", "Using tool")]
        {
            let agent_id = AgentId::new(2);
            let turn_id = AgentTurnId::new(7);
            let target = RuntimeTarget::provider("local", "qwen3");
            let mut orchestrator =
                AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
            orchestrator.register_child_for_test(
                agent_id,
                AgentId::MAIN,
                turn_id,
                test_title("working child"),
                test_context("working-child"),
                Box::new(StubMainRuntime {
                    events: vec![child_event(
                        agent_id,
                        turn_id,
                        &target,
                        AgentEventKind::ToolActivityStarted {
                            activity: runtime_domain::session::RuntimeToolActivity {
                                activity_id: "tool-1".to_string(),
                                title: event_title.to_string(),
                                kind: runtime_domain::session::RuntimeToolKind::Read,
                                status:
                                    runtime_domain::session::RuntimeToolActivityStatus::InProgress,
                                content: Vec::new(),
                                locations: Vec::new(),
                                raw_input: None,
                                raw_output: None,
                            },
                        },
                    )],
                }),
            );

            let _ = orchestrator.drain_child_events();
            orchestrator.observe_agents(AgentObservationRequestId::new(1));
            let rows = orchestrator
                .drain_projection_events()
                .into_iter()
                .find_map(|event| match event {
                    AgentProjectionEvent::AgentsOverviewSnapshotLoaded { snapshot, .. } => {
                        Some(snapshot.rows)
                    }
                    _ => None,
                })
                .expect("overview observation should deliver a snapshot");
            assert_eq!(rows.len(), 1);
            assert_eq!(
                rows[0].latest_activity,
                AgentActivitySummary::UsingTool {
                    title: projected_title.to_string()
                },
                "event title {event_title:?}"
            );
        }
    }

    #[test]
    fn outcome_fact_carries_the_frozen_terminal_elapsed() {
        // 有计时起点的 child：outcome fact 携带 terminal 定格的累计 elapsed；
        // 无计时起点的投影（test 注册未标记计时）保持 None。
        let (mut orchestrator, agent_id) =
            registered_child_with_terminal_event(finished_turn_event("terminal answer"));
        orchestrator.mark_child_elapsed_started_for_test(agent_id);
        let _ = orchestrator.drain_child_events();
        let timed = outcome_facts(&orchestrator.drain_projection_events())
            .into_iter()
            .find(|snapshot| snapshot.agent_id == agent_id)
            .expect("timed child should project an outcome fact");
        assert!(
            timed.duration_ms.is_some(),
            "timed child should carry the frozen elapsed"
        );

        let (mut orchestrator, agent_id) =
            registered_child_with_terminal_event(finished_turn_event("untimed answer"));
        let _ = orchestrator.drain_child_events();
        let untimed = outcome_facts(&orchestrator.drain_projection_events())
            .into_iter()
            .find(|snapshot| snapshot.agent_id == agent_id)
            .expect("untimed child should project an outcome fact");
        assert_eq!(untimed.duration_ms, None);
    }

    #[test]
    fn format_child_duration_uses_tiered_human_readable_format() {
        // 档位边界：秒档毫秒向下取整；分秒档秒两位补零；小时档丢弃秒。
        for (duration_ms, expected) in [
            (0, "0s"),
            (16_140, "16s"),
            (59_999, "59s"),
            (60_000, "1m 00s"),
            (125_000, "2m 05s"),
            (3_599_999, "59m 59s"),
            (3_600_000, "1h 00m"),
            (3_900_000, "1h 05m"),
            (125_000_000, "34h 43m"),
        ] {
            assert_eq!(
                format_child_duration(duration_ms),
                expected,
                "duration {duration_ms}ms"
            );
        }
    }

    #[test]
    fn overview_row_settled_at_ms_tracks_the_terminal_cycle() {
        // Active 期间没有 terminal 定格。
        let (mut orchestrator, agent_id) =
            registered_child_with_terminal_event(AgentEventKind::Thinking { is_thinking: true });
        let _ = orchestrator.drain_child_events();
        orchestrator.observe_agents(AgentObservationRequestId::new(1));
        let rows = loaded_overview_rows(orchestrator.drain_projection_events());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].agent_id, agent_id);
        assert_eq!(rows[0].settled_at_ms, None);

        // terminal 定格写入 settled 时刻，并透传到 overview 投影。
        let agent_id = AgentId::new(2);
        let turn_id = AgentTurnId::new(agent_id.get());
        let target = RuntimeTarget::provider("local", "qwen3");
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        orchestrator.register_child_for_test(
            agent_id,
            AgentId::MAIN,
            turn_id,
            test_title("settled cycle child"),
            test_context("settled-cycle-child"),
            Box::new(StubMainRuntime {
                events: vec![child_event(
                    agent_id,
                    turn_id,
                    &target,
                    finished_turn_event("first answer"),
                )],
            }),
        );
        orchestrator.mark_child_target_for_test(agent_id, target);
        let _ = orchestrator.drain_child_events();
        orchestrator.observe_agents(AgentObservationRequestId::new(1));
        let rows = loaded_overview_rows(orchestrator.drain_projection_events());
        assert_eq!(rows.len(), 1);
        assert!(rows[0].settled_at_ms.is_some());

        // followup turn 重新起算：settled 时刻随上一个 terminal 周期结束清除。
        orchestrator
            .send_child_message(agent_id, child_message("refine the answer"))
            .expect("settled child should start the followup turn");
        orchestrator.observe_agents(AgentObservationRequestId::new(2));
        let rows = loaded_overview_rows(orchestrator.drain_projection_events());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].settled_at_ms, None);
    }

    #[test]
    fn explicit_stop_freezes_settled_at_on_the_retained_projection() {
        // launch-group child 的显式 stop 保留投影行：定格时刻随 Cancelled 一并写入。
        let agent_id = AgentId::new(2);
        let turn_id = AgentTurnId::new(agent_id.get());
        let target = RuntimeTarget::provider("local", "qwen3");
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        orchestrator.register_child_for_test(
            agent_id,
            AgentId::MAIN,
            turn_id,
            test_title("stopped settled child"),
            test_context("stopped-settled-child"),
            Box::new(StubMainRuntime::default()),
        );
        orchestrator.mark_child_target_for_test(agent_id, target);
        orchestrator.mark_child_launch_group_for_test(agent_id, AgentLaunchGroupId::new(1));

        orchestrator
            .stop_child(agent_id)
            .expect("explicit stop should converge");

        orchestrator.observe_agents(AgentObservationRequestId::new(1));
        let rows = loaded_overview_rows(orchestrator.drain_projection_events());
        assert_eq!(rows.len(), 1);
        assert!(rows[0].settled_at_ms.is_some());
    }

    /// 把 test child 的 terminal 定格时刻回拨，模拟 settled 已超过自动销毁阈值。
    fn backdate_child_settled_at(
        orchestrator: &mut AgentOrchestrator,
        agent_id: AgentId,
        backdate_ms: i64,
    ) {
        let Some(record) = orchestrator.children.get_mut(&agent_id) else {
            panic!("test child {agent_id:?} should be registered");
        };
        let settled_at = record
            .settled_at_ms
            .expect("settled child must carry a terminal freeze timestamp");
        record.settled_at_ms = Some(settled_at - backdate_ms);
    }

    #[test]
    fn expired_settled_children_are_auto_destroyed_on_drain() {
        let agent_id = AgentId::new(2);
        let turn_id = AgentTurnId::new(agent_id.get());
        let target = RuntimeTarget::provider("local", "qwen3");
        let shutdown_calls = Arc::new(AtomicUsize::new(0));
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        orchestrator.register_child_for_test(
            agent_id,
            AgentId::MAIN,
            turn_id,
            test_title("auto destroy"),
            test_context("auto-destroy"),
            Box::new(ShutdownCountingRuntime {
                events: vec![child_event(
                    agent_id,
                    turn_id,
                    &target,
                    finished_turn_event("done"),
                )],
                shutdown_calls: Arc::clone(&shutdown_calls),
            }),
        );
        let _ = orchestrator.drain_child_events();
        assert_eq!(
            orchestrator.child_count(),
            1,
            "recently settled child must stay as the followup host"
        );
        // Remove delta 只发布给存活 observation；先建立 overview observation。
        orchestrator.observe_agents(AgentObservationRequestId::new(1));
        let _ = orchestrator.drain_projection_events();

        backdate_child_settled_at(
            &mut orchestrator,
            agent_id,
            SETTLED_CHILD_AUTO_DESTROY_AFTER_MS,
        );
        let _ = orchestrator.drain_child_events();

        assert_eq!(
            orchestrator.child_count(),
            0,
            "expired settled child must be auto destroyed through the delete path"
        );
        assert_eq!(
            shutdown_calls.load(Ordering::SeqCst),
            1,
            "auto destroy must run the full runtime cleanup"
        );
        let events = orchestrator.drain_projection_events();
        assert!(
            events.iter().any(|event| matches!(
                event,
                AgentProjectionEvent::AgentsOverviewUpdated { delta }
                    if matches!(
                        delta.kind,
                        AgentOverviewDeltaKind::Remove { agent_id } if agent_id == AgentId::new(2)
                    )
            )),
            "auto destroy must publish an overview Remove delta: {events:?}"
        );

        // 幂等：child 已从 registry 移除，后续 drain 不再产生事件或重复 Remove。
        assert!(orchestrator.drain_child_events().is_empty());
        assert!(orchestrator.drain_projection_events().is_empty());
    }

    #[test]
    fn recently_settled_children_survive_the_sweep() {
        let (mut orchestrator, agent_id) =
            registered_child_with_terminal_event(finished_turn_event("still warm"));
        let _ = orchestrator.drain_child_events();
        orchestrator.observe_agents(AgentObservationRequestId::new(1));
        let _ = orchestrator.drain_projection_events();

        // 阈值内：回拨一半窗口，避免 drain 间隔的毫秒漂移越过阈值造成 flake。
        backdate_child_settled_at(
            &mut orchestrator,
            agent_id,
            SETTLED_CHILD_AUTO_DESTROY_AFTER_MS / 2,
        );
        let _ = orchestrator.drain_child_events();

        assert_eq!(orchestrator.child_count(), 1);
        assert!(orchestrator.child_has_authority(agent_id));
        let events = orchestrator.drain_projection_events();
        assert!(
            !events.iter().any(|event| matches!(
                event,
                AgentProjectionEvent::AgentsOverviewUpdated { delta }
                    if matches!(delta.kind, AgentOverviewDeltaKind::Remove { .. })
            )),
            "within-threshold settled child must not be removed: {events:?}"
        );
    }

    #[test]
    fn followup_turn_resets_the_auto_destroy_window() {
        let agent_id = AgentId::new(2);
        let turn_id = AgentTurnId::new(agent_id.get());
        let target = RuntimeTarget::provider("local", "qwen3");
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        orchestrator.register_child_for_test(
            agent_id,
            AgentId::MAIN,
            turn_id,
            test_title("followup window"),
            test_context("followup-window"),
            Box::new(ShutdownCountingRuntime {
                events: vec![child_event(
                    agent_id,
                    turn_id,
                    &target,
                    finished_turn_event("first answer"),
                )],
                shutdown_calls: Arc::new(AtomicUsize::new(0)),
            }),
        );
        orchestrator.mark_child_target_for_test(agent_id, target);
        let _ = orchestrator.drain_child_events();

        // 名义上已过期，但 followup turn 开启了新的 terminal 周期。
        backdate_child_settled_at(
            &mut orchestrator,
            agent_id,
            SETTLED_CHILD_AUTO_DESTROY_AFTER_MS,
        );
        orchestrator
            .send_child_message(agent_id, child_message("refine the answer"))
            .expect("settled child should start the followup turn");

        let _ = orchestrator.drain_child_events();

        assert_eq!(
            orchestrator.child_count(),
            1,
            "a child mid-followup must not be swept"
        );
        assert_eq!(
            orchestrator.child_status(agent_id),
            Some(AgentProjectionStatus::Pending),
            "followup turn takes the child back to Active"
        );
    }

    #[test]
    fn expired_settled_child_drops_queued_messages_instead_of_followup() {
        let caller_id = AgentId::new(2);
        let target_id = AgentId::new(3);
        let target = RuntimeTarget::provider("local", "qwen3");
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        let caller_context =
            register_message_caller_child(&mut orchestrator, caller_id, AgentTurnId::new(20));
        let (runtime, _staged, _dispatched, submitted_turns) =
            ScriptedChildRuntime::new(vec![child_event(
                target_id,
                AgentTurnId::new(target_id.get()),
                &target,
                finished_turn_event("settled answer"),
            )]);
        orchestrator.register_child_for_test(
            target_id,
            caller_id,
            AgentTurnId::new(target_id.get()),
            test_title("expired queue"),
            test_context("expired-queue"),
            Box::new(runtime),
        );
        orchestrator.mark_child_target_for_test(target_id, target);
        let _ = orchestrator.drain_child_events();

        // durable outcome 未收敛（append 持续失败）时消息只能排队等待；这里直接
        // 置回未持久化态模拟该路径，persist pass 因无 pending snapshot 而跳过。
        {
            let Some(record) = orchestrator.children.get_mut(&target_id) else {
                panic!("target child should be registered");
            };
            record.outcome_persisted = false;
        }
        let mut receiver = child_caller_message_request(
            &mut orchestrator,
            caller_id,
            &caller_context,
            AgentTurnId::new(20),
            target_id,
            "late follow-up",
        );
        backdate_child_settled_at(
            &mut orchestrator,
            target_id,
            SETTLED_CHILD_AUTO_DESTROY_AFTER_MS,
        );

        let _ = orchestrator.drain_child_events();

        assert_eq!(
            orchestrator.child_count(),
            1,
            "the non-child caller row must survive; only the expired target is destroyed"
        );
        let failure = receiver
            .try_recv()
            .expect("auto destroy must settle the pending message waiter")
            .expect_err("queued message on an expired child must fail closed");
        assert_eq!(failure, SendAgentMessageFailure::TargetUnavailable);
        assert!(
            submitted_turns
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty(),
            "expired child must not consume the queued message as a followup turn"
        );
    }

    #[test]
    fn pending_group_waiter_defers_the_expired_settled_sweep() {
        let settled_id = AgentId::new(2);
        let running_id = AgentId::new(3);
        let target = RuntimeTarget::provider("local", "qwen3");
        let group_id = AgentLaunchGroupId::new(1);
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        orchestrator.register_child_for_test(
            settled_id,
            AgentId::MAIN,
            AgentTurnId::new(settled_id.get()),
            test_title("settled sibling"),
            test_context("settled-sibling"),
            Box::new(ShutdownCountingRuntime {
                events: vec![child_event(
                    settled_id,
                    AgentTurnId::new(settled_id.get()),
                    &target,
                    finished_turn_event("early answer"),
                )],
                shutdown_calls: Arc::new(AtomicUsize::new(0)),
            }),
        );
        orchestrator.mark_child_launch_group_for_test(settled_id, group_id);
        let (running_runtime, staged_running, _dispatched, _submitted) =
            ScriptedChildRuntime::new(Vec::new());
        orchestrator.register_child_for_test(
            running_id,
            AgentId::MAIN,
            AgentTurnId::new(running_id.get()),
            test_title("running sibling"),
            test_context("running-sibling"),
            Box::new(running_runtime),
        );
        orchestrator.mark_child_launch_group_for_test(running_id, group_id);
        let _ = orchestrator.drain_child_events();

        let (response, response_receiver) = oneshot::channel();
        orchestrator.group_waiters.insert(
            group_id,
            GroupWaiter {
                parent_agent_id: AgentId::MAIN,
                child_ids: vec![settled_id, running_id],
                response,
            },
        );
        orchestrator.observe_agents(AgentObservationRequestId::new(1));
        let _ = orchestrator.drain_projection_events();
        backdate_child_settled_at(
            &mut orchestrator,
            settled_id,
            SETTLED_CHILD_AUTO_DESTROY_AFTER_MS,
        );

        // 兄弟 child 未终态：group completion 仍需要已 settled child 的 record，
        // 过期清扫必须让位。
        let _ = orchestrator.drain_child_events();
        assert_eq!(
            orchestrator.child_count(),
            2,
            "a settled child in a pending launch group must not be swept"
        );

        // 兄弟 child 终态：同一 drain 内 waiter 先结算，随后清扫回收过期 child。
        staged_running
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(child_event(
                running_id,
                AgentTurnId::new(running_id.get()),
                &target,
                finished_turn_event("late answer"),
            ));
        let _ = orchestrator.drain_child_events();

        let completion = response_receiver
            .blocking_recv()
            .expect("group waiter should settle when the last child turns terminal")
            .expect("group completion should succeed");
        assert_eq!(completion.children.len(), 2);
        assert_eq!(orchestrator.child_count(), 1);
        let events = orchestrator.drain_projection_events();
        assert!(
            events.iter().any(|event| matches!(
                event,
                AgentProjectionEvent::AgentsOverviewUpdated { delta }
                    if matches!(
                        delta.kind,
                        AgentOverviewDeltaKind::Remove { agent_id } if agent_id == settled_id
                    )
            )),
            "the expired child must be swept once the group waiter settles: {events:?}"
        );
    }

    #[test]
    fn group_completion_without_committed_answer_uses_placeholder() {
        let child = completed_group_child(finished_turn_event(""));
        // 空正文收尾没有可回传的报告：信封显式标注为空，占位文本只进入 snapshot 摘要。
        assert_eq!(child.report, None);
        assert!(!child.truncated);

        let (mut orchestrator, agent_id) =
            registered_child_with_terminal_event(finished_turn_event(""));
        let _ = orchestrator.drain_child_events();
        let snapshot = outcome_facts(&orchestrator.drain_projection_events())
            .into_iter()
            .find(|snapshot| snapshot.agent_id == agent_id)
            .expect("completed child should project an outcome fact");
        assert_eq!(
            snapshot.summary.as_ref().map(AgentOutcomeSummary::as_str),
            Some(CHILD_COMPLETED_WITHOUT_REPORT_TEXT)
        );
    }

    #[test]
    fn group_completion_report_backtracks_past_empty_assistant_tail() {
        // 真机场景：turn 以空正文 assistant item 收尾（报告在更早的非空 item）。
        let agent_id = AgentId::new(2);
        let turn_id = AgentTurnId::new(7);
        let target = RuntimeTarget::provider("local", "qwen3");
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        orchestrator.register_child_for_test(
            agent_id,
            AgentId::MAIN,
            turn_id,
            test_title("backtracking child"),
            test_context("backtracking-child"),
            Box::new(StubMainRuntime::default()),
        );
        orchestrator.mark_child_target_for_test(agent_id, target.clone());
        {
            let record = orchestrator
                .children
                .get_mut(&agent_id)
                .expect("registered child should have a record");
            record.transcript.push(AgentTranscriptItem::Assistant {
                content: "the actual committed report".to_string(),
            });
            record.transcript.push(AgentTranscriptItem::Tool {
                title: "Read file".to_string(),
                content: "tool output".to_string(),
            });
            record.transcript.push(AgentTranscriptItem::Assistant {
                content: "   ".to_string(),
            });
            record.terminal_status = Some(AgentProjectionStatus::Completed);
            // completion gate：terminal fact 已交付且 durable outcome 已落盘。
            record.outcome_persisted = true;
        }
        let (response_sender, response_receiver) = oneshot::channel();
        orchestrator.group_waiters.insert(
            AgentLaunchGroupId::new(1),
            GroupWaiter {
                parent_agent_id: AgentId::MAIN,
                child_ids: vec![agent_id],
                response: response_sender,
            },
        );
        orchestrator.try_complete_group_waiters();

        let completion = response_receiver
            .blocking_recv()
            .expect("group waiter should settle from the prepared record")
            .expect("group completion should succeed");
        let child = &completion.children[0];
        assert_eq!(child.report.as_deref(), Some("the actual committed report"));
    }

    #[test]
    fn group_completion_report_and_snapshot_summary_are_separate_layers() {
        // 报告全文长于 240 显示列：completion report 携带全文，outcome snapshot 摘要
        // 截断单行（摘要只服务 TUI 面）。
        let long_report = "long report body".repeat(40);
        let child = completed_group_child(finished_turn_event(&long_report));
        assert_eq!(child.report.as_deref(), Some(long_report.as_str()));

        let (mut orchestrator, agent_id) =
            registered_child_with_terminal_event(finished_turn_event(&long_report));
        let _ = orchestrator.drain_child_events();
        let snapshot = outcome_facts(&orchestrator.drain_projection_events())
            .into_iter()
            .find(|snapshot| snapshot.agent_id == agent_id)
            .expect("completed child should project an outcome fact");
        let summary = snapshot
            .summary
            .expect("long report should still produce a summary");
        assert!(summary.as_str().ends_with("..."));
        assert!(summary.as_str().chars().count() < long_report.chars().count());
    }

    #[test]
    fn group_completion_report_truncates_at_char_limit() {
        let oversized_report = "x".repeat(AGENT_REPORT_MAX_CHARS + 100);
        let child = completed_group_child(finished_turn_event(&oversized_report));
        let report = child
            .report
            .expect("oversized report should still be carried in truncated form");
        assert!(child.truncated);
        assert!(report.chars().count() < oversized_report.chars().count());
        let note = format!(
            "[report truncated: full length {} chars]",
            oversized_report.chars().count()
        );
        assert!(report.contains(&note));
        // 截断保留正文主体：note 之前的内容仍达到字符上限。
        let body: String = report.chars().take(AGENT_REPORT_MAX_CHARS).collect();
        assert_eq!(body.chars().count(), AGENT_REPORT_MAX_CHARS);
    }

    #[test]
    fn group_completion_envelope_carries_terminal_metrics() {
        let agent_id = AgentId::new(2);
        let turn_id = AgentTurnId::new(7);
        let target = RuntimeTarget::provider("local", "qwen3");
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        orchestrator.register_child_for_test(
            agent_id,
            AgentId::MAIN,
            turn_id,
            test_title("metrics child"),
            test_context("metrics-child"),
            Box::new(StubMainRuntime {
                events: vec![
                    child_event(
                        agent_id,
                        turn_id,
                        &target,
                        AgentEventKind::ToolActivityStarted {
                            activity: runtime_domain::session::RuntimeToolActivity {
                                activity_id: "metrics-tool".to_string(),
                                title: "Read file".to_string(),
                                kind: runtime_domain::session::RuntimeToolKind::Read,
                                status:
                                    runtime_domain::session::RuntimeToolActivityStatus::InProgress,
                                content: vec![
                                    runtime_domain::session::RuntimeToolActivityContent::Text(
                                        "safe content".to_string(),
                                    ),
                                ],
                                locations: Vec::new(),
                                raw_input: None,
                                raw_output: None,
                            },
                        },
                    ),
                    child_event(
                        agent_id,
                        turn_id,
                        &target,
                        AgentEventKind::OutputTokenEstimate { total_tokens: 1200 },
                    ),
                    child_event(
                        agent_id,
                        turn_id,
                        &target,
                        finished_turn_event("metrics report"),
                    ),
                ],
            }),
        );
        orchestrator.mark_child_target_for_test(agent_id, target.clone());
        orchestrator.mark_child_elapsed_started_for_test(agent_id);
        let (response_sender, response_receiver) = oneshot::channel();
        orchestrator.group_waiters.insert(
            AgentLaunchGroupId::new(1),
            GroupWaiter {
                parent_agent_id: AgentId::MAIN,
                child_ids: vec![agent_id],
                response: response_sender,
            },
        );

        let _ = orchestrator.drain_child_events();

        let completion = response_receiver
            .blocking_recv()
            .expect("group waiter should receive a completion")
            .expect("group completion should succeed");
        let child = &completion.children[0];
        assert_eq!(child.tokens, Some(1200));
        assert_eq!(child.tool_uses, Some(1));
        assert!(
            child.duration.is_some(),
            "started child should carry a terminal duration"
        );
    }

    #[test]
    fn group_completion_failed_child_keeps_placeholder_summary() {
        // raw failure 文案不进入任何交付面；失败占位只进入 outcome snapshot 摘要。
        let failed_event = || AgentEventKind::TurnFailed {
            message: "provider connection closed".to_string(),
        };
        let child = completed_group_child(failed_event());
        assert_eq!(child.outcome, AgentOutcome::Failed);
        assert_eq!(child.report, None);

        let (mut orchestrator, agent_id) = registered_child_with_terminal_event(failed_event());
        let _ = orchestrator.drain_child_events();
        let snapshot = outcome_facts(&orchestrator.drain_projection_events())
            .into_iter()
            .find(|snapshot| snapshot.agent_id == agent_id)
            .expect("failed child should project an outcome fact");
        assert_eq!(snapshot.outcome, AgentOutcome::Failed);
        assert_eq!(
            snapshot.summary.as_ref().map(AgentOutcomeSummary::as_str),
            Some("Child Agent failed")
        );
    }

    #[test]
    fn group_completion_marks_explicitly_stopped_children() {
        let agent_id = AgentId::new(2);
        let turn_id = AgentTurnId::new(7);
        let target = RuntimeTarget::provider("local", "qwen3");
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        orchestrator.register_child_for_test(
            agent_id,
            AgentId::MAIN,
            turn_id,
            test_title("stopped group child"),
            test_context("stopped-group-child"),
            Box::new(StubMainRuntime::default()),
        );
        orchestrator.mark_child_target_for_test(agent_id, target.clone());
        orchestrator.mark_child_launch_group_for_test(agent_id, AgentLaunchGroupId::new(1));
        let (response_sender, response_receiver) = oneshot::channel();
        orchestrator.group_waiters.insert(
            AgentLaunchGroupId::new(1),
            GroupWaiter {
                parent_agent_id: AgentId::MAIN,
                child_ids: vec![agent_id],
                response: response_sender,
            },
        );

        orchestrator
            .stop_child(agent_id)
            .expect("explicit stop should converge");

        // stop 定格的 Cancelled 随即持久化；group completion 在下一次 drain 结算。
        let _ = orchestrator.drain_child_events();
        let completion = response_receiver
            .blocking_recv()
            .expect("group waiter should settle from the stopped terminal fact")
            .expect("group completion should succeed");
        assert_eq!(completion.children.len(), 1);
        assert_eq!(completion.children[0].agent_id, agent_id);
        assert_eq!(completion.children[0].outcome, AgentOutcome::Cancelled);
        // 显式停止与自然取消的区分只进入 outcome snapshot 摘要（TUI 面）。
        let snapshot = outcome_facts(&orchestrator.drain_projection_events())
            .into_iter()
            .find(|snapshot| snapshot.agent_id == agent_id)
            .expect("stopped child should project an outcome fact");
        assert_eq!(
            snapshot.summary.as_ref().map(AgentOutcomeSummary::as_str),
            Some(CHILD_STOPPED_BY_REQUEST_TEXT)
        );
    }

    #[test]
    fn group_completion_natural_cancel_keeps_generic_summary() {
        let (mut orchestrator, agent_id) =
            registered_child_with_terminal_event(AgentEventKind::TurnInterrupted);
        let (response_sender, response_receiver) = oneshot::channel();
        orchestrator.group_waiters.insert(
            AgentLaunchGroupId::new(1),
            GroupWaiter {
                parent_agent_id: AgentId::MAIN,
                child_ids: vec![agent_id],
                response: response_sender,
            },
        );

        let _ = orchestrator.drain_child_events();

        // adapter interrupt 等自然取消不携带 stop 来源：summary 保持通用 Cancelled 占位。
        let completion = response_receiver
            .blocking_recv()
            .expect("group waiter should settle from the interrupted terminal fact")
            .expect("group completion should succeed");
        assert_eq!(completion.children[0].agent_id, agent_id);
        assert_eq!(completion.children[0].outcome, AgentOutcome::Cancelled);
        let snapshot = outcome_facts(&orchestrator.drain_projection_events())
            .into_iter()
            .find(|snapshot| snapshot.agent_id == agent_id)
            .expect("interrupted child should project an outcome fact");
        assert_eq!(
            snapshot.summary.as_ref().map(AgentOutcomeSummary::as_str),
            Some(CHILD_CANCELLED_TEXT)
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
        let mut overview_events = None;
        orchestrator.observe_agents(AgentObservationRequestId::new(1));
        for event in orchestrator.drain_projection_events() {
            match event {
                AgentProjectionEvent::AgentsOverviewSnapshotLoaded { snapshot, .. } => {
                    overview_events = Some(snapshot);
                }
                // sessionless orchestrator 的 outcome append 是 no-op Ok，document fact 照常交付。
                AgentProjectionEvent::AgentOutcomeFact { .. } => {}
                other => panic!("unexpected projection event: {other:?}"),
            }
        }
        let overview = overview_events.expect("overview observation should deliver a snapshot");
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
    fn terminal_fact_settles_and_delivers_while_runtime_is_retained() {
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
        let shutdown_calls = Arc::new(AtomicUsize::new(0));
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        orchestrator.register_child_for_test(
            agent_id,
            AgentId::MAIN,
            turn_id,
            test_title("terminal settle"),
            test_context("terminal-settle"),
            Box::new(ShutdownCountingRuntime {
                events: vec![terminal.clone()],
                shutdown_calls: Arc::clone(&shutdown_calls),
            }),
        );

        // terminal 事实在同一 drain 内 settle 并交付，不等待任何 authority 清理。
        assert_eq!(orchestrator.drain_child_events(), vec![terminal]);
        assert_eq!(
            orchestrator.child_status(agent_id),
            Some(AgentProjectionStatus::Completed)
        );
        assert!(
            orchestrator.child_has_authority(agent_id),
            "settled child must retain its runtime and context"
        );
        assert_eq!(
            shutdown_calls.load(Ordering::SeqCst),
            0,
            "settle must not shut down the retained child runtime"
        );
        assert_eq!(
            orchestrator.active_child_count(),
            0,
            "settled child must not consume the active child quota"
        );
        assert_eq!(orchestrator.child_count(), 1);

        // exactly-once：terminal 已交付，后续 drain 不再重复。
        assert!(orchestrator.drain_child_events().is_empty());

        // 显式 stop 对 settled child 走完整清理：runtime 收敛、行移除。
        orchestrator
            .stop_child(agent_id)
            .expect("explicit stop should fully dispose the settled child");
        assert_eq!(shutdown_calls.load(Ordering::SeqCst), 1);
        assert!(!orchestrator.child_has_authority(agent_id));
        assert_eq!(orchestrator.child_count(), 0);
    }

    #[test]
    fn child_elapsed_freezes_during_permission_wait_and_at_terminal() {
        let agent_id = AgentId::new(2);
        let turn_id = AgentTurnId::new(agent_id.get());
        let target = RuntimeTarget::provider("local", "qwen3");
        let (runtime, events, _dispatched, _submitted) = ScriptedChildRuntime::new(Vec::new());
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        orchestrator.register_child_for_test(
            agent_id,
            AgentId::MAIN,
            turn_id,
            test_title("elapsed child"),
            test_context("elapsed-child"),
            Box::new(runtime),
        );
        orchestrator.mark_child_target_for_test(agent_id, target.clone());
        orchestrator.mark_child_elapsed_started_for_test(agent_id);

        let deliver = |orchestrator: &mut AgentOrchestrator, kind: AgentEventKind| {
            events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(child_event(agent_id, turn_id, &target, kind));
            orchestrator.drain_child_events();
        };
        let elapsed_of_overview_row = |orchestrator: &mut AgentOrchestrator, request: u64| {
            orchestrator.observe_agents(AgentObservationRequestId::new(request));
            orchestrator
                .drain_projection_events()
                .into_iter()
                .find_map(|event| match event {
                    AgentProjectionEvent::AgentsOverviewSnapshotLoaded { snapshot, .. } => snapshot
                        .rows
                        .iter()
                        .find(|row| row.agent_id == agent_id)
                        .map(|row| row.elapsed_ms),
                    _ => None,
                })
                .flatten()
                .expect("overview snapshot should deliver the child elapsed projection")
        };

        // 运行中：elapsed 随墙钟推进增长。
        deliver(
            &mut orchestrator,
            AgentEventKind::Thinking { is_thinking: true },
        );
        let running_start = elapsed_of_overview_row(&mut orchestrator, 41);
        std::thread::sleep(std::time::Duration::from_millis(40));
        let running_later = elapsed_of_overview_row(&mut orchestrator, 42);
        assert!(
            running_later > running_start,
            "working child elapsed must advance: {running_start} -> {running_later}"
        );

        // WaitingPermission：等待审批期间不计入 elapsed——投影两次值相同。
        deliver(
            &mut orchestrator,
            AgentEventKind::PermissionRequested {
                request: permission_request("elapsed-perm-1"),
            },
        );
        let waiting_start = elapsed_of_overview_row(&mut orchestrator, 43);
        std::thread::sleep(std::time::Duration::from_millis(40));
        let waiting_later = elapsed_of_overview_row(&mut orchestrator, 44);
        assert_eq!(
            waiting_start, waiting_later,
            "waiting-permission elapsed must freeze"
        );

        // 恢复 Working：从冻结值继续累计。
        deliver(
            &mut orchestrator,
            AgentEventKind::Thinking { is_thinking: true },
        );
        let resumed_start = elapsed_of_overview_row(&mut orchestrator, 45);
        std::thread::sleep(std::time::Duration::from_millis(40));
        let resumed_later = elapsed_of_overview_row(&mut orchestrator, 46);
        assert!(
            resumed_later > resumed_start,
            "resumed child elapsed must accumulate again"
        );
        assert!(
            resumed_later > waiting_later,
            "resume must continue from the frozen base: {resumed_later} vs {waiting_later}"
        );

        // 终态：elapsed 定格为终态时刻的值，时间推进后不再增长。
        deliver(
            &mut orchestrator,
            AgentEventKind::TurnFinished {
                response: runtime_domain::session::ConversationResponse::assistant_text("done"),
                metrics: None,
                context_usage: None,
            },
        );
        let terminal_start = elapsed_of_overview_row(&mut orchestrator, 47);
        std::thread::sleep(std::time::Duration::from_millis(40));
        let terminal_later = elapsed_of_overview_row(&mut orchestrator, 48);
        assert_eq!(
            terminal_start, terminal_later,
            "terminal child elapsed must stay frozen"
        );
    }

    #[test]
    fn settled_children_over_the_cap_are_evicted_oldest_first() {
        let target = RuntimeTarget::provider("local", "qwen3");
        let shutdown_calls = Arc::new(AtomicUsize::new(0));
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        for index in 0..=(MAX_SETTLED_CHILD_AGENTS as u64) {
            let agent_id = AgentId::new(index + 2);
            let turn_id = AgentTurnId::new(agent_id.get());
            orchestrator.register_child_for_test(
                agent_id,
                AgentId::MAIN,
                turn_id,
                test_title("cap settled child"),
                test_context(&format!("cap-settled-child-{index}")),
                Box::new(ShutdownCountingRuntime {
                    events: vec![child_event(
                        agent_id,
                        turn_id,
                        &target,
                        AgentEventKind::TurnFinished {
                            response: runtime_domain::session::ConversationResponse::assistant_text(
                                "settled answer",
                            ),
                            metrics: None,
                            context_usage: None,
                        },
                    )],
                    shutdown_calls: Arc::clone(&shutdown_calls),
                }),
            );
        }

        let _ = orchestrator.drain_child_events();

        // 超限淘汰最旧（agent id 最小）：其余 settled child 的 runtime 保留。
        assert_eq!(orchestrator.child_count(), MAX_SETTLED_CHILD_AGENTS);
        assert!(!orchestrator.child_has_authority(AgentId::new(2)));
        assert!(orchestrator.child_has_authority(AgentId::new(3)));
        assert_eq!(
            shutdown_calls.load(Ordering::SeqCst),
            1,
            "eviction must run the full cleanup path for the oldest settled child"
        );
        assert_eq!(orchestrator.active_child_count(), 0);
    }

    #[test]
    fn blocked_settled_eviction_retains_owner_and_retries_to_convergence() {
        let target = RuntimeTarget::provider("local", "qwen3");
        let finished_event = |agent_id: AgentId, turn_id: AgentTurnId| {
            child_event(
                agent_id,
                turn_id,
                &target,
                AgentEventKind::TurnFinished {
                    response: runtime_domain::session::ConversationResponse::assistant_text(
                        "settled answer",
                    ),
                    metrics: None,
                    context_usage: None,
                },
            )
        };
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        // 最旧 settled child 的 runtime shutdown 第一次失败：淘汰被阻断。
        let oldest_id = AgentId::new(2);
        orchestrator.register_child_for_test(
            oldest_id,
            AgentId::MAIN,
            AgentTurnId::new(oldest_id.get()),
            test_title("blocked eviction"),
            test_context("blocked-eviction"),
            Box::new(FailingShutdownRuntime {
                failures_remaining: 1,
                events: vec![finished_event(oldest_id, AgentTurnId::new(oldest_id.get()))],
            }),
        );
        for index in 1..=(MAX_SETTLED_CHILD_AGENTS as u64) {
            let agent_id = AgentId::new(index + 2);
            let turn_id = AgentTurnId::new(agent_id.get());
            orchestrator.register_child_for_test(
                agent_id,
                AgentId::MAIN,
                turn_id,
                test_title("cap settled child"),
                test_context(&format!("blocked-eviction-child-{index}")),
                Box::new(StubMainRuntime {
                    events: vec![finished_event(agent_id, turn_id)],
                }),
            );
        }

        // 首轮 drain：terminal 事实成立、淘汰被阻断，被阻断 child 的 terminal fact
        // 暂缓交付（Disposing 未收敛），其余 child 正常 settle。
        let delivered = orchestrator.drain_child_events();
        assert_eq!(delivered.len(), MAX_SETTLED_CHILD_AGENTS);
        assert!(delivered.iter().all(|event| event.agent_id != oldest_id));
        assert_eq!(
            orchestrator.child_status(oldest_id),
            Some(AgentProjectionStatus::CleanupBlocked)
        );
        assert!(orchestrator.child_has_authority(oldest_id));
        assert_eq!(orchestrator.child_count(), MAX_SETTLED_CHILD_AGENTS + 1);
        // durable outcome fact 不受阻断影响：全部 17 个 child 的 outcome 已持久化投影。
        assert_eq!(
            orchestrator
                .drain_projection_events()
                .iter()
                .filter(|event| matches!(event, AgentProjectionEvent::AgentOutcomeFact { .. }))
                .count(),
            MAX_SETTLED_CHILD_AGENTS + 1
        );

        // 下轮 settle 以同一 owner 重试：收敛后行随 stop 意图移除；held terminal fact
        // 随完全释放的行一并终结（durable outcome 已交付，AgentEvent 流不再补发）。
        assert!(orchestrator.drain_child_events().is_empty());
        assert!(!orchestrator.child_has_authority(oldest_id));
        assert_eq!(orchestrator.child_count(), MAX_SETTLED_CHILD_AGENTS);
    }

    /// 先交付 staged events、随后拒绝 SubmitTurn 的 fixture：构造 followup dispatch
    /// 失败路径。
    struct TerminalThenBusyRuntime {
        events: Vec<AgentEvent>,
    }

    impl AgentRuntime for TerminalThenBusyRuntime {
        fn dispatch(
            &mut self,
            command: AgentCommand,
        ) -> Result<AgentCommandReceipt, AgentRuntimeError> {
            match command {
                AgentCommand::SubmitTurn { .. } => Err(AgentRuntimeError::Busy),
                _ => Ok(AgentCommandReceipt::Accepted),
            }
        }

        fn drain_events(&mut self) -> Vec<AgentEvent> {
            std::mem::take(&mut self.events)
        }

        fn shutdown(&mut self) -> Result<(), AgentRuntimeError> {
            Ok(())
        }
    }

    impl AgentRuntimePort for TerminalThenBusyRuntime {
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

    fn loaded_view_snapshot(events: Vec<AgentProjectionEvent>) -> AgentViewSnapshot {
        events
            .into_iter()
            .find_map(|event| match event {
                AgentProjectionEvent::AgentViewSnapshotLoaded { snapshot, .. } => Some(snapshot),
                _ => None,
            })
            .expect("view observation should deliver a snapshot")
    }

    fn loaded_overview_rows(events: Vec<AgentProjectionEvent>) -> Vec<AgentOverviewRow> {
        events
            .into_iter()
            .find_map(|event| match event {
                AgentProjectionEvent::AgentsOverviewSnapshotLoaded { snapshot, .. } => {
                    Some(snapshot.rows)
                }
                _ => None,
            })
            .expect("overview observation should deliver a snapshot")
    }

    fn outcome_facts(
        events: &[AgentProjectionEvent],
    ) -> Vec<runtime_domain::agent::AgentOutcomeSnapshot> {
        events
            .iter()
            .filter_map(|event| match event {
                AgentProjectionEvent::AgentOutcomeFact { snapshot } => Some(snapshot.clone()),
                _ => None,
            })
            .collect()
    }

    fn child_message(content: &str) -> AgentChildMessage {
        AgentChildMessage::new(content).expect("test message should construct")
    }

    fn launch_request(objective: &str) -> runtime_domain::agent::AgentLaunchRequest {
        runtime_domain::agent::AgentLaunchRequest::new(
            AgentObjective::new(objective).expect("test objective should construct"),
            None,
        )
        .expect("test launch request should construct")
    }

    #[test]
    fn child_turn_request_appends_identity_instructions_after_the_objective() {
        let target = RuntimeTarget::provider("local", "qwen3");
        let request = launch_request("write a haiku about ports");
        let turn_request = child_turn_request(&target, &request);

        let (provider_request, transcript_message, direct_instructions) = turn_request.into_parts();
        let provider_text = provider_request.message_text();
        // provider 文本形态固定为 objective + 身份指令，两者之间空一行。
        assert_eq!(
            provider_text,
            format!("write a haiku about ports\n\n{CHILD_AGENT_IDENTITY_INSTRUCTIONS}")
        );
        assert!(direct_instructions.is_some());
        // transcript delivery 仍是纯 objective：身份指令不进入 transcript。
        assert_eq!(transcript_message.content, "write a haiku about ports");
    }

    #[test]
    fn child_followup_turn_request_does_not_reinject_identity_instructions() {
        let target = RuntimeTarget::provider("local", "qwen3");
        let turn_request = child_followup_turn_request(
            &target,
            &child_message("extend the research with citations"),
        );

        let (provider_request, _transcript_message, direct_instructions) =
            turn_request.into_parts();

        assert_eq!(
            provider_request.message_text(),
            "extend the research with citations"
        );
        assert!(direct_instructions.is_none());
    }

    #[test]
    fn child_turn_request_debug_does_not_echo_objective_or_instruction_bodies() {
        let target = RuntimeTarget::provider("local", "qwen3");
        let request = launch_request("secret objective body");
        let debug = format!("{:?}", child_turn_request(&target, &request));

        assert!(debug.contains("has_direct_instructions: true"));
        assert!(!debug.contains("secret objective body"));
        assert!(!debug.contains("child agent dispatched"));
    }

    fn submitted_turn_texts(submitted: &ScriptedSubmittedTurns) -> Vec<String> {
        submitted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .map(|(_, text)| text.clone())
            .collect()
    }

    #[test]
    fn active_child_message_queues_until_the_turn_boundary() {
        let agent_id = AgentId::new(2);
        let turn_id = AgentTurnId::new(agent_id.get());
        let target = RuntimeTarget::provider("local", "qwen3");
        let (runtime, staged, dispatched, submitted_turns) = ScriptedChildRuntime::new(Vec::new());
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        orchestrator.register_child_for_test(
            agent_id,
            AgentId::MAIN,
            turn_id,
            test_title("queued message child"),
            test_context("queued-message-child"),
            Box::new(runtime),
        );
        orchestrator.mark_child_target_for_test(agent_id, target.clone());

        let receipt = orchestrator
            .send_child_message(
                agent_id,
                child_message("extend the research with citations"),
            )
            .expect("active child should accept the message");
        assert_eq!(receipt, AgentCommandReceipt::MessageQueued { turn_id });
        // 队列对 dispatch 与 projection 完全不可见，消息在 turn 边界前不产生副作用。
        assert!(
            dispatched
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
        );
        assert!(submitted_turn_texts(&submitted_turns).is_empty());
        assert!(orchestrator.drain_projection_events().is_empty());

        // terminal 后同一 drain 内自动开始 followup turn：消息成为下一 turn 的
        // user 消息，record 回到 Active。
        staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(child_event(
                agent_id,
                turn_id,
                &target,
                finished_turn_event("first answer"),
            ));
        let accepted = orchestrator.drain_child_events();
        assert_eq!(accepted.len(), 1);
        assert!(accepted[0].kind.is_terminal());
        assert_eq!(
            *dispatched
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec!["submit_turn"]
        );
        assert_eq!(
            submitted_turn_texts(&submitted_turns),
            vec!["extend the research with citations".to_string()]
        );
        assert_eq!(
            orchestrator.child_status(agent_id),
            Some(AgentProjectionStatus::Pending)
        );
        assert_eq!(orchestrator.active_child_count(), 1);
        assert!(orchestrator.child_has_authority(agent_id));

        orchestrator.observe_agent_transcript(AgentObservationRequestId::new(1), agent_id);
        let snapshot = loaded_view_snapshot(orchestrator.drain_projection_events());
        assert_eq!(
            snapshot.transcript.items,
            vec![
                AgentTranscriptItem::Assistant {
                    content: "first answer".to_string()
                },
                AgentTranscriptItem::User {
                    content: "extend the research with citations".to_string()
                },
            ]
        );
    }

    #[test]
    fn settled_child_message_starts_followup_with_continuous_transcript() {
        let agent_id = AgentId::new(2);
        let turn_id = AgentTurnId::new(agent_id.get());
        let target = RuntimeTarget::provider("local", "qwen3");
        let (runtime, staged, dispatched, submitted_turns) = ScriptedChildRuntime::new(vec![
            child_event(
                agent_id,
                turn_id,
                &target,
                AgentEventKind::ToolActivityStarted {
                    activity: runtime_domain::session::RuntimeToolActivity {
                        activity_id: "tool-1".to_string(),
                        title: "Read file".to_string(),
                        kind: runtime_domain::session::RuntimeToolKind::Read,
                        status: runtime_domain::session::RuntimeToolActivityStatus::InProgress,
                        content: vec![runtime_domain::session::RuntimeToolActivityContent::Text(
                            "safe child tool content".to_string(),
                        )],
                        locations: Vec::new(),
                        raw_input: None,
                        raw_output: None,
                    },
                },
            ),
            child_event(
                agent_id,
                turn_id,
                &target,
                finished_turn_event("first answer"),
            ),
        ]);
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        orchestrator.register_child_for_test(
            agent_id,
            AgentId::MAIN,
            turn_id,
            test_title("followup child"),
            test_context("followup-child"),
            Box::new(runtime),
        );
        orchestrator.mark_child_target_for_test(agent_id, target.clone());
        orchestrator.mark_child_launch_group_for_test(agent_id, AgentLaunchGroupId::new(7));

        let accepted = orchestrator.drain_child_events();
        assert_eq!(accepted.len(), 2);
        assert_eq!(
            orchestrator.child_status(agent_id),
            Some(AgentProjectionStatus::Completed)
        );
        assert_eq!(orchestrator.active_child_count(), 0);
        let first_outcomes = outcome_facts(&orchestrator.drain_projection_events());
        assert_eq!(first_outcomes.len(), 1);
        assert_eq!(first_outcomes[0].group_id, Some(AgentLaunchGroupId::new(7)));

        // settled child 的消息立即开始 followup turn，不必等待下一次 drain。
        let receipt = orchestrator
            .send_child_message(agent_id, child_message("refine the report"))
            .expect("settled child should accept the message");
        assert_eq!(receipt, AgentCommandReceipt::MessageStarted { turn_id });
        assert_eq!(
            *dispatched
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec!["submit_turn"]
        );
        assert_eq!(
            submitted_turn_texts(&submitted_turns),
            vec!["refine the report".to_string()]
        );
        assert_eq!(
            orchestrator.child_status(agent_id),
            Some(AgentProjectionStatus::Pending)
        );
        assert_eq!(orchestrator.active_child_count(), 1);

        // followup terminal 复用既有管线：投影 settle、新 outcome fact 追加；
        // followup outcome 是消息触发的独立 durable fact，不归属 launch group。
        staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(child_event(
                agent_id,
                turn_id,
                &target,
                finished_turn_event("refined answer"),
            ));
        let accepted = orchestrator.drain_child_events();
        assert_eq!(accepted.len(), 1);
        assert!(accepted[0].kind.is_terminal());
        assert_eq!(
            orchestrator.child_status(agent_id),
            Some(AgentProjectionStatus::Completed)
        );
        let second_outcomes = outcome_facts(&orchestrator.drain_projection_events());
        assert_eq!(second_outcomes.len(), 1);
        assert_eq!(
            second_outcomes[0].group_id, None,
            "followup outcome must not re-associate with the launch group"
        );
        assert_eq!(
            second_outcomes[0]
                .summary
                .as_ref()
                .map(|summary| summary.as_str()),
            Some("refined answer")
        );
        assert_eq!(second_outcomes[0].parent_agent_id, Some(AgentId::MAIN));

        // transcript 连续：前序 tool/assistant 保留，followup user/assistant 顺序追加。
        orchestrator.observe_agent_transcript(AgentObservationRequestId::new(2), agent_id);
        let snapshot = loaded_view_snapshot(orchestrator.drain_projection_events());
        assert_eq!(
            snapshot.transcript.items,
            vec![
                AgentTranscriptItem::Tool {
                    title: "Read file".to_string(),
                    content: "safe child tool content".to_string()
                },
                AgentTranscriptItem::Assistant {
                    content: "first answer".to_string()
                },
                AgentTranscriptItem::User {
                    content: "refine the report".to_string()
                },
                AgentTranscriptItem::Assistant {
                    content: "refined answer".to_string()
                },
            ]
        );
    }

    #[test]
    fn queued_messages_consume_one_per_turn_in_fifo_order() {
        let agent_id = AgentId::new(2);
        let turn_id = AgentTurnId::new(agent_id.get());
        let target = RuntimeTarget::provider("local", "qwen3");
        let (runtime, staged, _dispatched, submitted_turns) = ScriptedChildRuntime::new(Vec::new());
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        orchestrator.register_child_for_test(
            agent_id,
            AgentId::MAIN,
            turn_id,
            test_title("multi message child"),
            test_context("multi-message-child"),
            Box::new(runtime),
        );
        orchestrator.mark_child_target_for_test(agent_id, target.clone());

        for content in ["first follow-up", "second follow-up"] {
            let receipt = orchestrator
                .send_child_message(agent_id, child_message(content))
                .expect("active child should queue every message");
            assert_eq!(receipt, AgentCommandReceipt::MessageQueued { turn_id });
        }

        // 单 turn 单消息：第一个 terminal 只消费队头。
        staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(child_event(
                agent_id,
                turn_id,
                &target,
                finished_turn_event("first answer"),
            ));
        let _ = orchestrator.drain_child_events();
        assert_eq!(
            submitted_turn_texts(&submitted_turns),
            vec!["first follow-up".to_string()]
        );

        staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(child_event(
                agent_id,
                turn_id,
                &target,
                finished_turn_event("second answer"),
            ));
        let _ = orchestrator.drain_child_events();
        assert_eq!(
            submitted_turn_texts(&submitted_turns),
            vec![
                "first follow-up".to_string(),
                "second follow-up".to_string()
            ]
        );

        orchestrator.observe_agent_transcript(AgentObservationRequestId::new(3), agent_id);
        let snapshot = loaded_view_snapshot(orchestrator.drain_projection_events());
        let user_items = snapshot
            .transcript
            .items
            .iter()
            .filter_map(|item| match item {
                AgentTranscriptItem::User { content } => Some(content.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            user_items,
            vec![
                "first follow-up".to_string(),
                "second follow-up".to_string()
            ]
        );
    }

    #[test]
    fn followup_dispatch_failure_marks_the_turn_failed() {
        let agent_id = AgentId::new(2);
        let turn_id = AgentTurnId::new(agent_id.get());
        let target = RuntimeTarget::provider("local", "qwen3");
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        orchestrator.register_child_for_test(
            agent_id,
            AgentId::MAIN,
            turn_id,
            test_title("failing followup"),
            test_context("failing-followup"),
            Box::new(TerminalThenBusyRuntime {
                events: vec![child_event(
                    agent_id,
                    turn_id,
                    &target,
                    finished_turn_event("first answer"),
                )],
            }),
        );
        orchestrator.mark_child_target_for_test(agent_id, target.clone());
        let _ = orchestrator.drain_child_events();
        let _ = orchestrator.drain_projection_events();
        assert_eq!(
            orchestrator.child_status(agent_id),
            Some(AgentProjectionStatus::Completed)
        );

        let receipt = orchestrator
            .send_child_message(agent_id, child_message("please continue"))
            .expect("settled child should accept the message");
        assert_eq!(receipt, AgentCommandReceipt::MessageStarted { turn_id });
        // dispatch 失败在受理后同步定格为 Failed terminal，不丢弃消息事实。
        assert_eq!(
            orchestrator.child_status(agent_id),
            Some(AgentProjectionStatus::Failed)
        );

        // terminal 事实与 followup outcome 由下一次 drain 交付。
        let accepted = orchestrator.drain_child_events();
        assert_eq!(accepted.len(), 1);
        assert!(matches!(
            &accepted[0].kind,
            AgentEventKind::TurnFailed { message } if message == "Child Agent failed to start"
        ));
        let outcomes = outcome_facts(&orchestrator.drain_projection_events());
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].outcome, AgentOutcome::Failed);
    }

    #[test]
    fn send_child_message_rejects_unknown_main_and_disposing_targets() {
        let agent_id = AgentId::new(2);
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);

        assert!(matches!(
            orchestrator.send_child_message(AgentId::new(99), child_message("unknown")),
            Err(AgentRuntimeError::UnknownAgent)
        ));
        assert!(matches!(
            orchestrator.send_child_message(AgentId::MAIN, child_message("main")),
            Err(AgentRuntimeError::UnknownAgent)
        ));

        // 显式清理未收敛（Disposing）的 child 不是合法消息目标；失败重试由同一
        // owner 继续，消息在此期间 closed 拒绝。
        orchestrator.register_child_for_test(
            agent_id,
            AgentId::MAIN,
            AgentTurnId::new(agent_id.get()),
            test_title("disposing child"),
            test_context("disposing-child"),
            Box::new(FailingShutdownRuntime {
                failures_remaining: 3,
                events: Vec::new(),
            }),
        );
        assert!(orchestrator.stop_child(agent_id).is_err());
        assert!(matches!(
            orchestrator.send_child_message(agent_id, child_message("blocked")),
            Err(AgentRuntimeError::UnknownAgent)
        ));
        assert_eq!(
            orchestrator.child_status(agent_id),
            Some(AgentProjectionStatus::CleanupBlocked)
        );
        assert_eq!(orchestrator.child_count(), 1);

        // 清理收敛后行移除，消息照旧 closed 拒绝（等同 unknown target）。
        while orchestrator.stop_child(agent_id).is_err() {}
        assert_eq!(orchestrator.child_count(), 0);
        assert!(matches!(
            orchestrator.send_child_message(agent_id, child_message("removed")),
            Err(AgentRuntimeError::UnknownAgent)
        ));
    }

    #[test]
    fn followup_turn_permissions_keep_the_fifo_pipeline() {
        let agent_id = AgentId::new(2);
        let turn_id = AgentTurnId::new(agent_id.get());
        let target = RuntimeTarget::provider("local", "qwen3");
        let (runtime, staged, dispatched, _submitted_turns) =
            ScriptedChildRuntime::new(vec![child_event(
                agent_id,
                turn_id,
                &target,
                finished_turn_event("first answer"),
            )]);
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        orchestrator.register_child_for_test(
            agent_id,
            AgentId::MAIN,
            turn_id,
            test_title("followup permission child"),
            test_context("followup-permission-child"),
            Box::new(runtime),
        );
        orchestrator.mark_child_target_for_test(agent_id, target.clone());
        let _ = orchestrator.drain_child_events();
        let _ = orchestrator.drain_projection_events();

        orchestrator
            .send_child_message(agent_id, child_message("run the safety checks"))
            .expect("settled child should start the followup turn");

        // followup turn 的 permission fact 照常进入 FIFO 并投影 head。
        staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(child_event(
                agent_id,
                turn_id,
                &target,
                AgentEventKind::PermissionRequested {
                    request: permission_request("perm-followup"),
                },
            ));
        let accepted = orchestrator.drain_child_events();
        assert_eq!(accepted.len(), 1);
        assert!(matches!(
            accepted[0].kind,
            AgentEventKind::PermissionRequested { .. }
        ));
        let updates = permission_updates(&orchestrator.drain_projection_events());
        assert_eq!(updates.len(), 1);
        let head = updates[0]
            .request
            .as_ref()
            .expect("followup permission should be the FIFO head");
        assert_eq!(head.target.request_id, "perm-followup");
        assert_eq!(head.target.turn_id, turn_id);
        assert_eq!(head.state, AgentPermissionState::Pending);

        // respond 照常经 identity 校验提交到 child runtime。
        orchestrator
            .respond_agent_permission(
                permission_target(
                    agent_id,
                    turn_id,
                    orchestrator.generation(),
                    &target,
                    "perm-followup",
                ),
                Some("allow-1".into()),
            )
            .expect("followup permission response should dispatch");
        assert_eq!(
            *dispatched
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec!["submit_turn", "respond_permission"]
        );
        let _ = orchestrator.drain_projection_events();

        // terminal 清空 FIFO 并投影 None。
        staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(child_event(
                agent_id,
                turn_id,
                &target,
                finished_turn_event("checks passed"),
            ));
        let _ = orchestrator.drain_child_events();
        let updates = permission_updates(&orchestrator.drain_projection_events());
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].request, None);
    }

    /// 以 child caller 身份构造一次 `send_agent_message` host request，并返回回执
    /// receiver；caller 是 target 的 direct parent，与生产 MAIN caller 共用同一校验面。
    fn child_caller_message_request(
        orchestrator: &mut AgentOrchestrator,
        caller_id: AgentId,
        caller_context: &AgentCapabilityContext,
        caller_turn_id: AgentTurnId,
        target_id: AgentId,
        message: &str,
    ) -> oneshot::Receiver<Result<AgentMessageDelivery, SendAgentMessageFailure>> {
        let (response, receiver) = oneshot::channel();
        orchestrator.handle_send_agent_message_request(SendAgentMessageRequest {
            identity: tool_runtime::ToolInvocationIdentity::new(
                caller_id.get(),
                caller_turn_id.get(),
                orchestrator.generation().get(),
                caller_context.epoch(),
            ),
            agent_id: target_id,
            message: child_message(message),
            response,
        });
        receiver
    }

    /// 注册一个 parented target child（caller 的 direct child），返回 staged 队列。
    fn register_message_target_child(
        orchestrator: &mut AgentOrchestrator,
        caller_id: AgentId,
        target_id: AgentId,
        target: RuntimeTarget,
        staged_events: Vec<AgentEvent>,
    ) -> ScriptedEventQueue {
        let (runtime, staged, _dispatched, _submitted) = ScriptedChildRuntime::new(staged_events);
        orchestrator.register_child_for_test(
            target_id,
            caller_id,
            AgentTurnId::new(target_id.get()),
            test_title("message target"),
            test_context("message-target"),
            Box::new(runtime),
        );
        orchestrator.mark_child_target_for_test(target_id, target);
        staged
    }

    fn register_message_caller_child(
        orchestrator: &mut AgentOrchestrator,
        caller_id: AgentId,
        caller_turn_id: AgentTurnId,
    ) -> AgentCapabilityContext {
        let (runtime, _staged, _dispatched, _submitted) = ScriptedChildRuntime::new(Vec::new());
        let context = test_context("message-caller");
        orchestrator.register_child_for_test(
            caller_id,
            AgentId::MAIN,
            caller_turn_id,
            test_title("message caller"),
            context.clone(),
            Box::new(runtime),
        );
        context
    }

    #[test]
    fn message_waiters_settle_in_fifo_order_across_followup_turns() {
        let caller_id = AgentId::new(2);
        let target_id = AgentId::new(3);
        let target = RuntimeTarget::provider("local", "qwen3");
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        let caller_context =
            register_message_caller_child(&mut orchestrator, caller_id, AgentTurnId::new(20));
        let staged = register_message_target_child(
            &mut orchestrator,
            caller_id,
            target_id,
            target.clone(),
            Vec::new(),
        );

        let mut first_receiver = child_caller_message_request(
            &mut orchestrator,
            caller_id,
            &caller_context,
            AgentTurnId::new(20),
            target_id,
            "first follow-up",
        );
        let mut second_receiver = child_caller_message_request(
            &mut orchestrator,
            caller_id,
            &caller_context,
            AgentTurnId::new(20),
            target_id,
            "second follow-up",
        );
        // Active child 的排队消息不产生投影副作用（与 spawn 前的 staging 语义一致）。
        assert!(orchestrator.drain_projection_events().is_empty());

        // launch turn terminal：不结算任何 waiter，只触发第一条消息的 followup turn。
        staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(child_event(
                target_id,
                AgentTurnId::new(target_id.get()),
                &target,
                finished_turn_event("first answer"),
            ));
        let _ = orchestrator.drain_child_events();
        assert!(
            first_receiver.try_recv().is_err(),
            "launch turn terminal must not settle a message waiter"
        );

        // followup turn 1 terminal：结算第一个 waiter；消息 2 的 followup 随后自动开始。
        staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(child_event(
                target_id,
                AgentTurnId::new(target_id.get()),
                &target,
                finished_turn_event("first refined"),
            ));
        let _ = orchestrator.drain_child_events();
        let first_delivery = first_receiver
            .try_recv()
            .expect("first waiter should settle on its follow-up turn")
            .expect("first delivery should succeed");
        // 断言 tool boundary 实际交付的 JSON face（与 spawn 的 child envelope 同构）。
        let payload = serde_json::to_value(&first_delivery).expect("delivery should serialize");
        assert_eq!(payload["outcome"], serde_json::json!("completed"));
        assert_eq!(payload["report"], serde_json::json!("first refined"));
        assert!(
            second_receiver.try_recv().is_err(),
            "the second waiter must wait for its own follow-up turn"
        );

        staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(child_event(
                target_id,
                AgentTurnId::new(target_id.get()),
                &target,
                finished_turn_event("second refined"),
            ));
        let _ = orchestrator.drain_child_events();
        let second_delivery = second_receiver
            .try_recv()
            .expect("second waiter should settle after the second follow-up turn")
            .expect("second delivery should succeed");
        let payload = serde_json::to_value(&second_delivery).expect("delivery should serialize");
        assert_eq!(payload["report"], serde_json::json!("second refined"));
    }

    #[test]
    fn stopping_a_child_fails_its_pending_message_waiters_closed() {
        let caller_id = AgentId::new(2);
        let target_id = AgentId::new(3);
        let target = RuntimeTarget::provider("local", "qwen3");
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        let caller_context =
            register_message_caller_child(&mut orchestrator, caller_id, AgentTurnId::new(20));
        let _staged = register_message_target_child(
            &mut orchestrator,
            caller_id,
            target_id,
            target,
            Vec::new(),
        );

        let mut receiver = child_caller_message_request(
            &mut orchestrator,
            caller_id,
            &caller_context,
            AgentTurnId::new(20),
            target_id,
            "work that will be cancelled",
        );
        orchestrator
            .stop_child(target_id)
            .expect("stopping the target child should converge");
        let failure = receiver
            .try_recv()
            .expect("disposal must settle the pending message waiter")
            .expect_err("waiter must receive a closed failure");
        // 显式 stop 的 waiter 结算携带可区分的 stopped 分类，模型能分辨"被停止"与
        // "目标不可用"。
        assert_eq!(failure, SendAgentMessageFailure::TargetStopped);
        assert_eq!(
            failure.delivery_message(),
            "send_agent_message target agent was stopped"
        );

        // 已 disposal 的 target 不再是可寻址消息目标。
        let mut receiver = child_caller_message_request(
            &mut orchestrator,
            caller_id,
            &caller_context,
            AgentTurnId::new(20),
            target_id,
            "after stop",
        );
        let failure = receiver
            .try_recv()
            .expect("stopped target should reject synchronously")
            .expect_err("unknown target must fail closed");
        assert!(matches!(failure, SendAgentMessageFailure::NotFound { .. }));
        assert!(
            failure
                .delivery_message()
                .contains("no child agent is available from this caller")
        );
    }

    /// 以 child caller 身份构造一次 `stop_agents` host request，并返回回执 receiver；
    /// caller 校验面与生产 MAIN caller 共用同一 identity 检查。
    fn caller_stop_request(
        orchestrator: &mut AgentOrchestrator,
        caller_id: AgentId,
        caller_context: &AgentCapabilityContext,
        caller_turn_id: AgentTurnId,
        target_id: AgentId,
    ) -> oneshot::Receiver<Result<AgentStopReceipt, StopAgentsFailure>> {
        let (response, receiver) = oneshot::channel();
        orchestrator.handle_stop_agents_request(StopAgentsRequest {
            identity: tool_runtime::ToolInvocationIdentity::new(
                caller_id.get(),
                caller_turn_id.get(),
                orchestrator.generation().get(),
                caller_context.epoch(),
            ),
            agent_id: target_id,
            response,
        });
        receiver
    }

    #[test]
    fn stop_agents_request_stops_active_subtree_and_returns_stopped_receipt() {
        let caller_id = AgentId::new(2);
        let target_id = AgentId::new(3);
        let grandchild_id = AgentId::new(4);
        let target = RuntimeTarget::provider("local", "qwen3");
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        let caller_context =
            register_message_caller_child(&mut orchestrator, caller_id, AgentTurnId::new(20));
        register_message_target_child(
            &mut orchestrator,
            caller_id,
            target_id,
            target.clone(),
            Vec::new(),
        );
        orchestrator.mark_child_launch_group_for_test(target_id, AgentLaunchGroupId::new(1));
        let (grandchild, _staged, _dispatched, _submitted) = ScriptedChildRuntime::new(Vec::new());
        orchestrator.register_child_for_test(
            grandchild_id,
            target_id,
            AgentTurnId::new(grandchild_id.get()),
            test_title("stop agents grandchild"),
            test_context("stop-agents-grandchild"),
            Box::new(grandchild),
        );

        let receipt = caller_stop_request(
            &mut orchestrator,
            caller_id,
            &caller_context,
            AgentTurnId::new(20),
            target_id,
        )
        .try_recv()
        .expect("stop receipt should settle synchronously")
        .expect("active child stop should succeed");
        assert_eq!(
            receipt,
            AgentStopReceipt::Stopped {
                agent_id: target_id,
                title: test_title("message target"),
            }
        );

        // subtree 完整清理：target 与 grandchild 的 authority 均释放；launch-group root
        // 的 stop 保留整个 subtree 的 terminal 投影行，caller 不受影响。
        assert!(!orchestrator.child_has_authority(target_id));
        assert!(!orchestrator.child_has_authority(grandchild_id));
        assert_eq!(orchestrator.child_count(), 3);
        assert_eq!(
            orchestrator.child_status(target_id),
            Some(AgentProjectionStatus::Cancelled)
        );

        // drain 交付 stop 定格的 terminal fact（Disposing 收敛后 exactly-once）。
        let events = orchestrator.drain_child_events();
        assert!(events.iter().any(|event| event.agent_id == target_id
            && matches!(event.kind, AgentEventKind::TurnInterrupted)));
        // grandchild 行随 subtree 级 retain intent 保留，但 fixture 未标 provider
        // target：disposal 不得为无 target 的行伪造 terminal fact。
        assert!(
            !events.iter().any(|event| event.agent_id == grandchild_id),
            "rows without a provider target must not synthesize a terminal fact"
        );
    }

    #[test]
    fn stop_agents_request_is_idempotent_for_settled_children() {
        let caller_id = AgentId::new(2);
        let target_id = AgentId::new(3);
        let target = RuntimeTarget::provider("local", "qwen3");
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        let caller_context =
            register_message_caller_child(&mut orchestrator, caller_id, AgentTurnId::new(20));
        register_message_target_child(
            &mut orchestrator,
            caller_id,
            target_id,
            target.clone(),
            vec![child_event(
                target_id,
                AgentTurnId::new(target_id.get()),
                &target,
                finished_turn_event("committed report"),
            )],
        );
        let _ = orchestrator.drain_child_events();
        assert!(orchestrator.child_has_authority(target_id));

        let expected = AgentStopReceipt::AlreadySettled {
            agent_id: target_id,
            title: test_title("message target"),
            outcome: AgentOutcome::Completed,
            summary: AgentOutcomeSummary::new("committed report").ok(),
        };
        // 已完成的 child 不报错：重复停止返回同一 already-settled 说明。
        for _ in 0..2 {
            let receipt = caller_stop_request(
                &mut orchestrator,
                caller_id,
                &caller_context,
                AgentTurnId::new(20),
                target_id,
            )
            .try_recv()
            .expect("settled stop should settle synchronously")
            .expect("settled child stop must be idempotent");
            assert_eq!(receipt, expected);
        }

        // settled 保留语义不受影响：runtime/context 仍是 followup 宿主。
        assert!(orchestrator.child_has_authority(target_id));
        assert_eq!(orchestrator.child_count(), 2);
    }

    #[test]
    fn stop_after_natural_terminal_keeps_the_generic_cancelled_summary() {
        let caller_id = AgentId::new(2);
        let target_id = AgentId::new(3);
        let target = RuntimeTarget::provider("local", "qwen3");
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        let caller_context =
            register_message_caller_child(&mut orchestrator, caller_id, AgentTurnId::new(20));
        register_message_target_child(
            &mut orchestrator,
            caller_id,
            target_id,
            target.clone(),
            vec![child_event(
                target_id,
                AgentTurnId::new(target_id.get()),
                &target,
                AgentEventKind::TurnInterrupted,
            )],
        );
        orchestrator.mark_child_launch_group_for_test(target_id, AgentLaunchGroupId::new(1));
        // adapter interrupt 等自然取消先定格 terminal（Settled）。
        let _ = orchestrator.drain_child_events();

        // 后补显式 stop（与 settled 淘汰同构的"已 terminal 再停"路径）：来源标志只在
        // terminal 定格块写入，既有 Cancelled 摘要不被改写为"被停止"。
        orchestrator
            .stop_child(target_id)
            .expect("stop on a terminal child should converge");

        let receipt = caller_stop_request(
            &mut orchestrator,
            caller_id,
            &caller_context,
            AgentTurnId::new(20),
            target_id,
        )
        .try_recv()
        .expect("post-terminal stop should settle synchronously")
        .expect("stop after a natural terminal stays idempotent");
        assert_eq!(
            receipt,
            AgentStopReceipt::AlreadySettled {
                agent_id: target_id,
                title: test_title("message target"),
                outcome: AgentOutcome::Cancelled,
                summary: AgentOutcomeSummary::new(CHILD_CANCELLED_TEXT).ok(),
            }
        );
    }

    #[test]
    fn stop_agent_deletes_settled_row_instead_of_retaining_projection() {
        // launch-group settled 行在 stop 语义下保留投影；用户 stop 走删除：
        // 完整清理 + registry 移除 + Remove delta。
        let agent_id = AgentId::new(2);
        let target = RuntimeTarget::provider("local", "qwen3");
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        orchestrator.register_child_for_test(
            agent_id,
            AgentId::MAIN,
            AgentTurnId::new(agent_id.get()),
            test_title("settled delete"),
            test_context("settled-delete"),
            Box::new(ShutdownCountingRuntime {
                events: vec![child_event(
                    agent_id,
                    AgentTurnId::new(agent_id.get()),
                    &target,
                    AgentEventKind::TurnFinished {
                        response: runtime_domain::session::ConversationResponse::assistant_text(
                            "settled report",
                        ),
                        metrics: None,
                        context_usage: None,
                    },
                )],
                shutdown_calls: Arc::new(AtomicUsize::new(0)),
            }),
        );
        orchestrator.mark_child_launch_group_for_test(agent_id, AgentLaunchGroupId::new(1));
        let _ = orchestrator.drain_child_events();
        assert_eq!(
            orchestrator.child_status(agent_id),
            Some(AgentProjectionStatus::Completed)
        );
        // Remove delta 只发布给存活 observation；先建立 overview observation 再删除。
        orchestrator.observe_agents(AgentObservationRequestId::new(1));
        let _ = orchestrator.drain_projection_events();

        orchestrator
            .stop_agent(agent_id, AgentRuntimeGeneration::new(1))
            .expect("settled delete should converge");

        assert_eq!(
            orchestrator.child_count(),
            0,
            "user stop on a settled row must remove the projection row"
        );
        let events = orchestrator.drain_projection_events();
        assert!(
            events.iter().any(|event| matches!(
                event,
                AgentProjectionEvent::AgentsOverviewUpdated { delta }
                    if matches!(
                        delta.kind,
                        AgentOverviewDeltaKind::Remove { agent_id } if agent_id == AgentId::new(2)
                    )
            )),
            "settled delete must publish an overview Remove delta: {events:?}"
        );
    }

    #[test]
    fn stop_agent_defers_settled_delete_while_the_group_report_is_pending() {
        // launch-group 的 completion 读取 registry 内全部 staged child 行：waiter
        // 未结算时删除任一 settled 成员会让 completion 永远无法凑齐。删除必须
        // 让位，report 交付后恢复可用。
        let settled_id = AgentId::new(2);
        let running_id = AgentId::new(3);
        let target = RuntimeTarget::provider("local", "qwen3");
        let group_id = AgentLaunchGroupId::new(1);
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        orchestrator.register_child_for_test(
            settled_id,
            AgentId::MAIN,
            AgentTurnId::new(settled_id.get()),
            test_title("settled sibling"),
            test_context("settled-sibling"),
            Box::new(ShutdownCountingRuntime {
                events: vec![child_event(
                    settled_id,
                    AgentTurnId::new(settled_id.get()),
                    &target,
                    finished_turn_event("early answer"),
                )],
                shutdown_calls: Arc::new(AtomicUsize::new(0)),
            }),
        );
        orchestrator.mark_child_launch_group_for_test(settled_id, group_id);
        let (running_runtime, staged_running, _dispatched, _submitted) =
            ScriptedChildRuntime::new(Vec::new());
        orchestrator.register_child_for_test(
            running_id,
            AgentId::MAIN,
            AgentTurnId::new(running_id.get()),
            test_title("running sibling"),
            test_context("running-sibling"),
            Box::new(running_runtime),
        );
        orchestrator.mark_child_launch_group_for_test(running_id, group_id);
        let _ = orchestrator.drain_child_events();
        assert_eq!(
            orchestrator.child_status(settled_id),
            Some(AgentProjectionStatus::Completed)
        );

        let (response, response_receiver) = oneshot::channel();
        orchestrator.group_waiters.insert(
            group_id,
            GroupWaiter {
                parent_agent_id: AgentId::MAIN,
                child_ids: vec![settled_id, running_id],
                response,
            },
        );

        // waiter 仍在等待：settled 行的删除被拒绝，行与 waiter 都保留。
        let rejection = orchestrator
            .stop_agent(settled_id, AgentRuntimeGeneration::new(1))
            .expect_err("delete must defer while the group report is pending");
        assert_eq!(rejection, AgentProductCommandRejection::ReportPending);
        assert_eq!(
            orchestrator.child_count(),
            2,
            "the settled row must stay until the group report is delivered"
        );

        // 兄弟 child 终态：同一 drain 内 waiter 先结算，随后删除恢复可用。
        staged_running
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(child_event(
                running_id,
                AgentTurnId::new(running_id.get()),
                &target,
                finished_turn_event("late answer"),
            ));
        let _ = orchestrator.drain_child_events();
        let completion = response_receiver
            .blocking_recv()
            .expect("group waiter should settle when the last child turns terminal")
            .expect("group completion should succeed");
        assert_eq!(completion.children.len(), 2);

        orchestrator
            .stop_agent(settled_id, AgentRuntimeGeneration::new(1))
            .expect("delete should converge once the group report is delivered");
        assert_eq!(
            orchestrator.child_count(),
            1,
            "only the running sibling row must remain after the deferred delete"
        );
    }

    #[test]
    fn stop_agent_on_active_child_keeps_terminal_projection_row() {
        // running 行语义保持 stop：terminal 投影按 launch-group 语义保留，不删除。
        let agent_id = AgentId::new(2);
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        orchestrator.register_child_for_test(
            agent_id,
            AgentId::MAIN,
            AgentTurnId::new(agent_id.get()),
            test_title("active stop"),
            test_context("active-stop"),
            Box::new(StubMainRuntime::default()),
        );
        orchestrator.mark_child_launch_group_for_test(agent_id, AgentLaunchGroupId::new(1));

        orchestrator
            .stop_agent(agent_id, AgentRuntimeGeneration::new(1))
            .expect("active stop should converge");

        assert_eq!(orchestrator.child_count(), 1);
        assert_eq!(
            orchestrator.child_status(agent_id),
            Some(AgentProjectionStatus::Cancelled)
        );
        assert!(!orchestrator.child_has_authority(agent_id));
    }

    #[test]
    fn stop_agent_deletes_disposed_terminal_projection_row() {
        // 第一段 stop 定格 Cancelled 并保留投影行（Disposed）；对同一行的第二次
        // stop 是删除请求：行移除。
        let agent_id = AgentId::new(2);
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        orchestrator.register_child_for_test(
            agent_id,
            AgentId::MAIN,
            AgentTurnId::new(agent_id.get()),
            test_title("disposed delete"),
            test_context("disposed-delete"),
            Box::new(StubMainRuntime::default()),
        );
        orchestrator.mark_child_launch_group_for_test(agent_id, AgentLaunchGroupId::new(1));

        orchestrator
            .stop_agent(agent_id, AgentRuntimeGeneration::new(1))
            .expect("first stop should converge");
        assert_eq!(orchestrator.child_count(), 1);

        orchestrator
            .stop_agent(agent_id, AgentRuntimeGeneration::new(1))
            .expect("second stop should delete the disposed projection row");

        assert_eq!(
            orchestrator.child_count(),
            0,
            "stop on a disposed projection row must delete it"
        );
    }

    #[test]
    fn stop_agents_request_fails_closed_for_unknown_main_and_stale_targets() {
        let caller_id = AgentId::new(2);
        let target_id = AgentId::new(3);
        let other_parent_child_id = AgentId::new(5);
        let target = RuntimeTarget::provider("local", "qwen3");
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        let caller_context =
            register_message_caller_child(&mut orchestrator, caller_id, AgentTurnId::new(20));
        register_message_target_child(
            &mut orchestrator,
            caller_id,
            target_id,
            target.clone(),
            Vec::new(),
        );
        let (runtime, _staged, _dispatched, _submitted) = ScriptedChildRuntime::new(Vec::new());
        orchestrator.register_child_for_test(
            other_parent_child_id,
            AgentId::MAIN,
            AgentTurnId::new(other_parent_child_id.get()),
            test_title("other parent child"),
            test_context("other-parent-child"),
            Box::new(runtime),
        );

        // unknown / MAIN / 其他 parent 的 child 一律 NotFound，附本 caller 可寻址 id。
        for unknown_id in [AgentId::new(99), AgentId::MAIN, other_parent_child_id] {
            let failure = caller_stop_request(
                &mut orchestrator,
                caller_id,
                &caller_context,
                AgentTurnId::new(20),
                unknown_id,
            )
            .try_recv()
            .expect("closed rejection should settle synchronously")
            .expect_err("non-addressable target must fail closed");
            match failure {
                StopAgentsFailure::NotFound {
                    available_agent_ids,
                } => assert_eq!(available_agent_ids, vec![target_id]),
                other => panic!("expected NotFound, got {other:?}"),
            }
        }

        // stale generation 与 parent turn 不匹配各自独立 closed。
        let (response, mut stale_receiver) = oneshot::channel();
        orchestrator.handle_stop_agents_request(StopAgentsRequest {
            identity: tool_runtime::ToolInvocationIdentity::new(
                caller_id.get(),
                AgentTurnId::new(20).get(),
                orchestrator.generation().get() + 1,
                caller_context.epoch(),
            ),
            agent_id: target_id,
            response,
        });
        assert_eq!(
            stale_receiver
                .try_recv()
                .expect("stale stop should settle synchronously"),
            Err(StopAgentsFailure::StaleGeneration)
        );

        let (response, mut wrong_turn_receiver) = oneshot::channel();
        orchestrator.handle_stop_agents_request(StopAgentsRequest {
            identity: tool_runtime::ToolInvocationIdentity::new(
                caller_id.get(),
                AgentTurnId::new(21).get(),
                orchestrator.generation().get(),
                caller_context.epoch(),
            ),
            agent_id: target_id,
            response,
        });
        assert_eq!(
            wrong_turn_receiver
                .try_recv()
                .expect("wrong-turn stop should settle synchronously"),
            Err(StopAgentsFailure::ParentUnavailable)
        );
    }

    #[test]
    fn stop_agents_request_reports_cleanup_pending_when_disposal_is_blocked() {
        let caller_id = AgentId::new(2);
        let target_id = AgentId::new(3);
        let target = RuntimeTarget::provider("local", "qwen3");
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        let caller_context =
            register_message_caller_child(&mut orchestrator, caller_id, AgentTurnId::new(20));
        orchestrator.register_child_for_test(
            target_id,
            caller_id,
            AgentTurnId::new(target_id.get()),
            test_title("blocked stop target"),
            test_context("blocked-stop-target"),
            Box::new(FailingShutdownRuntime {
                // 一次 stop 与 drain 内的 settle 重试都保持 blocked，terminal fact 持有。
                failures_remaining: 3,
                events: Vec::new(),
            }),
        );
        orchestrator.mark_child_target_for_test(target_id, target);

        let failure = caller_stop_request(
            &mut orchestrator,
            caller_id,
            &caller_context,
            AgentTurnId::new(20),
            target_id,
        )
        .try_recv()
        .expect("blocked stop should settle synchronously")
        .expect_err("blocked cleanup must surface as a closed failure");
        assert_eq!(failure, StopAgentsFailure::CleanupPending);
        assert_eq!(failure.delivery_message(), "stop_agents cleanup is pending");

        // owner 保留在同一 record：Disposing 未收敛时 terminal fact 持有，但显式停止
        // 定格的 outcome 照常持久化并投影。
        assert!(orchestrator.child_has_authority(target_id));
        assert!(
            orchestrator.drain_child_events().is_empty(),
            "terminal fact must be held while the stop cleanup is blocked"
        );
        let outcome = orchestrator
            .drain_projection_events()
            .into_iter()
            .find_map(|event| match event {
                AgentProjectionEvent::AgentOutcomeFact { snapshot } => Some(snapshot),
                _ => None,
            })
            .expect("stop must project the cancelled outcome fact");
        assert_eq!(outcome.agent_id, target_id);
        assert_eq!(outcome.outcome, AgentOutcome::Cancelled);
        assert_eq!(
            outcome.summary.as_ref().map(AgentOutcomeSummary::as_str),
            Some(CHILD_STOPPED_BY_REQUEST_TEXT)
        );
    }

    #[test]
    fn suspending_the_orchestrator_fails_pending_message_waiters() {
        let caller_id = AgentId::new(2);
        let target_id = AgentId::new(3);
        let target = RuntimeTarget::provider("local", "qwen3");
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        let caller_context =
            register_message_caller_child(&mut orchestrator, caller_id, AgentTurnId::new(20));
        let _staged = register_message_target_child(
            &mut orchestrator,
            caller_id,
            target_id,
            target,
            Vec::new(),
        );

        let mut receiver = child_caller_message_request(
            &mut orchestrator,
            caller_id,
            &caller_context,
            AgentTurnId::new(20),
            target_id,
            "message interrupted by suspend",
        );
        orchestrator.suspend().expect("suspend should converge");
        let failure = receiver
            .try_recv()
            .expect("suspend must settle every pending message waiter")
            .expect_err("waiter must receive a closed failure");
        assert_eq!(failure, SendAgentMessageFailure::TargetUnavailable);
    }

    #[test]
    fn session_transition_and_suspend_fully_dispose_settled_children() {
        let agent_id = AgentId::new(2);
        let turn_id = AgentTurnId::new(7);
        let target = RuntimeTarget::provider("local", "qwen3");
        let shutdown_calls = Arc::new(AtomicUsize::new(0));
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        let register_settled_child =
            |orchestrator: &mut AgentOrchestrator, shutdown_calls: Arc<AtomicUsize>| {
                orchestrator.register_child_for_test(
                    agent_id,
                    AgentId::MAIN,
                    turn_id,
                    test_title("settled disposal"),
                    test_context("settled-disposal"),
                    Box::new(ShutdownCountingRuntime {
                        events: vec![child_event(
                            agent_id,
                            turn_id,
                            &target,
                            AgentEventKind::TurnFinished {
                                response:
                                    runtime_domain::session::ConversationResponse::assistant_text(
                                        "settled answer",
                                    ),
                                metrics: None,
                                context_usage: None,
                            },
                        )],
                        shutdown_calls,
                    }),
                );
            };

        register_settled_child(&mut orchestrator, Arc::clone(&shutdown_calls));
        assert_eq!(orchestrator.drain_child_events().len(), 1);
        assert!(orchestrator.child_has_authority(agent_id));

        // session 切换对 settled child 立即完整清理，不经过 settled 保留。
        orchestrator
            .dispose_children_for_session_transition()
            .expect("session transition should fully dispose the settled child");
        assert!(!orchestrator.child_has_authority(agent_id));
        assert_eq!(orchestrator.child_count(), 0);
        assert_eq!(shutdown_calls.load(Ordering::SeqCst), 1);

        register_settled_child(&mut orchestrator, Arc::clone(&shutdown_calls));
        assert_eq!(orchestrator.drain_child_events().len(), 1);
        assert!(orchestrator.child_has_authority(agent_id));

        orchestrator
            .suspend()
            .expect("suspend should fully dispose the settled child");
        assert!(!orchestrator.child_has_authority(agent_id));
        assert_eq!(orchestrator.child_count(), 0);
        assert_eq!(shutdown_calls.load(Ordering::SeqCst), 2);
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
        assert_eq!(orchestrator.child_count(), 1);
        assert!(matches!(
            orchestrator.dispatch_child(AgentCommand::Interrupt {
                agent_id,
                target: None,
            }),
            Err(AgentRuntimeError::UnknownAgent)
        ));

        // dispatch 边界的 settle pass 以同一 owner 重试未收敛清理：inverse 第二次
        // 成功后 stop 意图完整完成（authority 释放 + 行移除）。
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert!(!orchestrator.child_has_authority(agent_id));
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

    /// 可分阶段注入 events、并记录 dispatched command 与提交 turn 文本的 child runtime fixture。
    type ScriptedEventQueue = Arc<Mutex<Vec<AgentEvent>>>;
    type ScriptedDispatchLog = Arc<Mutex<Vec<&'static str>>>;
    type ScriptedSubmittedTurns = Arc<Mutex<Vec<(AgentTurnId, String)>>>;

    struct ScriptedChildRuntime {
        events: ScriptedEventQueue,
        dispatched: ScriptedDispatchLog,
        submitted_turns: ScriptedSubmittedTurns,
        is_shutdown: bool,
    }

    impl ScriptedChildRuntime {
        fn new(
            events: Vec<AgentEvent>,
        ) -> (
            Self,
            ScriptedEventQueue,
            ScriptedDispatchLog,
            ScriptedSubmittedTurns,
        ) {
            let events = Arc::new(Mutex::new(events));
            let dispatched = Arc::new(Mutex::new(Vec::new()));
            let submitted_turns = Arc::new(Mutex::new(Vec::new()));
            (
                Self {
                    events: Arc::clone(&events),
                    dispatched: Arc::clone(&dispatched),
                    submitted_turns: Arc::clone(&submitted_turns),
                    is_shutdown: false,
                },
                events,
                dispatched,
                submitted_turns,
            )
        }
    }

    impl AgentRuntime for ScriptedChildRuntime {
        fn dispatch(
            &mut self,
            command: AgentCommand,
        ) -> Result<AgentCommandReceipt, AgentRuntimeError> {
            if self.is_shutdown {
                return Err(AgentRuntimeError::Disposed);
            }
            let label = match &command {
                AgentCommand::SubmitTurn { .. } => "submit_turn",
                AgentCommand::Interrupt { .. } => "interrupt",
                AgentCommand::RespondPermission { .. } => "respond_permission",
                // SendMessage 由 orchestrator 路由，adapter 边界 fail closed。
                AgentCommand::SendMessage { .. } => {
                    return Err(AgentRuntimeError::CommandRejected(
                        "child runtime does not route messages".to_string(),
                    ));
                }
            };
            self.dispatched
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(label);
            Ok(match command {
                AgentCommand::SubmitTurn {
                    turn_id, request, ..
                } => {
                    self.submitted_turns
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push((
                            turn_id,
                            request.conversation_request().message_text().to_string(),
                        ));
                    AgentCommandReceipt::TurnStarted {
                        turn_id,
                        target: request.target(),
                        activity_label: request.activity_label().to_string(),
                    }
                }
                AgentCommand::Interrupt { target, .. } => {
                    AgentCommandReceipt::Interrupted { target }
                }
                AgentCommand::RespondPermission { .. } => AgentCommandReceipt::Accepted,
                AgentCommand::SendMessage { .. } => {
                    unreachable!("SendMessage is rejected before the dispatch log records it")
                }
            })
        }

        fn drain_events(&mut self) -> Vec<AgentEvent> {
            std::mem::take(
                &mut *self
                    .events
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            )
        }

        fn shutdown(&mut self) -> Result<(), AgentRuntimeError> {
            self.is_shutdown = true;
            Ok(())
        }
    }

    impl AgentRuntimePort for ScriptedChildRuntime {
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

    fn child_event(
        agent_id: AgentId,
        turn_id: AgentTurnId,
        target: &RuntimeTarget,
        kind: AgentEventKind,
    ) -> AgentEvent {
        AgentEvent {
            agent_id,
            turn_id,
            target: target.clone(),
            kind,
        }
    }

    fn permission_request(request_id: &str) -> runtime_domain::session::RuntimePermissionRequest {
        runtime_domain::session::RuntimePermissionRequest::new(
            request_id,
            Some("Run shell command".to_string()),
            vec![
                runtime_domain::session::RuntimePermissionOption::new(
                    "allow-1",
                    "Allow once",
                    runtime_domain::session::RuntimePermissionOptionKind::AllowOnce,
                ),
                runtime_domain::session::RuntimePermissionOption::new(
                    "reject-1",
                    "Reject once",
                    runtime_domain::session::RuntimePermissionOptionKind::RejectOnce,
                ),
            ],
        )
    }

    fn permission_target(
        agent_id: AgentId,
        turn_id: AgentTurnId,
        generation: AgentRuntimeGeneration,
        runtime_target: &RuntimeTarget,
        request_id: &str,
    ) -> AgentPermissionTarget {
        AgentPermissionTarget {
            agent_id,
            turn_id,
            generation,
            runtime_target: runtime_target.clone(),
            request_id: request_id.to_string(),
        }
    }

    fn permission_updates(events: &[AgentProjectionEvent]) -> Vec<AgentPermissionUpdate> {
        events
            .iter()
            .filter_map(|event| match event {
                AgentProjectionEvent::AgentPermissionUpdated { update } => Some(update.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn overview_observation_delivers_snapshot_then_ordered_monotonic_deltas() {
        let agent_id = AgentId::new(2);
        let turn_id = AgentTurnId::new(7);
        let target = RuntimeTarget::provider("local", "qwen3");
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        orchestrator.register_child_for_test(
            agent_id,
            AgentId::MAIN,
            turn_id,
            test_title("observed child"),
            test_context("observed-child"),
            Box::new(StubMainRuntime {
                events: vec![
                    child_event(
                        agent_id,
                        turn_id,
                        &target,
                        AgentEventKind::AssistantDelta {
                            content: "streaming partial".to_string(),
                        },
                    ),
                    child_event(
                        agent_id,
                        turn_id,
                        &target,
                        AgentEventKind::TurnFinished {
                            response: runtime_domain::session::ConversationResponse::assistant_text(
                                "committed answer",
                            ),
                            metrics: None,
                            context_usage: None,
                        },
                    ),
                ],
            }),
        );

        orchestrator.observe_agents(AgentObservationRequestId::new(11));
        let snapshot_events = orchestrator.drain_projection_events();
        assert_eq!(snapshot_events.len(), 1);
        let (request_id, snapshot) = match &snapshot_events[0] {
            AgentProjectionEvent::AgentsOverviewSnapshotLoaded {
                request_id,
                snapshot,
            } => (*request_id, snapshot),
            other => panic!("expected overview snapshot, got {other:?}"),
        };
        assert_eq!(request_id, AgentObservationRequestId::new(11));
        assert_eq!(snapshot.rows.len(), 1);
        assert_eq!(snapshot.rows[0].status, AgentProjectionStatus::Pending);
        let observation_id = snapshot.observation_id;
        let snapshot_revision = snapshot.revision;

        let _ = orchestrator.drain_child_events();
        let events = orchestrator.drain_projection_events();
        // 同一 drain 边界先交付 observation deltas，再交付 sessionless outcome document fact
        //（no-op append 照常 push）；deltas 过滤后仍须严格单调。
        assert!(
            events
                .iter()
                .any(|event| matches!(event, AgentProjectionEvent::AgentOutcomeFact { .. }))
        );
        let deltas = events
            .into_iter()
            .filter_map(|event| match event {
                AgentProjectionEvent::AgentsOverviewUpdated { delta } => Some(delta),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(!deltas.is_empty());
        let mut previous_revision = snapshot_revision;
        for delta in &deltas {
            assert_eq!(delta.observation_id, observation_id);
            assert!(
                delta.revision > previous_revision,
                "revision must be strict"
            );
            previous_revision = delta.revision;
            assert!(matches!(delta.kind, AgentOverviewDeltaKind::Upsert(_)));
        }
        let last_delta = deltas.last().expect("overview deltas should not be empty");
        match &last_delta.kind {
            AgentOverviewDeltaKind::Upsert(row) => {
                assert_eq!(row.status, AgentProjectionStatus::Completed);
            }
            other => panic!("expected upsert delta, got {other:?}"),
        }

        orchestrator.stop_observation(observation_id, orchestrator.generation());
        orchestrator.register_child_for_test(
            AgentId::new(3),
            AgentId::MAIN,
            AgentTurnId::new(8),
            test_title("later child"),
            test_context("later-child"),
            Box::new(StubMainRuntime::default()),
        );
        assert!(
            orchestrator.drain_projection_events().is_empty(),
            "stopped observation must not receive fresh deltas"
        );
    }

    #[test]
    fn overview_remove_delta_and_view_observation_fail_closed_on_child_removal() {
        let agent_id = AgentId::new(2);
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        orchestrator.register_child_for_test(
            agent_id,
            AgentId::MAIN,
            AgentTurnId::new(1),
            test_title("removed child"),
            test_context("removed-child"),
            Box::new(StubMainRuntime::default()),
        );
        orchestrator.observe_agents(AgentObservationRequestId::new(1));
        orchestrator.observe_agent_transcript(AgentObservationRequestId::new(2), agent_id);
        assert_eq!(orchestrator.drain_projection_events().len(), 2);

        orchestrator
            .stop_child(agent_id)
            .expect("child disposal should converge");
        let events = orchestrator.drain_projection_events();
        let remove_deltas = events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    AgentProjectionEvent::AgentsOverviewUpdated {
                        delta: AgentOverviewDelta {
                            kind: AgentOverviewDeltaKind::Remove { .. },
                            ..
                        }
                    }
                )
            })
            .count();
        assert_eq!(remove_deltas, 1, "expected exactly one Remove delta");
        assert_eq!(orchestrator.child_count(), 0);

        // 移除后的 child 不再产生 view snapshot；overview observation 继续存活。
        orchestrator.register_child_for_test(
            AgentId::new(3),
            AgentId::MAIN,
            AgentTurnId::new(2),
            test_title("replacement child"),
            test_context("replacement-child"),
            Box::new(StubMainRuntime::default()),
        );
        let events = orchestrator.drain_projection_events();
        assert!(
            events
                .iter()
                .any(|event| matches!(event, AgentProjectionEvent::AgentsOverviewUpdated { .. }))
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, AgentProjectionEvent::AgentViewUpdated { .. })),
            "view observation bound to a removed child must fail closed"
        );
    }

    #[test]
    fn observations_fail_closed_after_replacement_suspend_and_session_transition() {
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        orchestrator.observe_agents(AgentObservationRequestId::new(1));
        assert_eq!(orchestrator.observation_count(), 1);
        orchestrator
            .suspend()
            .expect("clean suspend should invalidate observations");
        assert_eq!(orchestrator.observation_count(), 0);

        orchestrator.observe_agents(AgentObservationRequestId::new(2));
        orchestrator
            .replace_main(Box::new(StubMainRuntime::default()), None, None)
            .expect("clean replacement should invalidate observations");
        assert_eq!(orchestrator.observation_count(), 0);

        orchestrator.register_child_for_test(
            AgentId::new(2),
            AgentId::MAIN,
            AgentTurnId::new(1),
            test_title("session transition child"),
            test_context("session-transition-child"),
            Box::new(StubMainRuntime::default()),
        );
        orchestrator.observe_agents(AgentObservationRequestId::new(3));
        orchestrator
            .dispose_children_for_session_transition()
            .expect("session transition should converge child cleanup");
        assert_eq!(orchestrator.observation_count(), 0);

        // 失效后的 observation id 收不到 fresh delta。
        orchestrator.observe_agents(AgentObservationRequestId::new(4));
        let _ = orchestrator.drain_projection_events();
        orchestrator.stop_observation(AgentObservationId::new(1), AgentRuntimeGeneration::new(1));
        assert_eq!(orchestrator.observation_count(), 1);
        assert!(orchestrator.drain_projection_events().is_empty());
    }

    #[test]
    fn observe_and_stop_do_not_change_child_authority_or_permission_projection() {
        let agent_id = AgentId::new(2);
        let turn_id = AgentTurnId::new(7);
        let target = RuntimeTarget::provider("local", "qwen3");
        let (runtime, staged_events, dispatched, _submitted_turns) =
            ScriptedChildRuntime::new(vec![
                child_event(
                    agent_id,
                    turn_id,
                    &target,
                    AgentEventKind::PermissionRequested {
                        request: permission_request("perm-1"),
                    },
                ),
                child_event(
                    agent_id,
                    turn_id,
                    &target,
                    AgentEventKind::TurnFinished {
                        response: runtime_domain::session::ConversationResponse::assistant_text(
                            "committed answer",
                        ),
                        metrics: None,
                        context_usage: None,
                    },
                ),
            ]);
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        orchestrator.register_child_for_test(
            agent_id,
            AgentId::MAIN,
            turn_id,
            test_title("observed child"),
            test_context("observed-child"),
            Box::new(runtime),
        );

        // 打开 observation 后立即撤销；observation 是纯 projection，不触碰 child authority。
        orchestrator.observe_agents(AgentObservationRequestId::new(1));
        orchestrator.observe_agent_transcript(AgentObservationRequestId::new(2), agent_id);
        assert!(orchestrator.child_has_authority(agent_id));
        let observation_ids = orchestrator
            .drain_projection_events()
            .iter()
            .filter_map(|event| match event {
                AgentProjectionEvent::AgentsOverviewSnapshotLoaded { snapshot, .. } => {
                    Some(snapshot.observation_id)
                }
                AgentProjectionEvent::AgentViewSnapshotLoaded { snapshot, .. } => {
                    Some(snapshot.observation_id)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(observation_ids.len(), 2);
        for observation_id in observation_ids {
            orchestrator.stop_observation(observation_id, orchestrator.generation());
        }
        assert!(orchestrator.child_has_authority(agent_id));
        assert!(
            dispatched
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
        );

        let accepted = orchestrator.drain_child_events();
        assert_eq!(accepted.len(), 2);
        assert!(matches!(
            accepted[0].kind,
            AgentEventKind::PermissionRequested { .. }
        ));
        assert!(accepted[1].kind.is_terminal());
        assert_eq!(
            orchestrator.child_status(agent_id),
            Some(AgentProjectionStatus::Completed)
        );
        // settled child 保留 runtime/context 作 followup 宿主；投影只反映 terminal 定格。
        assert!(orchestrator.child_has_authority(agent_id));

        // permission 投影由 child fact 驱动，与 observation 打开/关闭无关。
        let updates = permission_updates(&orchestrator.drain_projection_events());
        assert_eq!(updates.len(), 2);
        assert_eq!(updates[0].agent_id, agent_id);
        assert!(updates[0].request.is_some());
        assert_eq!(updates[1].request, None);
        assert!(
            staged_events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
        );
    }

    #[test]
    fn transcript_projection_accumulates_committed_facts_only() {
        let agent_id = AgentId::new(2);
        let turn_id = AgentTurnId::new(7);
        let target = RuntimeTarget::provider("local", "qwen3");
        let tool_activity = runtime_domain::session::RuntimeToolActivity {
            activity_id: "tool-1".to_string(),
            title: "Read file".to_string(),
            kind: runtime_domain::session::RuntimeToolKind::Read,
            status: runtime_domain::session::RuntimeToolActivityStatus::InProgress,
            content: vec![runtime_domain::session::RuntimeToolActivityContent::Text(
                "safe content".to_string(),
            )],
            locations: Vec::new(),
            raw_input: Some(runtime_domain::session::RuntimeToolActivityRawValue::from(
                serde_json::json!({"secret": "PRIVATE_RAW_INPUT"}),
            )),
            raw_output: None,
        };
        let tool_update = runtime_domain::session::RuntimeToolActivityUpdate {
            activity_id: "tool-1".to_string(),
            content: Some(vec![
                runtime_domain::session::RuntimeToolActivityContent::Text(
                    "updated safe content".to_string(),
                ),
            ]),
            ..runtime_domain::session::RuntimeToolActivityUpdate::default()
        };
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        orchestrator.register_child_for_test(
            agent_id,
            AgentId::MAIN,
            turn_id,
            test_title("transcript child"),
            test_context("transcript-child"),
            Box::new(StubMainRuntime {
                events: vec![
                    child_event(
                        agent_id,
                        turn_id,
                        &target,
                        AgentEventKind::AssistantDelta {
                            content: "streaming partial".to_string(),
                        },
                    ),
                    child_event(
                        agent_id,
                        turn_id,
                        &target,
                        AgentEventKind::ToolActivityStarted {
                            activity: tool_activity,
                        },
                    ),
                    child_event(
                        agent_id,
                        turn_id,
                        &target,
                        AgentEventKind::ToolActivityUpdated {
                            update: tool_update,
                        },
                    ),
                    child_event(
                        agent_id,
                        turn_id,
                        &target,
                        AgentEventKind::TurnFinished {
                            response: runtime_domain::session::ConversationResponse::assistant_text(
                                "committed answer",
                            ),
                            metrics: None,
                            context_usage: None,
                        },
                    ),
                    child_event(
                        agent_id,
                        turn_id,
                        &target,
                        AgentEventKind::AssistantDelta {
                            content: "late partial".to_string(),
                        },
                    ),
                ],
            }),
        );

        orchestrator.observe_agent_transcript(AgentObservationRequestId::new(5), agent_id);
        let loaded = orchestrator.drain_projection_events();
        assert!(matches!(
            loaded.as_slice(),
            [AgentProjectionEvent::AgentViewSnapshotLoaded { .. }]
        ));

        let _ = orchestrator.drain_child_events();
        let updates = orchestrator
            .drain_projection_events()
            .iter()
            .filter_map(|event| match event {
                AgentProjectionEvent::AgentViewUpdated { snapshot } => Some(snapshot.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        let latest = updates
            .last()
            .expect("view observation should receive updated snapshots");
        assert_eq!(
            latest.transcript.items,
            vec![
                AgentTranscriptItem::Tool {
                    title: "Read file".to_string(),
                    content: "updated safe content".to_string(),
                },
                AgentTranscriptItem::Assistant {
                    content: "committed answer".to_string(),
                },
            ]
        );
        assert_eq!(
            latest.preview.latest_committed_answer,
            Some("committed answer".to_string())
        );
        let transcript_debug = format!("{latest:?}");
        assert!(!transcript_debug.contains("streaming partial"));
        assert!(!transcript_debug.contains("late partial"));
        assert!(!transcript_debug.contains("PRIVATE_RAW_INPUT"));
    }

    #[test]
    fn permission_fifo_enqueues_in_order_and_projects_head() {
        let agent_id = AgentId::new(2);
        let turn_id = AgentTurnId::new(7);
        let target = RuntimeTarget::provider("local", "qwen3");
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        orchestrator.register_child_for_test(
            agent_id,
            AgentId::MAIN,
            turn_id,
            test_title("permission child"),
            test_context("permission-child"),
            Box::new(
                ScriptedChildRuntime::new(vec![
                    child_event(
                        agent_id,
                        turn_id,
                        &target,
                        AgentEventKind::PermissionRequested {
                            request: permission_request("perm-1"),
                        },
                    ),
                    child_event(
                        agent_id,
                        turn_id,
                        &target,
                        AgentEventKind::PermissionRequested {
                            request: permission_request("perm-2"),
                        },
                    ),
                ])
                .0,
            ),
        );

        let _ = orchestrator.drain_child_events();
        // 未打开任何 observation 时 permission 投影仍然交付。
        let updates = permission_updates(&orchestrator.drain_projection_events());
        assert_eq!(updates.len(), 2);
        for update in &updates {
            assert_eq!(update.agent_id, agent_id);
            assert_eq!(update.generation, orchestrator.generation());
            let head = update.request.as_ref().expect("head should be pending");
            assert_eq!(head.target.request_id, "perm-1");
            assert_eq!(head.state, AgentPermissionState::Pending);
            assert_eq!(head.target.turn_id, turn_id);
            assert_eq!(head.target.runtime_target, target);
        }
    }

    #[test]
    fn duplicate_permission_request_id_is_ignored_within_the_same_turn() {
        let agent_id = AgentId::new(2);
        let turn_id = AgentTurnId::new(7);
        let target = RuntimeTarget::provider("local", "qwen3");
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        orchestrator.register_child_for_test(
            agent_id,
            AgentId::MAIN,
            turn_id,
            test_title("duplicate permission"),
            test_context("duplicate-permission"),
            Box::new(
                ScriptedChildRuntime::new(vec![
                    child_event(
                        agent_id,
                        turn_id,
                        &target,
                        AgentEventKind::PermissionRequested {
                            request: permission_request("perm-1"),
                        },
                    ),
                    child_event(
                        agent_id,
                        turn_id,
                        &target,
                        AgentEventKind::PermissionRequested {
                            request: permission_request("perm-1"),
                        },
                    ),
                ])
                .0,
            ),
        );

        let _ = orchestrator.drain_child_events();
        let updates = permission_updates(&orchestrator.drain_projection_events());
        assert_eq!(
            updates.len(),
            1,
            "duplicate request id must not enqueue twice"
        );
    }

    #[test]
    fn respond_agent_permission_validates_identity_and_option_before_dispatch() {
        let agent_id = AgentId::new(2);
        let turn_id = AgentTurnId::new(7);
        let target = RuntimeTarget::provider("local", "qwen3");
        let (runtime, _staged, dispatched, _submitted_turns) =
            ScriptedChildRuntime::new(vec![child_event(
                agent_id,
                turn_id,
                &target,
                AgentEventKind::PermissionRequested {
                    request: permission_request("perm-1"),
                },
            )]);
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        orchestrator.register_child_for_test(
            agent_id,
            AgentId::MAIN,
            turn_id,
            test_title("respond child"),
            test_context("respond-child"),
            Box::new(runtime),
        );
        let _ = orchestrator.drain_child_events();
        let _ = orchestrator.drain_projection_events();
        let generation = orchestrator.generation();

        let valid_target = || permission_target(agent_id, turn_id, generation, &target, "perm-1");

        // option_id: None 是封闭语义，runtime 校验层直接拒绝。
        assert_eq!(
            orchestrator.respond_agent_permission(valid_target(), None),
            Err(AgentProductCommandRejection::InvalidOption)
        );
        assert_eq!(
            orchestrator.respond_agent_permission(valid_target(), Some("unknown-option".into())),
            Err(AgentProductCommandRejection::InvalidOption)
        );
        assert_eq!(
            orchestrator.respond_agent_permission(
                permission_target(agent_id, turn_id, generation, &target, "perm-unknown"),
                Some("allow-1".into()),
            ),
            Err(AgentProductCommandRejection::UnknownRequest)
        );
        assert_eq!(
            orchestrator.respond_agent_permission(
                permission_target(AgentId::new(99), turn_id, generation, &target, "perm-1"),
                Some("allow-1".into()),
            ),
            Err(AgentProductCommandRejection::UnknownAgent)
        );
        assert_eq!(
            orchestrator.respond_agent_permission(
                permission_target(
                    agent_id,
                    turn_id,
                    AgentRuntimeGeneration::new(generation.get() + 1),
                    &target,
                    "perm-1"
                ),
                Some("allow-1".into()),
            ),
            Err(AgentProductCommandRejection::StaleGeneration)
        );
        assert!(
            dispatched
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
        );
        let _ = orchestrator.drain_projection_events();

        orchestrator
            .respond_agent_permission(valid_target(), Some("allow-1".into()))
            .expect("valid response should dispatch to the child runtime");
        assert_eq!(
            *dispatched
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec!["respond_permission"]
        );
        let updates = permission_updates(&orchestrator.drain_projection_events());
        assert_eq!(updates.len(), 1);
        let head = updates[0]
            .request
            .as_ref()
            .expect("submitted head should stay projected");
        assert_eq!(head.target.request_id, "perm-1");
        assert_eq!(head.state, AgentPermissionState::Submitted);

        // 重复提交同一 request fail closed，不再触碰 child runtime。
        assert_eq!(
            orchestrator.respond_agent_permission(valid_target(), Some("allow-1".into())),
            Err(AgentProductCommandRejection::AlreadySubmitted)
        );
        assert_eq!(
            *dispatched
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec!["respond_permission"]
        );
    }

    #[test]
    fn submitted_permission_converges_on_next_fact_and_clears_on_terminal() {
        let agent_id = AgentId::new(2);
        let turn_id = AgentTurnId::new(7);
        let target = RuntimeTarget::provider("local", "qwen3");
        let (runtime, staged, _dispatched, _submitted_turns) =
            ScriptedChildRuntime::new(vec![child_event(
                agent_id,
                turn_id,
                &target,
                AgentEventKind::PermissionRequested {
                    request: permission_request("perm-1"),
                },
            )]);
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        orchestrator.register_child_for_test(
            agent_id,
            AgentId::MAIN,
            turn_id,
            test_title("converge child"),
            test_context("converge-child"),
            Box::new(runtime),
        );
        let _ = orchestrator.drain_child_events();
        let _ = orchestrator.drain_projection_events();
        let generation = orchestrator.generation();
        orchestrator
            .respond_agent_permission(
                permission_target(agent_id, turn_id, generation, &target, "perm-1"),
                Some("allow-1".into()),
            )
            .expect("response should submit the head entry");
        let _ = orchestrator.drain_projection_events();

        // 第二个 permission 进入 FIFO；新 permission fact 收敛已 Submitted 的 head。
        staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend(vec![
                child_event(
                    agent_id,
                    turn_id,
                    &target,
                    AgentEventKind::PermissionRequested {
                        request: permission_request("perm-2"),
                    },
                ),
                child_event(
                    agent_id,
                    turn_id,
                    &target,
                    AgentEventKind::ToolActivityStarted {
                        activity: runtime_domain::session::RuntimeToolActivity {
                            activity_id: "tool-1".to_string(),
                            title: "Read file".to_string(),
                            kind: runtime_domain::session::RuntimeToolKind::Read,
                            status: runtime_domain::session::RuntimeToolActivityStatus::InProgress,
                            content: Vec::new(),
                            locations: Vec::new(),
                            raw_input: None,
                            raw_output: None,
                        },
                    },
                ),
            ]);
        let _ = orchestrator.drain_child_events();
        let updates = permission_updates(&orchestrator.drain_projection_events());
        // perm-2 enqueue 在同一 fact 内收敛已 Submitted 的 perm-1 并推进 head；
        // 随后的 tool activity 对 Pending head 是 no-op，不产生额外投影。
        assert_eq!(updates.len(), 1);
        let head = updates[0]
            .request
            .as_ref()
            .expect("perm-2 should become head");
        assert_eq!(head.target.request_id, "perm-2");
        assert_eq!(head.state, AgentPermissionState::Pending);
        let _ = orchestrator.drain_projection_events();

        orchestrator
            .respond_agent_permission(
                permission_target(agent_id, turn_id, generation, &target, "perm-2"),
                Some("reject-1".into()),
            )
            .expect("second response should submit");
        let _ = orchestrator.drain_projection_events();

        // terminal fact 清空整个 queue 并投影 None。
        staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(child_event(
                agent_id,
                turn_id,
                &target,
                AgentEventKind::TurnFinished {
                    response: runtime_domain::session::ConversationResponse::assistant_text(
                        "committed answer",
                    ),
                    metrics: None,
                    context_usage: None,
                },
            ));
        let _ = orchestrator.drain_child_events();
        let updates = permission_updates(&orchestrator.drain_projection_events());
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].request, None);

        // terminal 后 admission 关闭，再 respond fail closed。
        assert_eq!(
            orchestrator.respond_agent_permission(
                permission_target(agent_id, turn_id, generation, &target, "perm-2"),
                Some("allow-1".into()),
            ),
            Err(AgentProductCommandRejection::UnknownAgent)
        );
    }

    #[test]
    fn stop_child_clears_pending_permission_queue() {
        let agent_id = AgentId::new(2);
        let turn_id = AgentTurnId::new(7);
        let target = RuntimeTarget::provider("local", "qwen3");
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        orchestrator.register_child_for_test(
            agent_id,
            AgentId::MAIN,
            turn_id,
            test_title("stop with permission"),
            test_context("stop-with-permission"),
            Box::new(
                ScriptedChildRuntime::new(vec![child_event(
                    agent_id,
                    turn_id,
                    &target,
                    AgentEventKind::PermissionRequested {
                        request: permission_request("perm-1"),
                    },
                )])
                .0,
            ),
        );
        let _ = orchestrator.drain_child_events();
        let _ = orchestrator.drain_projection_events();

        orchestrator
            .stop_child(agent_id)
            .expect("stop should converge");
        let updates = permission_updates(&orchestrator.drain_projection_events());
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].agent_id, agent_id);
        assert_eq!(updates[0].request, None);
    }

    #[test]
    fn permission_updates_are_scoped_per_agent() {
        let first_id = AgentId::new(2);
        let second_id = AgentId::new(3);
        let first_turn = AgentTurnId::new(21);
        let second_turn = AgentTurnId::new(22);
        let target = RuntimeTarget::provider("local", "qwen3");
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        orchestrator.register_child_for_test(
            first_id,
            AgentId::MAIN,
            first_turn,
            test_title("first child"),
            test_context("first-child"),
            Box::new(
                ScriptedChildRuntime::new(vec![child_event(
                    first_id,
                    first_turn,
                    &target,
                    AgentEventKind::PermissionRequested {
                        request: permission_request("perm-first"),
                    },
                )])
                .0,
            ),
        );
        orchestrator.register_child_for_test(
            second_id,
            AgentId::MAIN,
            second_turn,
            test_title("second child"),
            test_context("second-child"),
            Box::new(
                ScriptedChildRuntime::new(vec![child_event(
                    second_id,
                    second_turn,
                    &target,
                    AgentEventKind::PermissionRequested {
                        request: permission_request("perm-second"),
                    },
                )])
                .0,
            ),
        );

        let _ = orchestrator.drain_child_events();
        let updates = permission_updates(&orchestrator.drain_projection_events());
        assert_eq!(updates.len(), 2);
        assert_eq!(updates[0].agent_id, first_id);
        assert_eq!(
            updates[0]
                .request
                .as_ref()
                .expect("first child head should be pending")
                .target
                .request_id,
            "perm-first"
        );
        assert_eq!(updates[1].agent_id, second_id);
        assert_eq!(
            updates[1]
                .request
                .as_ref()
                .expect("second child head should be pending")
                .target
                .request_id,
            "perm-second"
        );

        // 跨 agent 的 request id 不共享 FIFO；用 first child 的 request 回复 second child fail closed。
        assert_eq!(
            orchestrator.respond_agent_permission(
                permission_target(
                    second_id,
                    second_turn,
                    orchestrator.generation(),
                    &target,
                    "perm-first"
                ),
                Some("allow-1".into()),
            ),
            Err(AgentProductCommandRejection::UnknownRequest)
        );
    }

    #[test]
    fn stop_agent_validates_generation_and_stops_subtree() {
        let root_context = test_context("stop-agent-root");
        let child_context = root_context
            .child(
                AgentContextOwner::try_new("stop-agent-child").unwrap(),
                AgentChildCapabilityGrants::empty(),
            )
            .unwrap();
        let grandchild_context = child_context
            .child(
                AgentContextOwner::try_new("stop-agent-grandchild").unwrap(),
                AgentChildCapabilityGrants::empty(),
            )
            .unwrap();
        let order = Arc::new(Mutex::new(Vec::new()));
        let mut orchestrator =
            AgentOrchestrator::new(Box::new(StubMainRuntime::default()), None, None);
        orchestrator.register_child_for_test(
            AgentId::new(2),
            AgentId::MAIN,
            AgentTurnId::new(1),
            test_title("stop agent child"),
            child_context.clone(),
            Box::new(RecordingShutdownRuntime {
                label: "child",
                context: child_context,
                order: Arc::clone(&order),
            }),
        );
        orchestrator.register_child_for_test(
            AgentId::new(3),
            AgentId::new(2),
            AgentTurnId::new(2),
            test_title("stop agent grandchild"),
            grandchild_context.clone(),
            Box::new(RecordingShutdownRuntime {
                label: "grandchild",
                context: grandchild_context,
                order: Arc::clone(&order),
            }),
        );
        let generation = orchestrator.generation();

        assert_eq!(
            orchestrator.stop_agent(AgentId::MAIN, generation),
            Err(AgentProductCommandRejection::UnknownAgent)
        );
        assert_eq!(
            orchestrator.stop_agent(AgentId::new(99), generation),
            Err(AgentProductCommandRejection::UnknownAgent)
        );
        assert_eq!(
            orchestrator.stop_agent(
                AgentId::new(2),
                AgentRuntimeGeneration::new(generation.get() + 1)
            ),
            Err(AgentProductCommandRejection::StaleGeneration)
        );

        orchestrator
            .stop_agent(AgentId::new(2), generation)
            .expect("typed stop should reuse the descendants-first path");
        assert_eq!(orchestrator.child_count(), 0);
        assert_eq!(
            *order
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec!["grandchild", "child"]
        );
    }
}
