use agent_client_protocol::schema::{InitializeRequest, InitializeResponse, ProtocolVersion};

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
        protocol_version: response.protocol_version,
        agent_name: agent_info.as_ref().map(|info| info.name.clone()),
        agent_title: agent_info.as_ref().and_then(|info| info.title.clone()),
        agent_version: agent_info.as_ref().map(|info| info.version.clone()),
        agent_capabilities: response.agent_capabilities,
        auth_method_count: response.auth_methods.len(),
    }
}

/// `protocol_version_warning` 在协商版本超出 Lumos 支持范围时返回提示文案。
pub(crate) fn protocol_version_warning(outcome: &AcpInitializeOutcome) -> Option<String> {
    (outcome.protocol_version != SUPPORTED_PROTOCOL_VERSION)
        .then(|| protocol_version_warning_message(&outcome.protocol_version))
}

/// `debug_protocol_version_system_message` 返回 ACP debug 面板使用的协议版本提示样例。
pub fn debug_protocol_version_system_message() -> String {
    protocol_version_warning_message(&ProtocolVersion::V0)
}

fn protocol_version_warning_message(negotiated: &ProtocolVersion) -> String {
    format!(
        "ACP protocol version mismatch: Lumos supports v{SUPPORTED_PROTOCOL_VERSION}, agent negotiated v{negotiated}. Continuing may be unstable."
    )
}
