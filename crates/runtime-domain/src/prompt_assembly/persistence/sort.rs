use crate::text::natural_sort_text_cmp;

use super::super::requested_order_sort_key;
use super::state::{
    PersistedPromptAssemblyEntry, PersistedSkillDiscoverySkillEntry, PersistedToolSelectionEntry,
};

/// `sort_prompt_assembly_entries` 按领域展示顺序就地排序 persisted source entries。
pub fn sort_prompt_assembly_entries(entries: &mut [PersistedPromptAssemblyEntry]) {
    entries.sort_by(prompt_assembly_entry_order);
}

pub(super) fn sorted_prompt_assembly_entries(
    mut entries: Vec<PersistedPromptAssemblyEntry>,
) -> Vec<PersistedPromptAssemblyEntry> {
    sort_prompt_assembly_entries(&mut entries);
    entries
}

pub(super) fn sorted_prompt_assembly_entry_refs(
    entries: &[PersistedPromptAssemblyEntry],
) -> Vec<&PersistedPromptAssemblyEntry> {
    let mut entries = entries.iter().collect::<Vec<_>>();
    entries.sort_by(|left, right| prompt_assembly_entry_order(left, right));
    entries
}

fn prompt_assembly_entry_order(
    left: &PersistedPromptAssemblyEntry,
    right: &PersistedPromptAssemblyEntry,
) -> std::cmp::Ordering {
    requested_order_sort_key(left.requested_order)
        .cmp(&requested_order_sort_key(right.requested_order))
        .then_with(|| natural_sort_text_cmp(&left.title, &right.title))
        .then_with(|| left.reference_id.cmp(&right.reference_id))
}

/// `sort_skill_discovery_skill_entries` 按领域展示顺序就地排序 skill discovery entries。
pub fn sort_skill_discovery_skill_entries(entries: &mut [PersistedSkillDiscoverySkillEntry]) {
    entries.sort_by(skill_discovery_skill_entry_order);
}

pub(super) fn sorted_skill_discovery_skill_entries(
    mut entries: Vec<PersistedSkillDiscoverySkillEntry>,
) -> Vec<PersistedSkillDiscoverySkillEntry> {
    sort_skill_discovery_skill_entries(&mut entries);
    entries
}

pub(super) fn sorted_skill_discovery_skill_entry_refs(
    entries: &[PersistedSkillDiscoverySkillEntry],
) -> Vec<&PersistedSkillDiscoverySkillEntry> {
    let mut entries = entries.iter().collect::<Vec<_>>();
    entries.sort_by(|left, right| skill_discovery_skill_entry_order(left, right));
    entries
}

fn skill_discovery_skill_entry_order(
    left: &PersistedSkillDiscoverySkillEntry,
    right: &PersistedSkillDiscoverySkillEntry,
) -> std::cmp::Ordering {
    requested_order_sort_key(left.requested_order)
        .cmp(&requested_order_sort_key(right.requested_order))
        .then_with(|| natural_sort_text_cmp(&left.skill_name, &right.skill_name))
}

/// `sort_tool_selection_entries` 按领域展示顺序就地排序 tool selection entries。
pub fn sort_tool_selection_entries(entries: &mut [PersistedToolSelectionEntry]) {
    entries.sort_by(tool_selection_entry_order);
}

pub(super) fn sorted_tool_selection_entries(
    mut entries: Vec<PersistedToolSelectionEntry>,
) -> Vec<PersistedToolSelectionEntry> {
    sort_tool_selection_entries(&mut entries);
    entries
}

pub(super) fn sorted_tool_selection_entry_refs(
    entries: &[PersistedToolSelectionEntry],
) -> Vec<&PersistedToolSelectionEntry> {
    let mut entries = entries.iter().collect::<Vec<_>>();
    entries.sort_by(|left, right| tool_selection_entry_order(left, right));
    entries
}

fn tool_selection_entry_order(
    left: &PersistedToolSelectionEntry,
    right: &PersistedToolSelectionEntry,
) -> std::cmp::Ordering {
    requested_order_sort_key(left.requested_order)
        .cmp(&requested_order_sort_key(right.requested_order))
        .then_with(|| natural_sort_text_cmp(&left.tool_name, &right.tool_name))
}
