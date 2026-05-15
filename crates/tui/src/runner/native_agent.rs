use crate::Model;
#[cfg(test)]
use crate::runtime::RuntimeEventApply;
#[cfg(test)]
use mo_core::session::{NativeAgentEvent, RuntimeEvent, RuntimeRequestMetrics, RuntimeTarget};
use mo_core::session::{NativeAgentRequest, RuntimeCommand, RuntimeCommandReceipt};

use super::RuntimeCoordinator;

#[cfg(test)]
pub(super) fn apply_native_agent_event(
    model: &mut Model,
    target: Option<RuntimeTarget>,
    event: NativeAgentEvent,
) {
    let runtime_event = match event {
        NativeAgentEvent::Retrying { message } => RuntimeEvent::Retrying { target, message },
        NativeAgentEvent::OutputTokenEstimate { total_tokens } => {
            RuntimeEvent::OutputTokenEstimate {
                target,
                total_tokens,
            }
        }
        NativeAgentEvent::Thinking { is_thinking } => RuntimeEvent::Thinking {
            target,
            is_thinking,
        },
        NativeAgentEvent::ToolActivityStarted { activity } => RuntimeEvent::ToolActivityStarted {
            target: target.expect("native agent target should be available for tool activity"),
            activity,
        },
        NativeAgentEvent::ToolActivityUpdated { update } => RuntimeEvent::ToolActivityUpdated {
            target: target.expect("native agent target should be available for tool activity"),
            update,
        },
        NativeAgentEvent::Finished { response, metrics } => RuntimeEvent::MessageFinished {
            target,
            content: response.content,
            reasoning_content: response.reasoning_content,
            reasoning_duration: response.reasoning_duration,
            finish_reason: None,
            metrics: metrics.map(|metrics| {
                RuntimeRequestMetrics::new(metrics.latency, metrics.output_tokens, metrics.duration)
            }),
        },
        NativeAgentEvent::Failed { message } => RuntimeEvent::Failed { target, message },
        NativeAgentEvent::Interrupted => RuntimeEvent::Interrupted { target },
    };
    model.apply_runtime_event(runtime_event);
}

pub(super) fn run_send_native_agent_effect(
    model: &mut Model,
    runtime_coordinator: &mut impl RuntimeCoordinator,
    request: NativeAgentRequest,
) {
    match runtime_coordinator.dispatch_runtime_command(RuntimeCommand::submit_native_agent(request))
    {
        Ok(RuntimeCommandReceipt::NativeAgentStarted { activity_label }) => {
            model.show_stream_activity(activity_label)
        }
        Ok(_) => {}
        Err(message) => model.show_transient_status_notice(&message),
    }
}
