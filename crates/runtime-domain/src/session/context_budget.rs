use crate::context_budget::SegmentKind;
use crate::provider::ProviderKind;

/// Context budget snapshot payload for the `/context` overlay.
#[derive(Debug, Clone, PartialEq)]
pub struct ContextBudgetSnapshotPayload {
    pub model_id: String,
    pub segments: Vec<ContextBudgetSegmentPayload>,
    pub total_estimated_tokens: usize,
    pub context_limit: Option<u32>,
    pub display: ContextBudgetDisplayPayload,
}

/// One segment in a context budget snapshot event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextBudgetSegmentPayload {
    pub kind: SegmentKind,
    pub stack_order: usize,
    pub estimated_tokens: usize,
}

/// Display mode for context budget header and legend.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ContextBudgetDisplayPayload {
    Relative { used: u32 },
    Absolute { limit: u32, used: u32, percent: f32 },
}

/// Stable error category for context budget projection failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextBudgetProjectionErrorKind {
    Internal,
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
    ProjectionFailed {
        kind: ContextBudgetProjectionErrorKind,
        status: Option<u16>,
        detail: Option<String>,
    },
}
