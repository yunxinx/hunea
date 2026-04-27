use std::fmt;

/// `LlmError` 描述原生 LLM backend 调用失败。
#[derive(Debug)]
pub enum LlmError {
    MissingBaseUrl {
        provider_id: String,
    },
    InvalidBaseUrl {
        provider_id: String,
        base_url: String,
    },
    GenAi(genai::Error),
    Cancelled,
}

impl fmt::Display for LlmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingBaseUrl { provider_id } => {
                write!(f, "provider {provider_id} requires base_url")
            }
            Self::InvalidBaseUrl {
                provider_id,
                base_url,
            } => write!(
                f,
                "provider {provider_id} has invalid base_url {base_url:?}"
            ),
            Self::GenAi(source) => write!(f, "{source}"),
            Self::Cancelled => write!(f, "chat cancelled"),
        }
    }
}

impl std::error::Error for LlmError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::GenAi(source) => Some(source),
            Self::MissingBaseUrl { .. } | Self::InvalidBaseUrl { .. } | Self::Cancelled => None,
        }
    }
}

impl From<genai::Error> for LlmError {
    fn from(source: genai::Error) -> Self {
        Self::GenAi(source)
    }
}
