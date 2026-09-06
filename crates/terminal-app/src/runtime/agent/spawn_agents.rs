//! Host-owned `spawn_agents` tool boundary。

use runtime_domain::{
    agent::{
        AgentGroupCompletion, AgentInstructions, AgentLaunchBatch, AgentLaunchInputError,
        AgentLaunchRequest, AgentObjective,
    },
    event_notifier::RuntimeEventNotifier,
};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tool_runtime::{
    Tool, ToolActivityPayloadPolicy, ToolCall, ToolDefinition, ToolExecutionContext,
    ToolExecutionFuture, ToolInvocationIdentity, ToolResult,
};

pub(super) const SPAWN_AGENTS_TOOL_NAME: &str = "spawn_agents";
const SPAWN_AGENTS_DESCRIPTION: &str =
    "Launch one or more child Agents and wait for their typed completion results.";
const SPAWN_AGENTS_INVALID_INPUT: &str = "spawn_agents arguments are invalid";

/// `spawn_agents` 的 closed delivery error；control-plane source message 不跨越 tool boundary。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::runtime) enum SpawnAgentsFailure {
    Unavailable,
    RequestCancelled,
    ParentBusy,
    ParentUnavailable,
    RequestRejected,
    CleanupPending,
    CompletionUnavailable,
}

impl SpawnAgentsFailure {
    fn delivery_message(self) -> &'static str {
        match self {
            Self::Unavailable => "spawn_agents is unavailable in this Agent context",
            Self::RequestCancelled => "spawn_agents request cancelled",
            Self::ParentBusy => "spawn_agents parent is busy",
            Self::ParentUnavailable => "spawn_agents parent is unavailable",
            Self::RequestRejected => "spawn_agents request was rejected",
            Self::CleanupPending => "spawn_agents cleanup is pending",
            Self::CompletionUnavailable => "spawn_agents completion is unavailable",
        }
    }
}

/// Host 处理一次 `spawn_agents` invocation 的请求。
pub struct SpawnAgentsRequest {
    pub(crate) identity: ToolInvocationIdentity,
    pub(crate) batch: AgentLaunchBatch,
    pub(crate) response: oneshot::Sender<Result<AgentGroupCompletion, SpawnAgentsFailure>>,
}

/// `SpawnAgentsTool` 只拥有 host bridge sender，不拥有 orchestrator 或 child authority。
#[derive(Clone)]
pub struct SpawnAgentsTool {
    sender: mpsc::UnboundedSender<SpawnAgentsRequest>,
    notifier: RuntimeEventNotifier,
}

impl SpawnAgentsTool {
    pub(crate) fn channel(
        notifier: RuntimeEventNotifier,
    ) -> (Self, mpsc::UnboundedReceiver<SpawnAgentsRequest>) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (Self { sender, notifier }, receiver)
    }
}

impl Tool for SpawnAgentsTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(SPAWN_AGENTS_TOOL_NAME)
            .with_label("Spawn agents")
            .with_description(SPAWN_AGENTS_DESCRIPTION)
            .with_activity_payload_policy(ToolActivityPayloadPolicy::MetadataOnly)
            .with_input_schema(json!({
                "type": "object",
                "properties": {
                    "agents": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": 8,
                        "items": {
                            "type": "object",
                            "properties": {
                                "objective": { "type": "string" },
                                "display_title": { "type": "string" },
                                "instructions": { "type": "string" }
                            },
                            "required": ["objective"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["agents"],
                "additionalProperties": false
            }))
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        cancellation: &'a CancellationToken,
    ) -> ToolExecutionFuture<'a> {
        self.execute_with_context(call, ToolExecutionContext::new(cancellation))
    }

    fn execute_with_context<'a>(
        &'a self,
        call: ToolCall,
        context: ToolExecutionContext<'a>,
    ) -> ToolExecutionFuture<'a> {
        let sender = self.sender.clone();
        let notifier = self.notifier.clone();
        let cancellation = context.cancellation().clone();
        let identity = context.invocation_identity();
        Box::pin(async move {
            let Some(identity) = identity.filter(valid_identity) else {
                return ToolResult::error(
                    call.call_id,
                    SpawnAgentsFailure::Unavailable.delivery_message(),
                );
            };
            let batch = match parse_batch(call.arguments) {
                Ok(batch) => batch,
                Err(_) => return ToolResult::error(call.call_id, SPAWN_AGENTS_INVALID_INPUT),
            };
            let (response_sender, response_receiver) = oneshot::channel();
            if sender
                .send(SpawnAgentsRequest {
                    identity,
                    batch,
                    response: response_sender,
                })
                .is_err()
            {
                return ToolResult::error(
                    call.call_id,
                    SpawnAgentsFailure::Unavailable.delivery_message(),
                );
            }
            notifier.notify();

            let result = tokio::select! {
                biased;
                () = cancellation.cancelled() => Err(SpawnAgentsFailure::RequestCancelled),
                response = response_receiver => response
                    .unwrap_or(Err(SpawnAgentsFailure::Unavailable)),
            };
            match result {
                Ok(completion) => match serde_json::to_string(&completion) {
                    Ok(completion) => ToolResult::success(call.call_id, completion),
                    Err(_) => ToolResult::error(
                        call.call_id,
                        SpawnAgentsFailure::CompletionUnavailable.delivery_message(),
                    ),
                },
                Err(failure) => ToolResult::error(call.call_id, failure.delivery_message()),
            }
        })
    }
}

fn valid_identity(identity: &ToolInvocationIdentity) -> bool {
    identity.agent_id() != 0
        && identity.turn_id() != 0
        && identity.runtime_generation() != 0
        && identity.context_epoch() != 0
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpawnAgentsArguments {
    agents: Vec<SpawnAgentArgument>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpawnAgentArgument {
    objective: String,
    display_title: Option<String>,
    instructions: Option<String>,
}

fn parse_batch(arguments: serde_json::Value) -> Result<AgentLaunchBatch, AgentLaunchInputError> {
    let arguments = serde_json::from_value::<SpawnAgentsArguments>(arguments)
        .map_err(|_| AgentLaunchInputError::EmptyBatch)?;
    let requests = arguments
        .agents
        .into_iter()
        .map(|argument| {
            AgentLaunchRequest::new(
                AgentObjective::new(argument.objective)?,
                argument.display_title.as_deref(),
                AgentInstructions::new(argument.instructions.unwrap_or_default()),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    AgentLaunchBatch::new(requests)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tool_runtime::Tool;

    #[test]
    fn schema_is_metadata_only_and_bounded() {
        let (tool, _receiver) = SpawnAgentsTool::channel(RuntimeEventNotifier::default());
        let definition = tool.definition();
        assert_eq!(definition.name, SPAWN_AGENTS_TOOL_NAME);
        assert_eq!(
            definition.activity_payload_policy,
            ToolActivityPayloadPolicy::MetadataOnly
        );
        assert_eq!(
            definition.input_schema.as_ref().and_then(|schema| {
                schema
                    .get("properties")
                    .and_then(|properties| properties.get("agents"))
                    .and_then(|agents| agents.get("maxItems"))
                    .and_then(serde_json::Value::as_u64)
            }),
            Some(8)
        );
    }

    #[test]
    fn parser_freezes_titles_and_rejects_unknown_fields() {
        let batch = parse_batch(json!({
            "agents": [{"objective": "  写一首俳句  ", "display_title": "  haiku  "}]
        }))
        .expect("valid batch");
        assert_eq!(batch.requests()[0].title().as_str(), "haiku");
        assert!(
            parse_batch(json!({
                "agents": [{"objective": "work", "secret": "leak"}]
            }))
            .is_err()
        );
    }

    #[test]
    fn parser_rejects_empty_and_oversized_batches_before_identity() {
        assert!(parse_batch(json!({"agents": []})).is_err());
        assert!(
            parse_batch(json!({
                "agents": (0..9).map(|_| json!({"objective": "work"})).collect::<Vec<_>>()
            }))
            .is_err()
        );
    }

    #[tokio::test]
    async fn missing_host_identity_fails_closed() {
        let (tool, _receiver) = SpawnAgentsTool::channel(RuntimeEventNotifier::default());
        let result = tool
            .execute(
                ToolCall::new(
                    "call",
                    SPAWN_AGENTS_TOOL_NAME,
                    json!({
                        "agents": [{"objective": "work"}]
                    }),
                ),
                &CancellationToken::new(),
            )
            .await;
        assert!(result.is_error());
        assert_eq!(
            result.text_content(),
            SpawnAgentsFailure::Unavailable.delivery_message()
        );
    }

    fn test_identity() -> ToolInvocationIdentity {
        ToolInvocationIdentity::new(1, 2, 3, 4)
    }

    #[tokio::test]
    async fn host_rejection_returns_closed_error_without_exposing_arguments() {
        let (tool, mut receiver) = SpawnAgentsTool::channel(RuntimeEventNotifier::default());
        let sensitive_objective = "PRIVATE_OBJECTIVE";
        let cancellation = CancellationToken::new();
        let execution = tool.execute_with_context(
            ToolCall::new(
                "call",
                SPAWN_AGENTS_TOOL_NAME,
                json!({ "agents": [{ "objective": sensitive_objective }] }),
            ),
            ToolExecutionContext::new(&cancellation).with_invocation_identity(test_identity()),
        );
        let host = async move {
            let request = receiver.recv().await.expect("host request should arrive");
            request
                .response
                .send(Err(SpawnAgentsFailure::RequestRejected))
                .expect("tool should still await the response");
        };
        let (result, ()) = tokio::join!(execution, host);

        assert!(result.is_error());
        assert_eq!(result.text_content(), "spawn_agents request was rejected");
        assert!(!result.text_content().contains(sensitive_objective));
    }

    #[tokio::test]
    async fn caller_cancellation_only_stops_waiting_for_host_completion() {
        let (tool, mut receiver) = SpawnAgentsTool::channel(RuntimeEventNotifier::default());
        let cancellation = CancellationToken::new();
        let execution = tool.execute_with_context(
            ToolCall::new(
                "call",
                SPAWN_AGENTS_TOOL_NAME,
                json!({ "agents": [{ "objective": "work" }] }),
            ),
            ToolExecutionContext::new(&cancellation).with_invocation_identity(test_identity()),
        );
        let host = async {
            let request = receiver.recv().await.expect("host request should arrive");
            cancellation.cancel();
            tokio::task::yield_now().await;
            let _ = request
                .response
                .send(Err(SpawnAgentsFailure::RequestRejected));
        };
        let (result, ()) = tokio::join!(execution, host);

        assert!(result.is_error());
        assert_eq!(
            result.text_content(),
            SpawnAgentsFailure::RequestCancelled.delivery_message()
        );
    }
}
