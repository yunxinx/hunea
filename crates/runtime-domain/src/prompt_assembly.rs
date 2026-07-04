use std::collections::BTreeMap;
use std::{cmp::Ordering, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::dynamic_environment::{DynamicEnvironmentSnapshotKind, DynamicEnvironmentSourceKind};

pub mod persistence;
use persistence::PromptAssemblyScope;

/// `PromptAssemblyLifecycle` 表示 prompt assembly 生效的生命周期边界。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptAssemblyLifecycle {
    /// 仅影响下一次全新 session 的 prompt 装配，不回写当前 transcript。
    NextNewSession,
}

/// `PromptSourceKind` 表示 prompt source 的领域类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptSourceKind {
    CoreSystemPrompt,
    InstructionsFile,
    ExtraPrompt,
    SkillDiscovery,
    /// 长期注入型 skill，和当前消息里的 `$skill` 临时注入不同。
    LongLivedSkill,
    /// 工具使用指南，body 从工具注册表动态生成。
    ToolGuidelines,
    /// 首轮注入的环境基线。
    DynamicEnvironmentBaseline,
    /// 后续轮次按变化注入的环境差异。
    DynamicEnvironmentChanges,
}

/// `PromptSourceOrigin` 表示 prompt source 的来源层级。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptSourceOrigin {
    Builtin,
    Global,
    Project,
}

/// `PromptSourceInactiveReason` 表示 source 为何没有进入 active assembly。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptSourceInactiveReason {
    Disabled,
    Missing,
    Shadowed,
}

/// `PromptSourceStatus` 表示 source 当前是 active 还是 inactive。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptSourceStatus {
    Active { order: usize },
    Inactive { reason: PromptSourceInactiveReason },
}

/// `CoreSystemPromptInput` 描述 core system prompt 的 override 输入。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CoreSystemPromptInput {
    pub global_override_present: bool,
    pub project_override_present: bool,
}

/// `PromptSourceCandidate` 描述参与 resolution 的非 core prompt source。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptSourceCandidate {
    pub reference_id: String,
    pub kind: PromptSourceKind,
    pub title: String,
    pub origin: Option<PromptSourceOrigin>,
    pub collision_key: Option<String>,
    pub enabled: bool,
    pub resolvable: bool,
    pub requested_order: Option<u16>,
}

/// `PromptAssemblyInput` 描述一次 next-new-session prompt assembly resolution 的输入。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PromptAssemblyInput {
    pub core_system: CoreSystemPromptInput,
    pub candidates: Vec<PromptSourceCandidate>,
}

/// `ResolvedPromptSource` 表示 resolution 之后的 prompt source。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPromptSource {
    pub reference_id: String,
    pub kind: PromptSourceKind,
    pub title: String,
    pub origin: Option<PromptSourceOrigin>,
    pub status: PromptSourceStatus,
}

/// `PromptAssemblySnapshot` 表示共享 prompt assembly resolution 结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptAssemblySnapshot {
    pub lifecycle: PromptAssemblyLifecycle,
    pub active_sources: Vec<ResolvedPromptSource>,
    pub inactive_sources: Vec<ResolvedPromptSource>,
}

/// `PromptPreludeSection` 表示落入单个 session 的稳定 prompt prelude section。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptPreludeSection {
    pub reference_id: String,
    pub kind: PromptSourceKind,
    pub title: String,
    pub origin: Option<PromptSourceOrigin>,
    pub body: String,
}

/// `PromptPreludeSnapshot` 表示某个 session 启动时已经解析完成的 prompt prelude。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PromptPreludeSnapshot {
    pub sections: Vec<PromptPreludeSection>,
}

impl PromptPreludeSnapshot {
    /// `effective_system_prompt` 返回 provider 实际看到的拼装结果。
    #[must_use]
    pub fn effective_system_prompt(&self) -> Option<String> {
        let sections = self
            .sections
            .iter()
            .map(|section| section.body.trim())
            .filter(|body| !body.is_empty())
            .collect::<Vec<_>>();
        (!sections.is_empty()).then(|| sections.join("\n\n"))
    }
}

/// `PromptAssemblyManagerSource` 是 `/prompt` 管理器使用的可预览 source 物化视图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptAssemblyManagerSource {
    pub reference_id: String,
    pub kind: PromptSourceKind,
    pub title: String,
    pub origin: Option<PromptSourceOrigin>,
    pub resolved_body_origin: Option<PromptSourceOrigin>,
    pub backing_file_path: Option<PathBuf>,
    pub body: Option<String>,
}

/// `PromptAssemblyManagedSource` 表示 `/prompt` 左侧管理列表的一行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptAssemblyManagedSource {
    pub reference_id: String,
    pub kind: PromptSourceKind,
    pub title: String,
    pub origin: Option<PromptSourceOrigin>,
    pub scope: Option<PromptAssemblyScope>,
    pub enabled: bool,
    pub order: usize,
}

/// `PromptAssemblyExtraPromptCandidate` 表示 `/prompt` 右侧 Extra tab 的候选 prompt。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptAssemblyExtraPromptCandidate {
    pub reference_id: String,
    pub title: String,
    pub origin: PromptSourceOrigin,
    pub body: String,
    pub selected: bool,
}

/// `PromptAssemblyDiscoveredSkill` 表示 `/prompt` 右侧 Skills tab 可展示的已发现 skill。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptAssemblyDiscoveredSkill {
    pub skill_name: String,
    pub title: String,
    pub description: String,
    pub origin: PromptSourceOrigin,
    pub selection_scope: PromptAssemblyScope,
    pub skill_path: String,
    pub body: String,
    pub can_select_for_discovery: bool,
    pub selected: bool,
    pub selected_order: Option<usize>,
}

/// `PromptAssemblyToolCandidate` 表示 `/prompt` Tools Tab 中的一个工具候选项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptAssemblyToolCandidate {
    pub name: String,
    pub label: Option<String>,
    pub description: Option<String>,
    pub prompt_guidelines: Option<String>,
    pub origin: PromptSourceOrigin,
    pub selection_scope: PromptAssemblyScope,
    pub can_select: bool,
    pub selected: bool,
    pub selected_order: Option<usize>,
}

/// `PromptAssemblyDynamicEnvironmentCandidate` 表示 Dynamic Tab 中的一个环境来源开关。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptAssemblyDynamicEnvironmentCandidate {
    pub source_kind: DynamicEnvironmentSourceKind,
    pub label: String,
    pub origin: PromptSourceOrigin,
    pub baseline_selected: bool,
    pub changes_selected: bool,
    pub baseline_preview_body: String,
    pub changes_preview_body: String,
}

/// `PromptAssemblyManagerSnapshot` 表示 `/prompt` 所需的完整只读快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptAssemblyManagerSnapshot {
    pub snapshot: PromptAssemblySnapshot,
    pub prelude: PromptPreludeSnapshot,
    pub managed_sources: Vec<PromptAssemblyManagedSource>,
    pub sources: Vec<PromptAssemblyManagerSource>,
    pub extra_prompt_candidates: Vec<PromptAssemblyExtraPromptCandidate>,
    pub discovered_skills: Vec<PromptAssemblyDiscoveredSkill>,
    pub manual_skills: Vec<PromptAssemblyDiscoveredSkill>,
    pub tool_candidates: Vec<PromptAssemblyToolCandidate>,
    pub dynamic_environment_candidates: Vec<PromptAssemblyDynamicEnvironmentCandidate>,
    pub builtin_core_system_body: String,
    pub global_core_system_override: Option<String>,
    pub project_core_system_override: Option<String>,
}

/// `PromptAssemblyEditorTarget` 标识一次外部编辑器保存要落到哪里。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptAssemblyEditorTarget {
    CoreSystemOverride {
        scope: PromptAssemblyScope,
    },
    SkillDiscovery {
        scope: PromptAssemblyScope,
    },
    ToolGuidelines {
        scope: PromptAssemblyScope,
    },
    InstructionsFile {
        path: PathBuf,
    },
    ExtraPrompt {
        scope: PromptAssemblyScope,
        reference_id: String,
    },
    SkillFile {
        skill_name: String,
        origin: PromptSourceOrigin,
    },
}

/// `PromptAssemblyMutation` 描述 `/prompt` 发起的一次持久化变更。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptAssemblyMutation {
    SaveEditorTarget {
        target: PromptAssemblyEditorTarget,
        content: String,
    },
    SetExtraPromptSelected {
        scope: PromptAssemblyScope,
        reference_id: String,
        selected: bool,
    },
    SetPromptSourceEnabled {
        scope: PromptAssemblyScope,
        kind: PromptSourceKind,
        reference_id: String,
        enabled: bool,
    },
    SetDiscoveredSkillSelected {
        scope: PromptAssemblyScope,
        skill_name: String,
        selected: bool,
    },
    MoveDiscoveredSkill {
        scope: PromptAssemblyScope,
        skill_name: String,
        direction: PromptAssemblyMoveDirection,
    },
    ResetDiscoveredSkillOrder {
        scope: PromptAssemblyScope,
    },
    SetToolSelected {
        scope: PromptAssemblyScope,
        tool_name: String,
        selected: bool,
    },
    SetDynamicEnvironmentSourceSelected {
        snapshot_kind: DynamicEnvironmentSnapshotKind,
        source_kind: DynamicEnvironmentSourceKind,
        selected: bool,
    },
    MoveTool {
        scope: PromptAssemblyScope,
        tool_name: String,
        direction: PromptAssemblyMoveDirection,
    },
    ActivateLongLivedSkill {
        scope: PromptAssemblyScope,
        skill_name: String,
    },
    CreateExtraPrompt {
        scope: PromptAssemblyScope,
        content: String,
    },
    RemovePromptSource {
        scope: PromptAssemblyScope,
        kind: PromptSourceKind,
        reference_id: String,
    },
    MoveActiveSource {
        scope: PromptAssemblyScope,
        kind: PromptSourceKind,
        reference_id: String,
        direction: PromptAssemblyMoveDirection,
    },
    DeleteExtraPrompt {
        scope: PromptAssemblyScope,
        reference_id: String,
    },
    RestoreCoreSystemOverride {
        scope: PromptAssemblyScope,
    },
}

/// `PromptAssemblyMoveDirection` 描述 active source 的排序方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptAssemblyMoveDirection {
    Up,
    Down,
}

/// `derive_extra_prompt_title` 从 prompt body 提取列表展示标题。
#[must_use]
pub fn derive_extra_prompt_title(body: &str, fallback: &str) -> String {
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(heading) = trimmed.strip_prefix('#') {
            let title = heading.trim_start_matches('#').trim();
            if !title.is_empty() {
                return truncate_extra_prompt_title(title);
            }
        }
        return truncate_extra_prompt_title(trimmed);
    }
    truncate_extra_prompt_title(fallback)
}

/// `next_default_extra_prompt_title` 为默认新建 prompt 生成递增标题。
#[must_use]
pub fn next_default_extra_prompt_title<'a>(titles: impl IntoIterator<Item = &'a str>) -> String {
    const DEFAULT_TITLE_PREFIX: &str = "New prompt";

    let next_index = titles
        .into_iter()
        .filter_map(default_extra_prompt_title_index)
        .max()
        .unwrap_or(0)
        .saturating_add(1);

    format!("{DEFAULT_TITLE_PREFIX} {next_index}")
}

/// `default_extra_prompt_body` 返回新建 extra prompt 的默认正文模板。
#[must_use]
pub fn default_extra_prompt_body(title: &str) -> String {
    format!("# {title}\n")
}

fn truncate_extra_prompt_title(title: &str) -> String {
    const TITLE_LIMIT: usize = 80;
    let mut result = String::new();
    for character in title.chars().take(TITLE_LIMIT) {
        result.push(character);
    }
    result
}

fn default_extra_prompt_title_index(title: &str) -> Option<usize> {
    const DEFAULT_TITLE_PREFIX: &str = "New prompt";

    if title == DEFAULT_TITLE_PREFIX {
        return Some(1);
    }

    let suffix = title
        .strip_prefix(DEFAULT_TITLE_PREFIX)?
        .strip_prefix(' ')?;
    suffix.parse::<usize>().ok().filter(|index| *index > 0)
}

/// `resolve_prompt_assembly` 解析 next-new-session prompt assembly。
#[must_use]
pub fn resolve_prompt_assembly(input: &PromptAssemblyInput) -> PromptAssemblySnapshot {
    let mut active_sources = vec![ResolvedPromptSource {
        reference_id: "core-system".to_string(),
        kind: PromptSourceKind::CoreSystemPrompt,
        title: "Core system prompt".to_string(),
        origin: Some(resolve_core_system_origin(&input.core_system)),
        status: PromptSourceStatus::Active { order: 0 },
    }];
    let mut inactive_sources = Vec::new();
    let mut grouped_candidates: BTreeMap<String, Vec<PromptSourceCandidate>> = BTreeMap::new();
    let mut passthrough_candidates = Vec::new();

    for candidate in &input.candidates {
        if let Some(collision_key) = candidate.collision_key.as_ref() {
            grouped_candidates
                .entry(collision_key.clone())
                .or_default()
                .push(candidate.clone());
        } else {
            passthrough_candidates.push(candidate.clone());
        }
    }

    let mut active_candidates = Vec::new();
    for candidates in grouped_candidates.into_values() {
        let winner_index = collision_winner_index(&candidates);
        for (index, candidate) in candidates.into_iter().enumerate() {
            match inactive_reason_for_candidate(&candidate, winner_index == Some(index)) {
                Some(reason) => inactive_sources.push(resolved_inactive(candidate, reason)),
                None => active_candidates.push(candidate),
            }
        }
    }

    for candidate in passthrough_candidates {
        match inactive_reason_for_candidate(&candidate, true) {
            Some(reason) => inactive_sources.push(resolved_inactive(candidate, reason)),
            None => active_candidates.push(candidate),
        }
    }

    active_candidates.sort_by(active_candidate_order);
    for (index, candidate) in active_candidates.into_iter().enumerate() {
        active_sources.push(ResolvedPromptSource {
            reference_id: candidate.reference_id,
            kind: candidate.kind,
            title: candidate.title,
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

fn collision_winner_index(candidates: &[PromptSourceCandidate]) -> Option<usize> {
    candidates
        .iter()
        .enumerate()
        .filter(|(_, candidate)| candidate.enabled)
        .min_by(|(_, left), (_, right)| collision_priority(left).cmp(&collision_priority(right)))
        .map(|(index, _)| index)
}

fn collision_priority(candidate: &PromptSourceCandidate) -> (u8, &str) {
    let scope_rank = match candidate.origin {
        Some(PromptSourceOrigin::Project) => 0,
        Some(PromptSourceOrigin::Global) => 1,
        Some(PromptSourceOrigin::Builtin) | None => 2,
    };
    (scope_rank, candidate.reference_id.as_str())
}

fn inactive_reason_for_candidate(
    candidate: &PromptSourceCandidate,
    is_collision_winner: bool,
) -> Option<PromptSourceInactiveReason> {
    if !candidate.enabled {
        return Some(PromptSourceInactiveReason::Disabled);
    }
    if !is_collision_winner {
        return Some(PromptSourceInactiveReason::Shadowed);
    }
    if !candidate.resolvable {
        return Some(PromptSourceInactiveReason::Missing);
    }
    None
}

fn resolved_inactive(
    candidate: PromptSourceCandidate,
    reason: PromptSourceInactiveReason,
) -> ResolvedPromptSource {
    ResolvedPromptSource {
        reference_id: candidate.reference_id,
        kind: candidate.kind,
        title: candidate.title,
        origin: candidate.origin,
        status: PromptSourceStatus::Inactive { reason },
    }
}

fn active_candidate_order(left: &PromptSourceCandidate, right: &PromptSourceCandidate) -> Ordering {
    let left_order = left.requested_order.unwrap_or(u16::MAX);
    let right_order = right.requested_order.unwrap_or(u16::MAX);
    left_order
        .cmp(&right_order)
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

/// `natural_sort_text_cmp` 以更贴近人类直觉的方式比较文本中的数字片段。
#[must_use]
pub fn natural_sort_text_cmp(left: &str, right: &str) -> Ordering {
    let mut left_chars = left.chars().peekable();
    let mut right_chars = right.chars().peekable();

    loop {
        match (left_chars.peek().copied(), right_chars.peek().copied()) {
            (Some(left_char), Some(right_char))
                if left_char.is_ascii_digit() && right_char.is_ascii_digit() =>
            {
                let left_number = take_ascii_digit_run(&mut left_chars);
                let right_number = take_ascii_digit_run(&mut right_chars);
                let digit_cmp = left_number
                    .trim_start_matches('0')
                    .len()
                    .cmp(&right_number.trim_start_matches('0').len());
                if digit_cmp != Ordering::Equal {
                    return digit_cmp;
                }
                let value_cmp = left_number
                    .trim_start_matches('0')
                    .cmp(right_number.trim_start_matches('0'));
                if value_cmp != Ordering::Equal {
                    return value_cmp;
                }
                let zero_padding_cmp = left_number.len().cmp(&right_number.len());
                if zero_padding_cmp != Ordering::Equal {
                    return zero_padding_cmp;
                }
            }
            (Some(left_char), Some(right_char)) => {
                let normalized_left = left_char.to_ascii_lowercase();
                let normalized_right = right_char.to_ascii_lowercase();
                let char_cmp = normalized_left.cmp(&normalized_right);
                if char_cmp != Ordering::Equal {
                    return char_cmp;
                }
                left_chars.next();
                right_chars.next();
            }
            (Some(_), None) => return Ordering::Greater,
            (None, Some(_)) => return Ordering::Less,
            (None, None) => return Ordering::Equal,
        }
    }
}

fn take_ascii_digit_run<I>(chars: &mut std::iter::Peekable<I>) -> String
where
    I: Iterator<Item = char>,
{
    let mut digits = String::new();
    while let Some(character) = chars.peek().copied() {
        if !character.is_ascii_digit() {
            break;
        }
        digits.push(character);
        chars.next();
    }
    digits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extra_prompt(
        reference_id: &str,
        title: &str,
        origin: PromptSourceOrigin,
        requested_order: Option<u16>,
    ) -> PromptSourceCandidate {
        PromptSourceCandidate {
            reference_id: reference_id.to_string(),
            kind: PromptSourceKind::ExtraPrompt,
            title: title.to_string(),
            origin: Some(origin),
            collision_key: Some(reference_id.to_string()),
            enabled: true,
            resolvable: true,
            requested_order,
        }
    }

    fn long_lived_skill(
        reference_id: &str,
        title: &str,
        origin: PromptSourceOrigin,
        collision_key: &str,
        requested_order: Option<u16>,
    ) -> PromptSourceCandidate {
        PromptSourceCandidate {
            reference_id: reference_id.to_string(),
            kind: PromptSourceKind::LongLivedSkill,
            title: title.to_string(),
            origin: Some(origin),
            collision_key: Some(collision_key.to_string()),
            enabled: true,
            resolvable: true,
            requested_order,
        }
    }

    fn skill_discovery(requested_order: Option<u16>) -> PromptSourceCandidate {
        PromptSourceCandidate {
            reference_id: "skill-discovery".to_string(),
            kind: PromptSourceKind::SkillDiscovery,
            title: "Skill discovery source".to_string(),
            origin: None,
            collision_key: None,
            enabled: true,
            resolvable: true,
            requested_order,
        }
    }

    #[test]
    fn resolution_is_scoped_to_next_new_session_and_keeps_core_first() {
        let snapshot = resolve_prompt_assembly(&PromptAssemblyInput {
            core_system: CoreSystemPromptInput::default(),
            candidates: vec![
                extra_prompt(
                    "repo-review-rules",
                    "repo-review-rules",
                    PromptSourceOrigin::Project,
                    Some(20),
                ),
                skill_discovery(Some(5)),
                long_lived_skill(
                    "code-review-project",
                    "code-review",
                    PromptSourceOrigin::Project,
                    "code-review",
                    Some(10),
                ),
            ],
        });

        assert_eq!(snapshot.lifecycle, PromptAssemblyLifecycle::NextNewSession);
        assert_eq!(snapshot.active_sources.len(), 4);
        assert_eq!(
            snapshot.active_sources[0],
            ResolvedPromptSource {
                reference_id: "core-system".to_string(),
                kind: PromptSourceKind::CoreSystemPrompt,
                title: "Core system prompt".to_string(),
                origin: Some(PromptSourceOrigin::Builtin),
                status: PromptSourceStatus::Active { order: 0 },
            }
        );
        assert_eq!(
            snapshot
                .active_sources
                .iter()
                .map(|source| source.reference_id.as_str())
                .collect::<Vec<_>>(),
            vec![
                "core-system",
                "skill-discovery",
                "code-review-project",
                "repo-review-rules",
            ]
        );
    }

    #[test]
    fn core_system_prefers_project_override_then_global_override() {
        let project_snapshot = resolve_prompt_assembly(&PromptAssemblyInput {
            core_system: CoreSystemPromptInput {
                global_override_present: true,
                project_override_present: true,
            },
            candidates: Vec::new(),
        });
        assert_eq!(
            project_snapshot.active_sources[0].origin,
            Some(PromptSourceOrigin::Project)
        );

        let global_snapshot = resolve_prompt_assembly(&PromptAssemblyInput {
            core_system: CoreSystemPromptInput {
                global_override_present: true,
                project_override_present: false,
            },
            candidates: Vec::new(),
        });
        assert_eq!(
            global_snapshot.active_sources[0].origin,
            Some(PromptSourceOrigin::Global)
        );
    }

    #[test]
    fn disabled_sources_become_inactive_without_shadowing_enabled_candidates() {
        let mut disabled_project = long_lived_skill(
            "code-review-project",
            "code-review",
            PromptSourceOrigin::Project,
            "code-review",
            Some(1),
        );
        disabled_project.enabled = false;

        let snapshot = resolve_prompt_assembly(&PromptAssemblyInput {
            core_system: CoreSystemPromptInput::default(),
            candidates: vec![
                disabled_project,
                long_lived_skill(
                    "code-review-global",
                    "code-review",
                    PromptSourceOrigin::Global,
                    "code-review",
                    Some(2),
                ),
            ],
        });

        assert!(
            snapshot
                .active_sources
                .iter()
                .any(|source| source.reference_id == "code-review-global")
        );
        assert!(snapshot.inactive_sources.contains(&ResolvedPromptSource {
            reference_id: "code-review-project".to_string(),
            kind: PromptSourceKind::LongLivedSkill,
            title: "code-review".to_string(),
            origin: Some(PromptSourceOrigin::Project),
            status: PromptSourceStatus::Inactive {
                reason: PromptSourceInactiveReason::Disabled,
            },
        }));
    }

    #[test]
    fn project_collision_winner_can_be_missing_and_shadow_global_candidate() {
        let mut missing_project = long_lived_skill(
            "repo-bootstrap-project",
            "repo-bootstrap",
            PromptSourceOrigin::Project,
            "repo-bootstrap",
            Some(1),
        );
        missing_project.resolvable = false;

        let snapshot = resolve_prompt_assembly(&PromptAssemblyInput {
            core_system: CoreSystemPromptInput::default(),
            candidates: vec![
                long_lived_skill(
                    "repo-bootstrap-global",
                    "repo-bootstrap",
                    PromptSourceOrigin::Global,
                    "repo-bootstrap",
                    Some(2),
                ),
                missing_project,
            ],
        });

        assert!(snapshot.inactive_sources.contains(&ResolvedPromptSource {
            reference_id: "repo-bootstrap-project".to_string(),
            kind: PromptSourceKind::LongLivedSkill,
            title: "repo-bootstrap".to_string(),
            origin: Some(PromptSourceOrigin::Project),
            status: PromptSourceStatus::Inactive {
                reason: PromptSourceInactiveReason::Missing,
            },
        }));
        assert!(snapshot.inactive_sources.contains(&ResolvedPromptSource {
            reference_id: "repo-bootstrap-global".to_string(),
            kind: PromptSourceKind::LongLivedSkill,
            title: "repo-bootstrap".to_string(),
            origin: Some(PromptSourceOrigin::Global),
            status: PromptSourceStatus::Inactive {
                reason: PromptSourceInactiveReason::Shadowed,
            },
        }));
    }

    #[test]
    fn project_extra_prompt_overrides_global_extra_prompt_with_same_collision_key() {
        let global_prompt = extra_prompt(
            "review-rules-global",
            "review-rules",
            PromptSourceOrigin::Global,
            Some(20),
        );
        let mut project_prompt = extra_prompt(
            "review-rules-project",
            "review-rules",
            PromptSourceOrigin::Project,
            Some(10),
        );
        project_prompt.collision_key = Some("review-rules".to_string());
        let mut global_prompt = global_prompt;
        global_prompt.collision_key = Some("review-rules".to_string());

        let snapshot = resolve_prompt_assembly(&PromptAssemblyInput {
            core_system: CoreSystemPromptInput::default(),
            candidates: vec![global_prompt, project_prompt],
        });

        assert!(
            snapshot
                .active_sources
                .iter()
                .any(|source| source.reference_id == "review-rules-project")
        );
        assert!(snapshot.inactive_sources.contains(&ResolvedPromptSource {
            reference_id: "review-rules-global".to_string(),
            kind: PromptSourceKind::ExtraPrompt,
            title: "review-rules".to_string(),
            origin: Some(PromptSourceOrigin::Global),
            status: PromptSourceStatus::Inactive {
                reason: PromptSourceInactiveReason::Shadowed,
            },
        }));
    }

    #[test]
    fn inactive_sources_are_grouped_by_reason_then_sorted_by_title() {
        let mut disabled_beta = extra_prompt(
            "z-disabled",
            "z-disabled",
            PromptSourceOrigin::Project,
            Some(1),
        );
        disabled_beta.enabled = false;

        let mut disabled_alpha = extra_prompt(
            "a-disabled",
            "a-disabled",
            PromptSourceOrigin::Project,
            Some(2),
        );
        disabled_alpha.enabled = false;

        let mut missing_skill = long_lived_skill(
            "missing-project",
            "missing-skill",
            PromptSourceOrigin::Project,
            "missing-skill",
            Some(3),
        );
        missing_skill.resolvable = false;

        let snapshot = resolve_prompt_assembly(&PromptAssemblyInput {
            core_system: CoreSystemPromptInput::default(),
            candidates: vec![
                disabled_beta,
                missing_skill,
                long_lived_skill(
                    "shadowed-global",
                    "shadowed-skill",
                    PromptSourceOrigin::Global,
                    "shadowed-skill",
                    Some(5),
                ),
                long_lived_skill(
                    "shadowed-project",
                    "shadowed-skill",
                    PromptSourceOrigin::Project,
                    "shadowed-skill",
                    Some(4),
                ),
                disabled_alpha,
            ],
        });

        assert_eq!(
            snapshot
                .inactive_sources
                .iter()
                .map(|source| source.reference_id.as_str())
                .collect::<Vec<_>>(),
            vec![
                "a-disabled",
                "z-disabled",
                "missing-project",
                "shadowed-global",
            ]
        );
    }

    #[test]
    fn inactive_sources_keep_reason_grouping_and_use_natural_title_order() {
        let snapshot = resolve_prompt_assembly(&PromptAssemblyInput {
            core_system: CoreSystemPromptInput::default(),
            candidates: vec![
                PromptSourceCandidate {
                    reference_id: "disabled-10".to_string(),
                    kind: PromptSourceKind::ExtraPrompt,
                    title: "alpha 10".to_string(),
                    origin: Some(PromptSourceOrigin::Project),
                    collision_key: None,
                    enabled: false,
                    resolvable: true,
                    requested_order: None,
                },
                PromptSourceCandidate {
                    reference_id: "disabled-2".to_string(),
                    kind: PromptSourceKind::ExtraPrompt,
                    title: "alpha 2".to_string(),
                    origin: Some(PromptSourceOrigin::Project),
                    collision_key: None,
                    enabled: false,
                    resolvable: true,
                    requested_order: None,
                },
                PromptSourceCandidate {
                    reference_id: "missing".to_string(),
                    kind: PromptSourceKind::LongLivedSkill,
                    title: "missing".to_string(),
                    origin: Some(PromptSourceOrigin::Global),
                    collision_key: None,
                    enabled: true,
                    resolvable: false,
                    requested_order: None,
                },
                PromptSourceCandidate {
                    reference_id: "skill-shadow-global".to_string(),
                    kind: PromptSourceKind::LongLivedSkill,
                    title: "shadowed".to_string(),
                    origin: Some(PromptSourceOrigin::Global),
                    collision_key: Some("shared".to_string()),
                    enabled: true,
                    resolvable: true,
                    requested_order: None,
                },
                PromptSourceCandidate {
                    reference_id: "skill-shadow-project".to_string(),
                    kind: PromptSourceKind::LongLivedSkill,
                    title: "shadowed".to_string(),
                    origin: Some(PromptSourceOrigin::Project),
                    collision_key: Some("shared".to_string()),
                    enabled: true,
                    resolvable: true,
                    requested_order: None,
                },
            ],
        });

        assert_eq!(
            snapshot
                .inactive_sources
                .iter()
                .map(|source| source.reference_id.as_str())
                .collect::<Vec<_>>(),
            vec![
                "disabled-2",
                "disabled-10",
                "missing",
                "skill-shadow-global",
            ]
        );
    }

    #[test]
    fn active_sources_with_equal_requested_order_use_natural_title_order() {
        let snapshot = resolve_prompt_assembly(&PromptAssemblyInput {
            core_system: CoreSystemPromptInput::default(),
            candidates: vec![
                extra_prompt(
                    "new-prompt-10",
                    "New prompt 10",
                    PromptSourceOrigin::Project,
                    Some(10),
                ),
                extra_prompt(
                    "new-prompt-2",
                    "New prompt 2",
                    PromptSourceOrigin::Project,
                    Some(10),
                ),
                extra_prompt(
                    "new-prompt-1",
                    "New prompt 1",
                    PromptSourceOrigin::Project,
                    Some(10),
                ),
            ],
        });

        assert_eq!(
            snapshot
                .active_sources
                .iter()
                .skip(1)
                .map(|source| source.reference_id.as_str())
                .collect::<Vec<_>>(),
            vec!["new-prompt-1", "new-prompt-2", "new-prompt-10"]
        );
    }
}
