use std::{fmt, io};

/// `OpenAiCompatibleError` 描述 OpenAI-compatible chat completions 调用失败。
#[derive(Debug)]
pub enum OpenAiCompatibleError {
    BuildClient(reqwest::Error),
    Request {
        endpoint: String,
    },
    Http {
        endpoint: String,
        status: reqwest::StatusCode,
    },
    ReadStream(io::Error),
    InvalidStreamEvent,
}

impl fmt::Display for OpenAiCompatibleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BuildClient(source) => write!(f, "create HTTP client: {source}"),
            Self::Request { endpoint } => write!(f, "cannot reach {endpoint}"),
            Self::Http { endpoint, status } => write!(f, "HTTP {status} from {endpoint}"),
            Self::ReadStream(_) => write!(f, "read chat completion stream"),
            Self::InvalidStreamEvent => write!(f, "invalid chat completion stream event"),
        }
    }
}

impl std::error::Error for OpenAiCompatibleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::BuildClient(source) => Some(source),
            Self::ReadStream(source) => Some(source),
            Self::Request { .. } | Self::Http { .. } | Self::InvalidStreamEvent => None,
        }
    }
}
