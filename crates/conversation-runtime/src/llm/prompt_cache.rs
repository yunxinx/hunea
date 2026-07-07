use std::net::IpAddr;

use provider_protocol::{PromptCacheRetention, PromptOptions};
use runtime_domain::provider::ProviderKind;

use crate::conversation::PreparedConversationRequest;

/// 将 provider prompt cache 策略写入请求选项。
pub(super) fn apply_prompt_cache_options(
    options: &mut PromptOptions,
    request: &PreparedConversationRequest,
) {
    let Some(policy) = OpenAiPromptCachePolicy::for_request(request) else {
        return;
    };
    let Some(prompt_cache_key) = request.session_prompt_cache_key().map(str::to_string) else {
        return;
    };
    options.prompt_cache_key = Some(prompt_cache_key);
    options.prompt_cache_retention = policy.retention;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OpenAiPromptCachePolicy {
    retention: Option<PromptCacheRetention>,
}

impl OpenAiPromptCachePolicy {
    fn for_request(request: &PreparedConversationRequest) -> Option<Self> {
        Self::from_provider(request.provider_kind(), request.base_url())
    }

    fn from_provider(provider_kind: ProviderKind, base_url: Option<&str>) -> Option<Self> {
        match provider_kind {
            ProviderKind::OpenAi => Some(Self { retention: None }),
            ProviderKind::OpenAiResponses => {
                let retention = match base_url.and_then(openai_cache_endpoint_kind) {
                    Some(OpenAiCacheEndpointKind::RemoteCompatible) => {
                        Some(PromptCacheRetention::Long24h)
                    }
                    _ => None,
                };
                Some(Self { retention })
            }
            ProviderKind::OpenAiCompatible => {
                let endpoint_kind = base_url.and_then(openai_cache_endpoint_kind)?;
                let retention = match endpoint_kind {
                    OpenAiCacheEndpointKind::DirectOpenAi => None,
                    OpenAiCacheEndpointKind::RemoteCompatible => {
                        Some(PromptCacheRetention::Long24h)
                    }
                };
                Some(Self { retention })
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenAiCacheEndpointKind {
    DirectOpenAi,
    RemoteCompatible,
}

fn openai_cache_endpoint_kind(base_url: &str) -> Option<OpenAiCacheEndpointKind> {
    let url = reqwest::Url::parse(base_url).ok()?;
    let host = url.host_str()?;
    if host == "api.openai.com" {
        return Some(OpenAiCacheEndpointKind::DirectOpenAi);
    }
    if url.scheme() == "https" && !is_local_or_private_host(host) {
        return Some(OpenAiCacheEndpointKind::RemoteCompatible);
    }
    None
}

fn is_local_or_private_host(host: &str) -> bool {
    if host == "localhost" || host.ends_with(".localhost") || host.ends_with(".local") {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => {
            ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast()
        }
        Ok(IpAddr::V6(ip)) => {
            ip.is_loopback()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
                || ip.is_unspecified()
        }
        Err(_) => false,
    }
}
