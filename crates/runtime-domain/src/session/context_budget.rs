use crate::provider::ProviderKind;

/// Stable error category for context budget projection failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextBudgetProjectionErrorKind {
    Protocol,
    Transport,
    Provider,
}

/// Structured error payload for `/context` snapshot loading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextBudgetLoadErrorPayload {
    UnknownProvider {
        provider_id: String,
    },
    UnsupportedProvider {
        provider_kind: ProviderKind,
    },
    RuntimeInternal {
        detail: Option<String>,
    },
    ProjectionFailed {
        kind: ContextBudgetProjectionErrorKind,
        status: Option<u16>,
        detail: Option<String>,
    },
}
