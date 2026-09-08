//! Host-owned `send_agent_message` tool boundary。

use runtime_domain::agent::{
    AgentChildMessage, AgentId, AgentLaunchInputError, AgentOutcome, AgentOutcomeSummary,
    AgentTitle,
};
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

pub(super) const SEND_AGENT_MESSAGE_TOOL_NAME: &str = "send_agent_message";
const SEND_AGENT_MESSAGE_DESCRIPTION: &str = "\
Send a follow-up message to a child Agent you already dispatched and wait for its reply. \
Use it to add instructions or ask questions about that child's work: the child keeps its \
conversation context, so refer to its earlier objective and report. If the child is still \
running a task, the message runs after that task finishes. The call blocks until the child \
completes the turn triggered by this message and returns that turn's report summary. \
The agent_id must come from a spawn_agents completion result or a send_agent_message \
receipt.";
const SEND_AGENT_MESSAGE_PROMPT_GUIDELINES: &str = "\
When to use:
- Add instructions or constraints after reviewing a dispatched child's report.
- Ask a question about the child's work or request a refined deliverable.

Message content:
- The child sees the message, not this conversation's other tool results: keep it self-contained and refer to the child's own objective and report.
- Put one coherent follow-up in a single message instead of several calls in a row.

Wait semantics:
- The call blocks until the child completes the turn triggered by this message and returns that turn's report summary.
- If the child is still running an earlier task, the message runs after that task finishes.

Addressing:
- agent_id comes from a spawn_agents completion result or a send_agent_message receipt; an unknown id returns the child agent ids currently available to you.";
const SEND_AGENT_MESSAGE_INVALID_INPUT: &str = "send_agent_message arguments are invalid";

/// `send_agent_message` 的 closed delivery failure；control-plane source message 不跨越
/// tool boundary。`NotFound` 附 caller 可寻址的 child id 列表，帮助模型纠错。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::runtime) enum SendAgentMessageFailure {
    Unavailable,
    RequestCancelled,
    StaleGeneration,
    ParentUnavailable,
    NotFound { available_agent_ids: Vec<AgentId> },
    TargetUnavailable,
    TargetStopped,
    DeliveryUnavailable,
}

impl SendAgentMessageFailure {
    pub(in crate::runtime) fn delivery_message(&self) -> String {
        match self {
            Self::Unavailable => {
                "send_agent_message is unavailable in this Agent context".to_string()
            }
            Self::RequestCancelled => "send_agent_message request cancelled".to_string(),
            Self::StaleGeneration => "send_agent_message Agent generation is stale".to_string(),
            Self::ParentUnavailable => "send_agent_message parent is unavailable".to_string(),
            Self::NotFound {
                available_agent_ids,
            } => {
                if available_agent_ids.is_empty() {
                    "send_agent_message target agent was not found; no child agent is available from this caller"
                        .to_string()
                } else {
                    let ids = available_agent_ids
                        .iter()
                        .map(|agent_id| agent_id.get().to_string())
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!(
                        "send_agent_message target agent was not found; available agent ids: {ids}"
                    )
                }
            }
            Self::TargetUnavailable => {
                "send_agent_message target agent is no longer available".to_string()
            }
            Self::TargetStopped => "send_agent_message target agent was stopped".to_string(),
            Self::DeliveryUnavailable => "send_agent_message delivery is unavailable".to_string(),
        }
    }
}

/// `send_agent_message` 成功路径的 typed 回执：消息触发的 turn 已 terminal 且 outcome
/// 已持久化；summary 与 group completion 共用同一 delivery-safe 取值。
///
/// 同步等待语义下不存在独立的“排队”回执；`queued` 保留“消息曾在运行中的任务后
/// 排队”这一事实，供模型向用户转述时序。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(in crate::runtime) struct AgentMessageDelivery {
    agent_id: AgentId,
    title: AgentTitle,
    outcome: AgentOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    summary: Option<AgentOutcomeSummary>,
    queued: bool,
}

impl AgentMessageDelivery {
    pub(in crate::runtime) fn new(
        agent_id: AgentId,
        title: AgentTitle,
        outcome: AgentOutcome,
        summary: Option<AgentOutcomeSummary>,
        queued: bool,
    ) -> Self {
        Self {
            agent_id,
            title,
            outcome,
            summary,
            queued,
        }
    }
}

/// Host 处理一次 `send_agent_message` invocation 的请求。
pub(in crate::runtime) struct SendAgentMessageRequest {
    pub(crate) identity: ToolInvocationIdentity,
    pub(crate) agent_id: AgentId,
    pub(crate) message: AgentChildMessage,
    pub(crate) response: oneshot::Sender<Result<AgentMessageDelivery, SendAgentMessageFailure>>,
}

/// `SendAgentMessageTool` 只拥有 host bridge sender，不拥有 orchestrator 或 child authority。
#[derive(Clone)]
pub(in crate::runtime) struct SendAgentMessageTool {
    sender: mpsc::UnboundedSender<SendAgentMessageRequest>,
    notifier: RuntimeEventNotifier,
}

impl SendAgentMessageTool {
    pub(crate) fn channel(
        notifier: RuntimeEventNotifier,
    ) -> (Self, mpsc::UnboundedReceiver<SendAgentMessageRequest>) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (Self { sender, notifier }, receiver)
    }
}

impl Tool for SendAgentMessageTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(SEND_AGENT_MESSAGE_TOOL_NAME)
            .with_label("Send agent message")
            .with_description(SEND_AGENT_MESSAGE_DESCRIPTION)
            // guidelines 是 description 的展开版，经 prompt assembly 注入 system prompt。
            .with_prompt_guidelines(SEND_AGENT_MESSAGE_PROMPT_GUIDELINES)
            .with_activity_payload_policy(ToolActivityPayloadPolicy::MetadataOnly)
            // 消息触发的是 child 的下一 turn，不是本 session 的副作用；child 的实际
            // 工具调用各自走权限层。
            .with_permission_policy(ToolPermissionPolicy::Always)
            .with_input_schema(json!({
                "type": "object",
                "properties": {
                    "agent_id": { "type": "integer", "minimum": 1 },
                    "message": { "type": "string" }
                },
                "required": ["agent_id", "message"],
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
                    SendAgentMessageFailure::Unavailable.delivery_message(),
                );
            };
            let (agent_id, message) = match parse_arguments(call.arguments) {
                Ok(parsed) => parsed,
                Err(_) => return ToolResult::error(call.call_id, SEND_AGENT_MESSAGE_INVALID_INPUT),
            };
            let (response_sender, response_receiver) = oneshot::channel();
            if sender
                .send(SendAgentMessageRequest {
                    identity,
                    agent_id,
                    message,
                    response: response_sender,
                })
                .is_err()
            {
                return ToolResult::error(
                    call.call_id,
                    SendAgentMessageFailure::Unavailable.delivery_message(),
                );
            }
            notifier.notify();

            // 已提交 orchestrator 的消息 scope 不随 caller cancellation 撤销；
            // 这里只结束等待，回执由 explicit stop / disposal 收敛。
            let result = tokio::select! {
                biased;
                () = cancellation.cancelled() => Err(SendAgentMessageFailure::RequestCancelled),
                response = response_receiver => response
                    .unwrap_or(Err(SendAgentMessageFailure::Unavailable)),
            };
            match result {
                Ok(delivery) => match serde_json::to_string(&delivery) {
                    Ok(payload) => ToolResult::success(call.call_id, payload),
                    Err(_) => ToolResult::error(
                        call.call_id,
                        SendAgentMessageFailure::DeliveryUnavailable.delivery_message(),
                    ),
                },
                Err(failure) => ToolResult::error(call.call_id, failure.delivery_message()),
            }
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SendAgentMessageArguments {
    agent_id: u64,
    message: String,
}

fn parse_arguments(
    arguments: serde_json::Value,
) -> Result<(AgentId, AgentChildMessage), AgentLaunchInputError> {
    let arguments = serde_json::from_value::<SendAgentMessageArguments>(arguments)
        .map_err(|_| AgentLaunchInputError::EmptyChildMessage)?;
    let message = AgentChildMessage::new(arguments.message)?;
    Ok((AgentId::new(arguments.agent_id), message))
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_domain::agent::AgentObjective;
    use tool_runtime::Tool;

    fn test_identity() -> ToolInvocationIdentity {
        ToolInvocationIdentity::new(1, 2, 3, 4)
    }

    #[test]
    fn schema_is_metadata_only_and_always_allowed() {
        let (tool, _receiver) = SendAgentMessageTool::channel(RuntimeEventNotifier::default());
        let definition = tool.definition();
        assert_eq!(definition.name, SEND_AGENT_MESSAGE_TOOL_NAME);
        assert_eq!(
            definition.activity_payload_policy,
            ToolActivityPayloadPolicy::MetadataOnly
        );
        assert_eq!(definition.permission_policy, ToolPermissionPolicy::Always);
        let description = definition
            .description
            .as_deref()
            .expect("send_agent_message should keep a description");
        assert!(description.contains("follow-up message"));
        assert!(description.contains("report summary"));
        assert!(description.contains("agent_id"));
        let guidelines = definition
            .prompt_guidelines
            .as_deref()
            .expect("send_agent_message should ship prompt guidelines");
        assert!(guidelines.contains("self-contained"));
        assert!(guidelines.contains("blocks until the child completes the turn"));
        assert!(guidelines.contains("after that task finishes"));
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
    fn parser_rejects_unknown_fields_and_unsafe_messages() {
        let (agent_id, message) =
            parse_arguments(json!({ "agent_id": 2, "message": "please refine the report" }))
                .expect("valid message should parse");
        assert_eq!(agent_id, AgentId::new(2));
        assert_eq!(message.as_str(), "please refine the report");
        assert!(
            parse_arguments(json!({ "agent_id": 2, "message": "hi", "secret": "leak" })).is_err()
        );
        assert!(parse_arguments(json!({ "agent_id": 2, "message": "   " })).is_err());
        assert!(
            parse_arguments(json!({ "agent_id": 2, "message": "escape\u{001b}[31m" })).is_err()
        );
    }

    #[tokio::test]
    async fn missing_host_identity_fails_closed() {
        let (tool, _receiver) = SendAgentMessageTool::channel(RuntimeEventNotifier::default());
        let result = tool
            .execute(
                ToolCall::new(
                    "call",
                    SEND_AGENT_MESSAGE_TOOL_NAME,
                    json!({ "agent_id": 2, "message": "work" }),
                ),
                &CancellationToken::new(),
            )
            .await;
        assert!(result.is_error());
        assert_eq!(
            result.text_content(),
            SendAgentMessageFailure::Unavailable.delivery_message()
        );
    }

    #[tokio::test]
    async fn host_not_found_lists_available_ids_without_exposing_the_message() {
        let (tool, mut receiver) = SendAgentMessageTool::channel(RuntimeEventNotifier::default());
        let sensitive_message = "PRIVATE_MESSAGE_BODY";
        let cancellation = CancellationToken::new();
        let execution = tool.execute_with_context(
            ToolCall::new(
                "call",
                SEND_AGENT_MESSAGE_TOOL_NAME,
                json!({ "agent_id": 99, "message": sensitive_message }),
            ),
            ToolExecutionContext::new(&cancellation).with_invocation_identity(test_identity()),
        );
        let host = async move {
            let request = receiver.recv().await.expect("host request should arrive");
            request
                .response
                .send(Err(SendAgentMessageFailure::NotFound {
                    available_agent_ids: vec![AgentId::new(2), AgentId::new(5)],
                }))
                .expect("tool should still await the response");
        };
        let (result, ()) = tokio::join!(execution, host);

        assert!(result.is_error());
        let text = result.text_content();
        assert!(text.contains("available agent ids: 2, 5"));
        assert!(!text.contains(sensitive_message));
    }

    #[tokio::test]
    async fn host_delivery_returns_typed_receipt_json() {
        let (tool, mut receiver) = SendAgentMessageTool::channel(RuntimeEventNotifier::default());
        let cancellation = CancellationToken::new();
        let execution = tool.execute_with_context(
            ToolCall::new(
                "call",
                SEND_AGENT_MESSAGE_TOOL_NAME,
                json!({ "agent_id": 2, "message": "please refine the report" }),
            ),
            ToolExecutionContext::new(&cancellation).with_invocation_identity(test_identity()),
        );
        let host = async move {
            let request = receiver.recv().await.expect("host request should arrive");
            request
                .response
                .send(Ok(AgentMessageDelivery::new(
                    AgentId::new(2),
                    AgentTitle::resolve(
                        &AgentObjective::new("workspace scout")
                            .expect("test objective should be valid"),
                        None,
                    )
                    .expect("test title should resolve"),
                    AgentOutcome::Completed,
                    AgentOutcomeSummary::new("refined answer").ok(),
                    true,
                )))
                .expect("tool should still await the response");
        };
        let (result, ()) = tokio::join!(execution, host);

        assert!(!result.is_error());
        let payload: serde_json::Value =
            serde_json::from_str(&result.text_content()).expect("delivery should be JSON");
        assert_eq!(payload["agent_id"], serde_json::json!(2));
        assert_eq!(payload["outcome"], serde_json::json!("completed"));
        assert_eq!(payload["summary"], serde_json::json!("refined answer"));
        assert_eq!(payload["queued"], serde_json::json!(true));
    }

    #[tokio::test]
    async fn caller_cancellation_only_stops_waiting_for_the_child_turn() {
        let (tool, mut receiver) = SendAgentMessageTool::channel(RuntimeEventNotifier::default());
        let cancellation = CancellationToken::new();
        let execution = tool.execute_with_context(
            ToolCall::new(
                "call",
                SEND_AGENT_MESSAGE_TOOL_NAME,
                json!({ "agent_id": 2, "message": "work" }),
            ),
            ToolExecutionContext::new(&cancellation).with_invocation_identity(test_identity()),
        );
        let host = async {
            let request = receiver.recv().await.expect("host request should arrive");
            cancellation.cancel();
            tokio::task::yield_now().await;
            let _ = request
                .response
                .send(Err(SendAgentMessageFailure::TargetUnavailable));
        };
        let (result, ()) = tokio::join!(execution, host);

        assert!(result.is_error());
        assert_eq!(
            result.text_content(),
            SendAgentMessageFailure::RequestCancelled.delivery_message()
        );
    }
}
