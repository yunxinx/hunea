use std::{fmt, sync::Arc};

use provider_protocol::{PromptCacheRetention, PromptOptions, ProviderClient};
use runtime_domain::provider::ProviderKind;

/// `ProviderPromptCachePolicy` 描述 provider registration 对 session affinity 的支持。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProviderPromptCachePolicy {
    /// 不向 provider 发送 session affinity metadata。
    #[default]
    Disabled,
    /// 发送 session affinity key，但不请求扩展 retention。
    SessionAffinity,
    /// 发送 session affinity key，并请求 24 小时 retention。
    SessionAffinityLong24h,
}

/// `ProviderClientLease` 是一次 runtime operation 使用的不可变 provider generation 快照。
///
/// 该 lease 不暴露 endpoint、credential、factory 或 registration identity。owner 必须在撤销
/// provider registration 前先 quiesce 所有持有 lease 的 worker。
#[derive(Clone)]
pub struct ProviderClientLease {
    provider_id: String,
    provider_kind: ProviderKind,
    client: Arc<dyn ProviderClient>,
    prompt_cache_policy: ProviderPromptCachePolicy,
}

impl ProviderClientLease {
    /// `new` 绑定 provider-neutral client 与非敏感执行 metadata。
    pub fn new(
        provider_id: impl Into<String>,
        provider_kind: ProviderKind,
        client: Arc<dyn ProviderClient>,
        prompt_cache_policy: ProviderPromptCachePolicy,
    ) -> Self {
        Self {
            provider_id: provider_id.into(),
            provider_kind,
            client,
            prompt_cache_policy,
        }
    }

    /// `provider_id` 返回 lease 对应的 provider identity。
    pub fn provider_id(&self) -> &str {
        &self.provider_id
    }

    /// `provider_kind` 返回 provider protocol kind。
    pub const fn provider_kind(&self) -> ProviderKind {
        self.provider_kind
    }

    pub(crate) fn client(&self) -> &(dyn ProviderClient + 'static) {
        self.client.as_ref()
    }

    pub(crate) fn apply_prompt_cache_options(
        &self,
        options: &mut PromptOptions,
        session_prompt_cache_key: Option<&str>,
    ) {
        let Some(prompt_cache_key) = session_prompt_cache_key else {
            return;
        };
        match self.prompt_cache_policy {
            ProviderPromptCachePolicy::Disabled => {}
            ProviderPromptCachePolicy::SessionAffinity => {
                options.prompt_cache_key = Some(prompt_cache_key.to_string());
            }
            ProviderPromptCachePolicy::SessionAffinityLong24h => {
                options.prompt_cache_key = Some(prompt_cache_key.to_string());
                options.prompt_cache_retention = Some(PromptCacheRetention::Long24h);
            }
        }
    }
}

impl fmt::Debug for ProviderClientLease {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderClientLease")
            .field("provider_id", &self.provider_id)
            .field("provider_kind", &self.provider_kind)
            .field("prompt_cache_policy", &self.prompt_cache_policy)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use provider_protocol::{
        ModelDescriptor, PromptCompletion, PromptRequest, ProviderCapabilities, ProviderError,
        ProviderFuture, StreamEventSink,
    };

    use super::*;

    struct FakeProvider;

    impl ProviderClient for FakeProvider {
        fn stream_prompt<'a>(
            &'a self,
            _request: &'a PromptRequest,
            _sink: &'a mut (dyn StreamEventSink + Send),
        ) -> ProviderFuture<'a, Result<PromptCompletion, ProviderError>> {
            Box::pin(async { unreachable!("lease debug test must not call provider") })
        }

        fn list_models<'a>(
            &'a self,
        ) -> ProviderFuture<'a, Result<Vec<ModelDescriptor>, ProviderError>> {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities::chat_completions()
        }
    }

    #[test]
    fn debug_does_not_project_concrete_client() {
        let lease = ProviderClientLease::new(
            "local",
            ProviderKind::OpenAiCompatible,
            Arc::new(FakeProvider),
            ProviderPromptCachePolicy::Disabled,
        );

        let debug = format!("{lease:?}");

        assert!(debug.contains("local"));
        assert!(!debug.contains("FakeProvider"));
    }
}
