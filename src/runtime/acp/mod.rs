mod capabilities;
mod command;
mod event;
mod handshake;
mod identity;
mod initialize;
pub mod install;
mod permission;
mod protocol;
pub mod registry;
mod worker;

pub use command::{
    AcpSessionCatalog, AcpSessionCommand, AcpSessionResolveError, resolve_session_command,
};
pub use event::{AcpInitializeOutcome, AcpModelConfig, AcpModelOption, AcpSessionEvent};
pub use handshake::{
    AcpHandshakeError, initialize_agent_command, initialize_agent_command_blocking,
    initialize_agent_transport,
};
pub use identity::AcpAgentIdentity;
pub(crate) use identity::agent_display_name;
pub(crate) use initialize::debug_protocol_version_system_message;
pub use permission::{
    AcpPermissionOption, AcpPermissionOptionKind, AcpPermissionRequest, AcpPermissionRespondError,
};
pub use worker::{AcpSessionWorker, AcpWorkerSendError};

#[cfg(test)]
pub(crate) use permission::AcpPermissionRegistry;
#[cfg(test)]
pub(crate) use protocol::run_agent_transport_worker;
#[cfg(test)]
pub(crate) use worker::AcpWorkerCommand;

#[cfg(test)]
mod tests;
