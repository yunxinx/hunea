//! Heatmap cell allocation for context budget segments.

use runtime_domain::context_budget::ContextSegment;

/// Assigns grid cells to segments proportional to token share (largest remainder).
/// Returns per-segment cell counts in segment slice order (stack_order order).
pub(crate) fn allocate_heatmap_cells(
    segments: &[ContextSegment],
    total_cells: usize,
) -> Vec<usize> {
    if total_cells == 0 || segments.is_empty() {
        return vec![0; segments.len()];
    }
    let total_tokens: usize = segments.iter().map(|s| s.estimated_tokens).sum();
    if total_tokens == 0 {
        let base = total_cells / segments.len();
        let mut counts = vec![base; segments.len()];
        for i in 0..(total_cells - base * segments.len()) {
            counts[i % segments.len()] += 1;
        }
        return counts;
    }

    let mut counts = Vec::with_capacity(segments.len());
    let mut remainders = Vec::with_capacity(segments.len());
    let mut assigned = 0usize;
    for segment in segments {
        let exact = (segment.estimated_tokens as f64 / total_tokens as f64) * total_cells as f64;
        let floor = exact.floor() as usize;
        counts.push(floor);
        assigned = assigned.saturating_add(floor);
        remainders.push((exact - floor as f64, counts.len() - 1));
    }
    let mut leftover = total_cells.saturating_sub(assigned);
    remainders.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    for (_, index) in remainders {
        if leftover == 0 {
            break;
        }
        counts[index] += 1;
        leftover -= 1;
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_domain::context_budget::{ContextSegment, SegmentKind};

    fn seg(kind: SegmentKind, tokens: usize, order: u16) -> ContextSegment {
        ContextSegment {
            kind,
            stack_order: order,
            estimated_tokens: tokens,
            label: kind.default_label().to_string(),
        }
    }

    #[test]
    fn allocate_cells_sum_equals_grid_size() {
        let segments = vec![
            seg(SegmentKind::System, 100, 0),
            seg(SegmentKind::UserMessage, 200, 1),
            seg(SegmentKind::AssistantMessage, 100, 2),
        ];
        let counts = allocate_heatmap_cells(&segments, 24);
        assert_eq!(counts.iter().sum::<usize>(), 24);
    }
}
