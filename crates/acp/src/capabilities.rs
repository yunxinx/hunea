use agent_client_protocol::schema::ClientCapabilities;

/// `lumos_client_capabilities` 返回 Lumos 当前真正实现的 ACP client 能力。
///
/// Lumos 当前实现了完整 `terminal/*` 请求处理器；fs 读写仍不声明。
pub(crate) fn lumos_client_capabilities() -> ClientCapabilities {
    ClientCapabilities::new().terminal(true)
}
