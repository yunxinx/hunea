/// `ToolActivityPayloadPolicy` 控制 provider tool payload 是否进入用户可见 activity/replay。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ToolActivityPayloadPolicy {
    /// 保留参数、结果与由它们派生的 content/location；维持普通工具的既有行为。
    #[default]
    Full,
    /// 只保留 definition-owned title/kind/status，屏蔽全部 call/result payload projection。
    MetadataOnly,
}

impl ToolActivityPayloadPolicy {
    /// 返回当前 policy 是否允许 payload 进入 activity projection。
    pub const fn includes_payload(self) -> bool {
        matches!(self, Self::Full)
    }
}

#[cfg(test)]
mod tests {
    use super::ToolActivityPayloadPolicy;
    use crate::ToolDefinition;

    #[test]
    fn tool_definitions_preserve_full_activity_payloads_by_default() {
        assert_eq!(
            ToolDefinition::new("read").activity_payload_policy,
            ToolActivityPayloadPolicy::Full
        );
    }

    #[test]
    fn metadata_only_policy_is_explicit() {
        let definition = ToolDefinition::new("spawn_agents")
            .with_activity_payload_policy(ToolActivityPayloadPolicy::MetadataOnly);

        assert!(!definition.activity_payload_policy.includes_payload());
    }
}
