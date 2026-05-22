use thiserror::Error;

/// `ProviderError` describes failures at the provider-client seam.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ProviderError {
    /// The provider endpoint or transport could not be reached.
    #[error("transport error: {0}")]
    Transport(String),
    /// The provider returned data that does not match the expected protocol.
    #[error("protocol error: {0}")]
    Protocol(String),
    /// The provider returned a business/API error response.
    #[error("provider error{status_label}: {message}", status_label = status_label(*status))]
    Provider {
        status: Option<u16>,
        message: String,
    },
}

fn status_label(status: Option<u16>) -> String {
    status
        .map(|status| format!(" HTTP {status}"))
        .unwrap_or_default()
}
