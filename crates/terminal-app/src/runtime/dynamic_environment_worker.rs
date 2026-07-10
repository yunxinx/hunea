use std::{
    panic::AssertUnwindSafe,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    thread::{self, JoinHandle as ThreadJoinHandle},
};

use futures_util::FutureExt;
use runtime_domain::dynamic_environment::{
    DynamicEnvironmentObservation, DynamicEnvironmentSessionConfig, DynamicEnvironmentSnapshotKind,
    build_dynamic_environment_snapshot, dynamic_environment_changes,
    enabled_dynamic_environment_sources_for_session_config,
};
use tokio::sync::mpsc;
use tokio::task::JoinHandle as TokioJoinHandle;
use tokio_util::sync::CancellationToken;

use crate::dynamic_environment::DynamicEnvironmentObserver;
use conversation_runtime::RuntimeEventNotifier;

pub(super) struct DynamicEnvironmentWorker {
    observer: Arc<dyn DynamicEnvironmentObserver>,
    generation: Arc<AtomicU64>,
    command_tx: Option<mpsc::UnboundedSender<DynamicEnvironmentWorkerCommand>>,
    result_rx: mpsc::UnboundedReceiver<DynamicEnvironmentTaskEnvelope>,
    worker_thread: Option<ThreadJoinHandle<()>>,
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
    pub(super) appended_user_texts: Vec<String>,
    pub(super) next_observations: Option<Vec<DynamicEnvironmentObservation>>,
}

struct DynamicEnvironmentTaskEnvelope {
    generation: u64,
    result: Result<DynamicEnvironmentInjection, String>,
}

struct ActiveDynamicEnvironmentTask {
    cancellation: CancellationToken,
    handle: TokioJoinHandle<()>,
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
    pub(super) fn new(
        observer: Arc<dyn DynamicEnvironmentObserver>,
        event_notifier: RuntimeEventNotifier,
    ) -> Self {
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (result_tx, result_rx) = mpsc::unbounded_channel();
        let worker_event_notifier = event_notifier.clone();
        let worker_thread = thread::Builder::new()
            .name("dynamic-environment-runtime".to_string())
            .spawn(move || {
                let _exit_notification = worker_event_notifier.notify_on_drop();
                dynamic_environment_worker_loop(command_rx, result_tx, worker_event_notifier);
            });
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

    #[cfg(test)]
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
        if let Some(worker_thread) = self.worker_thread.take() {
            let _ = worker_thread.join();
        }
    }

    pub(super) fn try_recv_injection(
        &mut self,
    ) -> Option<Result<DynamicEnvironmentInjection, String>> {
        loop {
            match self.result_rx.try_recv() {
                Ok(envelope) => {
                    if self.active_generation == Some(envelope.generation)
                        && self.generation.load(Ordering::Relaxed) == envelope.generation
                    {
                        self.active_generation = None;
                        self.active_cancellation = None;
                        return Some(envelope.result);
                    }
                }
                Err(mpsc::error::TryRecvError::Empty) => return None,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    if self.active_generation.take().is_some() {
                        self.active_cancellation = None;
                        self.command_tx = None;
                        return Some(Err("dynamic environment worker stopped".to_string()));
                    }
                    return None;
                }
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
    event_notifier: RuntimeEventNotifier,
) {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return;
    };

    runtime.block_on(async move {
        let mut active_task: Option<ActiveDynamicEnvironmentTask> = None;
        while let Some(command) = command_rx.recv().await {
            match command {
                DynamicEnvironmentWorkerCommand::Load {
                    generation,
                    cancellation,
                    observer,
                    request,
                } => {
                    cancel_and_join_active_task(&mut active_task).await;
                    let result_tx = result_tx.clone();
                    let task_event_notifier = event_notifier.clone();
                    let task_cancellation = cancellation.clone();
                    let handle = tokio::spawn(async move {
                        let result = match AssertUnwindSafe(build_dynamic_environment_injection(
                            observer,
                            request,
                            &task_cancellation,
                        ))
                        .catch_unwind()
                        .await
                        {
                            Ok(result) => result,
                            Err(panic_payload) => {
                                // panic payload 的析构也允许再次 panic。这里不读取 payload，
                                // 避免已收敛的 worker 异常触发第二次 unwinding。
                                std::mem::forget(panic_payload);
                                Err("dynamic environment worker panicked".to_string())
                            }
                        };
                        if result_tx
                            .send(DynamicEnvironmentTaskEnvelope { generation, result })
                            .is_ok()
                        {
                            task_event_notifier.notify();
                        }
                    });
                    active_task = Some(ActiveDynamicEnvironmentTask {
                        cancellation,
                        handle,
                    });
                }
                DynamicEnvironmentWorkerCommand::Shutdown => {
                    cancel_and_join_active_task(&mut active_task).await;
                    break;
                }
            }
        }
        cancel_and_join_active_task(&mut active_task).await;
    });
}

async fn cancel_and_join_active_task(active_task: &mut Option<ActiveDynamicEnvironmentTask>) {
    let Some(active_task) = active_task.take() else {
        return;
    };
    active_task.cancellation.cancel();
    let _ = active_task.handle.await;
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
        DynamicEnvironmentSnapshotKind::Baseline => dynamic_environment_changes(
            &request.session_config.static_baseline_observations,
            &observations,
        ),
        DynamicEnvironmentSnapshotKind::Changes => {
            dynamic_environment_changes(&request.previous_observations, &observations)
        }
    };

    let mut injection = DynamicEnvironmentInjection {
        appended_user_texts: Vec::new(),
        next_observations: Some(observations),
    };
    if let Some(snapshot) = build_dynamic_environment_snapshot(snapshot_kind, snapshot_observations)
    {
        injection.appended_user_texts.push(snapshot.body);
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

#[cfg(test)]
mod tests {
    use std::{
        path::Path,
        sync::{Arc, Mutex, mpsc as std_mpsc},
        time::Duration,
    };

    use runtime_domain::dynamic_environment::{
        DynamicEnvironmentSourceKind, DynamicEnvironmentSourceSelection,
    };
    use tokio::sync::oneshot;

    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn baseline_skips_duplicate_static_env_but_advances_observations() {
        let observer = Arc::new(FixedObserver::new(vec![observation(
            DynamicEnvironmentSourceKind::Workdir,
            "workspace",
            "Workdir: /tmp/repo",
        )]));
        let request = DynamicEnvironmentRequest {
            work_dir: std::env::temp_dir(),
            session_config: DynamicEnvironmentSessionConfig {
                baseline_enabled: true,
                changes_enabled: false,
                source_selections: vec![DynamicEnvironmentSourceSelection {
                    snapshot_kind: DynamicEnvironmentSnapshotKind::Baseline,
                    source_kind: DynamicEnvironmentSourceKind::Workdir,
                    enabled: true,
                }],
                static_baseline_observations: vec![observation(
                    DynamicEnvironmentSourceKind::Workdir,
                    "workspace",
                    "Workdir: /tmp/repo",
                )],
            },
            is_first_turn: true,
            previous_observations: Vec::new(),
        };

        let injection =
            build_dynamic_environment_injection(observer, request, &CancellationToken::new())
                .await
                .expect("dynamic environment injection should succeed");

        assert!(injection.appended_user_texts.is_empty());
        assert_eq!(
            injection.next_observations,
            Some(vec![observation(
                DynamicEnvironmentSourceKind::Workdir,
                "workspace",
                "Workdir: /tmp/repo",
            )])
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn baseline_injects_only_diff_against_static_env() {
        let observer = Arc::new(FixedObserver::new(vec![
            observation(
                DynamicEnvironmentSourceKind::Date,
                "2026-07-07",
                "2026-07-07",
            ),
            observation(
                DynamicEnvironmentSourceKind::Workdir,
                "workspace-b",
                "Workdir: /tmp/other",
            ),
        ]));
        let request = DynamicEnvironmentRequest {
            work_dir: std::env::temp_dir(),
            session_config: DynamicEnvironmentSessionConfig {
                baseline_enabled: true,
                changes_enabled: false,
                source_selections: vec![
                    DynamicEnvironmentSourceSelection {
                        snapshot_kind: DynamicEnvironmentSnapshotKind::Baseline,
                        source_kind: DynamicEnvironmentSourceKind::Date,
                        enabled: true,
                    },
                    DynamicEnvironmentSourceSelection {
                        snapshot_kind: DynamicEnvironmentSnapshotKind::Baseline,
                        source_kind: DynamicEnvironmentSourceKind::Workdir,
                        enabled: true,
                    },
                ],
                static_baseline_observations: vec![
                    observation(
                        DynamicEnvironmentSourceKind::Date,
                        "2026-07-07",
                        "2026-07-07",
                    ),
                    observation(
                        DynamicEnvironmentSourceKind::Workdir,
                        "workspace-a",
                        "Workdir: /tmp/repo",
                    ),
                ],
            },
            is_first_turn: true,
            previous_observations: Vec::new(),
        };

        let injection =
            build_dynamic_environment_injection(observer, request, &CancellationToken::new())
                .await
                .expect("dynamic environment injection should succeed");

        assert_eq!(
            injection.appended_user_texts,
            vec![
                "<system-reminder>\nEnvironment baseline for this session:\n- Workdir: Workdir: /tmp/other\n</system-reminder>"
                    .to_string(),
            ]
        );
        assert_eq!(
            injection.next_observations,
            Some(vec![
                observation(
                    DynamicEnvironmentSourceKind::Date,
                    "2026-07-07",
                    "2026-07-07",
                ),
                observation(
                    DynamicEnvironmentSourceKind::Workdir,
                    "workspace-b",
                    "Workdir: /tmp/other",
                ),
            ])
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn changes_compare_against_previous_observations() {
        let observer = Arc::new(FixedObserver::new(vec![
            observation(
                DynamicEnvironmentSourceKind::Date,
                "2026-07-08",
                "2026-07-08",
            ),
            observation(
                DynamicEnvironmentSourceKind::Workdir,
                "workspace",
                "Workdir: /tmp/repo",
            ),
        ]));
        let request = DynamicEnvironmentRequest {
            work_dir: std::env::temp_dir(),
            session_config: DynamicEnvironmentSessionConfig {
                baseline_enabled: false,
                changes_enabled: true,
                source_selections: vec![
                    DynamicEnvironmentSourceSelection {
                        snapshot_kind: DynamicEnvironmentSnapshotKind::Changes,
                        source_kind: DynamicEnvironmentSourceKind::Date,
                        enabled: true,
                    },
                    DynamicEnvironmentSourceSelection {
                        snapshot_kind: DynamicEnvironmentSnapshotKind::Changes,
                        source_kind: DynamicEnvironmentSourceKind::Workdir,
                        enabled: true,
                    },
                ],
                static_baseline_observations: Vec::new(),
            },
            is_first_turn: false,
            previous_observations: vec![
                observation(
                    DynamicEnvironmentSourceKind::Date,
                    "2026-07-07",
                    "2026-07-07",
                ),
                observation(
                    DynamicEnvironmentSourceKind::Workdir,
                    "workspace",
                    "Workdir: /tmp/repo",
                ),
            ],
        };

        let injection =
            build_dynamic_environment_injection(observer, request, &CancellationToken::new())
                .await
                .expect("dynamic environment injection should succeed");

        assert_eq!(
            injection.appended_user_texts,
            vec![
                "<system-reminder>\nEnvironment changed since the last turn:\n- Date: 2026-07-08\n</system-reminder>"
                    .to_string(),
            ]
        );
    }

    #[test]
    fn shutdown_waits_for_active_observation_to_finish_after_cancellation() {
        let (started_tx, started_rx) = std_mpsc::channel();
        let (cancelled_tx, cancelled_rx) = std_mpsc::channel();
        let (finish_tx, finish_rx) = oneshot::channel();
        let observer = Arc::new(ShutdownBlockingObserver {
            started: Mutex::new(Some(started_tx)),
            cancelled: Mutex::new(Some(cancelled_tx)),
            finish: Mutex::new(Some(finish_rx)),
        });
        let mut worker = DynamicEnvironmentWorker::new(
            observer,
            conversation_runtime::RuntimeEventNotifier::default(),
        );

        worker
            .load(DynamicEnvironmentRequest {
                work_dir: std::env::temp_dir(),
                session_config: DynamicEnvironmentSessionConfig {
                    baseline_enabled: true,
                    changes_enabled: false,
                    source_selections: vec![DynamicEnvironmentSourceSelection {
                        snapshot_kind: DynamicEnvironmentSnapshotKind::Baseline,
                        source_kind: DynamicEnvironmentSourceKind::Date,
                        enabled: true,
                    }],
                    static_baseline_observations: Vec::new(),
                },
                is_first_turn: true,
                previous_observations: Vec::new(),
            })
            .expect("dynamic environment worker should start load");
        started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("observer should start before shutdown");

        let (shutdown_done_tx, shutdown_done_rx) = std_mpsc::channel();
        std::thread::spawn(move || {
            worker.shutdown();
            let _ = shutdown_done_tx.send(());
        });

        cancelled_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("shutdown should cancel the active observation");
        assert!(
            shutdown_done_rx
                .recv_timeout(Duration::from_millis(50))
                .is_err(),
            "shutdown returned before the active observation finished"
        );

        finish_tx
            .send(())
            .expect("observer finish signal should be delivered");
        shutdown_done_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("shutdown should finish after active observation exits");
    }

    #[test]
    fn dynamic_environment_result_wakes_after_the_payload_is_queued() {
        let (wake_sender, wake_receiver) = std_mpsc::channel();
        let notifier = conversation_runtime::RuntimeEventNotifier::default();
        notifier
            .install(move || {
                let _ = wake_sender.send(());
            })
            .expect("test notifier should install once");
        let observer = Arc::new(FixedObserver::new(vec![observation(
            DynamicEnvironmentSourceKind::Date,
            "2026-07-10",
            "2026-07-10",
        )]));
        let mut worker = DynamicEnvironmentWorker::new(observer, notifier);

        worker
            .load(DynamicEnvironmentRequest {
                work_dir: std::env::temp_dir(),
                session_config: DynamicEnvironmentSessionConfig {
                    baseline_enabled: true,
                    changes_enabled: false,
                    source_selections: vec![DynamicEnvironmentSourceSelection {
                        snapshot_kind: DynamicEnvironmentSnapshotKind::Baseline,
                        source_kind: DynamicEnvironmentSourceKind::Date,
                        enabled: true,
                    }],
                    static_baseline_observations: Vec::new(),
                },
                is_first_turn: true,
                previous_observations: Vec::new(),
            })
            .expect("dynamic environment load should queue");

        wake_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("dynamic environment result should wake its consumer");
        assert!(matches!(worker.try_recv_injection(), Some(Ok(_))));
        worker.shutdown();
    }

    #[test]
    fn panicking_dynamic_environment_observer_reports_failure_and_clears_active_request() {
        let (wake_sender, wake_receiver) = std_mpsc::channel();
        let notifier = conversation_runtime::RuntimeEventNotifier::default();
        notifier
            .install(move || {
                let _ = wake_sender.send(());
            })
            .expect("test notifier should install once");
        let mut worker = DynamicEnvironmentWorker::new(Arc::new(PanickingObserver), notifier);

        worker
            .load(DynamicEnvironmentRequest {
                work_dir: std::env::temp_dir(),
                session_config: DynamicEnvironmentSessionConfig {
                    baseline_enabled: true,
                    changes_enabled: false,
                    source_selections: vec![DynamicEnvironmentSourceSelection {
                        snapshot_kind: DynamicEnvironmentSnapshotKind::Baseline,
                        source_kind: DynamicEnvironmentSourceKind::Date,
                        enabled: true,
                    }],
                    static_baseline_observations: Vec::new(),
                },
                is_first_turn: true,
                previous_observations: Vec::new(),
            })
            .expect("dynamic environment load should queue");

        wake_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("observer panic should wake the dynamic environment consumer");
        assert!(matches!(
            worker.try_recv_injection(),
            Some(Err(message)) if message == "dynamic environment worker panicked"
        ));
        assert!(!worker.has_pending_work());
        worker.shutdown();
    }

    #[test]
    fn disconnected_dynamic_environment_worker_clears_active_request_and_reports_failure() {
        let observer = Arc::new(FixedObserver::new(Vec::new()));
        let mut worker = DynamicEnvironmentWorker::new(
            observer,
            conversation_runtime::RuntimeEventNotifier::default(),
        );
        let (result_tx, result_rx) = mpsc::unbounded_channel();
        drop(result_tx);
        worker.result_rx = result_rx;
        worker.active_generation = Some(7);
        worker.active_cancellation = Some(CancellationToken::new());

        let result = worker.try_recv_injection();

        assert!(matches!(
            result,
            Some(Err(message)) if message == "dynamic environment worker stopped"
        ));
        assert!(!worker.has_pending_work());
        assert!(worker.active_cancellation.is_none());
    }

    struct ShutdownBlockingObserver {
        started: Mutex<Option<std_mpsc::Sender<()>>>,
        cancelled: Mutex<Option<std_mpsc::Sender<()>>>,
        finish: Mutex<Option<oneshot::Receiver<()>>>,
    }

    struct FixedObserver {
        observations: Vec<DynamicEnvironmentObservation>,
    }

    struct PanickingObserver;

    impl FixedObserver {
        fn new(observations: Vec<DynamicEnvironmentObservation>) -> Self {
            Self { observations }
        }
    }

    impl crate::dynamic_environment::DynamicEnvironmentObserver for FixedObserver {
        fn observe<'a>(
            &'a self,
            _work_dir: &'a Path,
            _sources: &'a [DynamicEnvironmentSourceKind],
            _cancellation: &'a CancellationToken,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<
                            Vec<DynamicEnvironmentObservation>,
                            crate::dynamic_environment::DynamicEnvironmentObservationError,
                        >,
                    > + Send
                    + 'a,
            >,
        > {
            Box::pin(async move { Ok(self.observations.clone()) })
        }
    }

    impl crate::dynamic_environment::DynamicEnvironmentObserver for PanickingObserver {
        fn observe<'a>(
            &'a self,
            _work_dir: &'a Path,
            _sources: &'a [DynamicEnvironmentSourceKind],
            _cancellation: &'a CancellationToken,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<
                            Vec<DynamicEnvironmentObservation>,
                            crate::dynamic_environment::DynamicEnvironmentObservationError,
                        >,
                    > + Send
                    + 'a,
            >,
        > {
            Box::pin(async move { panic!("injected dynamic environment observer panic") })
        }
    }

    impl crate::dynamic_environment::DynamicEnvironmentObserver for ShutdownBlockingObserver {
        fn observe<'a>(
            &'a self,
            _work_dir: &'a Path,
            sources: &'a [DynamicEnvironmentSourceKind],
            cancellation: &'a CancellationToken,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<
                            Vec<DynamicEnvironmentObservation>,
                            crate::dynamic_environment::DynamicEnvironmentObservationError,
                        >,
                    > + Send
                    + 'a,
            >,
        > {
            Box::pin(async move {
                if let Some(started) = self.started.lock().expect("started lock").take() {
                    let _ = started.send(());
                }
                cancellation.cancelled().await;
                if let Some(cancelled) = self.cancelled.lock().expect("cancelled lock").take() {
                    let _ = cancelled.send(());
                }
                let finish = self
                    .finish
                    .lock()
                    .expect("finish lock")
                    .take()
                    .expect("test should provide finish receiver");
                finish.await.expect("test should release observer");
                Ok(sources
                    .iter()
                    .copied()
                    .map(|source_kind| DynamicEnvironmentObservation {
                        source_kind,
                        fingerprint: "observed".to_string(),
                        summary: "observed".to_string(),
                        details: None,
                    })
                    .collect())
            })
        }
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
