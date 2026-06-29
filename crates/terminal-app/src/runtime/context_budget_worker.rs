use std::{
    borrow::Cow,
    fmt,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use conversation_runtime::context_budget::{
    ContextBudgetProbe, build_context_budget_snapshot_with_cancellation,
    context_budget_tool_definitions,
};
use conversation_runtime::{ConversationItem, ToolDefinition};
use runtime_domain::{
    context_budget::ContextTokenLimit,
    provider::ProviderKind,
    session::{ContextBudgetLoadErrorPayload, RuntimeEvent, SessionLoadRequestId},
};
use tokio::{runtime::Runtime, task::JoinHandle};
use tool_runtime::ToolExecutorRegistry;

const CONTEXT_BUDGET_RUNTIME_SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(10);

pub(super) struct ContextBudgetWorker {
    runtime: Option<Runtime>,
    current_generation: Arc<AtomicU64>,
    next_generation: u64,
    active_task: Option<ContextBudgetTask>,
    queued_command: Option<ContextBudgetWorkerCommand>,
}

struct ContextBudgetTask {
    request_id: SessionLoadRequestId,
    generation: u64,
    handle: JoinHandle<ContextBudgetTaskResult>,
}

struct ContextBudgetWorkerCommand {
    request_id: SessionLoadRequestId,
    generation: u64,
    provider_kind: ProviderKind,
    model_id: String,
    items: Arc<[ConversationItem]>,
    tool_definitions: Vec<ToolDefinition>,
    context_limit: ContextTokenLimit,
}

enum ContextBudgetTaskResult {
    Cancelled,
    Event(Box<RuntimeEvent>),
}

#[derive(Debug)]
pub(super) struct ContextBudgetWorkerInitError {
    detail: String,
}

impl fmt::Display for ContextBudgetWorkerInitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "start context budget runtime: {}", self.detail)
    }
}

impl std::error::Error for ContextBudgetWorkerInitError {}

#[derive(Debug)]
pub(super) enum ContextBudgetWorkerLoadError {
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

impl fmt::Display for ContextBudgetWorkerLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WorkerStopped => f.write_str("context budget worker stopped"),
        }
    }
}

impl std::error::Error for ContextBudgetWorkerLoadError {}

impl ContextBudgetWorker {
    pub(super) fn new() -> Result<Self, ContextBudgetWorkerInitError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| ContextBudgetWorkerInitError {
                detail: error.to_string(),
            })?;
        Ok(Self {
            runtime: Some(runtime),
            current_generation: Arc::new(AtomicU64::new(0)),
            next_generation: 0,
            active_task: None,
            queued_command: None,
        })
    }

    pub(super) fn has_pending_work(&self) -> bool {
        self.active_task.is_some() || self.queued_command.is_some()
    }

    pub(super) fn load_snapshot(
        &mut self,
        request_id: SessionLoadRequestId,
        provider_kind: ProviderKind,
        model_id: String,
        items: Arc<[ConversationItem]>,
        tool_definitions: Vec<ToolDefinition>,
        context_limit: ContextTokenLimit,
    ) -> Result<(), ContextBudgetWorkerLoadError> {
        if self.runtime.is_none() {
            return Err(ContextBudgetWorkerLoadError::WorkerStopped);
        }

        let generation = self.bump_generation();
        let command = ContextBudgetWorkerCommand {
            request_id,
            generation,
            provider_kind,
            model_id,
            items,
            tool_definitions,
            context_limit,
        };

        if self.active_task.is_some() {
            self.queued_command = Some(command);
            return Ok(());
        }

        self.spawn_task(command)
    }

    pub(super) fn cancel_pending(&mut self) {
        self.bump_generation();
        self.active_task = None;
        self.queued_command = None;
    }

    pub(super) fn shutdown(&mut self) -> Result<(), String> {
        self.cancel_pending();
        self.active_task = None;
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_timeout(CONTEXT_BUDGET_RUNTIME_SHUTDOWN_TIMEOUT);
        }
        Ok(())
    }

    pub(super) fn drain_events(&mut self) -> Vec<RuntimeEvent> {
        let mut events = Vec::new();

        loop {
            if self.active_task.is_none() {
                let Some(command) = self.queued_command.take() else {
                    break;
                };
                let request_id = command.request_id;
                match self.spawn_task(command) {
                    Ok(()) => {}
                    Err(error) => {
                        events.push(RuntimeEvent::ContextBudgetSnapshotLoadFailed {
                            request_id,
                            error: error.into_payload(),
                        });
                    }
                }
            }

            let Some(task) = self.active_task.as_ref() else {
                break;
            };
            if !task.handle.is_finished() {
                break;
            }

            let Some(task) = self.active_task.take() else {
                break;
            };
            if let Some(event) = self.join_task(task) {
                events.push(event);
            }
        }

        events
    }

    fn bump_generation(&mut self) -> u64 {
        self.next_generation = self.next_generation.saturating_add(1);
        self.current_generation
            .store(self.next_generation, Ordering::Release);
        self.next_generation
    }

    fn spawn_task(
        &mut self,
        command: ContextBudgetWorkerCommand,
    ) -> Result<(), ContextBudgetWorkerLoadError> {
        let runtime = self
            .runtime
            .as_ref()
            .ok_or(ContextBudgetWorkerLoadError::WorkerStopped)?;
        let generation = Arc::clone(&self.current_generation);
        let request_id = command.request_id;
        let task_generation = command.generation;
        let handle = runtime
            .handle()
            .spawn_blocking(move || handle_context_budget_command(command, generation));
        self.active_task = Some(ContextBudgetTask {
            request_id,
            generation: task_generation,
            handle,
        });
        Ok(())
    }

    fn join_task(&mut self, task: ContextBudgetTask) -> Option<RuntimeEvent> {
        let runtime = self.runtime.as_ref()?;
        match runtime.block_on(task.handle) {
            Ok(ContextBudgetTaskResult::Cancelled) => None,
            Ok(ContextBudgetTaskResult::Event(event)) => Some(*event),
            Err(error) => {
                if task.generation != self.current_generation.load(Ordering::Acquire) {
                    return None;
                }
                Some(RuntimeEvent::ContextBudgetSnapshotLoadFailed {
                    request_id: task.request_id,
                    error: ContextBudgetLoadErrorPayload::RuntimeInternal {
                        detail: Some(format!("context budget task failed: {error}")),
                    },
                })
            }
        }
    }
}

fn handle_context_budget_command(
    command: ContextBudgetWorkerCommand,
    current_generation: Arc<AtomicU64>,
) -> ContextBudgetTaskResult {
    let ContextBudgetWorkerCommand {
        request_id,
        generation,
        provider_kind,
        model_id,
        items,
        tool_definitions,
        context_limit,
    } = command;

    let is_cancelled = || current_generation.load(Ordering::Acquire) != generation;
    if is_cancelled() {
        return ContextBudgetTaskResult::Cancelled;
    }

    let probe = ContextBudgetProbe::new(
        provider_kind,
        &model_id,
        items.iter().map(Cow::Borrowed).collect(),
        &tool_definitions,
        context_limit,
    );

    match build_context_budget_snapshot_with_cancellation(probe, is_cancelled) {
        Ok(Some(snapshot)) => {
            ContextBudgetTaskResult::Event(Box::new(RuntimeEvent::ContextBudgetSnapshotLoaded {
                request_id,
                payload: snapshot.into(),
            }))
        }
        Ok(None) => ContextBudgetTaskResult::Cancelled,
        Err(error) => ContextBudgetTaskResult::Event(Box::new(
            RuntimeEvent::ContextBudgetSnapshotLoadFailed {
                request_id,
                error: context_budget_load_error_payload(error),
            },
        )),
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
