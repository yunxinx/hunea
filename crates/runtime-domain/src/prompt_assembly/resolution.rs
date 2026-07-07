use std::cmp::Ordering;
use std::collections::HashMap;

use crate::text::natural_sort_text_cmp;

use super::types::{
    CoreSystemPromptInput, PromptAssemblyInput, PromptAssemblyLifecycle, PromptAssemblySnapshot,
    PromptSourceCandidate, PromptSourceCandidateState, PromptSourceInactiveReason,
    PromptSourceKind, PromptSourceOrigin, PromptSourceStatus, ResolvedPromptSource,
};
use super::{CORE_SYSTEM_REFERENCE_ID, CORE_SYSTEM_TITLE};

/// `requested_order_sort_key` 把可选用户顺序转成排序键；未设置的条目稳定排在所有显式顺序之后。
#[must_use]
pub const fn requested_order_sort_key(requested_order: Option<u16>) -> (bool, u16) {
    match requested_order {
        Some(order) => (false, order),
        None => (true, 0),
    }
}

/// `resolve_prompt_assembly` 解析 next-new-session prompt assembly。
#[must_use]
pub fn resolve_prompt_assembly(input: &PromptAssemblyInput) -> PromptAssemblySnapshot {
    let mut active_sources = vec![ResolvedPromptSource {
        reference_id: CORE_SYSTEM_REFERENCE_ID.to_string(),
        kind: PromptSourceKind::CoreSystemPrompt,
        title: CORE_SYSTEM_TITLE.to_string(),
        origin: Some(resolve_core_system_origin(&input.core_system)),
        status: PromptSourceStatus::Active { order: 0 },
    }];
    let mut inactive_sources = Vec::new();
    let mut grouped_candidates: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut passthrough_candidates = Vec::new();

    for (candidate_index, candidate) in input.candidates.iter().enumerate() {
        if let Some(collision_key) = candidate.collision_key.as_ref() {
            grouped_candidates
                .entry(collision_key.as_str())
                .or_default()
                .push(candidate_index);
        } else {
            passthrough_candidates.push(candidate_index);
        }
    }

    let mut active_candidates = Vec::new();
    for candidate_indices in grouped_candidates.into_values() {
        let winner_index = collision_winner_index(&input.candidates, &candidate_indices);
        for candidate_index in candidate_indices {
            let candidate = &input.candidates[candidate_index];
            match inactive_reason_for_grouped_candidate(
                candidate,
                winner_index == Some(candidate_index),
            ) {
                Some(reason) => inactive_sources.push(resolved_inactive(candidate, reason)),
                None => active_candidates.push(candidate_index),
            }
        }
    }

    for candidate_index in passthrough_candidates {
        let candidate = &input.candidates[candidate_index];
        match inactive_reason_for_passthrough_candidate(candidate) {
            Some(reason) => inactive_sources.push(resolved_inactive(candidate, reason)),
            None => active_candidates.push(candidate_index),
        }
    }

    active_candidates.sort_by(|left, right| {
        active_candidate_order(&input.candidates[*left], &input.candidates[*right])
    });
    for (index, candidate_index) in active_candidates.into_iter().enumerate() {
        let candidate = &input.candidates[candidate_index];
        active_sources.push(ResolvedPromptSource {
            reference_id: candidate.reference_id.clone(),
            kind: candidate.kind,
            title: candidate.title.clone(),
            origin: candidate.origin,
            status: PromptSourceStatus::Active { order: index + 1 },
        });
    }

    inactive_sources.sort_by(inactive_source_order);

    PromptAssemblySnapshot {
        lifecycle: PromptAssemblyLifecycle::NextNewSession,
        active_sources,
        inactive_sources,
    }
}

fn resolve_core_system_origin(input: &CoreSystemPromptInput) -> PromptSourceOrigin {
    if input.project_override_present {
        PromptSourceOrigin::Project
    } else if input.global_override_present {
        PromptSourceOrigin::Global
    } else {
        PromptSourceOrigin::Builtin
    }
}

fn collision_winner_index(
    candidates: &[PromptSourceCandidate],
    indices: &[usize],
) -> Option<usize> {
    indices
        .iter()
        .copied()
        .filter(|index| candidates[*index].state != PromptSourceCandidateState::Disabled)
        .min_by(|left, right| {
            collision_priority(&candidates[*left]).cmp(&collision_priority(&candidates[*right]))
        })
}

fn collision_priority(candidate: &PromptSourceCandidate) -> (u8, &str) {
    let scope_rank = match candidate.origin {
        Some(PromptSourceOrigin::Project) => 0,
        Some(PromptSourceOrigin::Global) => 1,
        Some(PromptSourceOrigin::Builtin) | None => 2,
    };
    (scope_rank, candidate.reference_id.as_str())
}

fn inactive_reason_for_grouped_candidate(
    candidate: &PromptSourceCandidate,
    is_collision_winner: bool,
) -> Option<PromptSourceInactiveReason> {
    if candidate.state == PromptSourceCandidateState::Disabled {
        return Some(PromptSourceInactiveReason::Disabled);
    }
    if !is_collision_winner {
        return Some(PromptSourceInactiveReason::Shadowed);
    }
    if candidate.state == PromptSourceCandidateState::Missing {
        return Some(PromptSourceInactiveReason::Missing);
    }
    None
}

fn inactive_reason_for_passthrough_candidate(
    candidate: &PromptSourceCandidate,
) -> Option<PromptSourceInactiveReason> {
    if candidate.state == PromptSourceCandidateState::Disabled {
        return Some(PromptSourceInactiveReason::Disabled);
    }
    if candidate.state == PromptSourceCandidateState::Missing {
        return Some(PromptSourceInactiveReason::Missing);
    }
    None
}

fn resolved_inactive(
    candidate: &PromptSourceCandidate,
    reason: PromptSourceInactiveReason,
) -> ResolvedPromptSource {
    ResolvedPromptSource {
        reference_id: candidate.reference_id.clone(),
        kind: candidate.kind,
        title: candidate.title.clone(),
        origin: candidate.origin,
        status: PromptSourceStatus::Inactive { reason },
    }
}

fn active_candidate_order(left: &PromptSourceCandidate, right: &PromptSourceCandidate) -> Ordering {
    requested_order_sort_key(left.requested_order)
        .cmp(&requested_order_sort_key(right.requested_order))
        .then_with(|| natural_sort_text_cmp(&left.title, &right.title))
        .then_with(|| left.reference_id.cmp(&right.reference_id))
}

fn inactive_reason_rank(reason: PromptSourceInactiveReason) -> u8 {
    match reason {
        PromptSourceInactiveReason::Disabled => 0,
        PromptSourceInactiveReason::Missing => 1,
        PromptSourceInactiveReason::Shadowed => 2,
    }
}

fn inactive_source_order(left: &ResolvedPromptSource, right: &ResolvedPromptSource) -> Ordering {
    let left_rank = match left.status {
        PromptSourceStatus::Inactive { reason } => inactive_reason_rank(reason),
        PromptSourceStatus::Active { .. } => u8::MAX,
    };
    let right_rank = match right.status {
        PromptSourceStatus::Inactive { reason } => inactive_reason_rank(reason),
        PromptSourceStatus::Active { .. } => u8::MAX,
    };
    left_rank
        .cmp(&right_rank)
        .then_with(|| natural_sort_text_cmp(&left.title, &right.title))
        .then_with(|| left.reference_id.cmp(&right.reference_id))
}
