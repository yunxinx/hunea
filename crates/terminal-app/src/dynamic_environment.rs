use std::{
    future::Future,
    path::Path,
    pin::Pin,
    process::Stdio,
    sync::{Arc, OnceLock, mpsc},
    time::Duration,
};

use runtime_domain::dynamic_environment::{
    DynamicEnvironmentObservation, DynamicEnvironmentSourceKind, git_working_tree_observation,
    stable_sha256,
};
use tokio_util::sync::CancellationToken;

const DYNAMIC_ENVIRONMENT_OBSERVATION_TIMEOUT: Duration = Duration::from_secs(2);
static DYNAMIC_ENVIRONMENT_RUNTIME: OnceLock<mpsc::Sender<DynamicEnvironmentRuntimeCommand>> =
    OnceLock::new();

enum DynamicEnvironmentRuntimeCommand {
    Observe {
        work_dir: std::path::PathBuf,
        sources: Vec<DynamicEnvironmentSourceKind>,
        response: mpsc::Sender<
            Result<Vec<DynamicEnvironmentObservation>, DynamicEnvironmentObservationError>,
        >,
    },
}

#[derive(Debug, thiserror::Error)]
#[error("dynamic environment observation failed")]
pub(crate) struct DynamicEnvironmentObservationError;

/// `DynamicEnvironmentObserver` 抽象动态环境来源的阻塞观察实现。
pub(crate) trait DynamicEnvironmentObserver: Send + Sync {
    fn observe<'a>(
        &'a self,
        work_dir: &'a Path,
        sources: &'a [DynamicEnvironmentSourceKind],
        cancellation: &'a CancellationToken,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = Result<
                        Vec<DynamicEnvironmentObservation>,
                        DynamicEnvironmentObservationError,
                    >,
                > + Send
                + 'a,
        >,
    >;
}

/// `CommandDynamicEnvironmentObserver` 通过系统命令读取 git/date 等动态环境来源。
#[derive(Debug, Default)]
pub(crate) struct CommandDynamicEnvironmentObserver;

impl DynamicEnvironmentObserver for CommandDynamicEnvironmentObserver {
    fn observe<'a>(
        &'a self,
        work_dir: &'a Path,
        sources: &'a [DynamicEnvironmentSourceKind],
        cancellation: &'a CancellationToken,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = Result<
                        Vec<DynamicEnvironmentObservation>,
                        DynamicEnvironmentObservationError,
                    >,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(observe_dynamic_environment_sources_with_cancellation(
            work_dir,
            sources,
            cancellation,
        ))
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
    let (response, receiver) = mpsc::channel();
    dynamic_environment_runtime_sender()
        .send(DynamicEnvironmentRuntimeCommand::Observe {
            work_dir: work_dir.to_path_buf(),
            sources: sources.to_vec(),
            response,
        })
        .map_err(|_| DynamicEnvironmentObservationError)?;
    receiver
        .recv()
        .map_err(|_| DynamicEnvironmentObservationError)?
}

fn dynamic_environment_runtime_sender() -> &'static mpsc::Sender<DynamicEnvironmentRuntimeCommand> {
    DYNAMIC_ENVIRONMENT_RUNTIME.get_or_init(|| {
        let (sender, receiver) = mpsc::channel::<DynamicEnvironmentRuntimeCommand>();
        std::thread::Builder::new()
            .name("dynamic-environment-sync-runtime".to_string())
            .spawn(move || run_dynamic_environment_runtime(receiver))
            .expect("dynamic environment runtime thread should start");
        sender
    })
}

fn run_dynamic_environment_runtime(receiver: mpsc::Receiver<DynamicEnvironmentRuntimeCommand>) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("dynamic environment runtime should initialize");
    while let Ok(command) = receiver.recv() {
        match command {
            DynamicEnvironmentRuntimeCommand::Observe {
                work_dir,
                sources,
                response,
            } => {
                let cancellation = CancellationToken::new();
                let result =
                    runtime.block_on(observe_dynamic_environment_sources_with_cancellation(
                        &work_dir,
                        &sources,
                        &cancellation,
                    ));
                let _ = response.send(result);
            }
        }
    }
}

pub(crate) async fn observe_dynamic_environment_sources_with_cancellation(
    work_dir: &Path,
    sources: &[DynamicEnvironmentSourceKind],
    cancellation: &CancellationToken,
) -> Result<Vec<DynamicEnvironmentObservation>, DynamicEnvironmentObservationError> {
    let mut observations = Vec::with_capacity(sources.len());
    for source in sources {
        if cancellation.is_cancelled() {
            return Err(DynamicEnvironmentObservationError);
        }
        observations
            .push(observe_dynamic_environment_source(work_dir, *source, cancellation).await);
    }
    Ok(observations)
}

async fn observe_dynamic_environment_source(
    work_dir: &Path,
    source: DynamicEnvironmentSourceKind,
    cancellation: &CancellationToken,
) -> DynamicEnvironmentObservation {
    match source {
        DynamicEnvironmentSourceKind::GitReference => {
            observe_git_reference(work_dir, cancellation).await
        }
        DynamicEnvironmentSourceKind::GitWorkingTree => {
            observe_git_working_tree(work_dir, cancellation).await
        }
        DynamicEnvironmentSourceKind::Date => observe_date(cancellation).await,
        DynamicEnvironmentSourceKind::Workdir => observe_workdir(work_dir),
    }
}

async fn observe_git_reference(
    work_dir: &Path,
    cancellation: &CancellationToken,
) -> DynamicEnvironmentObservation {
    let head = git_output(work_dir, ["rev-parse", "HEAD"], cancellation).await;
    let short_head = git_output(work_dir, ["rev-parse", "--short", "HEAD"], cancellation).await;
    let branch = git_output(work_dir, ["branch", "--show-current"], cancellation).await;

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

async fn observe_git_working_tree(
    work_dir: &Path,
    cancellation: &CancellationToken,
) -> DynamicEnvironmentObservation {
    match git_output(work_dir, ["status", "--porcelain=v1", "-b"], cancellation).await {
        Some(status) => git_working_tree_observation(&status),
        None => DynamicEnvironmentObservation {
            source_kind: DynamicEnvironmentSourceKind::GitWorkingTree,
            fingerprint: "not-git".to_string(),
            summary: "not a git repository".to_string(),
            details: None,
        },
    }
}

async fn observe_date(cancellation: &CancellationToken) -> DynamicEnvironmentObservation {
    let date = command_output(
        tokio::process::Command::new("date").arg("+%Y-%m-%d"),
        cancellation,
    )
    .await
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

async fn git_output<const N: usize>(
    work_dir: &Path,
    args: [&str; N],
    cancellation: &CancellationToken,
) -> Option<String> {
    let mut command = tokio::process::Command::new("git");
    command
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(work_dir)
        .args(args);
    command_output(&mut command, cancellation).await
}

async fn command_output(
    command: &mut tokio::process::Command,
    cancellation: &CancellationToken,
) -> Option<String> {
    command_output_with_timeout(
        command,
        DYNAMIC_ENVIRONMENT_OBSERVATION_TIMEOUT,
        cancellation,
    )
    .await
}

async fn command_output_with_timeout(
    command: &mut tokio::process::Command,
    timeout: Duration,
    cancellation: &CancellationToken,
) -> Option<String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let output = tokio::select! {
        _ = cancellation.cancelled() => return None,
        output = tokio::time::timeout(timeout, command.output()) => output.ok()?,
    }
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
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn command_output_with_timeout_kills_slow_command() {
        let started = Instant::now();
        let cancellation = CancellationToken::new();

        let output = super::command_output_with_timeout(
            tokio::process::Command::new("sh")
                .arg("-c")
                .arg("sleep 1; echo done"),
            Duration::from_millis(10),
            &cancellation,
        )
        .await;

        assert!(output.is_none());
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[tokio::test]
    async fn synchronous_observer_wrapper_can_be_called_inside_tokio_runtime() {
        let observations = super::observe_dynamic_environment_sources(
            std::path::Path::new("."),
            &[runtime_domain::dynamic_environment::DynamicEnvironmentSourceKind::Workdir],
        )
        .expect("sync wrapper should use its worker runtime instead of nesting block_on");

        assert_eq!(observations.len(), 1);
        assert_eq!(
            observations[0].source_kind,
            runtime_domain::dynamic_environment::DynamicEnvironmentSourceKind::Workdir
        );
    }
}
