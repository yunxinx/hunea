use agent_client_protocol::schema::{
    AgentCapabilities, InitializeRequest, InitializeResponse, PromptCapabilities, ProtocolVersion,
};
use mo_core::acp::{
    AcpAgentCapabilities, AcpPromptCapabilities, AcpProtocolVersion,
    acp_protocol_version_warning_message,
};

use super::{
    AcpInitializeOutcome, capabilities::lumos_client_capabilities, identity::lumos_client_info,
};

/// Lumos 当前明确支持的 ACP 协议版本。
pub const SUPPORTED_PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion::V1;

/// `build_initialize_request` 构造统一的 ACP initialize 请求。
pub(crate) fn build_initialize_request() -> InitializeRequest {
    InitializeRequest::new(SUPPORTED_PROTOCOL_VERSION)
        .client_capabilities(lumos_client_capabilities())
        .client_info(lumos_client_info())
}

/// `initialize_outcome_from_response` 将 ACP initialize 响应转换为内部事件数据。
pub(crate) fn initialize_outcome_from_response(
    response: InitializeResponse,
) -> AcpInitializeOutcome {
    let agent_info = response.agent_info;
    AcpInitializeOutcome {
        protocol_version: acp_protocol_version_from_schema(response.protocol_version),
        agent_name: agent_info.as_ref().map(|info| info.name.clone()),
        agent_title: agent_info.as_ref().and_then(|info| info.title.clone()),
        agent_version: agent_info.as_ref().map(|info| info.version.clone()),
        agent_capabilities: acp_agent_capabilities_from_schema(response.agent_capabilities),
        auth_method_count: response.auth_methods.len(),
    }
}

/// `protocol_version_warning` 在协商版本超出 Lumos 支持范围时返回提示文案。
pub(crate) fn protocol_version_warning(outcome: &AcpInitializeOutcome) -> Option<String> {
    (outcome.protocol_version != AcpProtocolVersion::V1)
        .then(|| acp_protocol_version_warning_message(&outcome.protocol_version))
}

fn acp_protocol_version_from_schema(version: ProtocolVersion) -> AcpProtocolVersion {
    match version {
        ProtocolVersion::V0 => AcpProtocolVersion::V0,
        ProtocolVersion::V1 => AcpProtocolVersion::V1,
        _ => AcpProtocolVersion::Other(version.to_string()),
    }
}

fn acp_agent_capabilities_from_schema(capabilities: AgentCapabilities) -> AcpAgentCapabilities {
    AcpAgentCapabilities {
        load_session: capabilities.load_session,
        prompt_capabilities: acp_prompt_capabilities_from_schema(capabilities.prompt_capabilities),
    }
}

fn acp_prompt_capabilities_from_schema(capabilities: PromptCapabilities) -> AcpPromptCapabilities {
    AcpPromptCapabilities {
        image: capabilities.image,
        audio: capabilities.audio,
        embedded_context: capabilities.embedded_context,
    }
}
