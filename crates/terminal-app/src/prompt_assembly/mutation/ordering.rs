use super::*;

pub(in crate::prompt_assembly) fn remove_prompt_source(
    state: &mut PromptAssemblyScopeState,
    kind: PromptSourceKind,
    reference_id: &str,
) {
    if matches!(
        kind,
        PromptSourceKind::InstructionsFile
            | PromptSourceKind::SkillDiscovery
            | PromptSourceKind::ToolGuidelines
            | PromptSourceKind::DynamicEnvironmentBaseline
            | PromptSourceKind::DynamicEnvironmentChanges
    ) {
        return;
    }
    state.remove_entry(kind, reference_id);
}

pub(in crate::prompt_assembly) fn move_active_source(
    global_state: &mut PromptAssemblyScopeState,
    project_state: &mut PromptAssemblyScopeState,
    scope: PromptAssemblyScope,
    kind: PromptSourceKind,
    reference_id: &str,
    direction: PromptAssemblyMoveDirection,
) -> Result<()> {
    let Some(current) = find_entry_address(global_state, project_state, scope, kind, reference_id)
    else {
        return Ok(());
    };
    let ordered = ordered_non_core_entry_addresses(global_state, project_state);
    let Some(position) = ordered.iter().position(|address| *address == current) else {
        return Ok(());
    };
    let neighbor_position = match direction {
        PromptAssemblyMoveDirection::Up => position.checked_sub(1),
        PromptAssemblyMoveDirection::Down => (position + 1 < ordered.len()).then_some(position + 1),
    };
    let Some(neighbor_position) = neighbor_position else {
        return Ok(());
    };
    let neighbor = ordered[neighbor_position];

    let current_order = entry_requested_order(global_state, project_state, current);
    let neighbor_order = entry_requested_order(global_state, project_state, neighbor);
    set_entry_requested_order(global_state, project_state, current, neighbor_order);
    set_entry_requested_order(global_state, project_state, neighbor, current_order);
    normalize_requested_orders(global_state, project_state);
    Ok(())
}

pub(in crate::prompt_assembly) fn find_entry_address(
    global_state: &PromptAssemblyScopeState,
    project_state: &PromptAssemblyScopeState,
    scope: PromptAssemblyScope,
    kind: PromptSourceKind,
    reference_id: &str,
) -> Option<PromptEntryAddress> {
    let state = match scope {
        PromptAssemblyScope::Global => global_state,
        PromptAssemblyScope::Project => project_state,
    };
    state
        .entries()
        .iter()
        .position(|entry| entry.kind == kind && entry.reference_id == reference_id)
        .map(|index| PromptEntryAddress { scope, index })
}

pub(in crate::prompt_assembly) fn ordered_non_core_entry_addresses(
    global_state: &PromptAssemblyScopeState,
    project_state: &PromptAssemblyScopeState,
) -> Vec<PromptEntryAddress> {
    let mut addresses = Vec::new();
    addresses.extend(state_entry_addresses(global_state));
    addresses.extend(state_entry_addresses(project_state));
    addresses.sort_by(|left, right| {
        let Some(left_entry) = entry_ref(global_state, project_state, *left) else {
            return std::cmp::Ordering::Equal;
        };
        let Some(right_entry) = entry_ref(global_state, project_state, *right) else {
            return std::cmp::Ordering::Equal;
        };
        requested_order_sort_key(left_entry.requested_order)
            .cmp(&requested_order_sort_key(right_entry.requested_order))
            .then_with(|| natural_sort_text_cmp(&left_entry.title, &right_entry.title))
            .then_with(|| left_entry.reference_id.cmp(&right_entry.reference_id))
            .then_with(|| {
                left.scope
                    .as_stored_value()
                    .cmp(right.scope.as_stored_value())
            })
    });
    addresses
}

pub(in crate::prompt_assembly) fn state_entry_addresses(
    state: &PromptAssemblyScopeState,
) -> Vec<PromptEntryAddress> {
    state
        .entries()
        .iter()
        .enumerate()
        .map(|(index, _)| PromptEntryAddress {
            scope: state.scope(),
            index,
        })
        .collect()
}

pub(in crate::prompt_assembly) fn entry_ref<'a>(
    global_state: &'a PromptAssemblyScopeState,
    project_state: &'a PromptAssemblyScopeState,
    address: PromptEntryAddress,
) -> Option<&'a PersistedPromptAssemblyEntry> {
    match address.scope {
        PromptAssemblyScope::Global => global_state.entry_at(address.index),
        PromptAssemblyScope::Project => project_state.entry_at(address.index),
    }
}

pub(in crate::prompt_assembly) fn entry_mut<'a>(
    global_state: &'a mut PromptAssemblyScopeState,
    project_state: &'a mut PromptAssemblyScopeState,
    address: PromptEntryAddress,
) -> Option<&'a mut PersistedPromptAssemblyEntry> {
    match address.scope {
        PromptAssemblyScope::Global => global_state.entry_at_mut(address.index),
        PromptAssemblyScope::Project => project_state.entry_at_mut(address.index),
    }
}

pub(in crate::prompt_assembly) fn entry_requested_order(
    global_state: &PromptAssemblyScopeState,
    project_state: &PromptAssemblyScopeState,
    address: PromptEntryAddress,
) -> Option<u16> {
    entry_ref(global_state, project_state, address).and_then(|entry| entry.requested_order)
}

pub(in crate::prompt_assembly) fn set_entry_requested_order(
    global_state: &mut PromptAssemblyScopeState,
    project_state: &mut PromptAssemblyScopeState,
    address: PromptEntryAddress,
    requested_order: Option<u16>,
) {
    if let Some(entry) = entry_mut(global_state, project_state, address) {
        entry.requested_order = requested_order;
    }
}

pub(in crate::prompt_assembly) fn normalize_requested_orders(
    global_state: &mut PromptAssemblyScopeState,
    project_state: &mut PromptAssemblyScopeState,
) {
    let ordered = ordered_non_core_entry_addresses(global_state, project_state);
    for (index, address) in ordered.into_iter().enumerate() {
        let normalized = u16::try_from((index + 1) * 10).unwrap_or(u16::MAX);
        set_entry_requested_order(global_state, project_state, address, Some(normalized));
    }
}
