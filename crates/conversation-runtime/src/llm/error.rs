use std::fmt;

use extension_hook_runtime::HookDispatchError;
use provider_protocol::ProviderError;

/// `ProviderRequestError` 描述 provider 请求失败。
pub enum ProviderRequestError {
    EmptyPrompt { provider_id: String },
    Provider { source: ProviderError },
    ExtensionHook { source: HookDispatchError },
    ToolTurnLimit { max_turns: usize },
    Cancelled,
}

impl fmt::Debug for ProviderRequestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyPrompt { provider_id } => formatter
                .debug_struct("EmptyPrompt")
                .field("provider_id", provider_id)
                .finish(),
            Self::Provider { .. } => formatter.write_str("Provider(REDACTED)"),
            Self::ExtensionHook { source } => formatter
                .debug_tuple("ExtensionHook")
                .field(source)
                .finish(),
            Self::ToolTurnLimit { max_turns } => formatter
                .debug_struct("ToolTurnLimit")
                .field("max_turns", max_turns)
                .finish(),
            Self::Cancelled => formatter.write_str("Cancelled"),
        }
    }
}

impl fmt::Display for ProviderRequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyPrompt { provider_id } => {
                write!(f, "provider {provider_id} received no prompt items")
            }
            Self::Provider { .. } => write!(f, "provider request failed"),
            Self::ExtensionHook { source } => write!(f, "{source}"),
            Self::ToolTurnLimit { max_turns } => {
                write!(f, "tool turn limit reached ({max_turns})")
            }
            Self::Cancelled => write!(f, "provider request cancelled"),
        }
    }
}

impl std::error::Error for ProviderRequestError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Provider { source } => Some(source),
            Self::ExtensionHook { source } => Some(source),
            Self::EmptyPrompt { .. } | Self::ToolTurnLimit { .. } | Self::Cancelled => None,
        }
    }
}

impl From<ProviderError> for ProviderRequestError {
    fn from(source: ProviderError) -> Self {
        Self::Provider { source }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use super::*;

    #[test]
    fn provider_error_keeps_structured_source_but_redacts_user_facing_output() {
        let sentinel = "https://credential.example/private instruction sentinel";
        let error = ProviderRequestError::from(ProviderError::Transport(sentinel.to_string()));

        assert_eq!(error.to_string(), "provider request failed");
        assert!(!format!("{error:?}").contains(sentinel));
        assert!(matches!(
            error.source().and_then(|source| source.downcast_ref::<ProviderError>()),
            Some(ProviderError::Transport(message)) if message == sentinel
        ));
    }
}
