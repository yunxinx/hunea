use std::fmt;

use genai::adapter::AdapterKind;

/// `ProviderKind` 描述 `models.toml` 中 provider 使用的上游协议。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProviderKind {
    #[default]
    OpenAiCompatible,
    OpenAi,
    Anthropic,
    Gemini,
    DeepSeek,
    Together,
    Groq,
    Fireworks,
    Xai,
    Ollama,
    OllamaCloud,
    Cohere,
    Zai,
    BigModel,
    Aliyun,
    Mimo,
    Nebius,
    Vertex,
    GithubCopilot,
}

impl ProviderKind {
    /// `from_config_value` 将 `models.toml` 的 snake_case kind 转为 provider kind。
    pub fn from_config_value(value: &str) -> Option<Self> {
        match value {
            "openai_compatible" => Some(Self::OpenAiCompatible),
            "openai" => Some(Self::OpenAi),
            "anthropic" => Some(Self::Anthropic),
            "gemini" => Some(Self::Gemini),
            "deepseek" => Some(Self::DeepSeek),
            "together" => Some(Self::Together),
            "groq" => Some(Self::Groq),
            "fireworks" => Some(Self::Fireworks),
            "xai" => Some(Self::Xai),
            "ollama" => Some(Self::Ollama),
            "ollama_cloud" => Some(Self::OllamaCloud),
            "cohere" => Some(Self::Cohere),
            "zai" => Some(Self::Zai),
            "bigmodel" => Some(Self::BigModel),
            "aliyun" => Some(Self::Aliyun),
            "mimo" => Some(Self::Mimo),
            "nebius" => Some(Self::Nebius),
            "vertex" => Some(Self::Vertex),
            "github_copilot" => Some(Self::GithubCopilot),
            _ => None,
        }
    }

    /// `as_config_value` 返回配置文件中使用的 snake_case kind。
    pub fn as_config_value(self) -> &'static str {
        match self {
            Self::OpenAiCompatible => "openai_compatible",
            Self::OpenAi => "openai",
            Self::Anthropic => "anthropic",
            Self::Gemini => "gemini",
            Self::DeepSeek => "deepseek",
            Self::Together => "together",
            Self::Groq => "groq",
            Self::Fireworks => "fireworks",
            Self::Xai => "xai",
            Self::Ollama => "ollama",
            Self::OllamaCloud => "ollama_cloud",
            Self::Cohere => "cohere",
            Self::Zai => "zai",
            Self::BigModel => "bigmodel",
            Self::Aliyun => "aliyun",
            Self::Mimo => "mimo",
            Self::Nebius => "nebius",
            Self::Vertex => "vertex",
            Self::GithubCopilot => "github_copilot",
        }
    }

    pub(crate) fn adapter_kind(self) -> AdapterKind {
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

    pub(crate) fn uses_openai_compatible_endpoint(self) -> bool {
        matches!(self, Self::OpenAiCompatible)
    }
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_config_value())
    }
}
