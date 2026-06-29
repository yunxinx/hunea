use crate::context_budget::{
    ContextBudgetSnapshot, ContextSegment, ContextTokenLimit, ContextWindowUsage, SegmentKind,
};
use crate::provider::ProviderKind;

/// Context budget snapshot payload for the `/context` overlay.
#[derive(Debug, Clone, PartialEq)]
pub struct ContextBudgetSnapshotPayload {
    pub model_id: String,
    pub segments: Vec<ContextBudgetSegmentPayload>,
    pub total_estimated_tokens: usize,
    pub usage: ContextWindowUsagePayload,
}

/// One segment in a context budget snapshot event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextBudgetSegmentPayload {
    pub kind: SegmentKind,
    pub stack_order: usize,
    pub estimated_tokens: usize,
}

/// Absolute usage summary for context budget header and legend.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContextWindowUsagePayload {
    pub limit: ContextTokenLimit,
    pub used: u32,
    pub percent: f32,
}

impl From<ContextBudgetSnapshot> for ContextBudgetSnapshotPayload {
    fn from(snapshot: ContextBudgetSnapshot) -> Self {
        Self {
            model_id: snapshot.model_id,
            segments: snapshot.segments.into_iter().map(Into::into).collect(),
            total_estimated_tokens: snapshot.total_estimated_tokens,
            usage: snapshot.usage.into(),
        }
    }
}

impl From<ContextSegment> for ContextBudgetSegmentPayload {
    fn from(segment: ContextSegment) -> Self {
        Self {
            kind: segment.kind,
            stack_order: segment.stack_order,
            estimated_tokens: segment.estimated_tokens,
        }
    }
}

impl From<ContextWindowUsage> for ContextWindowUsagePayload {
    fn from(usage: ContextWindowUsage) -> Self {
        Self {
            limit: usage.limit,
            used: usage.used,
            percent: usage.percent,
        }
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context_budget::{
        ContextBudgetSnapshot, ContextSegment, ContextTokenLimit, ContextWindowUsage,
    };

    fn limit(value: u32) -> ContextTokenLimit {
        ContextTokenLimit::try_from(value).expect("fixture limit should be valid")
    }

    #[test]
    fn snapshot_payload_conversion_preserves_all_snapshot_fields() {
        let payload: ContextBudgetSnapshotPayload = ContextBudgetSnapshot {
            model_id: "qwen3".to_string(),
            segments: vec![
                ContextSegment {
                    kind: SegmentKind::System,
                    stack_order: 0,
                    estimated_tokens: 128,
                },
                ContextSegment {
                    kind: SegmentKind::ToolDefinitions,
                    stack_order: 1,
                    estimated_tokens: 64,
                },
            ],
            total_estimated_tokens: 192,
            usage: ContextWindowUsage {
                limit: limit(256_000),
                used: 192,
                percent: 0.075,
            },
        }
        .into();

        assert_eq!(payload.model_id, "qwen3");
        assert_eq!(payload.total_estimated_tokens, 192);
        assert_eq!(
            payload.segments,
            vec![
                ContextBudgetSegmentPayload {
                    kind: SegmentKind::System,
                    stack_order: 0,
                    estimated_tokens: 128,
                },
                ContextBudgetSegmentPayload {
                    kind: SegmentKind::ToolDefinitions,
                    stack_order: 1,
                    estimated_tokens: 64,
                },
            ]
        );
        assert_eq!(
            payload.usage,
            ContextWindowUsagePayload {
                limit: limit(256_000),
                used: 192,
                percent: 0.075,
            }
        );
    }

    #[test]
    fn usage_payload_conversion_keeps_non_zero_limit_type() {
        let payload: ContextWindowUsagePayload = ContextWindowUsage {
            limit: limit(128_000),
            used: 42_000,
            percent: 32.8125,
        }
        .into();

        assert_eq!(
            payload,
            ContextWindowUsagePayload {
                limit: limit(128_000),
                used: 42_000,
                percent: 32.8125,
            }
        );
    }
}
