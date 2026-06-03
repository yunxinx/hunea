use std::fmt;

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

    /// `uses_openai_compatible_endpoint` 判断 provider 是否使用 OpenAI-compatible base URL。
    pub fn uses_openai_compatible_endpoint(self) -> bool {
        matches!(self, Self::OpenAiCompatible)
    }
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_config_value())
    }
}

/// `ProviderApiKey` 保存配置文件中直接写入的 provider API key。
#[derive(Clone, PartialEq, Eq)]
pub struct ProviderApiKey(String);

impl ProviderApiKey {
    /// `new` 创建一个直接可用的 API key。
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// `from_optional_config` 规范化配置文件中可选的 provider API key。
    pub fn from_optional_config(value: Option<String>) -> Option<Self> {
        value
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .map(Self)
    }

    /// `as_str` 返回 API key 原文。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ProviderApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ProviderApiKey(REDACTED)")
    }
}
