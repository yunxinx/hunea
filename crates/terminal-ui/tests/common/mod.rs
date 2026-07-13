//! terminal-ui 集成测试共享的构造 helper。

use runtime_domain::{
    model_catalog::{ModelCatalog, ModelEntry, ModelProvider, ModelSource},
    provider::ProviderKind,
};

/// 单 `local` provider、单 `qwen3` 模型的 catalog；
/// 与 `ModelSelection::new("local", "qwen3")` 配对即可构造可发送状态。
pub fn single_model_catalog() -> ModelCatalog {
    ModelCatalog::new(vec![ModelProvider::new(
        "local",
        ProviderKind::OpenAiCompatible,
        "Local",
        Some("http://127.0.0.1:1234/v1".to_string()),
        ModelSource::Configured,
        vec![ModelEntry::new("qwen3", None, ModelSource::Configured)],
    )])
}
