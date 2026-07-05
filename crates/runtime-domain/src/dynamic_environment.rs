use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const STATUS_OUTPUT_LIMIT: usize = 2000;

/// `DynamicEnvironmentSourceKind` 标识内置动态环境来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DynamicEnvironmentSourceKind {
    GitReference,
    GitWorkingTree,
    Date,
    Workdir,
}

/// `DynamicEnvironmentSnapshotKind` 标识动态环境快照的生命周期位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DynamicEnvironmentSnapshotKind {
    Baseline,
    Changes,
}

/// `DynamicEnvironmentSourceSelection` 表示某个动态环境来源是否启用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DynamicEnvironmentSourceSelection {
    pub snapshot_kind: DynamicEnvironmentSnapshotKind,
    pub source_kind: DynamicEnvironmentSourceKind,
    pub enabled: bool,
}

/// `DynamicEnvironmentSessionConfig` 表示某个会话绑定的动态环境注入配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DynamicEnvironmentSessionConfig {
    #[serde(default)]
    pub baseline_enabled: bool,
    #[serde(default)]
    pub changes_enabled: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_selections: Vec<DynamicEnvironmentSourceSelection>,
}

/// `DynamicEnvironmentObservation` 是一次环境观测的可比较结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DynamicEnvironmentObservation {
    pub source_kind: DynamicEnvironmentSourceKind,
    pub fingerprint: String,
    pub summary: String,
    pub details: Option<String>,
}

/// `DynamicEnvironmentSnapshot` 是发送后需要持久化的环境快照。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DynamicEnvironmentSnapshot {
    pub kind: DynamicEnvironmentSnapshotKind,
    pub observations: Vec<DynamicEnvironmentObservation>,
    pub body: String,
}

impl DynamicEnvironmentSourceKind {
    /// `label` 返回 `/prompt` 和快照正文使用的稳定来源名。
    pub const fn label(self) -> &'static str {
        match self {
            Self::GitReference => "Git reference",
            Self::GitWorkingTree => "Git working tree",
            Self::Date => "Date",
            Self::Workdir => "Workdir",
        }
    }
}

impl DynamicEnvironmentSnapshotKind {
    /// `source_title` 返回 `/prompt` 左侧 source 的短标题。
    pub const fn source_title(self) -> &'static str {
        match self {
            Self::Baseline => "Env baseline",
            Self::Changes => "Env changes",
        }
    }

    /// `reference_id` 返回 prompt assembly 使用的稳定引用 id。
    pub const fn reference_id(self) -> &'static str {
        match self {
            Self::Baseline => "env-baseline",
            Self::Changes => "env-changes",
        }
    }
}

impl DynamicEnvironmentSessionConfig {
    /// `snapshot_enabled` 返回某个会话内快照 source 是否整体启用。
    #[must_use]
    pub const fn snapshot_enabled(&self, snapshot_kind: DynamicEnvironmentSnapshotKind) -> bool {
        match snapshot_kind {
            DynamicEnvironmentSnapshotKind::Baseline => self.baseline_enabled,
            DynamicEnvironmentSnapshotKind::Changes => self.changes_enabled,
        }
    }
}

/// `default_dynamic_environment_selections` 返回开箱即用的动态环境来源配置。
#[must_use]
pub fn default_dynamic_environment_selections() -> Vec<DynamicEnvironmentSourceSelection> {
    [
        DynamicEnvironmentSnapshotKind::Baseline,
        DynamicEnvironmentSnapshotKind::Changes,
    ]
    .into_iter()
    .flat_map(|snapshot_kind| {
        [
            (DynamicEnvironmentSourceKind::GitReference, true),
            (DynamicEnvironmentSourceKind::GitWorkingTree, true),
            (DynamicEnvironmentSourceKind::Date, true),
            (DynamicEnvironmentSourceKind::Workdir, false),
        ]
        .into_iter()
        .map(
            move |(source_kind, enabled)| DynamicEnvironmentSourceSelection {
                snapshot_kind,
                source_kind,
                enabled,
            },
        )
    })
    .collect()
}

/// `enabled_dynamic_environment_sources` 返回某类快照当前启用的来源。
#[must_use]
pub fn enabled_dynamic_environment_sources(
    selections: &[DynamicEnvironmentSourceSelection],
    snapshot_kind: DynamicEnvironmentSnapshotKind,
) -> Vec<DynamicEnvironmentSourceKind> {
    let mut sources = selections
        .iter()
        .filter(|selection| selection.snapshot_kind == snapshot_kind && selection.enabled)
        .map(|selection| selection.source_kind)
        .collect::<Vec<_>>();
    sources.sort();
    sources.dedup();
    sources
}

/// `enabled_dynamic_environment_sources_for_session_config` 返回会话内某类快照当前启用的来源。
#[must_use]
pub fn enabled_dynamic_environment_sources_for_session_config(
    config: &DynamicEnvironmentSessionConfig,
    snapshot_kind: DynamicEnvironmentSnapshotKind,
) -> Vec<DynamicEnvironmentSourceKind> {
    if !config.snapshot_enabled(snapshot_kind) {
        return Vec::new();
    }
    enabled_dynamic_environment_sources(&config.source_selections, snapshot_kind)
}

/// `dynamic_environment_changes` 过滤出相对上一快照发生变化的观测。
#[must_use]
pub fn dynamic_environment_changes(
    previous: &[DynamicEnvironmentObservation],
    current: &[DynamicEnvironmentObservation],
) -> Vec<DynamicEnvironmentObservation> {
    current
        .iter()
        .filter(|observation| {
            previous
                .iter()
                .find(|previous| previous.source_kind == observation.source_kind)
                .is_none_or(|previous| previous.fingerprint != observation.fingerprint)
        })
        .cloned()
        .collect()
}

/// `build_dynamic_environment_snapshot` 格式化 provider-visible 环境快照。
#[must_use]
pub fn build_dynamic_environment_snapshot(
    kind: DynamicEnvironmentSnapshotKind,
    observations: Vec<DynamicEnvironmentObservation>,
) -> Option<DynamicEnvironmentSnapshot> {
    if observations.is_empty() {
        return None;
    }

    let heading = match kind {
        DynamicEnvironmentSnapshotKind::Baseline => "Environment baseline for this session:",
        DynamicEnvironmentSnapshotKind::Changes => "Environment changed since the last turn:",
    };
    let mut body = String::from("<system-reminder>\n");
    body.push_str(heading);
    for observation in &observations {
        body.push_str("\n- ");
        body.push_str(observation.source_kind.label());
        body.push_str(": ");
        body.push_str(observation.summary.trim());
        if let Some(details) = observation.details.as_deref().map(str::trim)
            && !details.is_empty()
        {
            body.push('\n');
            body.push_str(&indent_details(details));
        }
    }
    body.push_str("\n</system-reminder>");

    Some(DynamicEnvironmentSnapshot {
        kind,
        observations,
        body,
    })
}

/// `git_working_tree_observation` 构造 git working tree 来源的观测值。
#[must_use]
pub fn git_working_tree_observation(status_porcelain: &str) -> DynamicEnvironmentObservation {
    let trimmed_status = status_porcelain.trim();
    let summary = git_status_summary(trimmed_status);
    let details = (!trimmed_status.is_empty()).then(|| truncate_status_output(trimmed_status));
    DynamicEnvironmentObservation {
        source_kind: DynamicEnvironmentSourceKind::GitWorkingTree,
        fingerprint: stable_sha256(trimmed_status),
        summary,
        details,
    }
}

/// `stable_sha256` 返回稳定 SHA-256 hex，用于较长观测值的 fingerprint。
#[must_use]
pub fn stable_sha256(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    format!("{digest:x}")
}

fn git_status_summary(status_porcelain: &str) -> String {
    let mut changed_count = 0usize;
    let mut untracked_count = 0usize;
    for line in status_porcelain.lines() {
        if line.starts_with("##") {
            continue;
        }
        if line.starts_with("??") {
            untracked_count += 1;
        } else if !line.trim().is_empty() {
            changed_count += 1;
        }
    }

    match (changed_count, untracked_count) {
        (0, 0) => "clean".to_string(),
        (changed, 0) => format!("{changed} changed"),
        (0, untracked) => format!("{untracked} untracked"),
        (changed, untracked) => format!("{changed} changed, {untracked} untracked"),
    }
}

fn truncate_status_output(status_porcelain: &str) -> String {
    if status_porcelain.len() <= STATUS_OUTPUT_LIMIT {
        return status_porcelain.to_string();
    }

    let mut truncated = String::new();
    for character in status_porcelain.chars() {
        if truncated.len() + character.len_utf8() > STATUS_OUTPUT_LIMIT {
            break;
        }
        truncated.push(character);
    }
    truncated.push_str(&format!(
        "\n... (truncated because git status exceeds {STATUS_OUTPUT_LIMIT} characters)"
    ));
    truncated
}

fn indent_details(details: &str) -> String {
    details
        .lines()
        .map(|line| format!("  {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_selections_enable_git_and_date_for_both_snapshot_kinds() {
        let selections = default_dynamic_environment_selections();

        assert_eq!(
            enabled_dynamic_environment_sources(
                &selections,
                DynamicEnvironmentSnapshotKind::Baseline
            ),
            vec![
                DynamicEnvironmentSourceKind::GitReference,
                DynamicEnvironmentSourceKind::GitWorkingTree,
                DynamicEnvironmentSourceKind::Date,
            ]
        );
        assert_eq!(
            enabled_dynamic_environment_sources(
                &selections,
                DynamicEnvironmentSnapshotKind::Changes
            ),
            vec![
                DynamicEnvironmentSourceKind::GitReference,
                DynamicEnvironmentSourceKind::GitWorkingTree,
                DynamicEnvironmentSourceKind::Date,
            ]
        );
        assert!(selections.iter().any(|selection| selection.source_kind
            == DynamicEnvironmentSourceKind::Workdir
            && !selection.enabled));
    }

    #[test]
    fn changes_keep_only_sources_with_new_fingerprints() {
        let previous = vec![
            observation(
                DynamicEnvironmentSourceKind::GitReference,
                "branch-main",
                "main",
            ),
            observation(
                DynamicEnvironmentSourceKind::Date,
                "2026-07-04",
                "2026-07-04",
            ),
        ];
        let current = vec![
            observation(
                DynamicEnvironmentSourceKind::GitReference,
                "branch-feature",
                "feature",
            ),
            observation(
                DynamicEnvironmentSourceKind::Date,
                "2026-07-04",
                "2026-07-04",
            ),
        ];

        let changed = dynamic_environment_changes(&previous, &current);

        assert_eq!(changed.len(), 1);
        assert_eq!(
            changed[0].source_kind,
            DynamicEnvironmentSourceKind::GitReference
        );
        assert_eq!(changed[0].summary, "feature");
    }

    #[test]
    fn snapshot_body_wraps_observations_in_system_reminder() {
        let snapshot = build_dynamic_environment_snapshot(
            DynamicEnvironmentSnapshotKind::Baseline,
            vec![
                observation(
                    DynamicEnvironmentSourceKind::Date,
                    "2026-07-04",
                    "2026-07-04",
                ),
                DynamicEnvironmentObservation {
                    source_kind: DynamicEnvironmentSourceKind::GitWorkingTree,
                    fingerprint: "dirty".to_string(),
                    summary: "1 changed, 1 untracked".to_string(),
                    details: Some("## main\n M src/lib.rs\n?? scratch.md".to_string()),
                },
            ],
        )
        .expect("non-empty observations should build a snapshot");

        assert_eq!(snapshot.kind, DynamicEnvironmentSnapshotKind::Baseline);
        assert!(
            snapshot
                .body
                .starts_with("<system-reminder>\nEnvironment baseline")
        );
        assert!(snapshot.body.contains("- Date: 2026-07-04"));
        assert!(
            snapshot
                .body
                .contains("- Git working tree: 1 changed, 1 untracked")
        );
        assert!(
            snapshot
                .body
                .contains("  ## main\n   M src/lib.rs\n  ?? scratch.md")
        );
        assert!(snapshot.body.ends_with("</system-reminder>"));
    }

    #[test]
    fn git_working_tree_observation_summarizes_and_truncates_status() {
        let long_file_name = "a".repeat(2100);
        let status = format!("## main\n M src/lib.rs\n?? {long_file_name}");

        let observation = git_working_tree_observation(&status);

        assert_eq!(observation.summary, "1 changed, 1 untracked");
        assert_eq!(observation.fingerprint, stable_sha256(status.trim()));
        let details = observation
            .details
            .as_deref()
            .expect("dirty status should include details");
        assert!(details.len() > 2000);
        assert!(details.contains("truncated because git status exceeds 2000 characters"));
    }

    fn observation(
        source_kind: DynamicEnvironmentSourceKind,
        fingerprint: &str,
        summary: &str,
    ) -> DynamicEnvironmentObservation {
        DynamicEnvironmentObservation {
            source_kind,
            fingerprint: fingerprint.to_string(),
            summary: summary.to_string(),
            details: None,
        }
    }
}
