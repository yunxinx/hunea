//! Prompt capability 的 owner-scoped contribution 与不可变 session snapshot。

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::{Arc, Mutex, Weak},
};

use runtime_domain::{
    dynamic_environment::DynamicEnvironmentSessionConfig,
    prompt_assembly::persistence::PromptAssemblyScope,
    prompt_assembly::{
        PromptAssemblyManagerSnapshot, PromptPreludeSection, PromptPreludeSnapshot,
        PromptSourceKind, PromptSourceOrigin,
    },
};

/// Prompt section 的 host registration owner。
pub(super) type PromptOwner = String;

/// 一个由 runtime plugin 贡献的不可变 prompt section。
#[derive(Clone)]
pub(super) struct PromptSectionContribution {
    pub(super) stable_id: String,
    pub(super) scope: PromptAssemblyScope,
    pub(super) priority: i32,
    pub(super) is_trusted: bool,
    pub(super) estimated_tokens: Option<usize>,
    pub(super) section: PromptPreludeSection,
}

impl fmt::Debug for PromptSectionContribution {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PromptSectionContribution")
            .field("stable_id", &self.stable_id)
            .field("scope", &self.scope)
            .field("priority", &self.priority)
            .field("is_trusted", &self.is_trusted)
            .field("estimated_tokens", &self.estimated_tokens)
            .field("section_kind", &self.section.kind)
            .field("section_origin", &self.section.origin)
            .finish_non_exhaustive()
    }
}

/// 提供给 composition inspection 的脱敏 contribution metadata。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PromptContributionSnapshot {
    pub(super) effective_order: usize,
    pub(super) stable_id: String,
    pub(super) kind: PromptSourceKind,
    pub(super) origin: Option<PromptSourceOrigin>,
    pub(super) scope: Option<PromptAssemblyScope>,
    pub(super) priority: i32,
    pub(super) is_trusted: bool,
    pub(super) estimated_tokens: Option<usize>,
}

/// Native Agent 与 context-budget path 消费的不可变输入。
#[derive(Clone, Default)]
pub(super) struct PromptAssemblySessionSnapshot {
    pub(super) manager: Option<PromptAssemblyManagerSnapshot>,
    pub(super) prompt_prelude: Option<PromptPreludeSnapshot>,
    pub(super) dynamic_environment_session_config: Option<DynamicEnvironmentSessionConfig>,
}

impl fmt::Debug for PromptAssemblySessionSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PromptAssemblySessionSnapshot")
            .field("has_manager", &self.manager.is_some())
            .field(
                "prelude_section_count",
                &self
                    .prompt_prelude
                    .as_ref()
                    .map_or(0, |prelude| prelude.sections.len()),
            )
            .field(
                "has_dynamic_environment_session_config",
                &self.dynamic_environment_session_config.is_some(),
            )
            .finish()
    }
}

#[derive(Default)]
pub(super) struct PromptAssembly {
    state: Arc<Mutex<PromptAssemblyState>>,
}

#[derive(Default)]
struct PromptAssemblyState {
    is_active: bool,
    manager: Option<PromptAssemblyManagerSnapshot>,
    _manager_owner: Option<PromptOwner>,
    manager_registration_id: Option<u64>,
    next_registration_id: u64,
    contributions: BTreeMap<String, PromptContributionRecord>,
}

struct PromptContributionRecord {
    id: u64,
    _owner: PromptOwner,
    contribution: PromptSectionContribution,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(super) enum PromptAssemblyError {
    #[error("prompt contribution is invalid: {reason}")]
    InvalidContribution { reason: InvalidContributionReason },
    #[error("prompt contribution {stable_id} is already registered")]
    DuplicateContribution { stable_id: String },
    #[error("prompt manager registration is disposed")]
    ManagerUnavailable,
    #[error("prompt assembly is disposed")]
    Disposed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum InvalidContributionReason {
    EmptyStableId,
    StableIdMismatch,
    InvalidManagerSection,
}

impl fmt::Display for InvalidContributionReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = match self {
            Self::EmptyStableId => "stable id is empty",
            Self::StableIdMismatch => "stable id does not match section reference id",
            Self::InvalidManagerSection => "manager section has an empty reference id",
        };
        formatter.write_str(reason)
    }
}

/// registration 的幂等逆操作；Drop 与显式 dispose 等价。
pub(super) struct PromptRegistration {
    assembly: Weak<Mutex<PromptAssemblyState>>,
    entries: Vec<PromptRegistrationEntry>,
    is_disposed: bool,
}

enum PromptRegistrationEntry {
    Manager {
        registration_id: u64,
    },
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the first production prompt contributor is introduced by a later provider slice"
        )
    )]
    Contribution {
        stable_id: String,
        registration_id: u64,
    },
}

impl PromptAssembly {
    /// 在一个 lifecycle-owned registration 下接管当前 workspace manager。
    pub(super) fn adopt_manager(
        owner: impl Into<PromptOwner>,
        manager: Option<PromptAssemblyManagerSnapshot>,
    ) -> Result<(Self, PromptRegistration), PromptAssemblyError> {
        validate_manager(manager.as_ref())?;
        let assembly = Self::default();
        let owner = owner.into();
        let registration_id = {
            let mut state = assembly
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.manager = manager;
            state._manager_owner = Some(owner);
            state.is_active = true;
            let id = next_registration_id(&mut state);
            state.manager_registration_id = Some(id);
            id
        };
        let registration = PromptRegistration::new(
            &assembly.state,
            vec![PromptRegistrationEntry::Manager { registration_id }],
        );
        Ok((assembly, registration))
    }

    /// 注册一个 runtime-owned section contribution。
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "this slice establishes the crate-private provider contract before its first production contributor"
        )
    )]
    pub(crate) fn contribute(
        &self,
        owner: impl Into<PromptOwner>,
        contribution: PromptSectionContribution,
    ) -> Result<PromptRegistration, PromptAssemblyError> {
        if contribution.stable_id.is_empty()
            || contribution.stable_id != contribution.section.reference_id
        {
            return Err(PromptAssemblyError::InvalidContribution {
                reason: if contribution.stable_id.is_empty() {
                    InvalidContributionReason::EmptyStableId
                } else {
                    InvalidContributionReason::StableIdMismatch
                },
            });
        }
        let owner = owner.into();
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.is_active {
            return Err(PromptAssemblyError::Disposed);
        }
        if state.contributions.contains_key(&contribution.stable_id)
            || manager_contains_reference_id(state.manager.as_ref(), &contribution.stable_id)
        {
            return Err(PromptAssemblyError::DuplicateContribution {
                stable_id: contribution.stable_id.clone(),
            });
        }
        let registration_id = next_registration_id(&mut state);
        let stable_id = contribution.stable_id.clone();
        state.contributions.insert(
            stable_id.clone(),
            PromptContributionRecord {
                id: registration_id,
                _owner: owner,
                contribution,
            },
        );
        drop(state);
        Ok(PromptRegistration::new(
            &self.state,
            vec![PromptRegistrationEntry::Contribution {
                stable_id,
                registration_id,
            }],
        ))
    }

    /// 在编辑持久化成功后替换 workspace manager。
    pub(super) fn validate_manager_replacement(
        &self,
        manager: Option<&PromptAssemblyManagerSnapshot>,
    ) -> Result<(), PromptAssemblyError> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        validate_manager_replacement(&state, manager)
    }

    /// 在编辑持久化成功后替换 workspace manager。
    pub(super) fn replace_manager(
        &self,
        manager: Option<PromptAssemblyManagerSnapshot>,
    ) -> Result<(), PromptAssemblyError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.is_active {
            return Err(PromptAssemblyError::Disposed);
        }
        validate_manager_replacement(&state, manager.as_ref())?;
        state.manager = manager;
        Ok(())
    }

    /// 停用当前 capability generation，并立即隐藏所有 projection。
    pub(super) fn deactivate(&self) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_active = false;
    }

    pub(super) fn manager_snapshot(&self) -> Option<PromptAssemblyManagerSnapshot> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.is_active.then(|| state.manager.clone()).flatten()
    }

    /// 返回不可变的 provider/session snapshot。runtime body 会合并进 prelude，
    /// manager DTO 仍保持既有 `/prompt` inventory projection。
    pub(super) fn session_snapshot(&self) -> PromptAssemblySessionSnapshot {
        let (manager, contributions) = {
            let state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.is_active {
                (
                    state.manager.clone(),
                    state
                        .contributions
                        .values()
                        .map(|record| record.contribution.clone())
                        .collect::<Vec<_>>(),
                )
            } else {
                (None, Vec::new())
            }
        };
        let prompt_prelude = merged_prelude(manager.as_ref(), &contributions);
        let dynamic_environment_session_config = manager
            .as_ref()
            .map(crate::prompt_assembly::dynamic_environment_session_config_from_manager);
        PromptAssemblySessionSnapshot {
            manager,
            prompt_prelude,
            dynamic_environment_session_config,
        }
    }

    pub(super) fn inspection_snapshot(&self) -> Vec<PromptContributionSnapshot> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.is_active {
            return Vec::new();
        }
        let mut negative = state
            .contributions
            .values()
            .filter(|record| record.contribution.priority < 0)
            .collect::<Vec<_>>();
        let mut positive = state
            .contributions
            .values()
            .filter(|record| record.contribution.priority >= 0)
            .collect::<Vec<_>>();
        negative
            .sort_by(|left, right| compare_contributions(&left.contribution, &right.contribution));
        positive
            .sort_by(|left, right| compare_contributions(&left.contribution, &right.contribution));

        let mut snapshots = Vec::new();
        for record in negative {
            snapshots.push(runtime_contribution_snapshot(
                snapshots.len(),
                &record.contribution,
            ));
        }
        if let Some(manager) = state.manager.as_ref() {
            for section in &manager.resolution.prelude.sections {
                snapshots.push(PromptContributionSnapshot {
                    effective_order: snapshots.len(),
                    stable_id: section.reference_id.clone(),
                    kind: section.kind,
                    origin: section.origin,
                    scope: section.origin.and_then(prompt_scope_from_origin),
                    priority: 0,
                    is_trusted: section.origin == Some(PromptSourceOrigin::Builtin),
                    estimated_tokens: None,
                });
            }
        }
        for record in positive {
            snapshots.push(runtime_contribution_snapshot(
                snapshots.len(),
                &record.contribution,
            ));
        }
        snapshots
    }
}

impl PromptRegistration {
    fn new(
        assembly: &Arc<Mutex<PromptAssemblyState>>,
        entries: Vec<PromptRegistrationEntry>,
    ) -> Self {
        Self {
            assembly: Arc::downgrade(assembly),
            entries,
            is_disposed: false,
        }
    }

    pub(super) fn dispose(&mut self) {
        if self.is_disposed {
            return;
        }
        self.is_disposed = true;
        let Some(assembly) = self.assembly.upgrade() else {
            return;
        };
        let mut state = assembly
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for entry in self.entries.drain(..) {
            match entry {
                PromptRegistrationEntry::Manager { registration_id }
                    if state.manager_registration_id == Some(registration_id) =>
                {
                    state.manager = None;
                    state._manager_owner = None;
                    state.manager_registration_id = None;
                }
                PromptRegistrationEntry::Contribution {
                    stable_id,
                    registration_id,
                } => {
                    let owns_current = state
                        .contributions
                        .get(&stable_id)
                        .is_some_and(|record| record.id == registration_id);
                    if owns_current {
                        state.contributions.remove(&stable_id);
                    }
                }
                _ => {}
            }
        }
    }
}

impl Drop for PromptRegistration {
    fn drop(&mut self) {
        self.dispose();
    }
}

fn next_registration_id(state: &mut PromptAssemblyState) -> u64 {
    let id = state.next_registration_id;
    state.next_registration_id = state
        .next_registration_id
        .checked_add(1)
        .expect("prompt registration id space should be unreachable");
    id
}

fn manager_contains_reference_id(
    manager: Option<&PromptAssemblyManagerSnapshot>,
    stable_id: &str,
) -> bool {
    manager.is_some_and(|manager| {
        manager
            .resolution
            .prelude
            .sections
            .iter()
            .any(|section| section.reference_id == stable_id)
    })
}

fn validate_manager_replacement(
    state: &PromptAssemblyState,
    manager: Option<&PromptAssemblyManagerSnapshot>,
) -> Result<(), PromptAssemblyError> {
    if !state.is_active {
        return Err(PromptAssemblyError::Disposed);
    }
    if state.manager_registration_id.is_none() {
        return Err(PromptAssemblyError::ManagerUnavailable);
    }
    validate_manager(manager)?;
    if let Some(manager) = manager
        && let Some(stable_id) = state
            .contributions
            .keys()
            .find(|stable_id| manager_contains_reference_id(Some(manager), stable_id))
    {
        return Err(PromptAssemblyError::DuplicateContribution {
            stable_id: stable_id.clone(),
        });
    }
    Ok(())
}

fn validate_manager(
    manager: Option<&PromptAssemblyManagerSnapshot>,
) -> Result<(), PromptAssemblyError> {
    let Some(manager) = manager else {
        return Ok(());
    };
    let mut stable_ids = BTreeSet::new();
    for section in &manager.resolution.prelude.sections {
        if section.reference_id.is_empty() {
            return Err(PromptAssemblyError::InvalidContribution {
                reason: InvalidContributionReason::InvalidManagerSection,
            });
        }
        if !stable_ids.insert(section.reference_id.as_str()) {
            return Err(PromptAssemblyError::DuplicateContribution {
                stable_id: section.reference_id.clone(),
            });
        }
    }
    Ok(())
}

fn merged_prelude(
    manager: Option<&PromptAssemblyManagerSnapshot>,
    contributions: &[PromptSectionContribution],
) -> Option<PromptPreludeSnapshot> {
    let mut negative = contributions
        .iter()
        .filter(|contribution| contribution.priority < 0)
        .collect::<Vec<_>>();
    let mut positive = contributions
        .iter()
        .filter(|contribution| contribution.priority >= 0)
        .collect::<Vec<_>>();
    let compare = |left: &&PromptSectionContribution, right: &&PromptSectionContribution| {
        compare_contributions(left, right)
    };
    negative.sort_by(compare);
    positive.sort_by(compare);

    let mut sections = negative
        .into_iter()
        .map(|contribution| contribution.section.clone())
        .collect::<Vec<_>>();
    if let Some(manager) = manager {
        sections.extend(manager.resolution.prelude.sections.iter().cloned());
    }
    sections.extend(
        positive
            .into_iter()
            .map(|contribution| contribution.section.clone()),
    );
    if sections.is_empty() && manager.is_none() {
        None
    } else {
        Some(PromptPreludeSnapshot { sections })
    }
}

const fn scope_rank(scope: PromptAssemblyScope) -> u8 {
    match scope {
        PromptAssemblyScope::Global => 0,
        PromptAssemblyScope::Project => 1,
    }
}

fn compare_contributions(
    left: &PromptSectionContribution,
    right: &PromptSectionContribution,
) -> std::cmp::Ordering {
    left.priority
        .cmp(&right.priority)
        .then_with(|| scope_rank(left.scope).cmp(&scope_rank(right.scope)))
        .then_with(|| left.stable_id.cmp(&right.stable_id))
}

fn runtime_contribution_snapshot(
    effective_order: usize,
    contribution: &PromptSectionContribution,
) -> PromptContributionSnapshot {
    PromptContributionSnapshot {
        effective_order,
        stable_id: contribution.stable_id.clone(),
        kind: contribution.section.kind,
        origin: contribution.section.origin,
        scope: Some(contribution.scope),
        priority: contribution.priority,
        is_trusted: contribution.is_trusted,
        estimated_tokens: contribution.estimated_tokens,
    }
}

const fn prompt_scope_from_origin(origin: PromptSourceOrigin) -> Option<PromptAssemblyScope> {
    match origin {
        PromptSourceOrigin::Global => Some(PromptAssemblyScope::Global),
        PromptSourceOrigin::Project => Some(PromptAssemblyScope::Project),
        PromptSourceOrigin::Builtin => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::effect_scope::EffectScope;

    fn manager(section: &str) -> PromptAssemblyManagerSnapshot {
        let mut manager = PromptAssemblyManagerSnapshot::default();
        manager.resolution.prelude.sections = vec![PromptPreludeSection {
            reference_id: section.to_string(),
            kind: PromptSourceKind::InstructionsFile,
            title: section.to_string(),
            origin: Some(PromptSourceOrigin::Project),
            body: "workspace instruction".to_string(),
        }];
        manager
    }

    fn contribution(
        id: &str,
        priority: i32,
        scope: PromptAssemblyScope,
        body: &str,
    ) -> PromptSectionContribution {
        PromptSectionContribution {
            stable_id: id.to_string(),
            scope,
            priority,
            is_trusted: false,
            estimated_tokens: Some(3),
            section: PromptPreludeSection {
                reference_id: id.to_string(),
                kind: PromptSourceKind::ExtraPrompt,
                title: id.to_string(),
                origin: Some(PromptSourceOrigin::Builtin),
                body: body.to_string(),
            },
        }
    }

    fn section_ids(snapshot: &PromptAssemblySessionSnapshot) -> Vec<&str> {
        snapshot
            .prompt_prelude
            .as_ref()
            .map(|prelude| {
                prelude
                    .sections
                    .iter()
                    .map(|section| section.reference_id.as_str())
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn contribution_order_keeps_manager_sections_between_priority_bands() {
        let (assembly, _manager_registration) =
            PromptAssembly::adopt_manager("workspace-owner", Some(manager("workspace")))
                .expect("manager should be adopted");
        let _late = assembly
            .contribute(
                "late-owner",
                contribution("late", 1, PromptAssemblyScope::Project, "late"),
            )
            .expect("late contribution should register");
        let _early = assembly
            .contribute(
                "early-owner",
                contribution("early", -1, PromptAssemblyScope::Global, "early"),
            )
            .expect("early contribution should register");

        let ids = assembly
            .session_snapshot()
            .prompt_prelude
            .expect("manager should provide a prelude")
            .sections
            .into_iter()
            .map(|section| section.reference_id)
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["early", "workspace", "late"]);
    }

    #[test]
    fn duplicate_ids_are_rejected_without_mutating_the_previous_state() {
        let (assembly, _manager_registration) =
            PromptAssembly::adopt_manager("workspace-owner", Some(manager("workspace")))
                .expect("manager should be adopted");
        let error = match assembly.contribute(
            "owner",
            contribution("workspace", 0, PromptAssemblyScope::Global, "new"),
        ) {
            Ok(_) => panic!("manager section id must reserve the stable id"),
            Err(error) => error,
        };
        assert_eq!(
            error,
            PromptAssemblyError::DuplicateContribution {
                stable_id: "workspace".to_string(),
            }
        );
        assert_eq!(
            assembly
                .session_snapshot()
                .prompt_prelude
                .expect("manager should remain")
                .sections[0]
                .body,
            "workspace instruction"
        );
    }

    #[test]
    fn duplicate_runtime_ids_are_rejected_without_replacing_the_first_owner() {
        let (assembly, _manager_registration) =
            PromptAssembly::adopt_manager("workspace-owner", None)
                .expect("empty manager should be adopted");
        let _first = assembly
            .contribute(
                "first-owner",
                contribution("shared", 0, PromptAssemblyScope::Global, "first"),
            )
            .expect("first contribution should register");

        let error = match assembly.contribute(
            "second-owner",
            contribution("shared", 0, PromptAssemblyScope::Project, "second"),
        ) {
            Ok(_) => panic!("duplicate runtime id must be rejected"),
            Err(error) => error,
        };

        assert_eq!(
            error,
            PromptAssemblyError::DuplicateContribution {
                stable_id: "shared".to_string(),
            }
        );
        assert_eq!(
            assembly
                .session_snapshot()
                .prompt_prelude
                .expect("first contribution should remain")
                .sections[0]
                .body,
            "first"
        );
    }

    #[test]
    fn manager_adoption_rejects_duplicate_reference_ids_before_activation() {
        let mut duplicate_manager = manager("duplicate");
        duplicate_manager
            .resolution
            .prelude
            .sections
            .push(PromptPreludeSection {
                reference_id: "duplicate".to_string(),
                kind: PromptSourceKind::ExtraPrompt,
                title: "duplicate title".to_string(),
                origin: Some(PromptSourceOrigin::Global),
                body: "duplicate body".to_string(),
            });

        let error = match PromptAssembly::adopt_manager("workspace-owner", Some(duplicate_manager))
        {
            Ok(_) => panic!("duplicate manager ids must be rejected"),
            Err(error) => error,
        };

        assert_eq!(
            error,
            PromptAssemblyError::DuplicateContribution {
                stable_id: "duplicate".to_string(),
            }
        );
    }

    #[test]
    fn manager_replacement_collision_is_transactional() {
        let (assembly, _manager_registration) =
            PromptAssembly::adopt_manager("workspace-owner", Some(manager("old-manager")))
                .expect("manager should be adopted");
        let _runtime = assembly
            .contribute(
                "runtime-owner",
                contribution("runtime", 1, PromptAssemblyScope::Global, "runtime body"),
            )
            .expect("runtime contribution should register");

        let error = assembly
            .replace_manager(Some(manager("runtime")))
            .expect_err("replacement must reject an active runtime id collision");

        assert_eq!(
            error,
            PromptAssemblyError::DuplicateContribution {
                stable_id: "runtime".to_string(),
            }
        );
        assert_eq!(
            section_ids(&assembly.session_snapshot()),
            vec!["old-manager", "runtime"]
        );
    }

    #[test]
    fn registration_dispose_and_drop_are_idempotent_and_owner_isolated() {
        let (assembly, _manager_registration) =
            PromptAssembly::adopt_manager("workspace-owner", None)
                .expect("empty manager should be adopted");
        let mut first = assembly
            .contribute(
                "first",
                contribution("same", 0, PromptAssemblyScope::Global, "first"),
            )
            .expect("first should register");
        first.dispose();
        let _fresh = assembly
            .contribute(
                "fresh",
                contribution("same", 0, PromptAssemblyScope::Global, "fresh"),
            )
            .expect("fresh should register after disposal");
        first.dispose();
        assert_eq!(
            assembly
                .session_snapshot()
                .prompt_prelude
                .expect("fresh contribution should produce a prelude")
                .sections[0]
                .body,
            "fresh"
        );
    }

    #[test]
    fn dropping_runtime_registration_runs_the_inverse() {
        let (assembly, _manager_registration) =
            PromptAssembly::adopt_manager("workspace-owner", None)
                .expect("empty manager should be adopted");
        {
            let _runtime = assembly
                .contribute(
                    "runtime-owner",
                    contribution("runtime", 0, PromptAssemblyScope::Global, "runtime body"),
                )
                .expect("runtime contribution should register");
            assert_eq!(section_ids(&assembly.session_snapshot()), vec!["runtime"]);
        }

        assert!(assembly.session_snapshot().prompt_prelude.is_none());
    }

    #[test]
    fn dropping_manager_registration_removes_only_the_manager_slot() {
        let (assembly, manager_registration) =
            PromptAssembly::adopt_manager("workspace-owner", Some(manager("manager")))
                .expect("manager should be adopted");
        let mut contribution_registration = assembly
            .contribute(
                "runtime-owner",
                contribution("runtime", 0, PromptAssemblyScope::Global, "runtime body"),
            )
            .expect("runtime contribution should register");
        drop(manager_registration);

        assert!(assembly.manager_snapshot().is_none());
        assert_eq!(section_ids(&assembly.session_snapshot()), vec!["runtime"]);
        assert_eq!(
            assembly.replace_manager(Some(manager("replacement"))),
            Err(PromptAssemblyError::ManagerUnavailable)
        );
        contribution_registration.dispose();
        assert!(assembly.session_snapshot().prompt_prelude.is_none());
    }

    #[test]
    fn runtime_ties_sort_by_priority_scope_then_stable_id() {
        let (assembly, _manager_registration) =
            PromptAssembly::adopt_manager("workspace-owner", Some(manager("manager")))
                .expect("manager should be adopted");
        let mut registrations = Vec::new();
        for (id, priority, scope) in [
            ("project-b", -2, PromptAssemblyScope::Project),
            ("global-b", -2, PromptAssemblyScope::Global),
            ("global-a", -2, PromptAssemblyScope::Global),
            ("positive", 0, PromptAssemblyScope::Global),
            ("earliest", -3, PromptAssemblyScope::Project),
        ] {
            registrations.push(
                assembly
                    .contribute("runtime-owner", contribution(id, priority, scope, id))
                    .expect("runtime contribution should register"),
            );
        }

        assert_eq!(
            section_ids(&assembly.session_snapshot()),
            vec![
                "earliest",
                "global-a",
                "global-b",
                "project-b",
                "manager",
                "positive",
            ]
        );
    }

    #[test]
    fn inspection_order_matches_the_effective_session_prelude() {
        let (assembly, _manager_registration) =
            PromptAssembly::adopt_manager("workspace-owner", Some(manager("manager")))
                .expect("manager should be adopted");
        let _early = assembly
            .contribute(
                "runtime-owner",
                contribution("early", -1, PromptAssemblyScope::Global, "early body"),
            )
            .expect("early contribution should register");
        let _late = assembly
            .contribute(
                "runtime-owner",
                contribution("late", 1, PromptAssemblyScope::Project, "late body"),
            )
            .expect("late contribution should register");

        let session_snapshot = assembly.session_snapshot();
        let inspection = assembly.inspection_snapshot();

        assert_eq!(
            inspection
                .iter()
                .map(|source| source.effective_order)
                .collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert_eq!(
            inspection
                .iter()
                .map(|source| source.stable_id.as_str())
                .collect::<Vec<_>>(),
            section_ids(&session_snapshot)
        );
    }

    #[test]
    fn inspection_redacts_owner_registration_and_body() {
        let (assembly, _manager_registration) =
            PromptAssembly::adopt_manager("workspace-owner", None)
                .expect("empty manager should be adopted");
        let _registration = assembly
            .contribute(
                "private-owner",
                contribution(
                    "private-section",
                    0,
                    PromptAssemblyScope::Project,
                    "SECRET_BODY",
                ),
            )
            .expect("contribution should register");
        let debug = format!("{:?}", assembly.inspection_snapshot());
        assert!(debug.contains("private-section"));
        assert!(!debug.contains("private-owner"));
        assert!(!debug.contains("SECRET_BODY"));
    }

    #[test]
    fn manager_inspection_and_debug_views_redact_delivery_text_and_owner_identity() {
        const BODY_SENTINEL: &str = "SECRET_MANAGER_BODY";
        const TITLE_SENTINEL: &str = "SECRET_MANAGER_TITLE";
        const OWNER_SENTINEL: &str = "SECRET_MANAGER_OWNER";
        let mut private_manager = manager("manager-stable-id");
        private_manager.resolution.prelude.sections[0].body = BODY_SENTINEL.to_string();
        private_manager.resolution.prelude.sections[0].title = TITLE_SENTINEL.to_string();
        let (assembly, _manager_registration) =
            PromptAssembly::adopt_manager(OWNER_SENTINEL, Some(private_manager))
                .expect("manager should be adopted");

        let inspection_debug = format!("{:?}", assembly.inspection_snapshot());
        assert!(inspection_debug.contains("manager-stable-id"));
        for sentinel in [BODY_SENTINEL, TITLE_SENTINEL, OWNER_SENTINEL] {
            assert!(!inspection_debug.contains(sentinel));
        }

        let contribution_debug = format!(
            "{:?}",
            contribution(
                "runtime-stable-id",
                0,
                PromptAssemblyScope::Global,
                BODY_SENTINEL,
            )
        );
        let session_debug = format!("{:?}", assembly.session_snapshot());
        assert!(!contribution_debug.contains(BODY_SENTINEL));
        assert!(!contribution_debug.contains(TITLE_SENTINEL));
        assert!(!session_debug.contains(BODY_SENTINEL));
        assert!(!session_debug.contains(TITLE_SENTINEL));
    }

    #[test]
    fn invalid_stable_ids_are_rejected_without_leaking_control_or_delivery_data() {
        let (assembly, _manager_registration) =
            PromptAssembly::adopt_manager("private-owner", None)
                .expect("empty manager should be adopted");
        let mut mismatched =
            contribution("stable-id", 0, PromptAssemblyScope::Global, "SECRET_BODY");
        mismatched.section.reference_id = "different-id".to_string();

        let error = match assembly.contribute("private-owner", mismatched) {
            Ok(_) => panic!("stable id must match the section reference id"),
            Err(error) => error,
        };
        let debug = format!("{error:?}");
        let display = error.to_string();

        assert_eq!(
            error,
            PromptAssemblyError::InvalidContribution {
                reason: InvalidContributionReason::StableIdMismatch,
            }
        );
        for sentinel in ["private-owner", "stable-id", "different-id", "SECRET_BODY"] {
            assert!(!debug.contains(sentinel));
            assert!(!display.contains(sentinel));
        }
    }

    #[test]
    fn effect_scope_disposal_removes_runtime_contributions_and_manager_state() {
        let scope = EffectScope::default();
        let (assembly, mut manager_registration) =
            PromptAssembly::adopt_manager("workspace-owner", Some(manager("manager")))
                .expect("manager should be adopted");
        let mut runtime_registration = assembly
            .contribute(
                "runtime-owner",
                contribution("runtime", 0, PromptAssemblyScope::Global, "runtime body"),
            )
            .expect("runtime contribution should register");
        scope
            .register("prompt-manager", move || {
                manager_registration.dispose();
                Ok(())
            })
            .expect("manager inverse should register");
        scope
            .register("prompt-runtime", move || {
                runtime_registration.dispose();
                Ok(())
            })
            .expect("runtime inverse should register");

        assert_eq!(
            section_ids(&assembly.session_snapshot()),
            vec!["manager", "runtime"]
        );
        assert!(scope.dispose().failures.is_empty());
        assert!(assembly.manager_snapshot().is_none());
        assert!(assembly.inspection_snapshot().is_empty());
        assert!(assembly.session_snapshot().prompt_prelude.is_none());
    }

    #[test]
    fn dropping_assembly_makes_registration_a_noop() {
        let (assembly, mut registration) = PromptAssembly::adopt_manager("workspace-owner", None)
            .expect("empty manager should be adopted");
        drop(assembly);
        registration.dispose();
    }

    #[test]
    fn deactivating_capability_hides_projections_without_stealing_contribution_inverse() {
        let (assembly, _manager_registration) =
            PromptAssembly::adopt_manager("workspace-owner", None)
                .expect("empty manager should be adopted");
        let mut section_registration = assembly
            .contribute(
                "owner",
                contribution("section", 0, PromptAssemblyScope::Global, "body"),
            )
            .expect("section should register");

        assembly.deactivate();

        assert!(assembly.session_snapshot().prompt_prelude.is_none());
        assert!(assembly.inspection_snapshot().is_empty());
        assert_eq!(
            assembly.replace_manager(Some(manager("replacement"))),
            Err(PromptAssemblyError::Disposed)
        );
        assert!(matches!(
            assembly.contribute(
                "owner",
                contribution("replacement", 0, PromptAssemblyScope::Global, "body")
            ),
            Err(PromptAssemblyError::Disposed)
        ));
        section_registration.dispose();
        section_registration.dispose();
    }
}
