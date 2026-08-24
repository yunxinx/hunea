//! 进程外 Agent kernel 到 host lifecycle port 的窄 adapter。

use std::sync::Arc;

use agent_kernel_runtime::{AgentKernelSource, ExternalAgentRuntime, ExternalAgentRuntimeOptions};
use runtime_domain::agent::{
    AgentCommand, AgentCommandReceipt, AgentEvent, AgentRuntime, AgentRuntimeError,
};

use super::{
    AgentRuntimeActivationGrants, AgentRuntimeActivity, AgentRuntimePort, AgentSessionCapability,
};
use crate::runtime::context::{CapabilityLease, RuntimeEventStreamCapability};

/// 每次 activation 把一个 fresh kernel connection 绑定到当前 event-stream generation。
pub(super) struct ExternalAgentRuntimeAdapter {
    runtime: ExternalAgentRuntime,
    event_stream: Option<CapabilityLease<RuntimeEventStreamCapability>>,
}

impl ExternalAgentRuntimeAdapter {
    pub(super) fn new(
        source: Arc<dyn AgentKernelSource>,
        options: ExternalAgentRuntimeOptions,
    ) -> Self {
        Self {
            runtime: ExternalAgentRuntime::new(source, options),
            event_stream: None,
        }
    }
}

impl AgentRuntime for ExternalAgentRuntimeAdapter {
    fn dispatch(
        &mut self,
        command: AgentCommand,
    ) -> Result<AgentCommandReceipt, AgentRuntimeError> {
        self.runtime.dispatch(command)
    }

    fn drain_events(&mut self) -> Vec<AgentEvent> {
        self.runtime.drain_events()
    }

    fn shutdown(&mut self) -> Result<(), AgentRuntimeError> {
        let result = self.runtime.shutdown();
        self.event_stream = None;
        result
    }
}

impl AgentRuntimePort for ExternalAgentRuntimeAdapter {
    fn activate(&mut self, mut grants: AgentRuntimeActivationGrants) -> Result<(), String> {
        let event_stream = grants.take_event_stream()?;
        self.runtime
            .activate((*event_stream).clone())
            .map_err(|_| "External Agent adapter activation failed".to_string())?;
        self.event_stream = Some(event_stream);
        Ok(())
    }

    fn suspend(&mut self) -> Result<(), AgentRuntimeError> {
        let result = self.runtime.suspend();
        self.event_stream = None;
        result
    }

    fn activity(&self) -> AgentRuntimeActivity {
        if self.runtime.is_busy() {
            AgentRuntimeActivity::Busy
        } else {
            AgentRuntimeActivity::Idle
        }
    }

    fn session(&self) -> Option<&dyn AgentSessionCapability> {
        None
    }

    fn session_mut(&mut self) -> Option<&mut dyn AgentSessionCapability> {
        None
    }

    #[cfg(test)]
    fn has_pending_work(&self) -> bool {
        self.runtime.is_busy()
    }
}
