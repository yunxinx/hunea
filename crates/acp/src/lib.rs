mod capabilities;
mod command;
mod event;
mod handshake;
mod identity;
mod initialize;
pub mod install;
mod permission;
mod prompt_builder;
mod protocol;
pub mod registry;
pub(crate) mod terminal;
mod worker;

pub use command::{
    AcpAgentServerConfig, AcpAgentServerType, AcpSessionCatalog, AcpSessionCatalogConfig,
    AcpSessionCommand, AcpSessionResolveError, resolve_session_command,
};
pub use event::{
    AcpAvailableCommand, AcpAvailableCommandInput, AcpInitializeOutcome, AcpModelConfig,
    AcpModelOption, AcpSessionEvent, AcpTerminalExitStatus, AcpTerminalSnapshot, AcpToolCall,
    AcpToolCallContent, AcpToolCallLocation, AcpToolCallRawValue, AcpToolCallStatus,
    AcpToolCallUpdate, AcpToolKind,
};
pub use handshake::{
    AcpHandshakeError, initialize_agent_command, initialize_agent_command_blocking,
    initialize_agent_transport,
};
pub use identity::AcpAgentIdentity;
pub use identity::agent_display_name;
pub use mo_core::acp::debug_protocol_version_system_message;
pub use permission::{
    AcpPermissionOption, AcpPermissionOptionKind, AcpPermissionRequest, AcpPermissionRespondError,
};
pub use prompt_builder::{AcpPrompt, AcpPromptBlock, build_acp_prompt_from_composer_text};
pub use worker::{AcpSessionWorker, AcpWorkerSendError};

#[cfg(test)]
pub(crate) use permission::AcpPermissionRegistry;
#[cfg(test)]
pub(crate) use protocol::{
    AcpTransportState, run_agent_transport_worker, run_agent_transport_worker_with_terminal_control,
};
#[cfg(test)]
pub(crate) use worker::{AcpTerminalControlCommand, AcpWorkerCommand};

#[cfg(test)]
mod tests;
