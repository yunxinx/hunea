//! Host-owned `stop_agents` tool boundary。

use runtime_domain::agent::{AgentId, AgentOutcome, AgentOutcomeSummary, AgentTitle};
use runtime_domain::event_notifier::RuntimeEventNotifier;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tool_runtime::{
    Tool, ToolActivityPayloadPolicy, ToolCall, ToolDefinition, ToolExecutionContext,
    ToolExecutionFuture, ToolInvocationIdentity, ToolPermissionPolicy, ToolResult,
};

use super::spawn_agents::valid_identity;

pub(super) const STOP_AGENTS_TOOL_NAME: &str = "stop_agents";
const STOP_AGENTS_DESCRIPTION: &str = "\
Stop a child Agent you dispatched with spawn_agents. Use it when a child's direction is wrong \
or its work is no longer needed: the child and everything it spawned are stopped, and calls \
waiting on it (spawn_agents, send_agent_message) return a stopped result instead of hanging. \
Stopping is idempotent: a child that already finished returns an already-settled note with its \
existing outcome instead of an error, so there is no need to check its status first. An \
unknown agent_id is rejected with the list of child agent ids currently available to you. \
The agent_id must come from a spawn_agents completion result or a send_agent_message \
receipt.";
const STOP_AGENTS_PROMPT_GUIDELINES: &str = "\
When to stop:
- A child's direction turned out wrong or its work is no longer needed: stop it instead of waiting for it to finish.
- Stopping also releases calls waiting on that child: spawn_agents and send_agent_message return a stopped result instead of hanging.

Idempotence:
- A child that already finished returns an already-settled receipt with its existing outcome instead of an error; there is no need to check its status first.

Scope:
- The child and everything it spawned are stopped together.
- agent_id comes from a spawn_agents completion result or a send_agent_message receipt; an unknown id returns the child agent ids currently available to you.";
const STOP_AGENTS_INVALID_INPUT: &str = "stop_agents arguments are invalid";

/// `stop_agents` 的 closed delivery failure；control-plane source message 不跨越
/// tool boundary。`NotFound` 附 caller 可寻址的 child id 列表，帮助模型纠错。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::runtime) enum StopAgentsFailure {
    Unavailable,
    RequestCancelled,
    StaleGeneration,
    ParentUnavailable,
    NotFound { available_agent_ids: Vec<AgentId> },
    CleanupPending,
    DeliveryUnavailable,
}

impl StopAgentsFailure {
    pub(in crate::runtime) fn delivery_message(&self) -> String {
        match self {
            Self::Unavailable => "stop_agents is unavailable in this Agent context".to_string(),
            Self::RequestCancelled => "stop_agents request cancelled".to_string(),
            Self::StaleGeneration => "stop_agents Agent generation is stale".to_string(),
            Self::ParentUnavailable => "stop_agents parent is unavailable".to_string(),
            Self::NotFound {
                available_agent_ids,
            } => {
                if available_agent_ids.is_empty() {
                    "stop_agents target agent was not found; no child agent is available from this caller"
                        .to_string()
                } else {
                    let ids = available_agent_ids
                        .iter()
                        .map(|agent_id| agent_id.get().to_string())
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("stop_agents target agent was not found; available agent ids: {ids}")
                }
            }
            Self::CleanupPending => "stop_agents cleanup is pending".to_string(),
            Self::DeliveryUnavailable => "stop_agents delivery is unavailable".to_string(),
        }
    }
}

/// `stop_agents` 成功路径的 typed 回执：`Stopped` 表示本次请求完成了 subtree 停止；
/// `AlreadySettled` 表示 child 早已 terminal（报告已按既有 outcome 交付，无需再停）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(in crate::runtime) enum AgentStopReceipt {
    Stopped {
        agent_id: AgentId,
        title: AgentTitle,
    },
    AlreadySettled {
        agent_id: AgentId,
        title: AgentTitle,
        outcome: AgentOutcome,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<AgentOutcomeSummary>,
    },
}

/// Host 处理一次 `stop_agents` invocation 的请求。
pub(in crate::runtime) struct StopAgentsRequest {
    pub(crate) identity: ToolInvocationIdentity,
    pub(crate) agent_id: AgentId,
    pub(crate) response: oneshot::Sender<Result<AgentStopReceipt, StopAgentsFailure>>,
}

/// `StopAgentsTool` 只拥有 host bridge sender，不拥有 orchestrator 或 child authority。
#[derive(Clone)]
pub(in crate::runtime) struct StopAgentsTool {
    sender: mpsc::UnboundedSender<StopAgentsRequest>,
    notifier: RuntimeEventNotifier,
}

impl StopAgentsTool {
    pub(crate) fn channel(
        notifier: RuntimeEventNotifier,
    ) -> (Self, mpsc::UnboundedReceiver<StopAgentsRequest>) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (Self { sender, notifier }, receiver)
    }
}

impl Tool for StopAgentsTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(STOP_AGENTS_TOOL_NAME)
            .with_label("Stop agents")
            .with_description(STOP_AGENTS_DESCRIPTION)
            // guidelines 是 description 的展开版，经 prompt assembly 注入 system prompt。
            .with_prompt_guidelines(STOP_AGENTS_PROMPT_GUIDELINES)
            .with_activity_payload_policy(ToolActivityPayloadPolicy::MetadataOnly)
            // 停止的是模型自己创建的 child 资源，不扩大权限面；child 自身的工具
            // 权限仍由各自的 policy 触发。
            .with_permission_policy(ToolPermissionPolicy::Always)
            .with_input_schema(json!({
                "type": "object",
                "properties": {
                    "agent_id": { "type": "integer", "minimum": 1 }
                },
                "required": ["agent_id"],
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
                    StopAgentsFailure::Unavailable.delivery_message(),
                );
            };
            let Some(agent_id) = parse_arguments(call.arguments) else {
                return ToolResult::error(call.call_id, STOP_AGENTS_INVALID_INPUT);
            };
            let (response_sender, response_receiver) = oneshot::channel();
            if sender
                .send(StopAgentsRequest {
                    identity,
                    agent_id,
                    response: response_sender,
                })
                .is_err()
            {
                return ToolResult::error(
                    call.call_id,
                    StopAgentsFailure::Unavailable.delivery_message(),
                );
            }
            notifier.notify();

            // 停止回执同步返回，不等清理收敛；已发起的 disposal scope 归 orchestrator
            // 持有，caller cancellation 只结束等待。
            let result = tokio::select! {
                biased;
                () = cancellation.cancelled() => Err(StopAgentsFailure::RequestCancelled),
                response = response_receiver => response
                    .unwrap_or(Err(StopAgentsFailure::Unavailable)),
            };
            match result {
                Ok(receipt) => match serde_json::to_string(&receipt) {
                    Ok(payload) => ToolResult::success(call.call_id, payload),
                    Err(_) => ToolResult::error(
                        call.call_id,
                        StopAgentsFailure::DeliveryUnavailable.delivery_message(),
                    ),
                },
                Err(failure) => ToolResult::error(call.call_id, failure.delivery_message()),
            }
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StopAgentsArguments {
    agent_id: u64,
}

fn parse_arguments(arguments: serde_json::Value) -> Option<AgentId> {
    serde_json::from_value::<StopAgentsArguments>(arguments)
        .ok()
        .map(|arguments| AgentId::new(arguments.agent_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tool_runtime::Tool;

    fn test_identity() -> ToolInvocationIdentity {
        ToolInvocationIdentity::new(1, 2, 3, 4)
    }

    #[test]
    fn schema_is_metadata_only_and_always_allowed() {
        let (tool, _receiver) = StopAgentsTool::channel(RuntimeEventNotifier::default());
        let definition = tool.definition();
        assert_eq!(definition.name, STOP_AGENTS_TOOL_NAME);
        assert_eq!(
            definition.activity_payload_policy,
            ToolActivityPayloadPolicy::MetadataOnly
        );
        assert_eq!(definition.permission_policy, ToolPermissionPolicy::Always);
        let description = definition
            .description
            .as_deref()
            .expect("stop_agents should keep a description");
        assert!(description.contains("idempotent"));
        assert!(description.contains("already-settled"));
        assert!(description.contains("agent_id"));
        assert!(description.contains("unknown agent_id"));
        assert!(description.contains("available to you"));
        let guidelines = definition
            .prompt_guidelines
            .as_deref()
            .expect("stop_agents should ship prompt guidelines");
        assert!(guidelines.contains("no longer needed"));
        assert!(guidelines.contains("instead of an error"));
        assert!(guidelines.contains("everything it spawned"));
        assert!(guidelines.contains("available to you"));
        // 给模型的文本不引用用户界面：模型只需回执链即可正确使用。
        for text in [description, guidelines] {
            assert!(!text.contains("/agents"), "{text}");
            assert!(!text.contains("panel"), "{text}");
        }
        assert_eq!(
            definition
                .input_schema
                .as_ref()
                .and_then(|schema| schema.get("properties"))
                .and_then(|properties| properties.get("agent_id"))
                .and_then(|agent_id| agent_id.get("minimum"))
                .and_then(serde_json::Value::as_u64),
            Some(1)
        );
    }

    #[test]
    fn parser_rejects_unknown_fields() {
        assert_eq!(
            parse_arguments(json!({ "agent_id": 2 })),
            Some(AgentId::new(2))
        );
        assert!(parse_arguments(json!({ "agent_id": 2, "reason": "leak" })).is_none());
        assert!(parse_arguments(json!({})).is_none());
        assert!(parse_arguments(json!({ "agents": [2] })).is_none());
    }

    #[tokio::test]
    async fn missing_host_identity_fails_closed() {
        let (tool, _receiver) = StopAgentsTool::channel(RuntimeEventNotifier::default());
        let result = tool
            .execute(
                ToolCall::new("call", STOP_AGENTS_TOOL_NAME, json!({ "agent_id": 2 })),
                &CancellationToken::new(),
            )
            .await;
        assert!(result.is_error());
        assert_eq!(
            result.text_content(),
            StopAgentsFailure::Unavailable.delivery_message()
        );
    }

    #[tokio::test]
    async fn host_not_found_lists_available_ids() {
        let (tool, mut receiver) = StopAgentsTool::channel(RuntimeEventNotifier::default());
        let cancellation = CancellationToken::new();
        let execution = tool.execute_with_context(
            ToolCall::new("call", STOP_AGENTS_TOOL_NAME, json!({ "agent_id": 99 })),
            ToolExecutionContext::new(&cancellation).with_invocation_identity(test_identity()),
        );
        let host = async move {
            let request = receiver.recv().await.expect("host request should arrive");
            request
                .response
                .send(Err(StopAgentsFailure::NotFound {
                    available_agent_ids: vec![AgentId::new(2), AgentId::new(5)],
                }))
                .expect("tool should still await the response");
        };
        let (result, ()) = tokio::join!(execution, host);

        assert!(result.is_error());
        assert!(result.text_content().contains("available agent ids: 2, 5"));
    }

    #[tokio::test]
    async fn host_receipt_returns_typed_json() {
        let (tool, mut receiver) = StopAgentsTool::channel(RuntimeEventNotifier::default());
        let cancellation = CancellationToken::new();
        let execution = tool.execute_with_context(
            ToolCall::new("call", STOP_AGENTS_TOOL_NAME, json!({ "agent_id": 2 })),
            ToolExecutionContext::new(&cancellation).with_invocation_identity(test_identity()),
        );
        let host = async move {
            let request = receiver.recv().await.expect("host request should arrive");
            request
                .response
                .send(Ok(AgentStopReceipt::Stopped {
                    agent_id: AgentId::new(2),
                    title: AgentTitle::resolve(
                        &runtime_domain::agent::AgentObjective::new("workspace scout")
                            .expect("test objective should be valid"),
                        None,
                    )
                    .expect("test title should resolve"),
                }))
                .expect("tool should still await the response");
        };
        let (result, ()) = tokio::join!(execution, host);

        assert!(!result.is_error());
        let payload: serde_json::Value =
            serde_json::from_str(&result.text_content()).expect("receipt should be JSON");
        assert_eq!(payload["status"], serde_json::json!("stopped"));
        assert_eq!(payload["agent_id"], serde_json::json!(2));
        assert_eq!(payload["title"], serde_json::json!("workspace scout"));
    }

    #[tokio::test]
    async fn already_settled_receipt_carries_existing_outcome() {
        let (tool, mut receiver) = StopAgentsTool::channel(RuntimeEventNotifier::default());
        let cancellation = CancellationToken::new();
        let execution = tool.execute_with_context(
            ToolCall::new("call", STOP_AGENTS_TOOL_NAME, json!({ "agent_id": 3 })),
            ToolExecutionContext::new(&cancellation).with_invocation_identity(test_identity()),
        );
        let host = async move {
            let request = receiver.recv().await.expect("host request should arrive");
            request
                .response
                .send(Ok(AgentStopReceipt::AlreadySettled {
                    agent_id: AgentId::new(3),
                    title: AgentTitle::resolve(
                        &runtime_domain::agent::AgentObjective::new("finished scout")
                            .expect("test objective should be valid"),
                        None,
                    )
                    .expect("test title should resolve"),
                    outcome: AgentOutcome::Completed,
                    summary: AgentOutcomeSummary::new("committed answer").ok(),
                }))
                .expect("tool should still await the response");
        };
        let (result, ()) = tokio::join!(execution, host);

        assert!(!result.is_error());
        let payload: serde_json::Value =
            serde_json::from_str(&result.text_content()).expect("receipt should be JSON");
        assert_eq!(payload["status"], serde_json::json!("already_settled"));
        assert_eq!(payload["agent_id"], serde_json::json!(3));
        assert_eq!(payload["outcome"], serde_json::json!("completed"));
        assert_eq!(payload["summary"], serde_json::json!("committed answer"));
    }

    #[tokio::test]
    async fn caller_cancellation_only_stops_waiting_for_the_host_receipt() {
        let (tool, mut receiver) = StopAgentsTool::channel(RuntimeEventNotifier::default());
        let cancellation = CancellationToken::new();
        let execution = tool.execute_with_context(
            ToolCall::new("call", STOP_AGENTS_TOOL_NAME, json!({ "agent_id": 2 })),
            ToolExecutionContext::new(&cancellation).with_invocation_identity(test_identity()),
        );
        let host = async {
            let request = receiver.recv().await.expect("host request should arrive");
            cancellation.cancel();
            tokio::task::yield_now().await;
            let _ = request
                .response
                .send(Err(StopAgentsFailure::CleanupPending));
        };
        let (result, ()) = tokio::join!(execution, host);

        assert!(result.is_error());
        assert_eq!(
            result.text_content(),
            StopAgentsFailure::RequestCancelled.delivery_message()
        );
    }
}
