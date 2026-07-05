use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
};

use runtime_domain::dynamic_environment::{
    DynamicEnvironmentObservation, DynamicEnvironmentSessionConfig, DynamicEnvironmentSnapshotKind,
    build_dynamic_environment_snapshot, dynamic_environment_changes,
    enabled_dynamic_environment_sources_for_session_config,
};

use crate::dynamic_environment::DynamicEnvironmentObserver;

pub(super) struct DynamicEnvironmentWorker {
    observer: Arc<dyn DynamicEnvironmentObserver>,
    generation: Arc<AtomicU64>,
    result_tx: mpsc::Sender<DynamicEnvironmentTaskEnvelope>,
    result_rx: mpsc::Receiver<DynamicEnvironmentTaskEnvelope>,
    active_generation: Option<u64>,
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

impl DynamicEnvironmentWorker {
    pub(super) fn new(observer: Arc<dyn DynamicEnvironmentObserver>) -> Self {
        let (result_tx, result_rx) = mpsc::channel();
        Self {
            observer,
            generation: Arc::new(AtomicU64::new(0)),
            result_tx,
            result_rx,
            active_generation: None,
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
        self.active_generation = Some(generation);
        let observer = Arc::clone(&self.observer);
        let result_tx = self.result_tx.clone();
        thread::Builder::new()
            .name("dynamic-environment-worker".to_string())
            .spawn(move || {
                let result = build_dynamic_environment_injection(observer, request);
                let _ = result_tx.send(DynamicEnvironmentTaskEnvelope { generation, result });
            })
            .map_err(|error| {
                self.active_generation = None;
                format!("start dynamic environment worker: {error}")
            })?;
        Ok(())
    }

    pub(super) fn cancel_pending(&mut self) {
        self.bump_generation();
        self.active_generation = None;
        self.drain_result_channel();
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

pub(super) fn build_dynamic_environment_injection(
    observer: Arc<dyn DynamicEnvironmentObserver>,
    request: DynamicEnvironmentRequest,
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
        .observe(request.work_dir.as_path(), &sources)
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
