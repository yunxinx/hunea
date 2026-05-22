use agent_client_protocol::schema::Implementation;

pub use mo_core::acp::{AcpAgentIdentity, agent_display_name};

const LUMOS_CLIENT_NAME: &str = "lumos";
const LUMOS_CLIENT_TITLE: &str = "Lumos";

/// `lumos_client_info` 返回 Lumos 在 ACP initialize 中上报的 clientInfo。
pub(crate) fn lumos_client_info() -> Implementation {
    Implementation::new(LUMOS_CLIENT_NAME, env!("CARGO_PKG_VERSION")).title(LUMOS_CLIENT_TITLE)
}
