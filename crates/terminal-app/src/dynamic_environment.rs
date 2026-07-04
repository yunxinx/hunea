use std::{
    path::Path,
    process::{Command, Stdio},
};

use runtime_domain::dynamic_environment::{
    DynamicEnvironmentObservation, DynamicEnvironmentSourceKind, git_working_tree_observation,
    stable_sha256,
};

/// `observe_dynamic_environment_sources` 读取当前工作目录下已启用的动态环境来源。
pub(crate) fn observe_dynamic_environment_sources(
    work_dir: &Path,
    sources: &[DynamicEnvironmentSourceKind],
) -> Vec<DynamicEnvironmentObservation> {
    sources
        .iter()
        .copied()
        .map(|source| observe_dynamic_environment_source(work_dir, source))
        .collect()
}

fn observe_dynamic_environment_source(
    work_dir: &Path,
    source: DynamicEnvironmentSourceKind,
) -> DynamicEnvironmentObservation {
    match source {
        DynamicEnvironmentSourceKind::GitReference => observe_git_reference(work_dir),
        DynamicEnvironmentSourceKind::GitWorkingTree => observe_git_working_tree(work_dir),
        DynamicEnvironmentSourceKind::Date => observe_date(),
        DynamicEnvironmentSourceKind::Workdir => observe_workdir(work_dir),
    }
}

fn observe_git_reference(work_dir: &Path) -> DynamicEnvironmentObservation {
    let head = git_output(work_dir, ["rev-parse", "HEAD"]);
    let short_head = git_output(work_dir, ["rev-parse", "--short", "HEAD"]);
    let branch = git_output(work_dir, ["branch", "--show-current"]);

    let summary = match (branch.as_deref(), short_head.as_deref()) {
        (Some(branch), Some(short_head)) if !branch.is_empty() => {
            format!("{branch} @ {short_head}")
        }
        (_, Some(short_head)) => format!("detached @ {short_head}"),
        _ => "not a git repository".to_string(),
    };
    let fingerprint = match (branch.as_deref(), head.as_deref()) {
        (Some(branch), Some(head)) => format!("{branch}:{head}"),
        (_, Some(head)) => head.to_string(),
        _ => "not-git".to_string(),
    };

    DynamicEnvironmentObservation {
        source_kind: DynamicEnvironmentSourceKind::GitReference,
        fingerprint,
        summary,
        details: None,
    }
}

fn observe_git_working_tree(work_dir: &Path) -> DynamicEnvironmentObservation {
    match git_output(work_dir, ["status", "--porcelain=v1", "-b"]) {
        Some(status) => git_working_tree_observation(&status),
        None => DynamicEnvironmentObservation {
            source_kind: DynamicEnvironmentSourceKind::GitWorkingTree,
            fingerprint: "not-git".to_string(),
            summary: "not a git repository".to_string(),
            details: None,
        },
    }
}

fn observe_date() -> DynamicEnvironmentObservation {
    let date = command_output(Command::new("date").arg("+%Y-%m-%d"))
        .unwrap_or_else(|| "unknown".to_string());
    DynamicEnvironmentObservation {
        source_kind: DynamicEnvironmentSourceKind::Date,
        fingerprint: date.clone(),
        summary: date,
        details: None,
    }
}

fn observe_workdir(work_dir: &Path) -> DynamicEnvironmentObservation {
    let summary = work_dir.display().to_string();
    DynamicEnvironmentObservation {
        source_kind: DynamicEnvironmentSourceKind::Workdir,
        fingerprint: stable_sha256(&summary),
        summary,
        details: None,
    }
}

fn git_output<const N: usize>(work_dir: &Path, args: [&str; N]) -> Option<String> {
    let mut command = Command::new("git");
    command
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(work_dir)
        .args(args);
    command_output(&mut command)
}

fn command_output(command: &mut Command) -> Option<String> {
    let output = command
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Some(text)
}
