use agent_client_protocol::schema::{AgentCapabilities, Implementation};

use super::AcpInitializeOutcome;

const LUMOS_CLIENT_NAME: &str = "lumos";
const LUMOS_CLIENT_TITLE: &str = "Lumos";

/// `AcpAgentIdentity` 保存 ACP agent 通过 initialize 上报的实现信息。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AcpAgentIdentity {
    pub name: Option<String>,
    pub title: Option<String>,
    pub version: Option<String>,
    pub agent_capabilities: AgentCapabilities,
}

impl AcpAgentIdentity {
    /// `from_initialize_outcome` 从 initialize 结果提取 agent identity。
    pub fn from_initialize_outcome(outcome: &AcpInitializeOutcome) -> Self {
        Self {
            name: outcome.agent_name.clone(),
            title: outcome.agent_title.clone(),
            version: outcome.agent_version.clone(),
            agent_capabilities: outcome.agent_capabilities.clone(),
        }
    }

    /// `has_agent_info` 表示 initialize 响应是否包含可展示的 agent 信息。
    pub fn has_agent_info(&self) -> bool {
        self.title
            .as_deref()
            .is_some_and(|title| !title.trim().is_empty())
            || self
                .name
                .as_deref()
                .is_some_and(|name| !name.trim().is_empty())
            || self
                .version
                .as_deref()
                .is_some_and(|version| !version.trim().is_empty())
    }

    /// `supports_image` 表示 agent 是否声明支持 prompt image block。
    pub fn supports_image(&self) -> bool {
        self.agent_capabilities.prompt_capabilities.image
    }

    /// `supports_audio` 表示 agent 是否声明支持 prompt audio block。
    pub fn supports_audio(&self) -> bool {
        self.agent_capabilities.prompt_capabilities.audio
    }

    /// `supports_embedded_context` 表示 agent 是否声明支持嵌入资源上下文。
    pub fn supports_embedded_context(&self) -> bool {
        self.agent_capabilities.prompt_capabilities.embedded_context
    }

    /// `display_name` 返回面向用户的 agent 名称。
    pub fn display_name(&self) -> String {
        self.title
            .as_deref()
            .filter(|title| !title.trim().is_empty())
            .or_else(|| self.name.as_deref().filter(|name| !name.trim().is_empty()))
            .unwrap_or("unknown agent")
            .to_string()
    }

    /// `display_label` 返回带版本号的 agent 展示标签。
    pub fn display_label(&self) -> String {
        let name = self.display_name();
        match self
            .version
            .as_deref()
            .filter(|version| !version.trim().is_empty())
        {
            Some(version) => format!("{name} ({version})"),
            None => name,
        }
    }
}

/// `lumos_client_info` 返回 Lumos 在 ACP initialize 中上报的 clientInfo。
pub(crate) fn lumos_client_info() -> Implementation {
    Implementation::new(LUMOS_CLIENT_NAME, env!("CARGO_PKG_VERSION")).title(LUMOS_CLIENT_TITLE)
}

/// `agent_display_name` 返回 initialize 结果的面向用户展示名。
pub fn agent_display_name(outcome: &AcpInitializeOutcome) -> String {
    AcpAgentIdentity::from_initialize_outcome(outcome).display_name()
}
