use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
};

use runtime_domain::dynamic_environment::{
    DynamicEnvironmentObservation, DynamicEnvironmentSessionConfig, DynamicEnvironmentSnapshotKind,
    build_dynamic_environment_snapshot, dynamic_environment_changes,
    enabled_dynamic_environment_sources_for_session_config,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::dynamic_environment::DynamicEnvironmentObserver;

pub(super) struct DynamicEnvironmentWorker {
    observer: Arc<dyn DynamicEnvironmentObserver>,
    generation: Arc<AtomicU64>,
    command_tx: Option<mpsc::UnboundedSender<DynamicEnvironmentWorkerCommand>>,
    result_rx: mpsc::UnboundedReceiver<DynamicEnvironmentTaskEnvelope>,
    worker_thread: Option<JoinHandle<()>>,
    active_generation: Option<u64>,
    active_cancellation: Option<CancellationToken>,
}

#[derive(Debug)]
pub(super) struct DynamicEnvironmentRequest {
    pub(super) work_dir: PathBuf,
    pub(super) session_config: DynamicEnvironmentSessionConfig,
    pub(super) is_first_turn: bool,
    pub(super) previous_observations: Vec<DynamicEnvironmentObservation>,
}

#[derive(Debug, Default)]
pub(super) struct DynamicEnvironmentInjection {
    pub(super) prefix_texts: Vec<String>,
    pub(super) next_observations: Option<Vec<DynamicEnvironmentObservation>>,
}

struct DynamicEnvironmentTaskEnvelope {
    generation: u64,
    result: Result<DynamicEnvironmentInjection, String>,
}

enum DynamicEnvironmentWorkerCommand {
    Load {
        generation: u64,
        cancellation: CancellationToken,
        observer: Arc<dyn DynamicEnvironmentObserver>,
        request: DynamicEnvironmentRequest,
    },
    Shutdown,
}

impl DynamicEnvironmentWorker {
    pub(super) fn new(observer: Arc<dyn DynamicEnvironmentObserver>) -> Self {
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (result_tx, result_rx) = mpsc::unbounded_channel();
        let worker_thread = thread::Builder::new()
            .name("dynamic-environment-runtime".to_string())
            .spawn(move || dynamic_environment_worker_loop(command_rx, result_tx));
        Self {
            observer,
            generation: Arc::new(AtomicU64::new(0)),
            command_tx: worker_thread.as_ref().ok().map(|_| command_tx),
            result_rx,
            worker_thread: worker_thread.ok(),
            active_generation: None,
            active_cancellation: None,
        }
    }

    pub(super) fn has_pending_work(&self) -> bool {
        self.active_generation.is_some()
    }

    pub(super) fn load(&mut self, request: DynamicEnvironmentRequest) -> Result<(), String> {
        if self.active_generation.is_some() {
            return Err("dynamic environment worker is already running".to_string());
        }

        let generation = self.bump_generation();
        let cancellation = CancellationToken::new();
        let command_tx = self
            .command_tx
            .as_ref()
            .ok_or_else(|| "dynamic environment worker stopped".to_string())?;
        command_tx
            .send(DynamicEnvironmentWorkerCommand::Load {
                generation,
                cancellation: cancellation.clone(),
                observer: Arc::clone(&self.observer),
                request,
            })
            .map_err(|_| "dynamic environment worker stopped".to_string())?;
        self.active_generation = Some(generation);
        self.active_cancellation = Some(cancellation);
        Ok(())
    }

    pub(super) fn cancel_pending(&mut self) {
        self.bump_generation();
        if let Some(cancellation) = self.active_cancellation.take() {
            cancellation.cancel();
        }
        self.active_generation = None;
        self.drain_result_channel();
    }

    pub(super) fn shutdown(&mut self) {
        self.cancel_pending();
        if let Some(command_tx) = self.command_tx.take() {
            let _ = command_tx.send(DynamicEnvironmentWorkerCommand::Shutdown);
        }
        let _ = self.worker_thread.take();
    }

    pub(super) fn try_recv_injection(
        &mut self,
    ) -> Option<Result<DynamicEnvironmentInjection, String>> {
        loop {
            let envelope = self.result_rx.try_recv().ok()?;
            if self.active_generation == Some(envelope.generation)
                && self.generation.load(Ordering::Relaxed) == envelope.generation
            {
                self.active_generation = None;
                self.active_cancellation = None;
                return Some(envelope.result);
            }
        }
    }

    fn bump_generation(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn drain_result_channel(&mut self) {
        while self.result_rx.try_recv().is_ok() {}
    }
}

fn dynamic_environment_worker_loop(
    mut command_rx: mpsc::UnboundedReceiver<DynamicEnvironmentWorkerCommand>,
    result_tx: mpsc::UnboundedSender<DynamicEnvironmentTaskEnvelope>,
) {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return;
    };

    runtime.block_on(async move {
        while let Some(command) = command_rx.recv().await {
            match command {
                DynamicEnvironmentWorkerCommand::Load {
                    generation,
                    cancellation,
                    observer,
                    request,
                } => {
                    let result_tx = result_tx.clone();
                    tokio::spawn(async move {
                        let result =
                            build_dynamic_environment_injection(observer, request, &cancellation)
                                .await;
                        let _ =
                            result_tx.send(DynamicEnvironmentTaskEnvelope { generation, result });
                    });
                }
                DynamicEnvironmentWorkerCommand::Shutdown => {
                    break;
                }
            }
        }
    });
}

pub(super) async fn build_dynamic_environment_injection(
    observer: Arc<dyn DynamicEnvironmentObserver>,
    request: DynamicEnvironmentRequest,
    cancellation: &CancellationToken,
) -> Result<DynamicEnvironmentInjection, String> {
    let Some(snapshot_kind) =
        dynamic_environment_snapshot_for_turn(&request.session_config, request.is_first_turn)
    else {
        return Ok(DynamicEnvironmentInjection::default());
    };
    let sources = enabled_dynamic_environment_sources_for_session_config(
        &request.session_config,
        snapshot_kind,
    );
    if sources.is_empty() {
        return Ok(DynamicEnvironmentInjection::default());
    }

    let observations = observer
        .observe(request.work_dir.as_path(), &sources, cancellation)
        .await
        .map_err(|error| error.to_string())?;
    let snapshot_observations = match snapshot_kind {
        DynamicEnvironmentSnapshotKind::Baseline => observations.clone(),
        DynamicEnvironmentSnapshotKind::Changes => {
            dynamic_environment_changes(&request.previous_observations, &observations)
        }
    };

    let mut injection = DynamicEnvironmentInjection::default();
    if let Some(snapshot) = build_dynamic_environment_snapshot(snapshot_kind, snapshot_observations)
    {
        injection.prefix_texts.push(snapshot.body);
        injection.next_observations = Some(observations);
    }
    Ok(injection)
}

pub(super) fn dynamic_environment_snapshot_for_turn(
    session_config: &DynamicEnvironmentSessionConfig,
    is_first_turn: bool,
) -> Option<DynamicEnvironmentSnapshotKind> {
    if is_first_turn && session_config.snapshot_enabled(DynamicEnvironmentSnapshotKind::Baseline) {
        return Some(DynamicEnvironmentSnapshotKind::Baseline);
    }
    if !is_first_turn && session_config.snapshot_enabled(DynamicEnvironmentSnapshotKind::Changes) {
        return Some(DynamicEnvironmentSnapshotKind::Changes);
    }
    None
}
