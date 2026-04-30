use crate::runtime::provider::{ProviderApiKey, ProviderKind};

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
    pub runtime: ModelProviderRuntime,
    pub source: ModelSource,
    pub models: Vec<ModelEntry>,
    pub enabled: bool,
    pub sync_error: Option<String>,
}

/// `ModelProviderRuntime` 描述模型 provider 背后的 runtime 类型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelProviderRuntime {
    Native(NativeModelProviderRuntime),
    Acp,
}

/// `NativeModelProviderRuntime` 保存 native provider 发起请求所需的连接配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeModelProviderRuntime {
    pub kind: ProviderKind,
    pub base_url: Option<String>,
    pub api_key: Option<ProviderApiKey>,
    pub api_key_env: Option<String>,
}

impl ModelProvider {
    /// `native` 创建启用状态的 native provider。
    pub fn native(
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
            runtime: ModelProviderRuntime::Native(NativeModelProviderRuntime {
                kind,
                base_url,
                api_key: None,
                api_key_env: None,
            }),
            source,
            models,
            enabled: true,
            sync_error: None,
        }
    }

    /// `acp` 创建由 ACP agent 管理的 provider。
    pub fn acp(
        id: impl Into<String>,
        display_name: impl Into<String>,
        models: Vec<ModelEntry>,
    ) -> Self {
        Self {
            id: id.into(),
            display_name: display_name.into(),
            runtime: ModelProviderRuntime::Acp,
            source: ModelSource::Acp,
            models,
            enabled: true,
            sync_error: None,
        }
    }

    /// `native_runtime` 返回 native provider 的连接配置。
    pub fn native_runtime(&self) -> Option<&NativeModelProviderRuntime> {
        match &self.runtime {
            ModelProviderRuntime::Native(runtime) => Some(runtime),
            ModelProviderRuntime::Acp => None,
        }
    }

    /// `native_runtime_mut` 返回 native provider 的可变连接配置。
    pub fn native_runtime_mut(&mut self) -> Option<&mut NativeModelProviderRuntime> {
        match &mut self.runtime {
            ModelProviderRuntime::Native(runtime) => Some(runtime),
            ModelProviderRuntime::Acp => None,
        }
    }

    /// `with_sync_error` 附加模型同步失败原因。
    pub fn with_sync_error(mut self, error: impl Into<String>) -> Self {
        self.sync_error = Some(error.into());
        self
    }

    /// `with_api_key_env` 附加用于读取 Bearer token 的环境变量名。
    pub fn with_api_key_env(mut self, api_key_env: Option<String>) -> Self {
        if let Some(runtime) = self.native_runtime_mut() {
            runtime.api_key_env = api_key_env;
        }
        self
    }

    /// `with_api_key` 附加配置文件中直接提供的 Bearer token。
    pub fn with_api_key(mut self, api_key: Option<ProviderApiKey>) -> Self {
        if let Some(runtime) = self.native_runtime_mut() {
            runtime.api_key = api_key;
        }
        self
    }

    /// `disabled_native` 创建禁用的 native provider，保留配置但不参与展示。
    pub fn disabled_native(
        id: impl Into<String>,
        kind: ProviderKind,
        display_name: impl Into<String>,
        base_url: Option<String>,
        source: ModelSource,
        models: Vec<ModelEntry>,
    ) -> Self {
        Self {
            enabled: false,
            ..Self::native(id, kind, display_name, base_url, source, models)
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
    Acp,
}

impl ModelSource {
    /// `label` 返回适合 TUI 展示的来源说明。
    pub fn label(self) -> &'static str {
        match self {
            Self::Configured => "configured",
            Self::Synced => "synced from /v1/models",
            Self::Acp => "provided by ACP agent",
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
            ModelProvider::disabled_native(
                "disabled",
                ProviderKind::OpenAiCompatible,
                "Disabled",
                None,
                ModelSource::Configured,
                vec![ModelEntry::new("hidden", None, ModelSource::Configured)],
            ),
            ModelProvider::native(
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
}
