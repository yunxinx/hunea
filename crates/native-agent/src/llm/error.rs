use std::fmt;

/// `NativeLlmError` 描述原生 LLM backend 调用失败。
#[derive(Debug)]
pub enum NativeLlmError {
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

impl fmt::Display for NativeLlmError {
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
            Self::Cancelled => write!(f, "native LLM request cancelled"),
        }
    }
}

impl std::error::Error for NativeLlmError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::GenAi(source) => Some(source),
            Self::MissingBaseUrl { .. } | Self::InvalidBaseUrl { .. } | Self::Cancelled => None,
        }
    }
}

impl From<genai::Error> for NativeLlmError {
    fn from(source: genai::Error) -> Self {
        Self::GenAi(source)
    }
}
