use super::AcpPermissionRequest;
use agent_client_protocol::schema::{AgentCapabilities, ProtocolVersion};

/// `AcpAvailableCommandInput` 表示 ACP agent 广告的命令输入要求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcpAvailableCommandInput {
    /// `Unstructured` 表示命令名后的全部文本都会作为输入传给 agent。
    Unstructured { hint: String },
    /// `Unknown` 为未来 ACP schema 新增输入类型预留扩展点。
    Unknown,
}

/// `AcpAvailableCommand` 表示 ACP agent 广告的一条动态斜杠命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpAvailableCommand {
    pub name: String,
    pub description: String,
    pub input: Option<AcpAvailableCommandInput>,
}

/// `AcpModelOption` 表示 ACP agent 暴露的一个模型配置选项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpModelOption {
    pub value: String,
    pub name: String,
}

/// `AcpModelConfig` 表示 ACP session 当前的模型选择器状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpModelConfig {
    /// `config_id` 为 `Some` 时表示该 session 使用 `config_options` 模型选择器。
    /// `None` 时表示该 session 使用 legacy `models` 模型状态并应走 `session/set_model`。
    pub config_id: Option<String>,
    pub current_value: String,
    pub current_name: String,
    pub options: Vec<AcpModelOption>,
}

/// `AcpToolKind` 是 ACP tool call 的内部工具分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AcpToolKind {
    Read,
    Edit,
    Delete,
    Move,
    Search,
    Execute,
    Think,
    Fetch,
    SwitchMode,
    Other,
}

/// `AcpToolCallStatus` 是 ACP tool call 的内部生命周期状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AcpToolCallStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

/// `AcpToolCallLocation` 表示 tool call 关联的文件位置。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AcpToolCallLocation {
    pub path: String,
    pub line: Option<u32>,
}

/// `AcpToolCallContent` 表示 tool call 的富内容片段。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AcpToolCallContent {
    Text(String),
    Image {
        mime_type: String,
        uri: Option<String>,
    },
    Audio {
        mime_type: String,
    },
    ResourceLink {
        uri: String,
        name: String,
        title: Option<String>,
    },
    Resource {
        uri: String,
        mime_type: Option<String>,
        text: Option<String>,
    },
    Diff {
        path: String,
        old_text: Option<String>,
        new_text: String,
    },
    Terminal {
        terminal_id: String,
    },
    Unknown(String),
}

/// `AcpToolCall` 表示一次 ACP tool call 创建通知。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AcpToolCall {
    pub tool_call_id: String,
    pub title: String,
    pub kind: AcpToolKind,
    pub status: AcpToolCallStatus,
    pub content: Vec<AcpToolCallContent>,
    pub locations: Vec<AcpToolCallLocation>,
    pub raw_input: Option<String>,
    pub raw_output: Option<String>,
}

/// `AcpToolCallUpdate` 表示 ACP tool call 的增量更新。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AcpToolCallUpdate {
    pub tool_call_id: String,
    pub title: Option<String>,
    pub kind: Option<AcpToolKind>,
    pub status: Option<AcpToolCallStatus>,
    pub content: Option<Vec<AcpToolCallContent>>,
    pub locations: Option<Vec<AcpToolCallLocation>>,
    pub raw_input: Option<String>,
    pub raw_output: Option<String>,
}

/// `AcpInitializeOutcome` 表示 ACP initialize 握手后的 agent 基本信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpInitializeOutcome {
    pub protocol_version: ProtocolVersion,
    pub agent_name: Option<String>,
    pub agent_title: Option<String>,
    pub agent_version: Option<String>,
    pub agent_capabilities: AgentCapabilities,
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
    ToolCall {
        agent_id: String,
        call: AcpToolCall,
    },
    ToolCallUpdate {
        agent_id: String,
        update: AcpToolCallUpdate,
    },
    ModelConfigChanged {
        agent_id: String,
        config: AcpModelConfig,
    },
    AvailableCommandsChanged {
        agent_id: String,
        commands: Vec<AcpAvailableCommand>,
    },
    ConfigChangeSucceeded {
        agent_id: String,
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
