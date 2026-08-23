//! Agent loop 的内部可替换契约。

mod native;

use std::fmt;

use runtime_domain::{
    context_budget::ContextWindowUsage,
    session::{
        ConversationResponse, ConversationTurnRequest, RuntimePermissionRequest,
        RuntimeRequestMetrics, RuntimeTarget, RuntimeTerminalSnapshot, RuntimeToolActivity,
        RuntimeToolActivityUpdate, TranscriptCustomPromptBinding, TranscriptSkillBinding,
        TranscriptUserAttachment, TranscriptUserMessage,
    },
};

pub(super) use native::NativeAgentRuntime;

/// `AgentId` 标识一个由 runtime host 管理的 Agent handle。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct AgentId(u64);

impl AgentId {
    pub(super) const MAIN: Self = Self(1);
}

/// `AgentTurnId` 标识一次 Agent turn，避免事件依赖“唯一活跃 worker”的隐式假设。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct AgentTurnId(u64);

impl AgentTurnId {
    pub(super) const fn new(value: u64) -> Self {
        Self(value)
    }
}

/// `AgentUserDelivery` 只保存用户实际提交并应在 transcript 中显示的内容。
#[derive(Clone, PartialEq, Eq)]
pub(super) struct AgentUserDelivery {
    content: String,
    attachments: Vec<TranscriptUserAttachment>,
}

impl fmt::Debug for AgentUserDelivery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentUserDelivery")
            .field("content_chars", &self.content.chars().count())
            .field("attachment_count", &self.attachments.len())
            .finish()
    }
}

/// `AgentTurnControls` 保存结构化 skill/prompt 绑定，不把解析出的 instruction body
/// 混入 delivery DTO。
#[derive(Clone, PartialEq, Eq)]
pub(super) struct AgentTurnControls {
    skill_bindings: Vec<TranscriptSkillBinding>,
    custom_prompt_bindings: Vec<TranscriptCustomPromptBinding>,
}

impl fmt::Debug for AgentTurnControls {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentTurnControls")
            .field("skill_binding_count", &self.skill_bindings.len())
            .field(
                "custom_prompt_binding_count",
                &self.custom_prompt_bindings.len(),
            )
            .finish()
    }
}

/// `AgentTurnRequest` 把用户 delivery/control 与当前 native execution envelope 分开。
///
/// `ConversationTurnRequest` 仅作为迁移期的私有 native envelope；它不会出现在 Debug、
/// inspection 或 contract 的公开字段中。第二个 adapter 出现后应由 capability resolution
/// 取代该私有字段。
#[derive(Clone, PartialEq, Eq)]
pub(super) struct AgentTurnRequest {
    delivery: AgentUserDelivery,
    controls: AgentTurnControls,
    native_request: ConversationTurnRequest,
}

impl AgentTurnRequest {
    pub(super) fn from_conversation_request(request: ConversationTurnRequest) -> Self {
        let source_message = request
            .transcript_user_message()
            .cloned()
            .unwrap_or_else(|| TranscriptUserMessage {
                content: request.message_text(),
                attachments: Vec::new(),
                skill_bindings: Vec::new(),
                custom_prompt_bindings: Vec::new(),
            });
        Self {
            delivery: AgentUserDelivery {
                content: source_message.content,
                attachments: source_message.attachments,
            },
            controls: AgentTurnControls {
                skill_bindings: source_message.skill_bindings,
                custom_prompt_bindings: source_message.custom_prompt_bindings,
            },
            native_request: request,
        }
    }

    pub(super) fn target(&self) -> RuntimeTarget {
        self.native_request.target()
    }

    pub(super) fn activity_label(&self) -> &str {
        self.native_request.model_id()
    }

    fn into_parts(self) -> (ConversationTurnRequest, TranscriptUserMessage) {
        let source_message = TranscriptUserMessage {
            content: self.delivery.content,
            attachments: self.delivery.attachments,
            skill_bindings: self.controls.skill_bindings,
            custom_prompt_bindings: self.controls.custom_prompt_bindings,
        };
        (self.native_request, source_message)
    }
}

impl fmt::Debug for AgentTurnRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentTurnRequest")
            .field("target", &self.target())
            .field("delivery", &self.delivery)
            .field("controls", &self.controls)
            .finish()
    }
}

/// `AgentCommand` 只描述 Agent loop 行为；session tree、prompt editor、model refresh
/// 与 context budget 继续由 host capability 处理。
pub(super) enum AgentCommand {
    SubmitTurn {
        agent_id: AgentId,
        turn_id: AgentTurnId,
        request: Box<AgentTurnRequest>,
    },
    Interrupt {
        agent_id: AgentId,
        target: Option<RuntimeTarget>,
    },
    RespondPermission {
        agent_id: AgentId,
        target: Option<RuntimeTarget>,
        request_id: String,
        option_id: Option<String>,
    },
}

impl fmt::Debug for AgentCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SubmitTurn {
                agent_id,
                turn_id,
                request,
            } => f
                .debug_struct("SubmitTurn")
                .field("agent_id", agent_id)
                .field("turn_id", turn_id)
                .field("request", request)
                .finish(),
            Self::Interrupt { agent_id, target } => f
                .debug_struct("Interrupt")
                .field("agent_id", agent_id)
                .field("target", target)
                .finish(),
            Self::RespondPermission {
                agent_id,
                target,
                request_id,
                option_id,
            } => f
                .debug_struct("RespondPermission")
                .field("agent_id", agent_id)
                .field("target", target)
                .field("request_id", request_id)
                .field("has_option", &option_id.is_some())
                .finish(),
        }
    }
}

/// `AgentCommandReceipt` 只确认命令已原子准入；异步执行结果通过 `AgentEvent` 交付。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AgentCommandReceipt {
    Accepted,
    TurnStarted {
        turn_id: AgentTurnId,
        target: RuntimeTarget,
        activity_label: String,
    },
    Interrupted {
        target: Option<RuntimeTarget>,
    },
}

/// `AgentEvent` 是带显式 Agent/turn identity 的事实 envelope。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AgentEvent {
    pub(super) agent_id: AgentId,
    pub(super) turn_id: AgentTurnId,
    pub(super) target: RuntimeTarget,
    pub(super) kind: AgentEventKind,
}

/// `AgentEventKind` 只包含 Agent turn/progress/tool/permission/terminal 事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AgentEventKind {
    SystemMessage {
        message: String,
    },
    Retrying {
        message: String,
    },
    OutputTokenEstimate {
        total_tokens: usize,
    },
    InputTokenEstimate {
        total_tokens: usize,
    },
    Thinking {
        is_thinking: bool,
    },
    AssistantDelta {
        content: String,
    },
    ReasoningDelta {
        content: String,
    },
    ToolActivityStarted {
        activity: RuntimeToolActivity,
    },
    ToolActivityUpdated {
        update: RuntimeToolActivityUpdate,
    },
    TerminalUpdated {
        snapshot: RuntimeTerminalSnapshot,
    },
    PermissionRequested {
        request: RuntimePermissionRequest,
    },
    /// 环境探测失败仍按旧行为继续用空 injection 启动 turn，因此不是 terminal fact。
    PreparationWarning {
        message: String,
    },
    TurnFinished {
        response: ConversationResponse,
        metrics: Option<RuntimeRequestMetrics>,
        context_usage: Option<ContextWindowUsage>,
    },
    TurnFailed {
        message: String,
    },
    TurnInterrupted,
}

impl AgentEventKind {
    pub(super) const fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::TurnFinished { .. } | Self::TurnFailed { .. } | Self::TurnInterrupted
        )
    }
}

/// `AgentRuntimeError` 区分 command admission 与 lifecycle 失败。
#[derive(Debug, thiserror::Error)]
pub(super) enum AgentRuntimeError {
    #[error("Agent runtime is disposed")]
    Disposed,
    #[error("Unknown agent handle")]
    UnknownAgent,
    #[error("Conversation request is already running")]
    Busy,
    #[error("{0}")]
    CommandRejected(String),
    #[error("Agent runtime shutdown failed: {0}")]
    Shutdown(String),
}

/// `AgentRuntime` 是 Agent loop 的最小内部 seam。
pub(super) trait AgentRuntime {
    fn dispatch(&mut self, command: AgentCommand)
    -> Result<AgentCommandReceipt, AgentRuntimeError>;

    /// 非阻塞、FIFO 地取出当前已就绪的全部 Agent facts。
    fn drain_events(&mut self) -> Vec<AgentEvent>;

    /// 幂等撤销 runtime 拥有的全部副作用并等待 producer quiescence。
    fn shutdown(&mut self) -> Result<(), AgentRuntimeError>;
}

#[cfg(test)]
mod tests {
    use std::{
        sync::mpsc,
        thread,
        time::{Duration, Instant},
    };

    use conversation_runtime::RuntimeEventNotifier;
    use provider_protocol::{ConversationItem, Role};
    use runtime_domain::{
        prompt_assembly::PromptSourceOrigin,
        provider::ProviderKind,
        session::{TranscriptSkillBinding, TranscriptUserMessage},
    };
    use tool_runtime::ToolExecutorRegistry;

    use super::*;
    use crate::runtime::AppRuntimeOptions;

    fn native_runtime(event_notifier: RuntimeEventNotifier) -> NativeAgentRuntime {
        NativeAgentRuntime::new(
            &AppRuntimeOptions {
                runtime_request_policy: runtime_domain::request_policy::RuntimeRequestPolicy::new(
                    0,
                    Vec::new(),
                    1,
                ),
                ..AppRuntimeOptions::default()
            },
            ToolExecutorRegistry::default(),
            Vec::new(),
            event_notifier,
        )
        .expect("native Agent runtime should initialize")
    }

    fn failing_turn_request() -> AgentTurnRequest {
        AgentTurnRequest::from_conversation_request(ConversationTurnRequest::new(
            "openai",
            ProviderKind::OpenAi,
            "gpt-4o-mini",
            None,
            None,
            None,
            ConversationItem::text(Role::User, "hello"),
        ))
    }

    #[test]
    fn request_debug_redacts_delivery_controls_and_native_credentials() {
        let request = AgentTurnRequest::from_conversation_request(
            ConversationTurnRequest::new_user_source_message(
                "provider",
                ProviderKind::OpenAi,
                "model",
                Some("https://credential.example/v1".to_string()),
                Some(runtime_domain::provider::ProviderApiKey::new("secret-key")),
                Some("SECRET_ENV".to_string()),
                TranscriptUserMessage {
                    content: "visible-sentinel".to_string(),
                    attachments: Vec::new(),
                    skill_bindings: vec![TranscriptSkillBinding {
                        skill_name: "private-skill".to_string(),
                        origin: PromptSourceOrigin::Project,
                        skill_path: "/private/SKILL.md".to_string(),
                        start_char: 0,
                        end_char: 1,
                    }],
                    custom_prompt_bindings: Vec::new(),
                },
            ),
        );

        let debug = format!("{request:?}");
        for secret in [
            "visible-sentinel",
            "private-skill",
            "/private/SKILL.md",
            "credential.example",
            "secret-key",
            "SECRET_ENV",
        ] {
            assert!(!debug.contains(secret), "debug output leaked {secret}");
        }
        assert!(debug.contains("content_chars"));
        assert!(debug.contains("skill_binding_count"));
    }

    #[test]
    fn native_runtime_wakes_only_after_an_identified_event_is_available() {
        let (wake_tx, wake_rx) = mpsc::channel();
        let notifier = RuntimeEventNotifier::default();
        let _binding = notifier.bind_callback(move || {
            let _ = wake_tx.send(());
        });
        let mut runtime = native_runtime(notifier);
        let turn_id = AgentTurnId::new(17);
        let target = failing_turn_request().target();
        let runtime_contract: &mut dyn AgentRuntime = &mut runtime;

        runtime_contract
            .dispatch(AgentCommand::SubmitTurn {
                agent_id: AgentId::MAIN,
                turn_id,
                request: Box::new(failing_turn_request()),
            })
            .expect("native turn should be admitted before provider preflight");
        wake_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("accepted turn should wake after publishing an event");

        let mut events = runtime_contract.drain_events();
        let payload_deadline = Instant::now() + Duration::from_secs(2);
        while events.is_empty() && Instant::now() < payload_deadline {
            thread::sleep(Duration::from_millis(10));
            events.extend(runtime_contract.drain_events());
        }
        assert!(
            !events.is_empty(),
            "wake should be followed by an available payload"
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        while !events.iter().any(|event| event.kind.is_terminal()) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
            events.extend(runtime_contract.drain_events());
        }
        assert!(
            events.iter().any(|event| event.kind.is_terminal()),
            "provider preflight failure should terminate the test turn: {events:#?}"
        );
        assert!(events.iter().all(|event| {
            event.agent_id == AgentId::MAIN && event.turn_id == turn_id && event.target == target
        }));

        thread::sleep(Duration::from_millis(20));
        assert!(
            runtime_contract.drain_events().is_empty(),
            "terminal fact must close the turn against late deltas"
        );
        runtime_contract
            .shutdown()
            .expect("native runtime should shut down cleanly");
    }

    #[test]
    fn native_runtime_shutdown_is_idempotent_and_rejects_new_commands() {
        let mut runtime = native_runtime(RuntimeEventNotifier::default());
        let runtime_contract: &mut dyn AgentRuntime = &mut runtime;

        runtime_contract
            .shutdown()
            .expect("first shutdown should dispose native effects");
        runtime_contract
            .shutdown()
            .expect("repeated shutdown should remain a no-op");
        let error = runtime_contract
            .dispatch(AgentCommand::Interrupt {
                agent_id: AgentId::MAIN,
                target: None,
            })
            .expect_err("disposed runtime must reject commands");

        assert!(matches!(error, AgentRuntimeError::Disposed));
        assert!(runtime_contract.drain_events().is_empty());
    }

    #[test]
    fn permission_commands_are_routed_by_explicit_target() {
        let mut runtime = native_runtime(RuntimeEventNotifier::default());
        runtime.set_active_turn_for_test(
            AgentId::MAIN,
            AgentTurnId::new(23),
            RuntimeTarget::provider("openai", "gpt-4o-mini"),
        );
        let runtime_contract: &mut dyn AgentRuntime = &mut runtime;

        let error = runtime_contract
            .dispatch(AgentCommand::RespondPermission {
                agent_id: AgentId::MAIN,
                target: Some(RuntimeTarget::provider("local", "qwen3")),
                request_id: "permission-1".to_string(),
                option_id: None,
            })
            .expect_err("permission response for another target must be rejected");

        assert!(matches!(error, AgentRuntimeError::CommandRejected(_)));
    }

    #[test]
    fn reset_discards_queued_events_before_installing_a_new_generation() {
        let notifier = RuntimeEventNotifier::default();
        let mut runtime = native_runtime(notifier.clone());
        runtime.queue_event_for_test(AgentEvent {
            agent_id: AgentId::MAIN,
            turn_id: AgentTurnId::new(29),
            target: RuntimeTarget::provider("openai", "gpt-4o-mini"),
            kind: AgentEventKind::TurnInterrupted,
        });
        runtime
            .shutdown()
            .expect("old Agent generation should dispose cleanly");
        let mut replacement = native_runtime(notifier);

        assert!(replacement.drain_events().is_empty());
        replacement
            .shutdown()
            .expect("replacement Agent generation should dispose cleanly");
    }

    #[test]
    fn contract_module_does_not_import_terminal_or_native_worker_types() {
        let source = include_str!("mod.rs");
        for prohibited in [
            ["Conversation", "Worker"].concat(),
            ["LoopEvent", "Waker"].concat(),
        ] {
            assert!(
                !source.contains(&prohibited),
                "Agent contract must not expose {prohibited}"
            );
        }
    }
}
