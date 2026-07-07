use super::*;

pub(in crate::prompt_assembly) fn ensure_dynamic_environment_entry_exists(
    state: &mut PromptAssemblyScopeState,
    kind: PromptSourceKind,
) {
    let (reference_id, title, requested_order) = match kind {
        PromptSourceKind::DynamicEnvironmentBaseline => (
            "env-baseline",
            "Env baseline",
            DEFAULT_DYNAMIC_BASELINE_REQUESTED_ORDER,
        ),
        PromptSourceKind::DynamicEnvironmentChanges => (
            "env-changes",
            "Env changes",
            DEFAULT_DYNAMIC_CHANGES_REQUESTED_ORDER,
        ),
        _ => return,
    };
    if state
        .entries()
        .iter()
        .any(|entry| entry.kind == kind && entry.reference_id == reference_id)
    {
        return;
    }
    state.upsert_entry(PersistedPromptAssemblyEntry {
        reference_id: reference_id.to_string(),
        kind,
        title: title.to_string(),
        enabled: true,
        requested_order: Some(requested_order),
    });
}

pub(in crate::prompt_assembly) fn ensure_default_dynamic_environment_sources(
    global_state: &mut PromptAssemblyScopeState,
) {
    ensure_dynamic_environment_entry_exists(
        global_state,
        PromptSourceKind::DynamicEnvironmentBaseline,
    );
    ensure_dynamic_environment_entry_exists(
        global_state,
        PromptSourceKind::DynamicEnvironmentChanges,
    );
}

pub(in crate::prompt_assembly) fn merged_dynamic_environment_selection_state(
    global_state: &PromptAssemblyScopeState,
    _project_state: &PromptAssemblyScopeState,
) -> Vec<DynamicEnvironmentSourceSelection> {
    let mut selections = default_dynamic_environment_selections();
    apply_dynamic_environment_selection_overrides(
        &mut selections,
        global_state.dynamic_environment_sources(),
    );
    selections.sort_by_key(|selection| (selection.snapshot_kind, selection.source_kind));
    selections
}

pub(in crate::prompt_assembly) fn apply_dynamic_environment_selection_overrides(
    selections: &mut [DynamicEnvironmentSourceSelection],
    overrides: &[DynamicEnvironmentSourceSelection],
) {
    for override_selection in overrides {
        if let Some(selection) = selections.iter_mut().find(|selection| {
            selection.snapshot_kind == override_selection.snapshot_kind
                && selection.source_kind == override_selection.source_kind
        }) {
            selection.enabled = override_selection.enabled;
        }
    }
}

pub(in crate::prompt_assembly) fn dynamic_environment_candidate_inventory(
    observations_by_source: &HashMap<DynamicEnvironmentSourceKind, DynamicEnvironmentObservation>,
    selection_state: &[DynamicEnvironmentSourceSelection],
    _global_state: &PromptAssemblyScopeState,
    _project_state: &PromptAssemblyScopeState,
) -> Vec<PromptAssemblyDynamicEnvironmentCandidate> {
    [
        DynamicEnvironmentSourceKind::GitReference,
        DynamicEnvironmentSourceKind::GitWorkingTree,
        DynamicEnvironmentSourceKind::Date,
        DynamicEnvironmentSourceKind::Workdir,
    ]
    .into_iter()
    .map(|source_kind| {
        let baseline_selected = dynamic_environment_selection_enabled(
            selection_state,
            DynamicEnvironmentSnapshotKind::Baseline,
            source_kind,
        );
        let changes_selected = dynamic_environment_selection_enabled(
            selection_state,
            DynamicEnvironmentSnapshotKind::Changes,
            source_kind,
        );
        PromptAssemblyDynamicEnvironmentCandidate {
            source_kind,
            label: source_kind.label().to_string(),
            origin: PromptSourceOrigin::Builtin,
            baseline_selected,
            changes_selected,
            baseline_preview_body: dynamic_environment_candidate_preview_body(
                observations_by_source,
                DynamicEnvironmentSnapshotKind::Baseline,
                source_kind,
            ),
            changes_preview_body: dynamic_environment_candidate_preview_body(
                observations_by_source,
                DynamicEnvironmentSnapshotKind::Changes,
                source_kind,
            ),
        }
    })
    .collect()
}

pub(in crate::prompt_assembly) fn dynamic_environment_selection_enabled(
    selection_state: &[DynamicEnvironmentSourceSelection],
    snapshot_kind: DynamicEnvironmentSnapshotKind,
    source_kind: DynamicEnvironmentSourceKind,
) -> bool {
    selection_state
        .iter()
        .find(|selection| {
            selection.snapshot_kind == snapshot_kind && selection.source_kind == source_kind
        })
        .is_some_and(|selection| selection.enabled)
}

pub(in crate::prompt_assembly) fn set_dynamic_environment_source_selected(
    state: &mut PromptAssemblyScopeState,
    snapshot_kind: DynamicEnvironmentSnapshotKind,
    source_kind: DynamicEnvironmentSourceKind,
    selected: bool,
) {
    if let Some(selection) = state.dynamic_environment_source_mut(snapshot_kind, source_kind) {
        selection.enabled = selected;
        return;
    }

    state.upsert_dynamic_environment_source(DynamicEnvironmentSourceSelection {
        snapshot_kind,
        source_kind,
        enabled: selected,
    });
}

pub(in crate::prompt_assembly) fn dynamic_environment_preview_body(
    observations_by_source: &HashMap<DynamicEnvironmentSourceKind, DynamicEnvironmentObservation>,
    snapshot_kind: DynamicEnvironmentSnapshotKind,
    selection_state: &[DynamicEnvironmentSourceSelection],
) -> Option<String> {
    let observations = dynamic_environment_observations_for_snapshot_kind(
        observations_by_source,
        selection_state,
        snapshot_kind,
    );
    build_dynamic_environment_snapshot(snapshot_kind, observations).map(|snapshot| snapshot.body)
}

pub(in crate::prompt_assembly) fn observe_dynamic_environment_inventory(
    work_dir: &Path,
) -> HashMap<DynamicEnvironmentSourceKind, DynamicEnvironmentObservation> {
    [
        DynamicEnvironmentSourceKind::GitReference,
        DynamicEnvironmentSourceKind::GitWorkingTree,
        DynamicEnvironmentSourceKind::Date,
        DynamicEnvironmentSourceKind::Workdir,
    ]
    .into_iter()
    .filter_map(|source_kind| {
        crate::dynamic_environment::observe_dynamic_environment_sources(work_dir, &[source_kind])
            .ok()
            .into_iter()
            .flatten()
            .next()
            .map(|observation| (source_kind, observation))
    })
    .collect()
}

pub(in crate::prompt_assembly) fn dynamic_environment_observations_for_snapshot_kind(
    observations_by_source: &HashMap<DynamicEnvironmentSourceKind, DynamicEnvironmentObservation>,
    selection_state: &[DynamicEnvironmentSourceSelection],
    snapshot_kind: DynamicEnvironmentSnapshotKind,
) -> Vec<DynamicEnvironmentObservation> {
    enabled_dynamic_environment_sources(selection_state, snapshot_kind)
        .into_iter()
        .filter_map(|source_kind| observations_by_source.get(&source_kind).cloned())
        .collect()
}

pub(in crate::prompt_assembly) fn dynamic_environment_candidate_preview_body(
    observations_by_source: &HashMap<DynamicEnvironmentSourceKind, DynamicEnvironmentObservation>,
    snapshot_kind: DynamicEnvironmentSnapshotKind,
    source_kind: DynamicEnvironmentSourceKind,
) -> String {
    observations_by_source
        .get(&source_kind)
        .cloned()
        .and_then(|observation| {
            build_dynamic_environment_snapshot(snapshot_kind, vec![observation])
        })
        .map(|snapshot| snapshot.body)
        .unwrap_or_default()
}
