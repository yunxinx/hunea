use agent_client_protocol::schema::ClientCapabilities;

/// `lumos_client_capabilities` 返回 Lumos 当前真正实现的 ACP client 能力。
///
/// Lumos 当前只是纯 TUI client，尚未实现 agent 可反向请求的 fs 读写、
/// `terminal/*` 创建或其他 client 侧能力处理器，因此必须保持空声明。
/// 后续真正接入对应请求处理后，再按已实现能力逐项填充这里。
pub(crate) fn lumos_client_capabilities() -> ClientCapabilities {
    ClientCapabilities::new()
}
