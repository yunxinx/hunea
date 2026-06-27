use runtime_domain::session::{
    ContextBudgetDisplayPayload, ContextBudgetSegmentPayload, ContextBudgetSnapshotPayload,
};

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ContextBudgetState {
    pub(crate) loading: bool,
    pub(crate) error: Option<String>,
    pub(crate) snapshot: Option<ContextBudgetSnapshotPayload>,
}

impl Default for ContextBudgetState {
    fn default() -> Self {
        Self {
            loading: true,
            error: None,
            snapshot: None,
        }
    }
}

impl ContextBudgetState {
    pub(crate) fn apply_snapshot(&mut self, payload: ContextBudgetSnapshotPayload) {
        self.loading = false;
        self.error = None;
        self.snapshot = Some(payload);
    }

    pub(crate) fn set_error(&mut self, message: String) {
        self.loading = false;
        self.error = Some(message);
        self.snapshot = None;
    }
}

pub(crate) fn format_compact_tokens(tokens: u32) -> String {
    if tokens < 1_000 {
        return tokens.to_string();
    }
    let tenths = (tokens.saturating_mul(10).saturating_add(500)) / 1_000;
    let whole = tenths / 10;
    let fraction = tenths % 10;
    if fraction == 0 {
        format!("{whole}k")
    } else {
        format!("{whole}.{fraction}k")
    }
}

pub(crate) fn header_summary(model_id: &str, display: ContextBudgetDisplayPayload) -> String {
    match display {
        ContextBudgetDisplayPayload::Relative { used } => {
            format!(
                "Context budget · {model_id} · {} / ?",
                format_compact_tokens(used)
            )
        }
        ContextBudgetDisplayPayload::Absolute {
            limit,
            used,
            percent,
        } => {
            format!(
                "Context budget · {model_id} · {} / {} · {:.1}%",
                format_compact_tokens(used),
                format_compact_tokens(limit),
                percent
            )
        }
    }
}

pub(crate) fn segment_share_percent(
    segment_tokens: usize,
    total_tokens: usize,
    display: ContextBudgetDisplayPayload,
) -> f32 {
    if total_tokens == 0 {
        return 0.0;
    }
    match display {
        ContextBudgetDisplayPayload::Relative { .. } => {
            (segment_tokens as f32 / total_tokens as f32) * 100.0
        }
        ContextBudgetDisplayPayload::Absolute { limit, .. } => {
            (segment_tokens as f32 / limit as f32) * 100.0
        }
    }
}

pub(crate) fn sorted_legend_indices(segments: &[ContextBudgetSegmentPayload]) -> Vec<usize> {
    let mut indices: Vec<usize> = (0..segments.len()).collect();
    indices.sort_by(|&a, &b| {
        segments[b]
            .estimated_tokens
            .cmp(&segments[a].estimated_tokens)
    });
    indices
}
