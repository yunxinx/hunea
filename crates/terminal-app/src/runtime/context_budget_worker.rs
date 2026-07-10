use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
    mpsc,
};
use std::thread::{self, JoinHandle};

use conversation_runtime::context_budget::{
    ContextBudgetProbe, build_context_budget_snapshot_with_cancellation,
    context_budget_tool_definitions,
};
use conversation_runtime::{ConversationItem, RuntimeEventNotifier, ToolDefinition};
use runtime_domain::{
    context_budget::{ContextBudgetSnapshot, ContextTokenLimit},
    prompt_assembly::PromptPreludeSnapshot,
    provider::ProviderKind,
    session::{ContextBudgetLoadErrorPayload, RuntimeEvent, SessionLoadRequestId},
};
use tool_runtime::ToolExecutorRegistry;

pub(super) struct ContextBudgetWorker {
    current_generation: Arc<AtomicU64>,
    worker_tx: Option<mpsc::Sender<WorkerControl>>,
    result_rx: mpsc::Receiver<ContextBudgetTaskEnvelope>,
    worker_thread: Option<JoinHandle<()>>,
    active_request: Option<ActiveContextBudgetRequest>,
    queued_command: Option<ContextBudgetWorkerCommand>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ActiveContextBudgetRequest {
    request_id: SessionLoadRequestId,
    generation: u64,
}

#[derive(Debug)]
pub(super) struct ContextBudgetSnapshotRequest {
    pub(super) request_id: SessionLoadRequestId,
    pub(super) provider_kind: ProviderKind,
    pub(super) model_id: String,
    pub(super) items: Arc<[ConversationItem]>,
    pub(super) prompt_prelude: Option<PromptPreludeSnapshot>,
    pub(super) tool_definitions: Vec<ToolDefinition>,
    pub(super) context_limit: ContextTokenLimit,
    pub(super) upstream_context_tokens: Option<usize>,
}

#[derive(Debug)]
struct ContextBudgetWorkerCommand {
    request_id: SessionLoadRequestId,
    generation: u64,
    provider_kind: ProviderKind,
    model_id: String,
    items: Arc<[ConversationItem]>,
    prompt_prelude: Option<PromptPreludeSnapshot>,
    tool_definitions: Vec<ToolDefinition>,
    context_limit: ContextTokenLimit,
    upstream_context_tokens: Option<usize>,
}

enum WorkerControl {
    Load(ContextBudgetWorkerCommand),
    Shutdown,
}

enum WorkerLoopAction {
    Run(ContextBudgetWorkerCommand),
    Shutdown,
}

struct ContextBudgetTaskEnvelope {
    request_id: SessionLoadRequestId,
    generation: u64,
    result: ContextBudgetTaskResult,
}

enum ContextBudgetTaskResult {
    Cancelled,
    Loaded(ContextBudgetSnapshot),
    Failed(ContextBudgetLoadErrorPayload),
}

#[derive(Debug, thiserror::Error)]
#[error("start context budget worker thread: {detail}")]
pub(super) struct ContextBudgetWorkerInitError {
    detail: String,
}

#[derive(Debug, thiserror::Error)]
pub(super) enum ContextBudgetWorkerLoadError {
    #[error("context budget worker stopped")]
    WorkerStopped,
}

impl ContextBudgetWorkerLoadError {
    pub(super) fn into_payload(self) -> ContextBudgetLoadErrorPayload {
        match self {
            Self::WorkerStopped => ContextBudgetLoadErrorPayload::RuntimeInternal {
                detail: Some("context budget worker stopped".to_string()),
            },
        }
    }
}

impl ContextBudgetWorker {
    pub(super) fn new(
        event_notifier: RuntimeEventNotifier,
    ) -> Result<Self, ContextBudgetWorkerInitError> {
        let current_generation = Arc::new(AtomicU64::new(0));
        let (worker_tx, worker_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let worker_generation = Arc::clone(&current_generation);
        let worker_event_notifier = event_notifier.clone();
        // `/context` 投影偏 CPU 密集且会合并快速请求；专用线程避免占用 Tokio
        // shared blocking pool，generation 检查负责在投影阶段之间协作式取消。
        let worker_thread = thread::Builder::new()
            .name("context-budget-worker".to_string())
            .spawn(move || {
                let _exit_notification = worker_event_notifier.notify_on_drop();
                worker_loop(
                    worker_rx,
                    result_tx,
                    worker_generation,
                    worker_event_notifier,
                );
            })
            .map_err(|error| ContextBudgetWorkerInitError {
                detail: error.to_string(),
            })?;

        Ok(Self {
            current_generation,
            worker_tx: Some(worker_tx),
            result_rx,
            worker_thread: Some(worker_thread),
            active_request: None,
            queued_command: None,
        })
    }

    #[cfg(test)]
    pub(super) fn has_pending_work(&self) -> bool {
        self.active_request.is_some() || self.queued_command.is_some()
    }

    pub(super) fn load_snapshot(
        &mut self,
        request: ContextBudgetSnapshotRequest,
    ) -> Result<(), ContextBudgetWorkerLoadError> {
        if self.worker_tx.is_none() {
            return Err(ContextBudgetWorkerLoadError::WorkerStopped);
        }

        let generation = self.bump_generation();
        let ContextBudgetSnapshotRequest {
            request_id,
            provider_kind,
            model_id,
            items,
            prompt_prelude,
            tool_definitions,
            context_limit,
            upstream_context_tokens,
        } = request;
        let command = ContextBudgetWorkerCommand {
            request_id,
            generation,
            provider_kind,
            model_id,
            items,
            prompt_prelude,
            tool_definitions,
            context_limit,
            upstream_context_tokens,
        };

        if self.active_request.is_some() {
            self.queued_command = Some(command);
            return Ok(());
        }

        self.dispatch_command(command)
    }

    pub(super) fn cancel_pending(&mut self) {
        // `/context` 使用 soft cancel：UI 立刻停止追踪旧请求，但后台线程仍可能
        // 执行到下一次协作式取消检查或自然结束；结果会因 generation 不匹配而被丢弃。
        self.bump_generation();
        self.active_request = None;
        self.queued_command = None;
        self.drain_result_channel();
    }

    pub(super) fn shutdown(&mut self) -> Result<(), String> {
        self.cancel_pending();
        self.active_request = None;
        self.queued_command = None;

        if let Some(worker_tx) = self.worker_tx.take() {
            let _ = worker_tx.send(WorkerControl::Shutdown);
        }

        // `/context` cancellation is cooperative. Dropping the handle lets shutdown return even
        // when a stale projection is still between cancellation checkpoints.
        let _ = self.worker_thread.take();

        self.drain_result_channel();
        Ok(())
    }

    pub(super) fn drain_events(&mut self) -> Vec<RuntimeEvent> {
        let mut events = Vec::new();
        self.collect_finished_events(&mut events);

        if self.active_request.is_none()
            && let Some(command) = self.queued_command.take()
        {
            let request_id = command.request_id;
            match self.dispatch_command(command) {
                Ok(()) => {}
                Err(error) => {
                    events.push(RuntimeEvent::ContextBudgetSnapshotLoadFailed {
                        request_id,
                        error: error.into_payload(),
                    });
                }
            }
        }

        self.collect_finished_events(&mut events);
        events
    }

    fn bump_generation(&mut self) -> u64 {
        self.current_generation
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |generation| {
                Some(generation.saturating_add(1))
            })
            .map(|previous_generation| previous_generation.saturating_add(1))
            .unwrap_or(u64::MAX)
    }

    fn dispatch_command(
        &mut self,
        command: ContextBudgetWorkerCommand,
    ) -> Result<(), ContextBudgetWorkerLoadError> {
        let active_request = ActiveContextBudgetRequest {
            request_id: command.request_id,
            generation: command.generation,
        };
        let worker_tx = self
            .worker_tx
            .as_ref()
            .ok_or(ContextBudgetWorkerLoadError::WorkerStopped)?;
        worker_tx
            .send(WorkerControl::Load(command))
            .map_err(|_| ContextBudgetWorkerLoadError::WorkerStopped)?;
        self.active_request = Some(active_request);
        Ok(())
    }

    fn collect_finished_events(&mut self, events: &mut Vec<RuntimeEvent>) {
        loop {
            match self.result_rx.try_recv() {
                Ok(envelope) => {
                    if self
                        .active_request
                        .is_some_and(|active| active.generation == envelope.generation)
                    {
                        self.active_request = None;
                    }
                    if let Some(event) = self.runtime_event_from_envelope(envelope) {
                        events.push(event);
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    if let Some(active) = self.active_request.take() {
                        events.push(context_budget_worker_stopped_event(active.request_id));
                    }
                    if let Some(queued) = self.queued_command.take() {
                        events.push(context_budget_worker_stopped_event(queued.request_id));
                    }
                    self.worker_tx = None;
                    break;
                }
            }
        }
    }

    fn runtime_event_from_envelope(
        &self,
        envelope: ContextBudgetTaskEnvelope,
    ) -> Option<RuntimeEvent> {
        if envelope.generation != self.current_generation.load(Ordering::Acquire) {
            return None;
        }

        match envelope.result {
            ContextBudgetTaskResult::Cancelled => None,
            ContextBudgetTaskResult::Loaded(payload) => {
                Some(RuntimeEvent::ContextBudgetSnapshotLoaded {
                    request_id: envelope.request_id,
                    payload,
                })
            }
            ContextBudgetTaskResult::Failed(error) => {
                Some(RuntimeEvent::ContextBudgetSnapshotLoadFailed {
                    request_id: envelope.request_id,
                    error,
                })
            }
        }
    }

    fn drain_result_channel(&mut self) {
        while self.result_rx.try_recv().is_ok() {}
    }
}

fn worker_loop(
    worker_rx: mpsc::Receiver<WorkerControl>,
    result_tx: mpsc::Sender<ContextBudgetTaskEnvelope>,
    current_generation: Arc<AtomicU64>,
    event_notifier: RuntimeEventNotifier,
) {
    while let Ok(control) = worker_rx.recv() {
        match coalesce_worker_controls(control, worker_rx.try_iter()) {
            WorkerLoopAction::Run(command) => {
                let request_id = command.request_id;
                let generation = command.generation;
                let result = handle_context_budget_command(command, &current_generation);
                if result_tx
                    .send(ContextBudgetTaskEnvelope {
                        request_id,
                        generation,
                        result,
                    })
                    .is_err()
                {
                    break;
                }
                event_notifier.notify();
            }
            WorkerLoopAction::Shutdown => break,
        }
    }
}

fn context_budget_worker_stopped_event(request_id: SessionLoadRequestId) -> RuntimeEvent {
    RuntimeEvent::ContextBudgetSnapshotLoadFailed {
        request_id,
        error: ContextBudgetLoadErrorPayload::RuntimeInternal {
            detail: Some("context budget worker stopped".to_string()),
        },
    }
}

fn coalesce_worker_controls(
    first: WorkerControl,
    rest: impl IntoIterator<Item = WorkerControl>,
) -> WorkerLoopAction {
    let mut latest_load = match first {
        WorkerControl::Load(command) => Some(command),
        WorkerControl::Shutdown => return WorkerLoopAction::Shutdown,
    };

    for control in rest {
        match control {
            WorkerControl::Load(command) => latest_load = Some(command),
            WorkerControl::Shutdown => return WorkerLoopAction::Shutdown,
        }
    }

    match latest_load {
        Some(command) => WorkerLoopAction::Run(command),
        None => WorkerLoopAction::Shutdown,
    }
}

fn handle_context_budget_command(
    command: ContextBudgetWorkerCommand,
    current_generation: &AtomicU64,
) -> ContextBudgetTaskResult {
    let ContextBudgetWorkerCommand {
        request_id: _,
        generation,
        provider_kind,
        model_id,
        items,
        prompt_prelude,
        tool_definitions,
        context_limit,
        upstream_context_tokens,
    } = command;

    let is_cancelled = || current_generation.load(Ordering::Acquire) != generation;
    if is_cancelled() {
        return ContextBudgetTaskResult::Cancelled;
    }

    let probe = ContextBudgetProbe::new(
        provider_kind,
        &model_id,
        &items,
        &tool_definitions,
        context_limit,
    )
    .with_prompt_prelude(prompt_prelude.as_ref());
    let probe = probe.with_upstream_context_tokens(upstream_context_tokens);

    match build_context_budget_snapshot_with_cancellation(probe, is_cancelled) {
        Ok(Some(snapshot)) => ContextBudgetTaskResult::Loaded(snapshot),
        Ok(None) => ContextBudgetTaskResult::Cancelled,
        Err(error) => ContextBudgetTaskResult::Failed(context_budget_load_error_payload(error)),
    }
}

pub(super) fn context_budget_tool_definitions_for_worker(
    executor: &ToolExecutorRegistry,
) -> Vec<ToolDefinition> {
    context_budget_tool_definitions(executor)
}

fn context_budget_load_error_payload(
    error: conversation_runtime::ContextBudgetError,
) -> ContextBudgetLoadErrorPayload {
    match error {
        conversation_runtime::ContextBudgetError::UnsupportedProvider { provider_kind } => {
            ContextBudgetLoadErrorPayload::UnsupportedProvider { provider_kind }
        }
        conversation_runtime::ContextBudgetError::Projection { failure, .. } => {
            ContextBudgetLoadErrorPayload::ProjectionFailed {
                kind: failure.kind,
                status: failure.status,
                detail: failure.detail,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_domain::{
        context_budget::{ContextBudgetSnapshot, ContextTokenLimit, ContextWindowUsage},
        session::SessionLoadRequestId,
    };
    use std::{sync::mpsc, time::Duration};

    #[test]
    fn coalesce_worker_controls_keeps_only_the_latest_load() {
        let first = fixture_command(1, 1);
        let second = fixture_command(2, 2);
        let third = fixture_command(3, 3);

        let action = coalesce_worker_controls(
            WorkerControl::Load(first),
            [
                WorkerControl::Load(second),
                WorkerControl::Load(fixture_command_from(&third)),
            ],
        );

        match action {
            WorkerLoopAction::Run(command) => {
                assert_eq!(command.request_id, third.request_id);
                assert_eq!(command.generation, third.generation);
                assert_eq!(command.model_id, third.model_id);
            }
            WorkerLoopAction::Shutdown => panic!("latest load should remain runnable"),
        }
    }

    #[test]
    fn coalesce_worker_controls_prioritizes_shutdown_over_queued_loads() {
        let first = fixture_command(1, 1);

        let action = coalesce_worker_controls(
            WorkerControl::Load(first),
            [
                WorkerControl::Load(fixture_command(2, 2)),
                WorkerControl::Shutdown,
                WorkerControl::Load(fixture_command(3, 3)),
            ],
        );

        assert!(
            matches!(action, WorkerLoopAction::Shutdown),
            "shutdown should stop the worker loop instead of starting another stale load"
        );
    }

    #[test]
    fn stale_loaded_task_result_is_dropped_before_runtime_event_dispatch() {
        let worker = ContextBudgetWorker::new(RuntimeEventNotifier::default())
            .expect("worker should initialize");
        worker.current_generation.store(2, Ordering::Release);

        let event = worker.runtime_event_from_envelope(ContextBudgetTaskEnvelope {
            request_id: SessionLoadRequestId::new(7),
            generation: 1,
            result: ContextBudgetTaskResult::Loaded(ContextBudgetSnapshot {
                model_id: "stale-model".to_string(),
                segments: Vec::new(),
                total_estimated_tokens: 12,
                usage: ContextWindowUsage {
                    limit: ContextTokenLimit::try_from(1_000)
                        .expect("fixture limit should be valid"),
                    used: 12,
                },
            }),
        });

        assert!(
            event.is_none(),
            "stale loaded task results must not escape as runtime events"
        );
    }

    #[test]
    fn bump_generation_advances_current_generation_without_shadow_state() {
        let mut worker = ContextBudgetWorker::new(RuntimeEventNotifier::default())
            .expect("worker should initialize");

        assert_eq!(worker.current_generation.load(Ordering::Acquire), 0);
        assert_eq!(worker.bump_generation(), 1);
        assert_eq!(worker.current_generation.load(Ordering::Acquire), 1);
        assert_eq!(worker.bump_generation(), 2);
        assert_eq!(worker.current_generation.load(Ordering::Acquire), 2);
    }

    #[test]
    fn context_budget_result_wakes_after_the_payload_is_queued() {
        let (wake_sender, wake_receiver) = mpsc::channel();
        let notifier = conversation_runtime::RuntimeEventNotifier::default();
        notifier
            .install(move || {
                let _ = wake_sender.send(());
            })
            .expect("test notifier should install once");
        let mut worker = ContextBudgetWorker::new(notifier).expect("worker should initialize");

        worker
            .load_snapshot(ContextBudgetSnapshotRequest {
                request_id: SessionLoadRequestId::new(11),
                provider_kind: ProviderKind::Anthropic,
                model_id: "claude".to_string(),
                items: Arc::from([ConversationItem::text(
                    provider_protocol::Role::User,
                    "hello",
                )]),
                prompt_prelude: None,
                tool_definitions: Vec::new(),
                context_limit: ContextTokenLimit::try_from(1_000)
                    .expect("fixture limit should be valid"),
                upstream_context_tokens: None,
            })
            .expect("context budget work should queue");

        wake_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("context budget result should wake its consumer");
        assert_eq!(worker.drain_events().len(), 1);
    }

    #[test]
    fn disconnected_context_budget_worker_clears_active_request_and_reports_failure() {
        let mut worker =
            ContextBudgetWorker::new(conversation_runtime::RuntimeEventNotifier::default())
                .expect("worker should initialize");
        let (result_tx, result_rx) = mpsc::channel();
        drop(result_tx);
        worker.result_rx = result_rx;
        worker.active_request = Some(ActiveContextBudgetRequest {
            request_id: SessionLoadRequestId::new(12),
            generation: 7,
        });

        let events = worker.drain_events();

        assert!(!worker.has_pending_work());
        assert!(matches!(
            events.as_slice(),
            [RuntimeEvent::ContextBudgetSnapshotLoadFailed { request_id, error }]
                if *request_id == SessionLoadRequestId::new(12)
                    && matches!(
                        error,
                        ContextBudgetLoadErrorPayload::RuntimeInternal { detail: Some(detail) }
                            if detail == "context budget worker stopped"
                    )
        ));
    }

    fn fixture_command(request_value: u64, generation: u64) -> ContextBudgetWorkerCommand {
        ContextBudgetWorkerCommand {
            request_id: SessionLoadRequestId::new(request_value),
            generation,
            provider_kind: ProviderKind::OpenAiCompatible,
            model_id: format!("model-{request_value}"),
            items: Arc::from([ConversationItem::text(
                provider_protocol::Role::User,
                "hello",
            )]),
            prompt_prelude: None,
            tool_definitions: Vec::new(),
            context_limit: ContextTokenLimit::try_from(1_000)
                .expect("fixture limit should be valid"),
            upstream_context_tokens: None,
        }
    }

    fn fixture_command_from(command: &ContextBudgetWorkerCommand) -> ContextBudgetWorkerCommand {
        ContextBudgetWorkerCommand {
            request_id: command.request_id,
            generation: command.generation,
            provider_kind: command.provider_kind,
            model_id: command.model_id.clone(),
            items: Arc::clone(&command.items),
            prompt_prelude: command.prompt_prelude.clone(),
            tool_definitions: command.tool_definitions.clone(),
            context_limit: command.context_limit,
            upstream_context_tokens: command.upstream_context_tokens,
        }
    }
}
