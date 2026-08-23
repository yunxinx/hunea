use provider_protocol::PromptOptions;

use crate::conversation::PreparedConversationRequest;
use crate::llm::ProviderClientLease;

/// 将 lease 的 provider prompt cache 策略写入请求选项。
pub(super) fn apply_prompt_cache_options(
    lease: &ProviderClientLease,
    options: &mut PromptOptions,
    request: &PreparedConversationRequest,
) {
    lease.apply_prompt_cache_options(options, request.session_prompt_cache_key());
}
