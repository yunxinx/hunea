use crate::{
    model_context_limit::ModelContextLimits,
    provider::{ProviderApiKey, ProviderKind},
};

/// `ModelCatalog` 保存 TUI 可展示与可选择的模型目录。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelCatalog {
    providers: Vec<ModelProvider>,
}

impl ModelCatalog {
    /// `new` 创建模型目录。
    pub fn new(providers: Vec<ModelProvider>) -> Self {
        Self { providers }
    }

    /// `enabled_providers` 返回当前允许展示的 provider。
    pub fn enabled_providers(&self) -> impl Iterator<Item = &ModelProvider> {
        self.providers.iter().filter(|provider| provider.enabled)
    }

    /// `enabled_provider_count` 返回可展示 provider 数量。
    pub fn enabled_provider_count(&self) -> usize {
        self.enabled_providers().count()
    }

    /// `enabled_provider_at` 返回指定展示索引处的 provider。
    pub fn enabled_provider_at(&self, index: usize) -> Option<&ModelProvider> {
        self.enabled_providers().nth(index)
    }

    /// `enabled_provider_by_id` 返回指定 id 的启用 provider。
    pub fn enabled_provider_by_id(&self, provider_id: &str) -> Option<&ModelProvider> {
        self.enabled_providers()
            .find(|provider| provider.id == provider_id)
    }

    /// `provider_by_id_mut` 返回指定 id 的 provider 可变引用。
    pub fn provider_by_id_mut(&mut self, provider_id: &str) -> Option<&mut ModelProvider> {
        self.providers
            .iter_mut()
            .find(|provider| provider.id == provider_id)
    }

    /// `contains_selection` 判断目录是否包含指定模型。
    pub fn contains_selection(&self, selection: &ModelSelection) -> bool {
        self.enabled_providers().any(|provider| {
            provider.id == selection.provider_id
                && provider
                    .models
                    .iter()
                    .any(|model| model.id == selection.model_id)
        })
    }

    /// `accepts_selection` 判断目录是否可以使用指定模型选择。
    pub fn accepts_selection(&self, selection: &ModelSelection) -> bool {
        let Some(provider) = self.enabled_provider_by_id(&selection.provider_id) else {
            return false;
        };
        if provider.models.is_empty() && provider.source == ModelSource::NotLoaded {
            return true;
        }
        provider
            .models
            .iter()
            .any(|model| model.id == selection.model_id)
    }

    /// `selection_for_model_id` 从当前目录中按 model id 解析可用模型选择。
    ///
    /// 历史 session 只持久化 model id；如果多个 provider 暴露同名 model，恢复时不猜测 provider。
    pub fn selection_for_model_id(&self, model_id: &str) -> Option<ModelSelection> {
        let mut matches = self.enabled_providers().filter(|provider| {
            provider.models.is_empty() && provider.source == ModelSource::NotLoaded
                || provider.models.iter().any(|model| model.id == model_id)
        });
        let provider = matches.next()?;
        if matches.next().is_some() {
            return None;
        }
        Some(ModelSelection::new(
            provider.id.clone(),
            model_id.to_string(),
        ))
    }

    /// `enabled_provider_index_for` 返回指定 provider 在展示列表中的索引。
    pub fn enabled_provider_index_for(&self, provider_id: &str) -> Option<usize> {
        self.enabled_providers()
            .position(|provider| provider.id == provider_id)
    }

    /// `context_limit_for` 解析当前选择的 context limit（tokens）。
    pub fn context_limit_for(
        &self,
        limits: &ModelContextLimits,
        selection: &ModelSelection,
    ) -> Option<u32> {
        limits.resolve(self, selection)
    }
}

/// `ModelProvider` 描述一个模型供应商。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelProvider {
    pub id: String,
    pub display_name: String,
    pub connection: ProviderConnection,
    pub source: ModelSource,
    pub models: Vec<ModelEntry>,
    pub enabled: bool,
    pub sync_error: Option<String>,
}

/// `ProviderConnection` 保存 provider 发起请求所需的连接配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderConnection {
    pub kind: ProviderKind,
    pub base_url: Option<String>,
    pub api_key: Option<ProviderApiKey>,
    pub api_key_env: Option<String>,
}

/// `ProviderSyncRequest` 描述一次 provider 模型列表同步请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSyncRequest {
    pub provider_id: String,
    pub kind: ProviderKind,
    pub display_name: String,
    pub base_url: Option<String>,
    pub api_key: Option<ProviderApiKey>,
    pub api_key_env: Option<String>,
}

/// `ModelProviderRefreshEvent` 是 provider 模型列表刷新后的消费层事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelProviderRefreshEvent {
    Finished {
        provider_id: String,
        model_ids: Vec<String>,
    },
    Failed {
        provider_id: String,
        message: String,
    },
}

impl ModelProvider {
    /// `new` 创建启用状态的 provider。
    pub fn new(
        id: impl Into<String>,
        kind: ProviderKind,
        display_name: impl Into<String>,
        base_url: Option<String>,
        source: ModelSource,
        models: Vec<ModelEntry>,
    ) -> Self {
        Self {
            id: id.into(),
            display_name: display_name.into(),
            connection: ProviderConnection {
                kind,
                base_url,
                api_key: None,
                api_key_env: None,
            },
            source,
            models,
            enabled: true,
            sync_error: None,
        }
    }

    /// `connection` 返回 provider 的连接配置。
    pub const fn connection(&self) -> &ProviderConnection {
        &self.connection
    }

    /// `connection_mut` 返回 provider 的可变连接配置。
    pub fn connection_mut(&mut self) -> &mut ProviderConnection {
        &mut self.connection
    }

    /// `with_sync_error` 附加模型同步失败原因。
    pub fn with_sync_error(mut self, error: impl Into<String>) -> Self {
        self.sync_error = Some(error.into());
        self
    }

    /// `with_api_key_env` 附加用于读取 Bearer token 的环境变量名。
    pub fn with_api_key_env(mut self, api_key_env: Option<String>) -> Self {
        self.connection_mut().api_key_env = api_key_env;
        self
    }

    /// `with_api_key` 附加配置文件中直接提供的 Bearer token。
    pub fn with_api_key(mut self, api_key: Option<ProviderApiKey>) -> Self {
        self.connection_mut().api_key = api_key;
        self
    }

    /// `disabled` 创建禁用的 provider，保留配置但不参与展示。
    pub fn disabled(
        id: impl Into<String>,
        kind: ProviderKind,
        display_name: impl Into<String>,
        base_url: Option<String>,
        source: ModelSource,
        models: Vec<ModelEntry>,
    ) -> Self {
        Self {
            enabled: false,
            ..Self::new(id, kind, display_name, base_url, source, models)
        }
    }
}

/// `ModelEntry` 描述一个可选择模型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelEntry {
    pub id: String,
    pub description: Option<String>,
    pub source: ModelSource,
}

impl ModelEntry {
    /// `new` 创建模型条目。
    pub fn new(id: impl Into<String>, description: Option<String>, source: ModelSource) -> Self {
        Self {
            id: id.into(),
            description,
            source,
        }
    }
}

/// `ModelSelection` 标识当前选中的 provider/model。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSelection {
    pub provider_id: String,
    pub model_id: String,
}

impl ModelSelection {
    /// `new` 创建模型选择。
    pub fn new(provider_id: impl Into<String>, model_id: impl Into<String>) -> Self {
        Self {
            provider_id: provider_id.into(),
            model_id: model_id.into(),
        }
    }

    /// `display_name` 返回 `provider/model` 形式的展示文本。
    pub fn display_name(&self) -> String {
        format!("{}/{}", self.provider_id, self.model_id)
    }
}

/// `ModelSource` 描述模型列表来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelSource {
    Configured,
    NotLoaded,
    Synced,
}

impl ModelSource {
    /// `label` 返回适合 TUI 展示的来源说明。
    pub fn label(self) -> &'static str {
        match self {
            Self::Configured => "configured",
            Self::NotLoaded => "not loaded",
            Self::Synced => "synced from /v1/models",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ModelCatalog, ModelEntry, ModelProvider, ModelSelection, ModelSource, ProviderKind,
    };

    #[test]
    fn catalog_filters_disabled_providers() {
        let catalog = ModelCatalog::new(vec![
            ModelProvider::disabled(
                "disabled",
                ProviderKind::OpenAiCompatible,
                "Disabled",
                None,
                ModelSource::Configured,
                vec![ModelEntry::new("hidden", None, ModelSource::Configured)],
            ),
            ModelProvider::new(
                "enabled",
                ProviderKind::OpenAiCompatible,
                "Enabled",
                None,
                ModelSource::Configured,
                vec![ModelEntry::new("visible", None, ModelSource::Configured)],
            ),
        ]);

        let providers = catalog
            .enabled_providers()
            .map(|provider| provider.id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(providers, vec!["enabled"]);
        assert!(catalog.contains_selection(&ModelSelection::new("enabled", "visible")));
        assert!(!catalog.contains_selection(&ModelSelection::new("disabled", "hidden")));
    }

    #[test]
    fn catalog_accepts_selection_when_models_are_not_loaded() {
        let catalog = ModelCatalog::new(vec![ModelProvider::new(
            "local",
            ProviderKind::OpenAiCompatible,
            "Local",
            None,
            ModelSource::NotLoaded,
            Vec::new(),
        )]);

        assert!(catalog.accepts_selection(&ModelSelection::new("local", "qwen3")));
        assert!(!catalog.contains_selection(&ModelSelection::new("local", "qwen3")));
    }

    #[test]
    fn catalog_rejects_selection_outside_configured_allowlist() {
        let catalog = ModelCatalog::new(vec![ModelProvider::new(
            "local",
            ProviderKind::OpenAiCompatible,
            "Local",
            None,
            ModelSource::Configured,
            vec![ModelEntry::new("qwen3", None, ModelSource::Configured)],
        )]);

        assert!(catalog.accepts_selection(&ModelSelection::new("local", "qwen3")));
        assert!(!catalog.accepts_selection(&ModelSelection::new("local", "qwen4")));
    }

    #[test]
    fn catalog_resolves_unique_model_id_to_selection() {
        let catalog = ModelCatalog::new(vec![ModelProvider::new(
            "local",
            ProviderKind::OpenAiCompatible,
            "Local",
            None,
            ModelSource::Configured,
            vec![ModelEntry::new("qwen3", None, ModelSource::Configured)],
        )]);

        assert_eq!(
            catalog.selection_for_model_id("qwen3"),
            Some(ModelSelection::new("local", "qwen3"))
        );
        assert_eq!(catalog.selection_for_model_id("missing"), None);
    }

    #[test]
    fn catalog_does_not_guess_ambiguous_model_id() {
        let catalog = ModelCatalog::new(vec![
            ModelProvider::new(
                "first",
                ProviderKind::OpenAiCompatible,
                "First",
                None,
                ModelSource::Configured,
                vec![ModelEntry::new("shared", None, ModelSource::Configured)],
            ),
            ModelProvider::new(
                "second",
                ProviderKind::OpenAiCompatible,
                "Second",
                None,
                ModelSource::Configured,
                vec![ModelEntry::new("shared", None, ModelSource::Configured)],
            ),
        ]);

        assert_eq!(catalog.selection_for_model_id("shared"), None);
    }
}
