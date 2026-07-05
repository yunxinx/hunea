use std::{
    path::Path,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

use runtime_domain::dynamic_environment::{
    DynamicEnvironmentObservation, DynamicEnvironmentSourceKind, git_working_tree_observation,
    stable_sha256,
};

const DYNAMIC_ENVIRONMENT_OBSERVATION_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, thiserror::Error)]
pub(crate) enum DynamicEnvironmentObservationError {
    #[error("dynamic environment observation timed out after {timeout:?}")]
    TimedOut { timeout: Duration },
    #[error("dynamic environment observation worker stopped")]
    WorkerStopped,
    #[error("start dynamic environment observation worker: {detail}")]
    WorkerStart { detail: String },
}

/// `observe_dynamic_environment_sources` 读取当前工作目录下已启用的动态环境来源。
pub(crate) fn observe_dynamic_environment_sources(
    work_dir: &Path,
    sources: &[DynamicEnvironmentSourceKind],
) -> Result<Vec<DynamicEnvironmentObservation>, DynamicEnvironmentObservationError> {
    let work_dir = work_dir.to_path_buf();
    let sources = sources.to_vec();
    observe_with_timeout(DYNAMIC_ENVIRONMENT_OBSERVATION_TIMEOUT, move || {
        sources
            .into_iter()
            .map(|source| observe_dynamic_environment_source(work_dir.as_path(), source))
            .collect()
    })
}

fn observe_with_timeout<T>(
    timeout: Duration,
    observe: impl FnOnce() -> T + Send + 'static,
) -> Result<T, DynamicEnvironmentObservationError>
where
    T: Send + 'static,
{
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("dynamic-environment-observer".to_string())
        .spawn(move || {
            let result = observe();
            let _ = sender.send(result);
        })
        .map_err(|error| DynamicEnvironmentObservationError::WorkerStart {
            detail: error.to_string(),
        })?;

    match receiver.recv_timeout(timeout) {
        Ok(result) => Ok(result),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            Err(DynamicEnvironmentObservationError::TimedOut { timeout })
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err(DynamicEnvironmentObservationError::WorkerStopped)
        }
    }
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

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    #[test]
    fn observe_with_timeout_returns_before_blocking_observer_finishes() {
        let started = Instant::now();

        let error = super::observe_with_timeout(Duration::from_millis(10), || {
            std::thread::sleep(Duration::from_secs(1));
            Vec::<()>::new()
        })
        .expect_err("blocking observer should time out");

        assert!(matches!(
            error,
            super::DynamicEnvironmentObservationError::TimedOut { .. }
        ));
        assert!(started.elapsed() < Duration::from_millis(500));
    }
}
