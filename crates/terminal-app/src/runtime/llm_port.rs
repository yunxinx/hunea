//! runtime host 拥有的 provider registry 与 immutable provider lease。

use std::{
    collections::BTreeMap,
    env, fmt,
    net::IpAddr,
    sync::{Arc, Mutex, Weak},
    time::Duration,
};

use conversation_runtime::{
    ProviderClientLease, ProviderPromptCachePolicy,
    models::{LoadedProviderConfig, MODEL_LIST_TIMEOUT},
};
use openai_compat_provider::{
    DEFAULT_OPENAI_BASE_URL, OpenAiChatCompletionsClient, OpenAiClientConfig,
    OpenAiCompatibleClient, OpenAiResponsesClient,
};
use provider_protocol::ProviderClient;
use runtime_domain::{model_catalog::ModelSelection, provider::ProviderKind};
use url::Url;

/// `LlmPortError` 只携带可安全投影到 runtime/TUI 的 provider metadata。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(super) enum LlmPortError {
    #[error("unknown provider {provider_id}")]
    UnknownProvider { provider_id: String },
    #[error("provider {provider_id} is already registered")]
    DuplicateProvider { provider_id: String },
    #[error("provider {provider_id} credentials are unavailable")]
    CredentialsUnavailable { provider_id: String },
    #[error("provider {provider_id} configuration is invalid")]
    ProviderConfigurationInvalid { provider_id: String },
    #[error("provider {provider_id} uses unsupported provider kind {provider_kind}")]
    UnsupportedProvider {
        provider_id: String,
        provider_kind: ProviderKind,
    },
    #[error("provider {provider_id} client is unavailable")]
    ClientUnavailable { provider_id: String },
    #[error("llm port is disposed")]
    Disposed,
}

/// composition consumer 拥有的 provider-specific client 构造边界。
pub(super) trait ProviderClientFactory: Send + Sync {
    fn create_client(
        &self,
        idle_timeout: Duration,
    ) -> Result<Arc<dyn ProviderClient>, LlmPortError>;

    fn provider_kind(&self) -> ProviderKind;

    fn prompt_cache_policy(&self) -> ProviderPromptCachePolicy;

    fn adapter_kind(&self) -> &'static str;
}

struct ProviderEntry {
    _owner: String,
    provider_kind: ProviderKind,
    registration_id: u64,
    factory: Arc<dyn ProviderClientFactory>,
}

struct LlmPortState {
    is_active: bool,
    next_registration_id: u64,
    providers: BTreeMap<String, ProviderEntry>,
}

impl Default for LlmPortState {
    fn default() -> Self {
        Self {
            is_active: true,
            next_registration_id: 0,
            providers: BTreeMap::new(),
        }
    }
}

/// provider capability 的脱敏 inspection projection。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LlmProviderSnapshot {
    pub(super) provider_id: String,
    pub(super) provider_kind: ProviderKind,
    pub(super) adapter_kind: String,
}

/// `LlmPort` 是 runtime host 当前 live provider generation 的唯一解析 authority。
#[derive(Clone)]
pub(super) struct LlmPort {
    state: Arc<Mutex<LlmPortState>>,
}

impl LlmPort {
    pub(super) fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(LlmPortState::default())),
        }
    }

    /// 挂载当前配置中所有 enabled built-in provider。
    pub(super) fn mount_builtin_providers(
        &self,
        owner: impl Into<String>,
        configs: &[LoadedProviderConfig],
    ) -> Result<ProviderRegistrations, LlmPortError> {
        let owner = owner.into();
        let mut registrations = ProviderRegistrations::default();
        for config in configs
            .iter()
            .filter(|config| config.is_enabled() && supports_builtin_provider(config.kind()))
        {
            let registration = self.register(
                owner.clone(),
                config.provider_id(),
                Arc::new(OpenAiCompatibleProviderFactory::new(config.clone())),
            )?;
            registrations.push(registration);
        }
        Ok(registrations)
    }

    /// 注册一个 provider slot；duplicate 在 identity 分配与 map mutation 前被拒绝。
    pub(super) fn register(
        &self,
        owner: impl Into<String>,
        provider_id: impl Into<String>,
        factory: Arc<dyn ProviderClientFactory>,
    ) -> Result<ProviderRegistration, LlmPortError> {
        let owner = owner.into();
        let provider_id = provider_id.into();
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.is_active {
            return Err(LlmPortError::Disposed);
        }
        if state.providers.contains_key(&provider_id) {
            return Err(LlmPortError::DuplicateProvider { provider_id });
        }

        let registration_id = state.next_registration_id;
        state.next_registration_id = state
            .next_registration_id
            .checked_add(1)
            .expect("provider registration id space should be unreachable");
        state.providers.insert(
            provider_id.clone(),
            ProviderEntry {
                _owner: owner,
                provider_kind: factory.provider_kind(),
                registration_id,
                factory,
            },
        );
        drop(state);

        Ok(ProviderRegistration {
            state: Arc::downgrade(&self.state),
            provider_id,
            registration_id,
            is_disposed: false,
        })
    }

    /// 为一次 conversation operation 解析 immutable client lease。
    pub(super) fn resolve(
        &self,
        selection: &ModelSelection,
        idle_timeout: Duration,
    ) -> Result<ProviderClientLease, LlmPortError> {
        self.resolve_provider(&selection.provider_id, idle_timeout)
    }

    /// 为一次 model-list operation 解析同一 provider registration。
    pub(super) fn resolve_model_listing(
        &self,
        provider_id: &str,
    ) -> Result<ProviderClientLease, LlmPortError> {
        self.resolve_provider(provider_id, MODEL_LIST_TIMEOUT)
    }

    fn resolve_provider(
        &self,
        provider_id: &str,
        idle_timeout: Duration,
    ) -> Result<ProviderClientLease, LlmPortError> {
        let (provider_kind, prompt_cache_policy, factory) =
            {
                let state = self
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if !state.is_active {
                    return Err(LlmPortError::Disposed);
                }
                let entry = state.providers.get(provider_id).ok_or_else(|| {
                    LlmPortError::UnknownProvider {
                        provider_id: provider_id.to_string(),
                    }
                })?;
                (
                    entry.provider_kind,
                    entry.factory.prompt_cache_policy(),
                    Arc::clone(&entry.factory),
                )
            };

        let client = factory.create_client(idle_timeout)?;
        Ok(ProviderClientLease::new(
            provider_id,
            provider_kind,
            client,
            prompt_cache_policy,
        ))
    }

    pub(super) fn inspection_snapshot(&self) -> Vec<LlmProviderSnapshot> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.is_active {
            return Vec::new();
        }
        state
            .providers
            .iter()
            .map(|(provider_id, entry)| LlmProviderSnapshot {
                provider_id: provider_id.clone(),
                provider_kind: entry.provider_kind,
                adapter_kind: entry.factory.adapter_kind().to_string(),
            })
            .collect()
    }

    /// 停止当前 generation 的 mutation、resolution 与 inspection。
    pub(super) fn deactivate(&self) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_active = false;
    }
}

fn supports_builtin_provider(provider_kind: ProviderKind) -> bool {
    matches!(
        provider_kind,
        ProviderKind::OpenAi | ProviderKind::OpenAiCompatible | ProviderKind::OpenAiResponses
    )
}

impl fmt::Debug for LlmPort {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        formatter
            .debug_struct("LlmPort")
            .field("is_active", &state.is_active)
            .field("provider_count", &state.providers.len())
            .finish()
    }
}

/// 一次 provider registration 的幂等、stale-safe inverse。
pub(super) struct ProviderRegistration {
    state: Weak<Mutex<LlmPortState>>,
    provider_id: String,
    registration_id: u64,
    is_disposed: bool,
}

impl ProviderRegistration {
    pub(super) fn dispose(&mut self) {
        if self.is_disposed {
            return;
        }
        self.is_disposed = true;
        let Some(state) = self.state.upgrade() else {
            return;
        };
        let mut state = state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let owns_current_entry = state
            .providers
            .get(&self.provider_id)
            .is_some_and(|entry| entry.registration_id == self.registration_id);
        if owns_current_entry {
            state.providers.remove(&self.provider_id);
        }
    }
}

impl fmt::Debug for ProviderRegistration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderRegistration")
            .field("provider_id", &self.provider_id)
            .field("is_disposed", &self.is_disposed)
            .finish_non_exhaustive()
    }
}

impl Drop for ProviderRegistration {
    fn drop(&mut self) {
        self.dispose();
    }
}

/// 一组 provider registration 的逆序 aggregate inverse。
#[derive(Default)]
pub(super) struct ProviderRegistrations {
    registrations: Vec<ProviderRegistration>,
    is_disposed: bool,
}

impl ProviderRegistrations {
    fn push(&mut self, registration: ProviderRegistration) {
        self.registrations.push(registration);
    }

    pub(super) fn dispose(&mut self) {
        if self.is_disposed {
            return;
        }
        self.is_disposed = true;
        for registration in self.registrations.iter_mut().rev() {
            registration.dispose();
        }
        self.registrations.clear();
    }
}

impl Drop for ProviderRegistrations {
    fn drop(&mut self) {
        self.dispose();
    }
}

/// 内建 OpenAI-compatible adapter factory。
struct OpenAiCompatibleProviderFactory {
    config: LoadedProviderConfig,
}

impl OpenAiCompatibleProviderFactory {
    fn new(config: LoadedProviderConfig) -> Self {
        Self { config }
    }
}

impl fmt::Debug for OpenAiCompatibleProviderFactory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenAiCompatibleProviderFactory")
            .field("provider_id", &self.config.provider_id())
            .field("provider_kind", &self.config.kind())
            .field("has_base_url", &self.config.base_url().is_some())
            .field("has_inline_api_key", &self.config.api_key().is_some())
            .field("has_api_key_env", &self.config.api_key_env().is_some())
            .finish()
    }
}

impl ProviderClientFactory for OpenAiCompatibleProviderFactory {
    fn create_client(
        &self,
        idle_timeout: Duration,
    ) -> Result<Arc<dyn ProviderClient>, LlmPortError> {
        let provider_id = self.config.provider_id();
        let provider_kind = self.config.kind();
        let api_key = resolve_api_key(&self.config)?;
        let base_url = match provider_kind {
            ProviderKind::OpenAi => self
                .config
                .base_url()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(DEFAULT_OPENAI_BASE_URL),
            ProviderKind::OpenAiCompatible | ProviderKind::OpenAiResponses => self
                .config
                .base_url()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| LlmPortError::ProviderConfigurationInvalid {
                    provider_id: provider_id.to_string(),
                })?,
            unsupported => {
                return Err(LlmPortError::UnsupportedProvider {
                    provider_id: provider_id.to_string(),
                    provider_kind: unsupported,
                });
            }
        };

        let client_config =
            OpenAiClientConfig::new(base_url, api_key, idle_timeout).map_err(|_| {
                LlmPortError::ProviderConfigurationInvalid {
                    provider_id: provider_id.to_string(),
                }
            })?;
        let client = match provider_kind {
            ProviderKind::OpenAiResponses => {
                OpenAiResponsesClient::new(client_config).map(OpenAiCompatibleClient::Responses)
            }
            ProviderKind::OpenAi | ProviderKind::OpenAiCompatible => {
                OpenAiChatCompletionsClient::new(client_config)
                    .map(OpenAiCompatibleClient::ChatCompletions)
            }
            unsupported => {
                return Err(LlmPortError::UnsupportedProvider {
                    provider_id: provider_id.to_string(),
                    provider_kind: unsupported,
                });
            }
        }
        .map_err(|_| LlmPortError::ClientUnavailable {
            provider_id: provider_id.to_string(),
        })?;
        Ok(Arc::new(client))
    }

    fn provider_kind(&self) -> ProviderKind {
        self.config.kind()
    }

    fn prompt_cache_policy(&self) -> ProviderPromptCachePolicy {
        prompt_cache_policy(self.config.kind(), self.config.base_url())
    }

    fn adapter_kind(&self) -> &'static str {
        "openai-compatible"
    }
}

fn resolve_api_key(config: &LoadedProviderConfig) -> Result<Option<String>, LlmPortError> {
    if let Some(api_key) = config.api_key() {
        return Ok(Some(api_key.as_str().to_string()));
    }
    if let Some(api_key_env) = config
        .api_key_env()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return env::var(api_key_env)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .map(Some)
            .ok_or_else(|| LlmPortError::CredentialsUnavailable {
                provider_id: config.provider_id().to_string(),
            });
    }
    if config.kind() == ProviderKind::OpenAi {
        return Err(LlmPortError::CredentialsUnavailable {
            provider_id: config.provider_id().to_string(),
        });
    }
    Ok(None)
}

fn prompt_cache_policy(
    provider_kind: ProviderKind,
    base_url: Option<&str>,
) -> ProviderPromptCachePolicy {
    match provider_kind {
        ProviderKind::OpenAi => ProviderPromptCachePolicy::SessionAffinity,
        ProviderKind::OpenAiResponses => match base_url.and_then(cache_endpoint_kind) {
            Some(CacheEndpointKind::RemoteCompatible) => {
                ProviderPromptCachePolicy::SessionAffinityLong24h
            }
            _ => ProviderPromptCachePolicy::SessionAffinity,
        },
        ProviderKind::OpenAiCompatible => match base_url.and_then(cache_endpoint_kind) {
            Some(CacheEndpointKind::DirectOpenAi) => ProviderPromptCachePolicy::SessionAffinity,
            Some(CacheEndpointKind::RemoteCompatible) => {
                ProviderPromptCachePolicy::SessionAffinityLong24h
            }
            None => ProviderPromptCachePolicy::Disabled,
        },
        _ => ProviderPromptCachePolicy::Disabled,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CacheEndpointKind {
    DirectOpenAi,
    RemoteCompatible,
}

fn cache_endpoint_kind(base_url: &str) -> Option<CacheEndpointKind> {
    let url = Url::parse(base_url).ok()?;
    let host = url.host_str()?;
    if host == "api.openai.com" {
        return Some(CacheEndpointKind::DirectOpenAi);
    }
    if url.scheme() == "https" && !is_local_or_private_host(host) {
        return Some(CacheEndpointKind::RemoteCompatible);
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use provider_protocol::{
        ModelDescriptor, PromptCompletion, PromptRequest, ProviderCapabilities, ProviderError,
        ProviderFuture, StreamEventSink,
    };
    use runtime_domain::provider::ProviderApiKey;

    use super::*;

    struct FakeProvider;

    impl ProviderClient for FakeProvider {
        fn stream_prompt<'a>(
            &'a self,
            _request: &'a PromptRequest,
            _sink: &'a mut (dyn StreamEventSink + Send),
        ) -> ProviderFuture<'a, Result<PromptCompletion, ProviderError>> {
            Box::pin(async { unreachable!("registry contract test must not stream") })
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

    struct FakeFactory {
        create_count: Arc<AtomicUsize>,
    }

    impl ProviderClientFactory for FakeFactory {
        fn create_client(
            &self,
            _idle_timeout: Duration,
        ) -> Result<Arc<dyn ProviderClient>, LlmPortError> {
            self.create_count.fetch_add(1, Ordering::Relaxed);
            Ok(Arc::new(FakeProvider))
        }

        fn provider_kind(&self) -> ProviderKind {
            ProviderKind::OpenAiCompatible
        }

        fn prompt_cache_policy(&self) -> ProviderPromptCachePolicy {
            ProviderPromptCachePolicy::Disabled
        }

        fn adapter_kind(&self) -> &'static str {
            "fake"
        }
    }

    fn fake_factory(create_count: &Arc<AtomicUsize>) -> Arc<dyn ProviderClientFactory> {
        Arc::new(FakeFactory {
            create_count: Arc::clone(create_count),
        })
    }

    #[test]
    fn resolve_creates_one_immutable_lease_per_operation() {
        let create_count = Arc::new(AtomicUsize::new(0));
        let port = LlmPort::new();
        let _registration = port
            .register("fixture-owner", "local", fake_factory(&create_count))
            .expect("provider should register");

        let lease = port
            .resolve(
                &ModelSelection::new("local", "qwen3"),
                Duration::from_secs(30),
            )
            .expect("provider should resolve");

        assert_eq!(lease.provider_id(), "local");
        assert_eq!(lease.provider_kind(), ProviderKind::OpenAiCompatible);
        assert_eq!(create_count.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn duplicate_is_rejected_before_registry_mutation() {
        let create_count = Arc::new(AtomicUsize::new(0));
        let port = LlmPort::new();
        let _first = port
            .register("first-owner", "local", fake_factory(&create_count))
            .expect("first registration should succeed");
        let before = port.inspection_snapshot();

        let error = port
            .register(
                "secret-owner-sentinel",
                "local",
                fake_factory(&create_count),
            )
            .expect_err("duplicate should fail");

        assert!(matches!(error, LlmPortError::DuplicateProvider { .. }));
        assert_eq!(port.inspection_snapshot(), before);
    }

    #[test]
    fn explicit_dispose_and_drop_remove_only_owned_registration() {
        let create_count = Arc::new(AtomicUsize::new(0));
        let port = LlmPort::new();
        let mut first = port
            .register("owner", "first", fake_factory(&create_count))
            .expect("first provider should register");
        {
            let _second = port
                .register("owner", "second", fake_factory(&create_count))
                .expect("second provider should register");
            assert_eq!(port.inspection_snapshot().len(), 2);
        }
        assert_eq!(port.inspection_snapshot().len(), 1);

        first.dispose();
        first.dispose();
        assert!(port.inspection_snapshot().is_empty());
    }

    #[test]
    fn stale_handle_cannot_remove_replacement() {
        let create_count = Arc::new(AtomicUsize::new(0));
        let port = LlmPort::new();
        let mut first = port
            .register("owner", "local", fake_factory(&create_count))
            .expect("first provider should register");
        port.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .providers
            .remove("local");
        let _replacement = port
            .register("replacement-owner", "local", fake_factory(&create_count))
            .expect("replacement should register");

        first.dispose();

        assert_eq!(port.inspection_snapshot().len(), 1);
    }

    #[test]
    fn builtin_mount_rolls_back_earlier_registrations_on_duplicate() {
        let create_count = Arc::new(AtomicUsize::new(0));
        let port = LlmPort::new();
        let _existing = port
            .register("existing-owner", "duplicate", fake_factory(&create_count))
            .expect("existing provider should register");
        let configs = vec![
            LoadedProviderConfig::new(
                "first",
                ProviderKind::OpenAiCompatible,
                Some("http://localhost:11434/v1".to_string()),
                None,
                None,
                true,
            ),
            LoadedProviderConfig::new(
                "duplicate",
                ProviderKind::OpenAiCompatible,
                Some("http://localhost:11434/v1".to_string()),
                None,
                None,
                true,
            ),
        ];

        let error = match port.mount_builtin_providers("models-config", &configs) {
            Ok(_) => panic!("duplicate should abort the aggregate mount"),
            Err(error) => error,
        };

        assert!(matches!(error, LlmPortError::DuplicateProvider { .. }));
        assert_eq!(
            port.inspection_snapshot()
                .into_iter()
                .map(|snapshot| snapshot.provider_id)
                .collect::<Vec<_>>(),
            vec!["duplicate".to_string()]
        );
    }

    #[test]
    fn builtin_mount_skips_disabled_provider_configs() {
        let port = LlmPort::new();
        let mut registrations = port
            .mount_builtin_providers(
                "models-config",
                &[LoadedProviderConfig::new(
                    "disabled",
                    ProviderKind::OpenAiCompatible,
                    Some("http://localhost:11434/v1".to_string()),
                    None,
                    None,
                    false,
                )],
            )
            .expect("disabled config should not fail composition");

        assert!(port.inspection_snapshot().is_empty());
        registrations.dispose();
    }

    #[test]
    fn builtin_mount_skips_provider_kinds_without_a_builtin_adapter() {
        let port = LlmPort::new();
        let mut registrations = port
            .mount_builtin_providers(
                "models-config",
                &[
                    LoadedProviderConfig::new(
                        "anthropic",
                        ProviderKind::Anthropic,
                        Some("https://secret-unsupported.invalid/v1".to_string()),
                        None,
                        None,
                        true,
                    ),
                    LoadedProviderConfig::new(
                        "local",
                        ProviderKind::OpenAiCompatible,
                        Some("http://localhost:11434/v1".to_string()),
                        None,
                        None,
                        true,
                    ),
                ],
            )
            .expect("unsupported provider kinds should not poison built-in composition");

        assert_eq!(
            port.inspection_snapshot()
                .into_iter()
                .map(|snapshot| (snapshot.provider_id, snapshot.adapter_kind))
                .collect::<Vec<_>>(),
            vec![("local".to_string(), "openai-compatible".to_string())]
        );
        assert!(matches!(
            port.resolve_model_listing("anthropic"),
            Err(LlmPortError::UnknownProvider { provider_id }) if provider_id == "anthropic"
        ));
        registrations.dispose();
    }

    #[test]
    fn explicit_dispose_makes_provider_unresolvable_for_stream_and_model_listing() {
        let create_count = Arc::new(AtomicUsize::new(0));
        let port = LlmPort::new();
        let mut registration = port
            .register("owner", "local", fake_factory(&create_count))
            .expect("provider should register");

        registration.dispose();

        assert!(matches!(
            port.resolve(
                &ModelSelection::new("local", "qwen3"),
                Duration::from_secs(30)
            ),
            Err(LlmPortError::UnknownProvider { provider_id }) if provider_id == "local"
        ));
        assert!(matches!(
            port.resolve_model_listing("local"),
            Err(LlmPortError::UnknownProvider { provider_id }) if provider_id == "local"
        ));
        assert_eq!(create_count.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn deactivation_hides_projection_and_rejects_mutation_and_resolution() {
        let create_count = Arc::new(AtomicUsize::new(0));
        let port = LlmPort::new();
        let _registration = port
            .register("owner", "local", fake_factory(&create_count))
            .expect("provider should register");

        port.deactivate();

        assert!(port.inspection_snapshot().is_empty());
        assert!(matches!(
            port.resolve_model_listing("local"),
            Err(LlmPortError::Disposed)
        ));
        assert!(matches!(
            port.register("owner", "next", fake_factory(&create_count)),
            Err(LlmPortError::Disposed)
        ));
    }

    #[test]
    fn official_openai_without_any_credential_is_rejected_at_resolve_time() {
        let config =
            LoadedProviderConfig::new("openai", ProviderKind::OpenAi, None, None, None, true);
        let factory = OpenAiCompatibleProviderFactory::new(config);

        assert!(matches!(
            factory.create_client(Duration::from_secs(1)),
            Err(LlmPortError::CredentialsUnavailable { provider_id }) if provider_id == "openai"
        ));
    }

    #[test]
    fn unknown_and_invalid_provider_errors_are_sanitized() {
        let port = LlmPort::new();
        assert!(matches!(
            port.resolve_model_listing("missing"),
            Err(LlmPortError::UnknownProvider { provider_id }) if provider_id == "missing"
        ));

        let missing_url = LoadedProviderConfig::new(
            "compatible",
            ProviderKind::OpenAiCompatible,
            None,
            None,
            None,
            true,
        );
        let factory = OpenAiCompatibleProviderFactory::new(missing_url);
        assert!(matches!(
            factory.create_client(Duration::from_secs(1)),
            Err(LlmPortError::ProviderConfigurationInvalid { provider_id })
                if provider_id == "compatible"
        ));

        let secret_url = "not a valid URL secret-url-sentinel";
        let invalid_url = LoadedProviderConfig::new(
            "invalid",
            ProviderKind::OpenAiCompatible,
            Some(secret_url.to_string()),
            None,
            None,
            true,
        );
        let error = match OpenAiCompatibleProviderFactory::new(invalid_url)
            .create_client(Duration::from_secs(1))
        {
            Ok(_) => panic!("invalid URL should fail before client publication"),
            Err(error) => error,
        };
        let diagnostic = format!("{error:?}; {error}");
        assert!(!diagnostic.contains(secret_url));
    }

    #[test]
    fn prompt_cache_policy_classifies_remote_and_local_endpoints() {
        assert_eq!(
            prompt_cache_policy(ProviderKind::OpenAi, None),
            ProviderPromptCachePolicy::SessionAffinity
        );
        assert_eq!(
            prompt_cache_policy(
                ProviderKind::OpenAiCompatible,
                Some("https://api.openai.com/v1")
            ),
            ProviderPromptCachePolicy::SessionAffinity
        );
        assert_eq!(
            prompt_cache_policy(
                ProviderKind::OpenAiCompatible,
                Some("https://provider.example/v1")
            ),
            ProviderPromptCachePolicy::SessionAffinityLong24h
        );
        for local_endpoint in [
            "http://localhost:11434/v1",
            "https://host.local/v1",
            "http://127.0.0.1:11434/v1",
            "http://10.0.0.8/v1",
            "http://[::1]:11434/v1",
            "http://[fd00::1]/v1",
        ] {
            assert_eq!(
                prompt_cache_policy(ProviderKind::OpenAiCompatible, Some(local_endpoint)),
                ProviderPromptCachePolicy::Disabled,
                "local endpoint must not opt into remote cache semantics: {local_endpoint}"
            );
        }
        assert_eq!(
            prompt_cache_policy(
                ProviderKind::OpenAiResponses,
                Some("http://localhost:11434/v1")
            ),
            ProviderPromptCachePolicy::SessionAffinity
        );
        assert_eq!(
            prompt_cache_policy(
                ProviderKind::OpenAiResponses,
                Some("https://provider.example/v1")
            ),
            ProviderPromptCachePolicy::SessionAffinityLong24h
        );
    }

    #[test]
    fn redacted_debug_and_errors_do_not_expose_control_authority() {
        let url_sentinel = "https://secret-provider.invalid/v1";
        let key_sentinel = "secret-api-key-sentinel";
        let env_sentinel = "SECRET_ENV_SENTINEL";
        let owner_sentinel = "secret-owner-sentinel";
        let config = LoadedProviderConfig::new(
            "provider",
            ProviderKind::OpenAiCompatible,
            Some(url_sentinel.to_string()),
            Some(ProviderApiKey::new(key_sentinel)),
            Some(env_sentinel.to_string()),
            true,
        );
        let factory = OpenAiCompatibleProviderFactory::new(config);
        let factory_debug = format!("{factory:?}");
        let factory = Arc::new(factory);
        let port = LlmPort::new();
        let registration = port
            .register(owner_sentinel, "provider", factory)
            .expect("provider should register");

        let text = format!(
            "factory={factory_debug}; port={port:?}; registration={registration:?}; snapshot={:?}; error={:?}",
            port.inspection_snapshot(),
            LlmPortError::ProviderConfigurationInvalid {
                provider_id: "provider".to_string()
            }
        );

        for sentinel in [url_sentinel, key_sentinel, env_sentinel, owner_sentinel] {
            assert!(!text.contains(sentinel), "debug leaked {sentinel}");
        }
    }
}
