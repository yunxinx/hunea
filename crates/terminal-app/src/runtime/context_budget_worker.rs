use std::{
    borrow::Cow,
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::Duration,
};

use conversation_runtime::context_budget::{
    ContextBudgetProbe, build_context_budget_snapshot, context_budget_tool_definitions,
};
use conversation_runtime::{ConversationItem, ToolDefinition};
use runtime_domain::{
    context_budget::{ContextBudgetSnapshot, ContextTokenLimit, ContextWindowUsage},
    provider::ProviderKind,
    session::{
        ContextBudgetLoadErrorPayload, ContextBudgetSegmentPayload, ContextBudgetSnapshotPayload,
        ContextWindowUsagePayload, RuntimeEvent, SessionLoadRequestId,
    },
};
use tool_runtime::ToolExecutorRegistry;
use tracing::debug;

const CONTEXT_BUDGET_EVENT_DRAIN_WAIT: Duration = Duration::from_millis(2);

pub(super) struct ContextBudgetWorker {
    command_sender: Option<Sender<ContextBudgetWorkerCommand>>,
    event_receiver: Receiver<RuntimeEvent>,
    worker_handle: Option<thread::JoinHandle<()>>,
    pending_commands: usize,
}

pub(super) struct ContextBudgetWorkerCommand {
    pub(super) request_id: SessionLoadRequestId,
    pub(super) provider_kind: ProviderKind,
    pub(super) model_id: String,
    pub(super) items: Vec<ConversationItem>,
    pub(super) tool_definitions: Vec<ToolDefinition>,
    pub(super) context_limit: ContextTokenLimit,
}

impl Default for ContextBudgetWorker {
    fn default() -> Self {
        Self::new()
    }
}

impl ContextBudgetWorker {
    pub(super) fn new() -> Self {
        let (command_sender, command_receiver) = mpsc::channel();
        let (event_sender, event_receiver) = mpsc::channel();
        // `/context` 的 projection 与 token 估算是同步 CPU 工作。
        // 专用线程把它移出 TUI 命令热路径，避免在协调器里阻塞事件分发。
        let worker_handle =
            thread::spawn(move || run_context_budget_worker(command_receiver, event_sender));
        Self {
            command_sender: Some(command_sender),
            event_receiver,
            worker_handle: Some(worker_handle),
            pending_commands: 0,
        }
    }

    pub(super) fn has_pending_work(&self) -> bool {
        self.pending_commands > 0
    }

    pub(super) fn load_snapshot(
        &mut self,
        request_id: SessionLoadRequestId,
        provider_kind: ProviderKind,
        model_id: String,
        items: Vec<ConversationItem>,
        tool_definitions: Vec<ToolDefinition>,
        context_limit: ContextTokenLimit,
    ) -> Result<(), String> {
        let Some(command_sender) = self.command_sender.as_ref() else {
            return Err("context budget worker stopped".to_string());
        };
        command_sender
            .send(ContextBudgetWorkerCommand {
                request_id,
                provider_kind,
                model_id,
                items,
                tool_definitions,
                context_limit,
            })
            .map_err(|_| "context budget worker stopped".to_string())?;
        self.pending_commands = self.pending_commands.saturating_add(1);
        Ok(())
    }

    pub(super) fn shutdown(&mut self) -> Result<(), String> {
        self.pending_commands = 0;
        self.command_sender.take();
        if let Some(worker_handle) = self.worker_handle.take() {
            worker_handle
                .join()
                .map_err(|_| "context budget worker panicked during shutdown".to_string())?;
        }
        while self.event_receiver.try_recv().is_ok() {}
        Ok(())
    }

    pub(super) fn drain_events(&mut self) -> Vec<RuntimeEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.event_receiver.try_recv() {
            self.pending_commands = self.pending_commands.saturating_sub(1);
            events.push(event);
        }

        if events.is_empty()
            && self.pending_commands > 0
            && let Ok(event) = self
                .event_receiver
                .recv_timeout(CONTEXT_BUDGET_EVENT_DRAIN_WAIT)
        {
            self.pending_commands = self.pending_commands.saturating_sub(1);
            events.push(event);
            while let Ok(event) = self.event_receiver.try_recv() {
                self.pending_commands = self.pending_commands.saturating_sub(1);
                events.push(event);
            }
        }

        events
    }
}

fn run_context_budget_worker(
    command_receiver: Receiver<ContextBudgetWorkerCommand>,
    event_sender: Sender<RuntimeEvent>,
) {
    while let Ok(command) = command_receiver.recv() {
        let event = handle_context_budget_command(command);
        if event_sender.send(event).is_err() {
            debug!("context budget worker dropped runtime event because receiver closed");
            break;
        }
    }
}

fn handle_context_budget_command(command: ContextBudgetWorkerCommand) -> RuntimeEvent {
    let ContextBudgetWorkerCommand {
        request_id,
        provider_kind,
        model_id,
        items,
        tool_definitions,
        context_limit,
    } = command;

    let probe = ContextBudgetProbe::new(
        provider_kind,
        &model_id,
        items.iter().map(Cow::Borrowed).collect(),
        &tool_definitions,
        context_limit,
    );

    match build_context_budget_snapshot(probe) {
        Ok(snapshot) => RuntimeEvent::ContextBudgetSnapshotLoaded {
            request_id,
            payload: snapshot_to_payload(snapshot),
        },
        Err(error) => RuntimeEvent::ContextBudgetSnapshotLoadFailed {
            request_id,
            error: context_budget_load_error_payload(error),
        },
    }
}

pub(super) fn context_budget_tool_definitions_for_worker(
    executor: &ToolExecutorRegistry,
) -> Vec<ToolDefinition> {
    context_budget_tool_definitions(executor)
}

fn snapshot_to_payload(snapshot: ContextBudgetSnapshot) -> ContextBudgetSnapshotPayload {
    ContextBudgetSnapshotPayload {
        model_id: snapshot.model_id,
        total_estimated_tokens: snapshot.total_estimated_tokens,
        usage: usage_to_payload(snapshot.usage),
        segments: snapshot
            .segments
            .into_iter()
            .map(|segment| ContextBudgetSegmentPayload {
                kind: segment.kind,
                stack_order: segment.stack_order,
                estimated_tokens: segment.estimated_tokens,
            })
            .collect(),
    }
}

fn usage_to_payload(usage: ContextWindowUsage) -> ContextWindowUsagePayload {
    ContextWindowUsagePayload {
        limit: usage.limit.get(),
        used: usage.used,
        percent: usage.percent,
    }
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
