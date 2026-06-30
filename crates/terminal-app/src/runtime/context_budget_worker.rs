use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use conversation_runtime::context_budget::{
    ContextBudgetProbe, build_context_budget_snapshot_with_cancellation,
    context_budget_tool_definitions,
};
use conversation_runtime::{ConversationItem, ToolDefinition};
use runtime_domain::{
    context_budget::{ContextBudgetSnapshot, ContextTokenLimit},
    provider::ProviderKind,
    session::{ContextBudgetLoadErrorPayload, RuntimeEvent, SessionLoadRequestId},
};
use tokio::{runtime::Runtime, task::JoinHandle};
use tool_runtime::ToolExecutorRegistry;

pub(super) struct ContextBudgetWorker {
    runtime: Option<Runtime>,
    current_generation: Arc<AtomicU64>,
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
    Loaded(ContextBudgetSnapshot),
    Failed(ContextBudgetLoadErrorPayload),
}

#[derive(Debug, thiserror::Error)]
#[error("start context budget runtime: {detail}")]
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
        // `/context` 使用 soft cancel：UI 立刻停止追踪旧请求，但已经开始运行的
        // `spawn_blocking` 任务仍会在后台跑到下一次协作式取消检查或自然结束。
        self.bump_generation();
        self.active_task = None;
        self.queued_command = None;
    }

    pub(super) fn shutdown(&mut self) -> Result<(), String> {
        self.cancel_pending();
        self.active_task = None;
        if let Some(runtime) = self.runtime.take() {
            // `/context` 任务通过 generation 做协作式取消。这里不假装等待
            // `spawn_blocking` 自然结束；runtime drop 后，已启动任务可能继续在后台
            // 运行到下一次取消检查或正常完成，但 UI 不再追踪它们的结果。
            drop(runtime);
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
        self.current_generation
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |generation| {
                Some(generation.saturating_add(1))
            })
            .map(|previous_generation| previous_generation.saturating_add(1))
            .unwrap_or(u64::MAX)
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
            Ok(ContextBudgetTaskResult::Loaded(payload)) => {
                if task.generation != self.current_generation.load(Ordering::Acquire) {
                    return None;
                }
                Some(RuntimeEvent::ContextBudgetSnapshotLoaded {
                    request_id: task.request_id,
                    payload,
                })
            }
            Ok(ContextBudgetTaskResult::Failed(error)) => {
                if task.generation != self.current_generation.load(Ordering::Acquire) {
                    return None;
                }
                Some(RuntimeEvent::ContextBudgetSnapshotLoadFailed {
                    request_id: task.request_id,
                    error,
                })
            }
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
        request_id: _,
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
        &items,
        &tool_definitions,
        context_limit,
    );

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

    #[test]
    fn stale_loaded_task_result_is_dropped_before_runtime_event_dispatch() {
        let mut worker = ContextBudgetWorker::new().expect("worker should initialize");
        let runtime = worker
            .runtime
            .as_ref()
            .expect("fresh worker should own a runtime");

        worker.current_generation.store(2, Ordering::Release);
        let handle = runtime.handle().spawn(async {
            ContextBudgetTaskResult::Loaded(ContextBudgetSnapshot {
                model_id: "stale-model".to_string(),
                segments: Vec::new(),
                total_estimated_tokens: 12,
                usage: ContextWindowUsage {
                    limit: ContextTokenLimit::try_from(1_000)
                        .expect("fixture limit should be valid"),
                    used: 12,
                },
            })
        });

        let event = worker.join_task(ContextBudgetTask {
            request_id: SessionLoadRequestId::new(7),
            generation: 1,
            handle,
        });

        assert!(
            event.is_none(),
            "stale loaded task results must not escape as runtime events"
        );
    }

    #[test]
    fn bump_generation_advances_current_generation_without_shadow_state() {
        let mut worker = ContextBudgetWorker::new().expect("worker should initialize");

        assert_eq!(worker.current_generation.load(Ordering::Acquire), 0);
        assert_eq!(worker.bump_generation(), 1);
        assert_eq!(worker.current_generation.load(Ordering::Acquire), 1);
        assert_eq!(worker.bump_generation(), 2);
        assert_eq!(worker.current_generation.load(Ordering::Acquire), 2);
    }
}
