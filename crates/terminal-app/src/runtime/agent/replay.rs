use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use runtime_domain::{
    event_notifier::RuntimeEventNotifier,
    session::{RuntimePermissionRequest, RuntimeTarget},
};

use super::{
    AgentCommand, AgentCommandReceipt, AgentEvent, AgentEventKind, AgentId, AgentRuntime,
    AgentRuntimeActivity, AgentRuntimeError, AgentRuntimePort, AgentTurnId,
};
use crate::runtime::context::{CapabilityLease, RuntimeEventStreamCapability};

/// 一个只在测试中使用的、经过校验的 Agent fact 序列。
#[derive(Clone)]
pub(in crate::runtime) struct ReplayFixture {
    facts: Arc<[AgentEventKind]>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(in crate::runtime) enum ReplayFixtureError {
    #[error("replay fixture must contain exactly one terminal fact")]
    MissingOrMultipleTerminalFacts,
    #[error("replay fixture cannot contain facts after its terminal fact")]
    TerminalFactIsNotLast,
    #[error("replay fixture repeats a permission request id")]
    DuplicatePermissionRequest,
}

impl ReplayFixture {
    pub(in crate::runtime) fn new(facts: Vec<AgentEventKind>) -> Result<Self, ReplayFixtureError> {
        let terminal_indices = facts
            .iter()
            .enumerate()
            .filter_map(|(index, fact)| fact.is_terminal().then_some(index))
            .collect::<Vec<_>>();
        if terminal_indices.len() != 1 {
            return Err(ReplayFixtureError::MissingOrMultipleTerminalFacts);
        }
        if terminal_indices[0] + 1 != facts.len() {
            return Err(ReplayFixtureError::TerminalFactIsNotLast);
        }

        let mut permission_ids = std::collections::HashSet::new();
        for fact in &facts {
            if let AgentEventKind::PermissionRequested { request } = fact
                && !permission_ids.insert(request.request_id.clone())
            {
                return Err(ReplayFixtureError::DuplicatePermissionRequest);
            }
        }

        Ok(Self {
            facts: Arc::from(facts.into_boxed_slice()),
        })
    }
}

/// Replay owner 的 test-only lifecycle 计数；不保存 fixture 或 runtime payload。
#[derive(Default)]
pub(in crate::runtime) struct ReplayLifecycleProbe {
    constructions: AtomicUsize,
    activations: AtomicUsize,
    shutdowns: AtomicUsize,
    drops: AtomicUsize,
}

impl ReplayLifecycleProbe {
    pub(in crate::runtime) fn record_construction(&self) {
        self.constructions.fetch_add(1, Ordering::SeqCst);
    }

    pub(in crate::runtime) fn constructions(&self) -> usize {
        self.constructions.load(Ordering::SeqCst)
    }

    pub(in crate::runtime) fn activations(&self) -> usize {
        self.activations.load(Ordering::SeqCst)
    }

    pub(in crate::runtime) fn shutdowns(&self) -> usize {
        self.shutdowns.load(Ordering::SeqCst)
    }

    pub(in crate::runtime) fn drops(&self) -> usize {
        self.drops.load(Ordering::SeqCst)
    }
}

struct ReplayTurnIdentity {
    agent_id: AgentId,
    turn_id: AgentTurnId,
    target: RuntimeTarget,
}

/// 只回放 typed Agent facts 的第二个 adapter；它不启动 producer，也不触碰 native resources。
pub(in crate::runtime) struct ReplayAgentRuntime {
    fixture: ReplayFixture,
    event_notifier: RuntimeEventNotifier,
    active_turn: Option<ReplayTurnIdentity>,
    remaining_facts: VecDeque<AgentEventKind>,
    pending_events: VecDeque<AgentEvent>,
    pending_permission: Option<RuntimePermissionRequest>,
    event_stream: Option<CapabilityLease<RuntimeEventStreamCapability>>,
    lifecycle_probe: Option<Arc<ReplayLifecycleProbe>>,
    is_shutdown: bool,
    is_finalized: bool,
}

impl ReplayAgentRuntime {
    pub(in crate::runtime) fn new(
        fixture: ReplayFixture,
        event_notifier: RuntimeEventNotifier,
    ) -> Self {
        Self {
            fixture,
            event_notifier,
            active_turn: None,
            remaining_facts: VecDeque::new(),
            pending_events: VecDeque::new(),
            pending_permission: None,
            event_stream: None,
            lifecycle_probe: None,
            is_shutdown: false,
            is_finalized: false,
        }
    }

    pub(in crate::runtime) fn new_with_lifecycle_probe(
        fixture: ReplayFixture,
        event_notifier: RuntimeEventNotifier,
        lifecycle_probe: Arc<ReplayLifecycleProbe>,
    ) -> Self {
        let mut runtime = Self::new(fixture, event_notifier);
        runtime.lifecycle_probe = Some(lifecycle_probe);
        runtime
    }

    fn ensure_agent(&self, agent_id: AgentId) -> Result<(), AgentRuntimeError> {
        if self.is_shutdown {
            return Err(AgentRuntimeError::Disposed);
        }
        if agent_id != AgentId::MAIN {
            return Err(AgentRuntimeError::UnknownAgent);
        }
        Ok(())
    }

    fn active_target(&self) -> Option<&RuntimeTarget> {
        self.active_turn.as_ref().map(|turn| &turn.target)
    }

    fn materialize_until_gate(&mut self) {
        let Some(identity) = self.active_turn.as_ref() else {
            return;
        };
        let agent_id = identity.agent_id;
        let turn_id = identity.turn_id;
        let target = identity.target.clone();
        while let Some(kind) = self.remaining_facts.pop_front() {
            let is_permission = matches!(kind, AgentEventKind::PermissionRequested { .. });
            let is_terminal = kind.is_terminal();
            if let AgentEventKind::PermissionRequested { request } = &kind {
                self.pending_permission = Some(request.clone());
            }
            self.pending_events.push_back(AgentEvent {
                agent_id,
                turn_id,
                target: target.clone(),
                kind,
            });
            if is_permission || is_terminal {
                break;
            }
        }
    }

    fn notify_if_ready(&self, had_pending_events: bool) {
        if !had_pending_events && !self.pending_events.is_empty() {
            self.notify();
        }
    }

    fn notify(&self) {
        if let Some(event_stream) = &self.event_stream {
            event_stream.notify();
        } else {
            self.event_notifier.notify();
        }
    }

    fn clear_active_state(&mut self) {
        self.active_turn = None;
        self.remaining_facts.clear();
        self.pending_events.clear();
        self.pending_permission = None;
        self.event_stream = None;
    }

    fn interrupt(
        &mut self,
        agent_id: AgentId,
        target: Option<&RuntimeTarget>,
    ) -> Result<AgentCommandReceipt, AgentRuntimeError> {
        self.ensure_agent(agent_id)?;
        let Some(active) = self.active_turn.as_ref() else {
            ensure_target(None, target)?;
            return Ok(AgentCommandReceipt::Accepted);
        };
        ensure_target(Some(&active.target), target)?;
        let (agent_id, turn_id, active_target) =
            (active.agent_id, active.turn_id, active.target.clone());
        let interrupted = AgentEvent {
            agent_id,
            turn_id,
            target: active_target.clone(),
            kind: AgentEventKind::TurnInterrupted,
        };
        self.remaining_facts.clear();
        self.pending_events.clear();
        self.pending_permission = None;
        self.pending_events.push_back(interrupted);
        self.notify();
        Ok(AgentCommandReceipt::Interrupted {
            target: Some(active_target),
        })
    }

    fn respond_permission(
        &mut self,
        agent_id: AgentId,
        target: Option<&RuntimeTarget>,
        request_id: &str,
        option_id: Option<String>,
    ) -> Result<AgentCommandReceipt, AgentRuntimeError> {
        self.ensure_agent(agent_id)?;
        ensure_target(self.active_target(), target)?;
        let Some(request) = self.pending_permission.as_ref() else {
            return Err(AgentRuntimeError::CommandRejected(
                "Replay has no pending permission request".to_string(),
            ));
        };
        if request.request_id != request_id {
            return Err(AgentRuntimeError::CommandRejected(
                "Replay permission request id does not match".to_string(),
            ));
        }
        if let Some(option_id) = option_id.as_ref()
            && !request
                .options
                .iter()
                .any(|option| option.option_id == *option_id)
        {
            return Err(AgentRuntimeError::CommandRejected(
                "Replay permission option does not match".to_string(),
            ));
        }
        self.pending_permission = None;
        let had_pending_events = !self.pending_events.is_empty();
        self.materialize_until_gate();
        self.notify_if_ready(had_pending_events);
        Ok(AgentCommandReceipt::Accepted)
    }
}

impl AgentRuntime for ReplayAgentRuntime {
    fn dispatch(
        &mut self,
        command: AgentCommand,
    ) -> Result<AgentCommandReceipt, AgentRuntimeError> {
        match command {
            AgentCommand::SubmitTurn {
                agent_id,
                turn_id,
                request,
            } => {
                self.ensure_agent(agent_id)?;
                if self.active_turn.is_some() {
                    return Err(AgentRuntimeError::Busy);
                }
                let target = request.target();
                let activity_label = request.activity_label().to_string();
                self.active_turn = Some(ReplayTurnIdentity {
                    agent_id,
                    turn_id,
                    target: target.clone(),
                });
                self.remaining_facts = self.fixture.facts.iter().cloned().collect();
                self.pending_permission = None;
                let had_pending_events = !self.pending_events.is_empty();
                self.materialize_until_gate();
                self.notify_if_ready(had_pending_events);
                Ok(AgentCommandReceipt::TurnStarted {
                    turn_id,
                    target,
                    activity_label,
                })
            }
            AgentCommand::Interrupt { agent_id, target } => {
                self.interrupt(agent_id, target.as_ref())
            }
            AgentCommand::RespondPermission {
                agent_id,
                target,
                request_id,
                option_id,
            } => self.respond_permission(agent_id, target.as_ref(), &request_id, option_id),
        }
    }

    fn drain_events(&mut self) -> Vec<AgentEvent> {
        if self.is_shutdown {
            return Vec::new();
        }
        let events = self.pending_events.drain(..).collect::<Vec<_>>();
        if events.iter().any(|event| event.kind.is_terminal()) {
            self.active_turn = None;
            self.remaining_facts.clear();
            self.pending_permission = None;
        }
        events
    }

    fn shutdown(&mut self) -> Result<(), AgentRuntimeError> {
        if self.is_finalized {
            return Ok(());
        }
        if let Some(probe) = &self.lifecycle_probe {
            probe.shutdowns.fetch_add(1, Ordering::SeqCst);
        }
        self.is_finalized = true;
        self.is_shutdown = true;
        self.clear_active_state();
        Ok(())
    }
}

impl AgentRuntimePort for ReplayAgentRuntime {
    fn activate(
        &mut self,
        event_stream: CapabilityLease<RuntimeEventStreamCapability>,
    ) -> Result<(), String> {
        if self.is_finalized {
            return Err("Agent adapter is finalized".to_string());
        }
        if self.active_turn.is_some() {
            return Err("Cannot replace runtime event stream while Agent is busy".to_string());
        }
        self.event_stream = Some(event_stream);
        self.is_shutdown = false;
        if let Some(probe) = &self.lifecycle_probe {
            probe.activations.fetch_add(1, Ordering::SeqCst);
        }
        Ok(())
    }

    fn suspend(&mut self) -> Result<(), AgentRuntimeError> {
        if self.is_finalized || self.is_shutdown {
            return Ok(());
        }
        self.is_shutdown = true;
        self.clear_active_state();
        Ok(())
    }

    fn activity(&self) -> AgentRuntimeActivity {
        if self.active_turn.is_some() {
            AgentRuntimeActivity::Busy
        } else {
            AgentRuntimeActivity::Idle
        }
    }

    fn session(&self) -> Option<&dyn super::AgentSessionCapability> {
        None
    }

    fn session_mut(&mut self) -> Option<&mut dyn super::AgentSessionCapability> {
        None
    }

    fn has_pending_work(&self) -> bool {
        self.active_turn.is_some()
            || !self.remaining_facts.is_empty()
            || !self.pending_events.is_empty()
            || self.pending_permission.is_some()
    }
}

impl Drop for ReplayAgentRuntime {
    fn drop(&mut self) {
        if let Some(probe) = &self.lifecycle_probe {
            probe.drops.fetch_add(1, Ordering::SeqCst);
        }
    }
}

fn ensure_target(
    active_target: Option<&RuntimeTarget>,
    command_target: Option<&RuntimeTarget>,
) -> Result<(), AgentRuntimeError> {
    match command_target {
        Some(target) if active_target == Some(target) => Ok(()),
        Some(target) => Err(AgentRuntimeError::CommandRejected(format!(
            "Replay target is not active: {}",
            target.display_label()
        ))),
        None => Ok(()),
    }
}
