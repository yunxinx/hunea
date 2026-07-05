use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, mpsc},
    thread,
    time::Duration,
};

use runtime_domain::dynamic_environment::{
    DynamicEnvironmentObservation, DynamicEnvironmentSourceKind, git_working_tree_observation,
    stable_sha256,
};

const DYNAMIC_ENVIRONMENT_OBSERVATION_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, thiserror::Error)]
#[error("dynamic environment observation failed")]
pub(crate) struct DynamicEnvironmentObservationError;

/// `DynamicEnvironmentObserver` 抽象动态环境来源的阻塞观察实现。
pub(crate) trait DynamicEnvironmentObserver: Send + Sync {
    fn observe(
        &self,
        work_dir: &Path,
        sources: &[DynamicEnvironmentSourceKind],
    ) -> Result<Vec<DynamicEnvironmentObservation>, DynamicEnvironmentObservationError>;
}

/// `CommandDynamicEnvironmentObserver` 通过系统命令读取 git/date 等动态环境来源。
#[derive(Debug, Default)]
pub(crate) struct CommandDynamicEnvironmentObserver;

impl DynamicEnvironmentObserver for CommandDynamicEnvironmentObserver {
    fn observe(
        &self,
        work_dir: &Path,
        sources: &[DynamicEnvironmentSourceKind],
    ) -> Result<Vec<DynamicEnvironmentObservation>, DynamicEnvironmentObservationError> {
        observe_dynamic_environment_sources(work_dir, sources)
    }
}

pub(crate) fn default_dynamic_environment_observer() -> Arc<dyn DynamicEnvironmentObserver> {
    Arc::new(CommandDynamicEnvironmentObserver)
}

/// `observe_dynamic_environment_sources` 读取当前工作目录下已启用的动态环境来源。
pub(crate) fn observe_dynamic_environment_sources(
    work_dir: &Path,
    sources: &[DynamicEnvironmentSourceKind],
) -> Result<Vec<DynamicEnvironmentObservation>, DynamicEnvironmentObservationError> {
    Ok(sources
        .iter()
        .copied()
        .map(|source| observe_dynamic_environment_source(work_dir, source))
        .collect())
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
    command_output_with_timeout(command, DYNAMIC_ENVIRONMENT_OBSERVATION_TIMEOUT)
}

fn command_output_with_timeout(command: &mut Command, timeout: Duration) -> Option<String> {
    let spec = ProcessCommandSpec::from_command(command);
    if tokio::runtime::Handle::try_current().is_ok() {
        let (sender, receiver) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("dynamic-environment-command".to_string())
            .spawn(move || {
                let _ = sender.send(run_process_command_with_timeout(spec, timeout));
            })
            .ok()?;
        return receiver.recv().ok().flatten();
    }
    run_process_command_with_timeout(spec, timeout)
}

struct ProcessCommandSpec {
    program: OsString,
    args: Vec<OsString>,
    current_dir: Option<PathBuf>,
    envs: Vec<(OsString, Option<OsString>)>,
}

impl ProcessCommandSpec {
    fn from_command(command: &Command) -> Self {
        Self {
            program: command.get_program().to_os_string(),
            args: command.get_args().map(OsString::from).collect(),
            current_dir: command.get_current_dir().map(Path::to_path_buf),
            envs: command
                .get_envs()
                .map(|(key, value)| (key.to_os_string(), value.map(OsString::from)))
                .collect(),
        }
    }

    fn into_tokio_command(self) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(self.program);
        command.args(self.args);
        if let Some(current_dir) = self.current_dir {
            command.current_dir(current_dir);
        }
        for (key, value) in self.envs {
            match value {
                Some(value) => {
                    command.env(key, value);
                }
                None => {
                    command.env_remove(key);
                }
            }
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        command
    }
}

fn run_process_command_with_timeout(spec: ProcessCommandSpec, timeout: Duration) -> Option<String> {
    let mut command = spec.into_tokio_command();

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;
    let output = runtime
        .block_on(async move { tokio::time::timeout(timeout, command.output()).await })
        .ok()?
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
    fn command_output_with_timeout_kills_slow_command() {
        let started = Instant::now();

        let output = super::command_output_with_timeout(
            std::process::Command::new("sh")
                .arg("-c")
                .arg("sleep 1; echo done"),
            Duration::from_millis(10),
        );

        assert!(output.is_none());
        assert!(started.elapsed() < Duration::from_millis(500));
    }
}
