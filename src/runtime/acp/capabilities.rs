use agent_client_protocol::schema::ClientCapabilities;

/// `lumos_client_capabilities` 返回 Lumos 当前真正实现的 ACP client 能力。
pub(crate) fn lumos_client_capabilities() -> ClientCapabilities {
    ClientCapabilities::new()
}
