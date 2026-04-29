use super::AcpPermissionRequest;

/// `AcpModelOption` 表示 ACP agent 暴露的一个模型配置选项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpModelOption {
    pub value: String,
    pub name: String,
}

/// `AcpModelConfig` 表示 ACP session 当前的模型选择器状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpModelConfig {
    pub config_id: String,
    pub current_value: String,
    pub current_name: String,
    pub options: Vec<AcpModelOption>,
}

/// `AcpInitializeOutcome` 表示 ACP initialize 握手后的 agent 基本信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpInitializeOutcome {
    pub protocol_version: agent_client_protocol::schema::ProtocolVersion,
    pub agent_name: Option<String>,
    pub agent_title: Option<String>,
    pub agent_version: Option<String>,
    pub auth_method_count: usize,
}

/// `AcpSessionEvent` 表示后台 ACP 会话 worker 产生的运行事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcpSessionEvent {
    Started {
        agent_id: String,
        session_id: String,
        outcome: AcpInitializeOutcome,
    },
    StartFailed {
        agent_id: String,
        message: String,
    },
    SystemMessage {
        agent_id: String,
        message: String,
    },
    PromptStarted {
        agent_id: String,
    },
    AgentMessageChunk {
        agent_id: String,
        content: String,
    },
    AgentThoughtChunk {
        agent_id: String,
        content: String,
    },
    ModelConfigChanged {
        agent_id: String,
        config: AcpModelConfig,
    },
    ConfigChangeFailed {
        agent_id: String,
        message: String,
    },
    PromptResponse {
        agent_id: String,
        content: String,
        stop_reason: String,
    },
    PromptFailed {
        agent_id: String,
        message: String,
    },
    PromptInterrupted {
        agent_id: String,
    },
    PermissionRequested {
        agent_id: String,
        request: AcpPermissionRequest,
    },
    PermissionRequestCancelled {
        agent_id: String,
    },
    Stopped {
        agent_id: String,
        message: Option<String>,
    },
}
