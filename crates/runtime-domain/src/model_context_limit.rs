use std::collections::BTreeMap;

use crate::model_catalog::{ModelCatalog, ModelSelection};

/// `ModelContextLimits` 保存从 `models.toml` 合并后的 context limit 配置。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelContextLimits {
    defaults: Option<u32>,
    by_provider_model: BTreeMap<(String, String), u32>,
}

impl ModelContextLimits {
    /// `new` 创建空的 limit 配置。
    pub fn new(defaults: Option<u32>, by_provider_model: BTreeMap<(String, String), u32>) -> Self {
        Self {
            defaults,
            by_provider_model,
        }
    }

    /// `resolve` 按 profile → defaults → built-in → None 解析 context limit。
    pub fn resolve(&self, _catalog: &ModelCatalog, selection: &ModelSelection) -> Option<u32> {
        let key = (selection.provider_id.clone(), selection.model_id.clone());
        if let Some(limit) = self.by_provider_model.get(&key) {
            return Some(*limit);
        }

        if let Some(limit) = self.model_id_only_profile_limit(selection.model_id.as_str()) {
            return Some(limit);
        }

        if let Some(limit) = self.defaults {
            return Some(limit);
        }

        built_in_context_limit(selection.model_id.as_str())
    }

    fn model_id_only_profile_limit(&self, model_id: &str) -> Option<u32> {
        let mut matches = self
            .by_provider_model
            .iter()
            .filter(|((_, id), _)| id == model_id)
            .map(|(_, limit)| *limit);
        let first = matches.next()?;
        if matches.next().is_some() {
            return None;
        }
        Some(first)
    }
}

fn built_in_context_limit(model_id: &str) -> Option<u32> {
    let lower = model_id.to_ascii_lowercase();
    if lower.contains("gpt-4o") && !lower.contains("mini") {
        return Some(128_000);
    }
    if lower.contains("gpt-4o-mini") {
        return Some(128_000);
    }
    if lower.contains("claude-sonnet-4") || lower.contains("claude-opus-4") {
        return Some(200_000);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_catalog::{ModelCatalog, ModelEntry, ModelProvider, ModelSource};
    use crate::provider::ProviderKind;

    fn catalog_with_local_qwen() -> ModelCatalog {
        ModelCatalog::new(vec![ModelProvider::new(
            "local",
            ProviderKind::OpenAiCompatible,
            "Local",
            Some("http://127.0.0.1:1234/v1".to_string()),
            ModelSource::Configured,
            vec![ModelEntry::new("qwen3", None, ModelSource::Configured)],
        )])
    }

    #[test]
    fn resolve_uses_provider_model_profile_first() {
        let mut profiles = BTreeMap::new();
        profiles.insert(("local".to_string(), "qwen3".to_string()), 32_768);
        let limits = ModelContextLimits::new(Some(128_000), profiles);
        let selection = ModelSelection::new("local", "qwen3");

        assert_eq!(
            limits.resolve(&catalog_with_local_qwen(), &selection),
            Some(32_768)
        );
    }

    #[test]
    fn resolve_falls_back_to_defaults() {
        let limits = ModelContextLimits::new(Some(64_000), BTreeMap::new());
        let selection = ModelSelection::new("local", "unknown-model");

        assert_eq!(
            limits.resolve(&catalog_with_local_qwen(), &selection),
            Some(64_000)
        );
    }

    #[test]
    fn resolve_uses_builtin_when_no_config() {
        let limits = ModelContextLimits::default();
        let selection = ModelSelection::new("openai", "gpt-4o");

        assert_eq!(
            limits.resolve(&ModelCatalog::default(), &selection),
            Some(128_000)
        );
    }

    #[test]
    fn resolve_returns_none_for_unknown_model() {
        let limits = ModelContextLimits::default();
        let selection = ModelSelection::new("local", "totally-custom");

        assert_eq!(limits.resolve(&catalog_with_local_qwen(), &selection), None);
    }
}
