//! Host-owned `spawn_agents` tool boundary。

use runtime_domain::{
    agent::{
        AgentGroupCompletion, AgentLaunchBatch, AgentLaunchInputError, AgentLaunchRequest,
        AgentObjective,
    },
    event_notifier::RuntimeEventNotifier,
};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tool_runtime::{
    Tool, ToolActivityPayloadPolicy, ToolCall, ToolDefinition, ToolExecutionContext,
    ToolExecutionFuture, ToolInvocationIdentity, ToolPermissionPolicy, ToolResult,
};

pub(super) const SPAWN_AGENTS_TOOL_NAME: &str = "spawn_agents";
const SPAWN_AGENTS_DESCRIPTION: &str = "\
Launch child Agents for independent subtasks and wait for their completion reports. \
Dispatch when subtasks can run in parallel or when exploratory work would flood the main \
context: each child runs in its own context and only its final report returns. Put multiple \
independent subtasks in one batch; the call blocks until every child in the batch finishes. \
Do not dispatch for a single file read or a simple lookup — use read, list_dir, or grep \
directly. Each objective must be self-contained: children cannot see this conversation, so \
include the background, constraints, and the exact deliverable. Children inherit this \
session's tool permissions, so their tool calls may require user approval. Each report \
returns only to you: restate or quote it in your reply. To ask a follow-up question about a \
dispatched child later, use send_agent_message with its agent_id. If a child's direction \
turns out wrong or its work is no longer needed, stop it with stop_agents instead of \
waiting for it to finish.";
const SPAWN_AGENTS_PROMPT_GUIDELINES: &str = "\
When to dispatch:
- Independent subtasks that can run in parallel: put them in one batch (up to 8 agents) instead of multiple serial calls.
- Exploratory or high-output work (broad searches, multi-file investigation, drafting) where intermediate output would flood the main context. A child runs in its own context; only its final report returns.

When not to dispatch:
- Reading a single file or running a simple lookup: use read, list_dir, grep, or find directly.
- Work that depends on conversation context or tool results a child cannot see.

Writing objectives:
- Make each objective self-contained: include the background, constraints, and the exact deliverable.
- Use display_title for a short human-readable label; put detailed requirements in the objective.

Wait semantics:
- The call blocks until every child in the batch completes. Prefer one batch of parallel agents over several serial calls.

Results:
- Each child returns its final report. Only this conversation receives it: restate or quote the report in your own reply.

Follow-ups:
- To follow up on a dispatched child or ask about its report, call send_agent_message with its agent_id (from a spawn_agents completion result or a send_agent_message receipt).

Stopping:
- If a child's direction is wrong or its work is no longer needed, call stop_agents with its agent_id instead of waiting for it to finish.
- A child that already finished does not need to be stopped.

Permissions:
- Children inherit this session's tool permissions; their tool calls may require user approval. Consider the approval cost before dispatching permission-heavy work.";
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
            // guidelines 是 description 的展开版，经 prompt assembly 注入 system prompt。
            .with_prompt_guidelines(SPAWN_AGENTS_PROMPT_GUIDELINES)
            .with_activity_payload_policy(ToolActivityPayloadPolicy::MetadataOnly)
            // spawn 是启动动作而非副作用；child 的实际工具调用各自走权限层。
            .with_permission_policy(ToolPermissionPolicy::Always)
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
                                "display_title": { "type": "string" }
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
                // tool result 只序列化 children 数组：group 归属等 host 控制元数据与
                // 240 列单行摘要对模型无用，报告与 metrics 信封直达父 Agent。
                Ok(completion) => match serde_json::to_string(&completion.children) {
                    Ok(children) => ToolResult::success(call.call_id, children),
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

/// host-owned Agent 工具共用的 invocation identity 前置校验；缺任一 identity 字段
/// 的调用在进入 bridge 前 fail closed。
pub(super) fn valid_identity(identity: &ToolInvocationIdentity) -> bool {
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
        assert_eq!(definition.permission_policy, ToolPermissionPolicy::Always);
        let guidelines = definition
            .prompt_guidelines
            .as_deref()
            .expect("spawn_agents should ship prompt guidelines");
        assert!(guidelines.contains("one batch"));
        assert!(guidelines.contains("self-contained"));
        assert!(guidelines.contains("may require user approval"));
        assert!(guidelines.contains("send_agent_message"));
        assert!(guidelines.contains("stop_agents"));
        assert!(guidelines.contains("no longer needed"));
        assert!(guidelines.contains("does not need to be stopped"));
        let description = definition
            .description
            .as_deref()
            .expect("spawn_agents should keep a description");
        assert!(description.contains("self-contained"));
        assert!(description.contains("send_agent_message"));
        assert!(description.contains("stop_agents"));
        // 给模型的文本不引用用户界面：模型只需回执链即可正确使用。
        for text in [description, guidelines] {
            assert!(!text.contains("/agents"), "{text}");
            assert!(!text.contains("panel"), "{text}");
        }
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
        // `instructions` 已从 caller 输入面删除：objective 承载全部任务输入。
        let item_properties = definition
            .input_schema
            .as_ref()
            .and_then(|schema| schema.get("properties"))
            .and_then(|properties| properties.get("agents"))
            .and_then(|agents| agents.get("items"))
            .and_then(|items| items.get("properties"))
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            item_properties,
            json!({"objective": {"type": "string"}, "display_title": {"type": "string"}}),
            "spawn_agents item schema must only offer objective and display_title"
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
        // 旧客户端传 instructions 必须被拒绝，而不是静默丢弃。
        assert!(
            parse_batch(json!({
                "agents": [{"objective": "work", "instructions": "extra guidance"}]
            }))
            .is_err(),
            "instructions is no longer a spawn_agents argument"
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

    #[tokio::test]
    async fn host_completion_serializes_the_flat_children_array() {
        use runtime_domain::agent::{
            AgentChildCompletion, AgentId, AgentLaunchGroupId, AgentOutcome, AgentTitle,
        };

        let (tool, mut receiver) = SpawnAgentsTool::channel(RuntimeEventNotifier::default());
        let cancellation = CancellationToken::new();
        let execution = tool.execute_with_context(
            ToolCall::new(
                "call",
                SPAWN_AGENTS_TOOL_NAME,
                json!({ "agents": [{ "objective": "scout the workspace layout" }] }),
            ),
            ToolExecutionContext::new(&cancellation).with_invocation_identity(test_identity()),
        );
        let host = async move {
            let request = receiver.recv().await.expect("host request should arrive");
            request
                .response
                .send(Ok(AgentGroupCompletion {
                    group_id: AgentLaunchGroupId::new(7),
                    parent_agent_id: AgentId::new(1),
                    children: vec![AgentChildCompletion {
                        agent_id: AgentId::new(2),
                        title: AgentTitle::resolve(
                            &AgentObjective::new("scout the workspace layout")
                                .expect("test objective should be valid"),
                            Some("workspace scout"),
                        )
                        .expect("test title should resolve"),
                        outcome: AgentOutcome::Completed,
                        report: Some("scouted report body".to_string()),
                        tokens: Some(1200),
                        tool_uses: Some(3),
                        duration: Some("45s".to_string()),
                        truncated: true,
                    }],
                    occurred_at_ms: 123,
                }))
                .expect("tool should still await the response");
        };
        let (result, ()) = tokio::join!(execution, host);

        assert!(!result.is_error());
        let payload: serde_json::Value =
            serde_json::from_str(&result.text_content()).expect("spawn result should be JSON");
        // tool result 是子结果数组本身：host 控制元数据（group_id/parent_agent_id/
        // occurred_at_ms）与单行 summary 不进入模型可见面。
        let children = payload
            .as_array()
            .unwrap_or_else(|| panic!("spawn result should be a flat array: {payload}"));
        assert_eq!(children.len(), 1);
        let child = &children[0];
        assert_eq!(
            child,
            &json!({
                "agent_id": 2,
                "title": "workspace scout",
                "outcome": "completed",
                "report": "scouted report body",
                "tokens": 1200,
                "tool_uses": 3,
                "duration": "45s",
                "truncated": true,
            }),
            "spawn result face should be exactly the 8-field child envelope"
        );
    }
}
