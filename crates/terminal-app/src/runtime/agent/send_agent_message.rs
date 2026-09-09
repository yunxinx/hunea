//! Host-owned `send_agent_message` tool boundary。

use runtime_domain::agent::{
    AgentChildMessage, AgentId, AgentLaunchInputError, AgentOutcome, AgentTitle,
    SEND_AGENT_MESSAGE_TOOL_LABEL,
};
use runtime_domain::event_notifier::RuntimeEventNotifier;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;
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
completes the turn triggered by this message and returns that turn's full report; if the \
reply is not ready within about 30 seconds, it returns a still_running receipt instead and \
the child keeps running. The agent_id must come from a spawn_agents completion result or a \
send_agent_message receipt.";
const SEND_AGENT_MESSAGE_PROMPT_GUIDELINES: &str = "\
When to use:
- Add instructions or constraints after reviewing a dispatched child's report.
- Ask a question about the child's work or request a refined deliverable.

Message content:
- The child sees the message, not this conversation's other tool results: keep it self-contained and refer to the child's own objective and report.
- Put one coherent follow-up in a single message instead of several calls in a row.

Wait semantics:
- The call blocks until the child completes the turn triggered by this message and returns that turn's full report.
- If the child is still running an earlier task, the message runs after that task finishes.
- If the reply is not ready within about 30 seconds, the call returns a still_running receipt naming the agent_id: the child keeps running and is not interrupted. Call send_agent_message again to keep waiting for the reply, or do other work first and follow up later.

Addressing:
- agent_id comes from a spawn_agents completion result or a send_agent_message receipt; an unknown id returns the child agent ids currently available to you.
- A finished child stays addressable for a short window (about 20 seconds) after it settles; once that window passes it is no longer a valid target, and the not-found receipt lists the agent ids currently available to you.";
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
/// 已持久化。字段面与 `AgentChildCompletion` 同构——`report` 携带完整 committed
/// assistant 正文，单行摘要只服务 TUI projection，不进入模型可见面。
#[derive(Clone, PartialEq, Eq, Serialize)]
pub(in crate::runtime) struct AgentMessageDelivery {
    agent_id: AgentId,
    title: AgentTitle,
    outcome: AgentOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    report: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_uses: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    truncated: bool,
}

/// completion/delivery tool result 共享的报告信封：完整 committed assistant 正文、
/// 截断标记与终态 metrics。取值与 240 列单行 summary 分层——summary 服务 TUI/面板，
/// 信封只面向父 Agent。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::runtime) struct AgentReportEnvelope {
    /// 完整 committed assistant 正文；reasoning-only 收尾时为 `None`。
    pub report: Option<String>,
    /// `report` 超出字符上限被截断时为 `true`。
    pub truncated: bool,
    /// 终态定格的 token usage。
    pub tokens: Option<usize>,
    /// 终态定格的工具调用次数。
    pub tool_uses: Option<usize>,
    /// 终态定格的累计耗时（人类可读档位，如 `16s` / `2m 05s` / `1h 02m 03s`）。
    pub duration: Option<String>,
}

impl std::fmt::Debug for AgentMessageDelivery {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentMessageDelivery")
            .field("agent_id", &self.agent_id)
            .field("title", &self.title)
            .field("outcome", &self.outcome)
            // 报告正文只进入 tool result，不进入诊断输出。
            .field(
                "report_chars",
                &self.report.as_ref().map(|report| report.chars().count()),
            )
            .field("tokens", &self.tokens)
            .field("tool_uses", &self.tool_uses)
            .field("duration", &self.duration)
            .field("truncated", &self.truncated)
            .finish()
    }
}

impl AgentMessageDelivery {
    pub(in crate::runtime) fn new(
        agent_id: AgentId,
        title: AgentTitle,
        outcome: AgentOutcome,
        envelope: AgentReportEnvelope,
    ) -> Self {
        let AgentReportEnvelope {
            report,
            truncated,
            tokens,
            tool_uses,
            duration,
        } = envelope;
        Self {
            agent_id,
            title,
            outcome,
            report,
            tokens,
            tool_uses,
            duration,
            truncated,
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

/// 工具侧等待的三种收尾：host 回执（含 caller 取消/通道关闭折算的失败）与等待上限
/// 到达。超时不是 failure——目标仍在运行，回执走 success 面告知模型可继续等待。
enum HostWaitOutcome {
    Response(Result<AgentMessageDelivery, SendAgentMessageFailure>),
    TimedOut(AgentId),
}

/// `SendAgentMessageTool` 只拥有 host bridge sender，不拥有 orchestrator 或 child authority。
#[derive(Clone)]
pub(in crate::runtime) struct SendAgentMessageTool {
    sender: mpsc::UnboundedSender<SendAgentMessageRequest>,
    notifier: RuntimeEventNotifier,
    wait_timeout: Duration,
}

impl SendAgentMessageTool {
    pub(crate) fn channel(
        notifier: RuntimeEventNotifier,
    ) -> (Self, mpsc::UnboundedReceiver<SendAgentMessageRequest>) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (
            Self {
                sender,
                notifier,
                wait_timeout: super::HOST_AGENT_TOOL_WAIT_TIMEOUT,
            },
            receiver,
        )
    }

    /// 注入测试用等待上限；生产构造恒用 `HOST_AGENT_TOOL_WAIT_TIMEOUT`。
    #[cfg(test)]
    pub(super) fn with_wait_timeout(mut self, wait_timeout: Duration) -> Self {
        self.wait_timeout = wait_timeout;
        self
    }
}

impl Tool for SendAgentMessageTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(SEND_AGENT_MESSAGE_TOOL_NAME)
            .with_label(SEND_AGENT_MESSAGE_TOOL_LABEL)
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
        let wait_timeout = self.wait_timeout;
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

            // 已提交 orchestrator 的消息 scope 不随 caller cancellation / 等待上限撤销；
            // 这里只结束等待，回执由 explicit stop / disposal 收敛，迟到的结算 send
            // 落入已关闭通道被忽略。
            let outcome = tokio::select! {
                biased;
                () = cancellation.cancelled() => HostWaitOutcome::Response(
                    Err(SendAgentMessageFailure::RequestCancelled),
                ),
                response = response_receiver => HostWaitOutcome::Response(
                    response.unwrap_or(Err(SendAgentMessageFailure::Unavailable)),
                ),
                _ = tokio::time::sleep(wait_timeout) => HostWaitOutcome::TimedOut(agent_id),
            };
            match outcome {
                HostWaitOutcome::Response(result) => match result {
                    Ok(delivery) => match serde_json::to_string(&delivery) {
                        Ok(payload) => ToolResult::success(call.call_id, payload),
                        Err(_) => ToolResult::error(
                            call.call_id,
                            SendAgentMessageFailure::DeliveryUnavailable.delivery_message(),
                        ),
                    },
                    Err(failure) => ToolResult::error(call.call_id, failure.delivery_message()),
                },
                // 超时回执与 8 字段成功信封可区分：目标仍在运行、报告未到。
                HostWaitOutcome::TimedOut(agent_id) => ToolResult::success(
                    call.call_id,
                    json!({ "agent_id": agent_id.get(), "still_running": true }).to_string(),
                ),
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
        .map_err(|_| AgentLaunchInputError::InvalidArguments)?;
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
        assert!(description.contains("full report"));
        assert!(description.contains("agent_id"));
        assert!(description.contains("still_running receipt"));
        let guidelines = definition
            .prompt_guidelines
            .as_deref()
            .expect("send_agent_message should ship prompt guidelines");
        assert!(guidelines.contains("self-contained"));
        assert!(guidelines.contains("blocks until the child completes the turn"));
        assert!(guidelines.contains("after that task finishes"));
        assert!(guidelines.contains("still_running"));
        assert!(guidelines.contains("about 20 seconds"));
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

    #[test]
    fn parser_maps_schema_violations_to_invalid_arguments() {
        // schema 违规（类型错/缺字段）是结构失败，不得伪装成"消息为空"的值语义
        // 变体——EmptyChildMessage 只由真实的空消息触发。
        assert!(matches!(
            parse_arguments(json!({ "agent_id": "two", "message": "work" })),
            Err(AgentLaunchInputError::InvalidArguments)
        ));
        assert!(matches!(
            parse_arguments(json!({ "agent_id": 2 })),
            Err(AgentLaunchInputError::InvalidArguments)
        ));
        assert!(matches!(
            parse_arguments(json!({ "agent_id": 2, "message": "   " })),
            Err(AgentLaunchInputError::EmptyChildMessage)
        ));
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
                    AgentReportEnvelope {
                        report: Some("the full refined report body".to_string()),
                        truncated: false,
                        tokens: Some(1200),
                        tool_uses: Some(3),
                        duration: Some("45s".to_string()),
                    },
                )))
                .expect("tool should still await the response");
        };
        let (result, ()) = tokio::join!(execution, host);

        assert!(!result.is_error());
        let payload: serde_json::Value =
            serde_json::from_str(&result.text_content()).expect("delivery should be JSON");
        // 字段面与 spawn 的 child envelope 同构：无 summary/queued 等冗余键。
        assert_eq!(
            payload,
            serde_json::json!({
                "agent_id": 2,
                "title": "workspace scout",
                "outcome": "completed",
                "report": "the full refined report body",
                "tokens": 1200,
                "tool_uses": 3,
                "duration": "45s",
            }),
            "send receipt face should only carry the child envelope fields"
        );
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

    #[tokio::test]
    async fn wait_timeout_returns_still_running_receipt_and_ignores_late_settlement() {
        let (tool, mut receiver) = SendAgentMessageTool::channel(RuntimeEventNotifier::default());
        // 注入短等待上限：测试不等待生产的 30s 上限。
        let tool = tool.with_wait_timeout(Duration::from_millis(50));
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
            // 模拟 orchestrator waiter 被卡住：持有 response sender 不结算，
            // 直到远超等待上限后才尝试迟到结算。
            tokio::time::sleep(Duration::from_millis(150)).await;
            // 工具已超时返回，迟到结算落入已关闭的通道，被安全忽略。
            assert!(
                request
                    .response
                    .send(Err(SendAgentMessageFailure::TargetUnavailable))
                    .is_err()
            );
        };
        let (result, ()) = tokio::join!(execution, host);

        // 超时回执走 success 面：目标仍在运行，与 8 字段成功信封可区分。
        assert!(!result.is_error(), "{}", result.text_content());
        let payload: serde_json::Value =
            serde_json::from_str(&result.text_content()).expect("timeout receipt should be JSON");
        assert_eq!(
            payload,
            json!({ "agent_id": 2, "still_running": true }),
            "timeout receipt face should name the target and report it is still running"
        );
    }

    #[tokio::test]
    async fn wait_timeout_leaves_host_response_path_intact() {
        let (tool, mut receiver) = SendAgentMessageTool::channel(RuntimeEventNotifier::default());
        let tool = tool.with_wait_timeout(Duration::from_millis(50));
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
            // 等待上限内正常结算：response 臂必须先于 timeout 臂完成。
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
                    AgentReportEnvelope {
                        report: Some("the full refined report body".to_string()),
                        truncated: false,
                        tokens: Some(1200),
                        tool_uses: Some(3),
                        duration: Some("45s".to_string()),
                    },
                )))
                .expect("tool should still await the response");
        };
        let (result, ()) = tokio::join!(execution, host);

        assert!(!result.is_error());
        let payload: serde_json::Value =
            serde_json::from_str(&result.text_content()).expect("delivery should be JSON");
        assert_eq!(payload["agent_id"], json!(2));
        assert_eq!(payload["report"], json!("the full refined report body"));
        assert!(
            payload.get("still_running").is_none(),
            "in-time delivery must not surface the timeout receipt face"
        );
    }
}
