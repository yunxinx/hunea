use genai::adapter::AdapterKind;

use mo_core::provider::ProviderKind;

pub(crate) trait ProviderKindGenAiExt {
    fn adapter_kind(self) -> AdapterKind;
}

impl ProviderKindGenAiExt for ProviderKind {
    fn adapter_kind(self) -> AdapterKind {
        match self {
            Self::OpenAiCompatible | Self::OpenAi => AdapterKind::OpenAI,
            Self::Anthropic => AdapterKind::Anthropic,
            Self::Gemini => AdapterKind::Gemini,
            Self::DeepSeek => AdapterKind::DeepSeek,
            Self::Together => AdapterKind::Together,
            Self::Groq => AdapterKind::Groq,
            Self::Fireworks => AdapterKind::Fireworks,
            Self::Xai => AdapterKind::Xai,
            Self::Ollama => AdapterKind::Ollama,
            Self::OllamaCloud => AdapterKind::OllamaCloud,
            Self::Cohere => AdapterKind::Cohere,
            Self::Zai => AdapterKind::Zai,
            Self::BigModel => AdapterKind::BigModel,
            Self::Aliyun => AdapterKind::Aliyun,
            Self::Mimo => AdapterKind::Mimo,
            Self::Nebius => AdapterKind::Nebius,
            Self::Vertex => AdapterKind::Vertex,
            Self::GithubCopilot => AdapterKind::GithubCopilot,
        }
    }
}
