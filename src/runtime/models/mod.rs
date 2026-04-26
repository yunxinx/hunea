mod config;

pub use config::{
    LoadedModelCatalog, ModelsConfigError, ProviderSyncRequest, load, load_from_paths,
    load_from_paths_with_sync, write_default_model,
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

    /// `enabled_provider_index_for` 返回指定 provider 在展示列表中的索引。
    pub fn enabled_provider_index_for(&self, provider_id: &str) -> Option<usize> {
        self.enabled_providers()
            .position(|provider| provider.id == provider_id)
    }
}

/// `ModelProvider` 描述一个模型供应商。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelProvider {
    pub id: String,
    pub display_name: String,
    pub base_url: Option<String>,
    pub api_key_env: Option<String>,
    pub source: ModelSource,
    pub models: Vec<ModelEntry>,
    pub enabled: bool,
    pub sync_error: Option<String>,
}

impl ModelProvider {
    /// `new` 创建启用状态的 provider。
    pub fn new(
        id: impl Into<String>,
        display_name: impl Into<String>,
        base_url: Option<String>,
        source: ModelSource,
        models: Vec<ModelEntry>,
    ) -> Self {
        Self {
            id: id.into(),
            display_name: display_name.into(),
            base_url,
            api_key_env: None,
            source,
            models,
            enabled: true,
            sync_error: None,
        }
    }

    /// `with_sync_error` 附加模型同步失败原因。
    pub fn with_sync_error(mut self, error: impl Into<String>) -> Self {
        self.sync_error = Some(error.into());
        self
    }

    /// `with_api_key_env` 附加用于读取 Bearer token 的环境变量名。
    pub fn with_api_key_env(mut self, api_key_env: Option<String>) -> Self {
        self.api_key_env = api_key_env;
        self
    }

    /// `disabled` 创建禁用 provider，保留配置但不参与展示。
    pub fn disabled(
        id: impl Into<String>,
        display_name: impl Into<String>,
        base_url: Option<String>,
        source: ModelSource,
        models: Vec<ModelEntry>,
    ) -> Self {
        Self {
            enabled: false,
            ..Self::new(id, display_name, base_url, source, models)
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
    Synced,
}

impl ModelSource {
    /// `label` 返回适合 TUI 展示的来源说明。
    pub fn label(self) -> &'static str {
        match self {
            Self::Configured => "configured",
            Self::Synced => "synced from /v1/models",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ModelCatalog, ModelEntry, ModelProvider, ModelSelection, ModelSource};

    #[test]
    fn catalog_filters_disabled_providers() {
        let catalog = ModelCatalog::new(vec![
            ModelProvider::disabled(
                "disabled",
                "Disabled",
                None,
                ModelSource::Configured,
                vec![ModelEntry::new("hidden", None, ModelSource::Configured)],
            ),
            ModelProvider::new(
                "enabled",
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
}
