use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::dynamic_environment::{
    DynamicEnvironmentSnapshotKind, DynamicEnvironmentSourceKind, DynamicEnvironmentSourceSelection,
};

use super::super::PromptSourceKind;
use super::scope::PromptAssemblyScope;

/// `PersistedPromptAssemblyEntry` 表示一个可排序、可启停的 prompt source 引用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedPromptAssemblyEntry {
    pub reference_id: String,
    pub kind: PromptSourceKind,
    pub title: String,
    pub enabled: bool,
    pub requested_order: Option<u16>,
}

/// `PersistedSkillDiscoverySkillEntry` 表示 skill discovery 里单个 skill 的选中与顺序。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedSkillDiscoverySkillEntry {
    pub skill_name: String,
    pub enabled: bool,
    pub requested_order: Option<u16>,
}

/// `PersistedToolSelectionEntry` 表示 tool guidelines 里单个工具的选中与顺序。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedToolSelectionEntry {
    pub tool_name: String,
    pub enabled: bool,
    pub requested_order: Option<u16>,
}

/// `PersistedToolEnablementEntry` 表示单个工具本体的启用/禁用状态。
///
/// 与 `PersistedToolSelectionEntry`（guidelines 注入选择）语义不同：
/// 启停覆盖全部注册工具且无排序语义，未记录的工具默认启用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedToolEnablementEntry {
    pub tool_name: String,
    pub enabled: bool,
}

/// `StoredPromptBody` 表示持久化的 prompt 文本实体。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredPromptBody {
    pub reference_id: String,
    pub title: String,
    pub body: String,
}

/// `PromptAssemblyScopeState` 表示单个 scope 下完整的 prompt assembly 持久化状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptAssemblyScopeState {
    scope: PromptAssemblyScope,
    core_system_override: Option<String>,
    skill_discovery_override: Option<String>,
    tool_guidelines_override: Option<String>,
    entries: Vec<PersistedPromptAssemblyEntry>,
    skill_discovery_skills: Vec<PersistedSkillDiscoverySkillEntry>,
    tool_selections: Vec<PersistedToolSelectionEntry>,
    tool_enablement: Vec<PersistedToolEnablementEntry>,
    dynamic_environment_sources: Vec<DynamicEnvironmentSourceSelection>,
    extra_prompts: Vec<StoredPromptBody>,
}

impl PromptAssemblyScopeState {
    /// `new` 构造一个空 scope 状态。
    #[must_use]
    pub fn new(scope: PromptAssemblyScope) -> Self {
        Self {
            scope,
            core_system_override: None,
            skill_discovery_override: None,
            tool_guidelines_override: None,
            entries: Vec::new(),
            skill_discovery_skills: Vec::new(),
            tool_selections: Vec::new(),
            tool_enablement: Vec::new(),
            dynamic_environment_sources: Vec::new(),
            extra_prompts: Vec::new(),
        }
    }

    /// `scope` 返回该状态所属的 prompt assembly scope。
    #[must_use]
    pub const fn scope(&self) -> PromptAssemblyScope {
        self.scope
    }

    /// `core_system_override` 返回 core system override 文本。
    #[must_use]
    pub fn core_system_override(&self) -> Option<&str> {
        self.core_system_override.as_deref()
    }

    /// `set_core_system_override` 设置 core system override 文本。
    pub fn set_core_system_override(&mut self, body: Option<String>) {
        self.core_system_override = body;
    }

    /// `skill_discovery_override` 返回 skill discovery override 文本。
    #[must_use]
    pub fn skill_discovery_override(&self) -> Option<&str> {
        self.skill_discovery_override.as_deref()
    }

    /// `set_skill_discovery_override` 设置 skill discovery override 文本。
    pub fn set_skill_discovery_override(&mut self, body: Option<String>) {
        self.skill_discovery_override = body;
    }

    /// `tool_guidelines_override` 返回 tool guidelines override 文本。
    #[must_use]
    pub fn tool_guidelines_override(&self) -> Option<&str> {
        self.tool_guidelines_override.as_deref()
    }

    /// `set_tool_guidelines_override` 设置 tool guidelines override 文本。
    pub fn set_tool_guidelines_override(&mut self, body: Option<String>) {
        self.tool_guidelines_override = body;
    }

    /// `entries` 返回该 scope 下持久化的 source entries。
    #[must_use]
    pub fn entries(&self) -> &[PersistedPromptAssemblyEntry] {
        &self.entries
    }

    /// `set_entries` 替换该 scope 下持久化的 source entries。
    pub fn set_entries(&mut self, entries: Vec<PersistedPromptAssemblyEntry>) {
        self.entries = entries;
    }

    /// `entry_mut` 返回指定 source entry 的可变引用。
    pub fn entry_mut(
        &mut self,
        kind: PromptSourceKind,
        reference_id: &str,
    ) -> Option<&mut PersistedPromptAssemblyEntry> {
        self.entries
            .iter_mut()
            .find(|entry| entry.kind == kind && entry.reference_id == reference_id)
    }

    /// `entry_at` 返回指定位置的 source entry。
    #[must_use]
    pub fn entry_at(&self, index: usize) -> Option<&PersistedPromptAssemblyEntry> {
        self.entries.get(index)
    }

    /// `entry_at_mut` 返回指定位置的 source entry 可变引用。
    pub fn entry_at_mut(&mut self, index: usize) -> Option<&mut PersistedPromptAssemblyEntry> {
        self.entries.get_mut(index)
    }

    /// `upsert_entry` 按 kind/reference_id 插入或替换 source entry。
    pub fn upsert_entry(&mut self, entry: PersistedPromptAssemblyEntry) {
        if let Some(existing) = self.entry_mut(entry.kind, &entry.reference_id) {
            *existing = entry;
        } else {
            self.entries.push(entry);
        }
    }

    /// `remove_entry` 删除指定 kind/reference_id 的 source entry，返回是否删除了内容。
    pub fn remove_entry(&mut self, kind: PromptSourceKind, reference_id: &str) -> bool {
        let original_len = self.entries.len();
        self.entries
            .retain(|entry| !(entry.kind == kind && entry.reference_id == reference_id));
        self.entries.len() != original_len
    }

    /// `skill_discovery_skills` 返回 skill discovery 中单个 skill 的持久化选择。
    #[must_use]
    pub fn skill_discovery_skills(&self) -> &[PersistedSkillDiscoverySkillEntry] {
        &self.skill_discovery_skills
    }

    /// `set_skill_discovery_skills` 替换 skill discovery skill 选择。
    pub fn set_skill_discovery_skills(&mut self, skills: Vec<PersistedSkillDiscoverySkillEntry>) {
        self.skill_discovery_skills = skills;
    }

    /// `skill_discovery_skill_mut` 返回指定 skill 选择项的可变引用。
    pub fn skill_discovery_skill_mut(
        &mut self,
        skill_name: &str,
    ) -> Option<&mut PersistedSkillDiscoverySkillEntry> {
        self.skill_discovery_skills
            .iter_mut()
            .find(|entry| entry.skill_name == skill_name)
    }

    /// `skill_discovery_skill_at_mut` 返回指定位置的 skill 选择项可变引用。
    pub fn skill_discovery_skill_at_mut(
        &mut self,
        index: usize,
    ) -> Option<&mut PersistedSkillDiscoverySkillEntry> {
        self.skill_discovery_skills.get_mut(index)
    }

    /// `upsert_skill_discovery_skill` 按 skill_name 插入或替换 skill 选择项。
    pub fn upsert_skill_discovery_skill(&mut self, entry: PersistedSkillDiscoverySkillEntry) {
        if let Some(existing) = self.skill_discovery_skill_mut(&entry.skill_name) {
            *existing = entry;
        } else {
            self.skill_discovery_skills.push(entry);
        }
    }

    /// `swap_skill_discovery_skills` 交换两个 skill 选择项。
    pub fn swap_skill_discovery_skills(&mut self, left: usize, right: usize) {
        self.skill_discovery_skills.swap(left, right);
    }

    /// `tool_selections` 返回 tool guideline 中单个 tool 的持久化选择。
    #[must_use]
    pub fn tool_selections(&self) -> &[PersistedToolSelectionEntry] {
        &self.tool_selections
    }

    /// `set_tool_selections` 替换 tool guideline tool 选择。
    pub fn set_tool_selections(&mut self, tool_selections: Vec<PersistedToolSelectionEntry>) {
        self.tool_selections = tool_selections;
    }

    /// `tool_selection_mut` 返回指定 tool 选择项的可变引用。
    pub fn tool_selection_mut(
        &mut self,
        tool_name: &str,
    ) -> Option<&mut PersistedToolSelectionEntry> {
        self.tool_selections
            .iter_mut()
            .find(|entry| entry.tool_name == tool_name)
    }

    /// `tool_selection_at_mut` 返回指定位置的 tool 选择项可变引用。
    pub fn tool_selection_at_mut(
        &mut self,
        index: usize,
    ) -> Option<&mut PersistedToolSelectionEntry> {
        self.tool_selections.get_mut(index)
    }

    /// `upsert_tool_selection` 按 tool_name 插入或替换 tool 选择项。
    pub fn upsert_tool_selection(&mut self, entry: PersistedToolSelectionEntry) {
        if let Some(existing) = self.tool_selection_mut(&entry.tool_name) {
            *existing = entry;
        } else {
            self.tool_selections.push(entry);
        }
    }

    /// `swap_tool_selections` 交换两个 tool 选择项。
    pub fn swap_tool_selections(&mut self, left: usize, right: usize) {
        self.tool_selections.swap(left, right);
    }

    /// `tool_enablement` 返回工具本体启停的持久化状态。
    #[must_use]
    pub fn tool_enablement(&self) -> &[PersistedToolEnablementEntry] {
        &self.tool_enablement
    }

    /// `set_tool_enablement` 替换工具本体启停状态。
    pub fn set_tool_enablement(&mut self, tool_enablement: Vec<PersistedToolEnablementEntry>) {
        self.tool_enablement = tool_enablement;
    }

    /// `tool_enablement_mut` 返回指定工具启停项的可变引用。
    pub fn tool_enablement_mut(
        &mut self,
        tool_name: &str,
    ) -> Option<&mut PersistedToolEnablementEntry> {
        self.tool_enablement
            .iter_mut()
            .find(|entry| entry.tool_name == tool_name)
    }

    /// `upsert_tool_enablement` 按 tool_name 插入或替换工具启停项。
    pub fn upsert_tool_enablement(&mut self, entry: PersistedToolEnablementEntry) {
        if let Some(existing) = self.tool_enablement_mut(&entry.tool_name) {
            *existing = entry;
        } else {
            self.tool_enablement.push(entry);
        }
    }

    /// `dynamic_environment_sources` 返回 dynamic environment source 选择。
    #[must_use]
    pub fn dynamic_environment_sources(&self) -> &[DynamicEnvironmentSourceSelection] {
        &self.dynamic_environment_sources
    }

    /// `set_dynamic_environment_sources` 替换 dynamic environment source 选择。
    pub fn set_dynamic_environment_sources(
        &mut self,
        sources: Vec<DynamicEnvironmentSourceSelection>,
    ) {
        if !sources.is_empty() {
            self.assert_global_dynamic_environment_scope();
        }
        self.dynamic_environment_sources = sources;
    }

    /// `dynamic_environment_source_mut` 返回指定 dynamic environment source 选择项。
    pub fn dynamic_environment_source_mut(
        &mut self,
        snapshot_kind: DynamicEnvironmentSnapshotKind,
        source_kind: DynamicEnvironmentSourceKind,
    ) -> Option<&mut DynamicEnvironmentSourceSelection> {
        if self.scope != PromptAssemblyScope::Global {
            return None;
        }
        self.dynamic_environment_sources
            .iter_mut()
            .find(|selection| {
                selection.snapshot_kind == snapshot_kind && selection.source_kind == source_kind
            })
    }

    /// `upsert_dynamic_environment_source` 按 snapshot/source kind 插入或替换 dynamic environment 选择。
    pub fn upsert_dynamic_environment_source(&mut self, source: DynamicEnvironmentSourceSelection) {
        self.assert_global_dynamic_environment_scope();
        if let Some(existing) =
            self.dynamic_environment_source_mut(source.snapshot_kind, source.source_kind)
        {
            *existing = source;
        } else {
            self.dynamic_environment_sources.push(source);
        }
    }

    fn assert_global_dynamic_environment_scope(&self) {
        assert!(
            self.scope == PromptAssemblyScope::Global,
            "dynamic environment source selections are global-only, got {} scope",
            self.scope.as_stored_value()
        );
    }

    /// `extra_prompts` 返回该 scope 下持久化的 custom prompt bodies。
    #[must_use]
    pub fn extra_prompts(&self) -> &[StoredPromptBody] {
        &self.extra_prompts
    }

    /// `set_extra_prompts` 替换该 scope 下持久化的 custom prompt bodies。
    pub fn set_extra_prompts(&mut self, prompts: Vec<StoredPromptBody>) {
        self.extra_prompts = prompts;
    }

    /// `extra_prompt_mut` 返回指定 custom prompt body 的可变引用。
    pub fn extra_prompt_mut(&mut self, reference_id: &str) -> Option<&mut StoredPromptBody> {
        self.extra_prompts
            .iter_mut()
            .find(|prompt| prompt.reference_id == reference_id)
    }

    /// `upsert_extra_prompt` 按 reference_id 插入或替换 custom prompt body。
    pub fn upsert_extra_prompt(&mut self, prompt: StoredPromptBody) {
        if let Some(existing) = self.extra_prompt_mut(&prompt.reference_id) {
            *existing = prompt;
        } else {
            self.extra_prompts.push(prompt);
        }
    }

    /// `remove_extra_prompt` 删除指定 custom prompt body，返回是否删除了内容。
    pub fn remove_extra_prompt(&mut self, reference_id: &str) -> bool {
        let original_len = self.extra_prompts.len();
        self.extra_prompts
            .retain(|prompt| prompt.reference_id != reference_id);
        self.extra_prompts.len() != original_len
    }
}

/// `extra_prompt_bodies_by_reference` 把 extra prompt bodies 投影成按 reference_id 索引的映射。
#[must_use]
pub fn extra_prompt_bodies_by_reference(
    state: &PromptAssemblyScopeState,
) -> BTreeMap<&str, &StoredPromptBody> {
    state
        .extra_prompts
        .iter()
        .map(|prompt| (prompt.reference_id.as_str(), prompt))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_state_new_builds_state_through_domain_api() {
        let mut state = PromptAssemblyScopeState::new(PromptAssemblyScope::Global);
        state.set_core_system_override(Some("core".to_string()));
        state.upsert_entry(PersistedPromptAssemblyEntry {
            reference_id: "skill-discovery".to_string(),
            kind: PromptSourceKind::SkillDiscovery,
            title: "Skill discovery".to_string(),
            enabled: true,
            requested_order: Some(1),
        });

        assert_eq!(state.scope(), PromptAssemblyScope::Global);
        assert_eq!(state.core_system_override(), Some("core"));
        assert_eq!(state.entries()[0].reference_id, "skill-discovery");
    }

    #[test]
    fn scope_state_upserts_entries_by_kind_and_reference() {
        let mut state = PromptAssemblyScopeState::new(PromptAssemblyScope::Project);

        state.upsert_entry(PersistedPromptAssemblyEntry {
            reference_id: "review".to_string(),
            kind: PromptSourceKind::ExtraPrompt,
            title: "Review".to_string(),
            enabled: true,
            requested_order: Some(10),
        });
        state.upsert_entry(PersistedPromptAssemblyEntry {
            reference_id: "review".to_string(),
            kind: PromptSourceKind::ExtraPrompt,
            title: "Updated review".to_string(),
            enabled: false,
            requested_order: Some(20),
        });

        assert_eq!(state.entries().len(), 1);
        assert_eq!(state.entries()[0].title, "Updated review");
        assert!(!state.entries()[0].enabled);
        assert_eq!(state.entries()[0].requested_order, Some(20));
        assert!(state.remove_entry(PromptSourceKind::ExtraPrompt, "review"));
        assert!(state.entries().is_empty());
    }

    #[test]
    fn scope_state_upserts_extra_prompts_by_reference() {
        let mut state = PromptAssemblyScopeState::new(PromptAssemblyScope::Project);

        state.upsert_extra_prompt(StoredPromptBody {
            reference_id: "review".to_string(),
            title: "Review".to_string(),
            body: "First".to_string(),
        });
        state.upsert_extra_prompt(StoredPromptBody {
            reference_id: "review".to_string(),
            title: "Review updated".to_string(),
            body: "Second".to_string(),
        });

        assert_eq!(state.extra_prompts().len(), 1);
        assert_eq!(state.extra_prompts()[0].title, "Review updated");
        assert_eq!(state.extra_prompts()[0].body, "Second");
        assert!(state.remove_extra_prompt("review"));
        assert!(state.extra_prompts().is_empty());
    }

    #[test]
    fn scope_state_updates_selection_collections_by_key() {
        let mut state = PromptAssemblyScopeState::new(PromptAssemblyScope::Global);

        state.upsert_skill_discovery_skill(PersistedSkillDiscoverySkillEntry {
            skill_name: "review".to_string(),
            enabled: true,
            requested_order: Some(1),
        });
        state.upsert_skill_discovery_skill(PersistedSkillDiscoverySkillEntry {
            skill_name: "review".to_string(),
            enabled: false,
            requested_order: Some(2),
        });
        assert_eq!(state.skill_discovery_skills().len(), 1);
        assert!(!state.skill_discovery_skills()[0].enabled);
        assert_eq!(state.skill_discovery_skills()[0].requested_order, Some(2));

        state.upsert_tool_selection(PersistedToolSelectionEntry {
            tool_name: "bash".to_string(),
            enabled: true,
            requested_order: Some(1),
        });
        state.upsert_tool_selection(PersistedToolSelectionEntry {
            tool_name: "bash".to_string(),
            enabled: false,
            requested_order: Some(2),
        });
        assert_eq!(state.tool_selections().len(), 1);
        assert!(!state.tool_selections()[0].enabled);
        assert_eq!(state.tool_selections()[0].requested_order, Some(2));

        state.upsert_tool_enablement(PersistedToolEnablementEntry {
            tool_name: "bash".to_string(),
            enabled: false,
        });
        state.upsert_tool_enablement(PersistedToolEnablementEntry {
            tool_name: "bash".to_string(),
            enabled: true,
        });
        assert_eq!(state.tool_enablement().len(), 1);
        assert!(state.tool_enablement()[0].enabled);
    }

    #[test]
    #[should_panic(
        expected = "dynamic environment source selections are global-only, got project scope"
    )]
    fn project_scope_rejects_dynamic_environment_source_state() {
        let mut state = PromptAssemblyScopeState::new(PromptAssemblyScope::Project);

        state.set_dynamic_environment_sources(vec![DynamicEnvironmentSourceSelection {
            snapshot_kind: DynamicEnvironmentSnapshotKind::Baseline,
            source_kind: DynamicEnvironmentSourceKind::Date,
            enabled: true,
        }]);
    }
}
