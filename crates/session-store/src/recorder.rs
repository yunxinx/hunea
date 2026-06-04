#![allow(dead_code)]

use std::{
    mem,
    path::PathBuf,
    sync::{Mutex, MutexGuard, mpsc as std_mpsc},
    thread::{self, JoinHandle},
    time::Duration,
};

use tokio::sync::{mpsc, oneshot};
use tracing::warn;

use crate::{SessionEntry, SessionStoreError, jsonl::JsonlWriter};

const RECORD_COMMAND_CAPACITY: usize = 256;
const DROP_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) struct SessionRecorder {
    jsonl_path: PathBuf,
    runtime: Mutex<RecorderRuntime>,
}

enum RecordCommand {
    Buffer(SessionEntry),
    Persist {
        ack: oneshot::Sender<Result<(), SessionStoreError>>,
    },
    Flush {
        ack: oneshot::Sender<Result<(), SessionStoreError>>,
    },
    Shutdown {
        ack: oneshot::Sender<Result<(), SessionStoreError>>,
    },
}

struct RecorderWorker {
    jsonl_path: PathBuf,
    writer: JsonlWriter,
    pending_entries: Vec<SessionEntry>,
}

struct RecorderRuntime {
    state: RecorderState,
}

enum RecorderState {
    Running {
        tx: mpsc::Sender<RecordCommand>,
        worker_thread: JoinHandle<()>,
    },
    Shutdown,
}

struct RunningRecorder {
    tx: mpsc::Sender<RecordCommand>,
    worker_thread: JoinHandle<()>,
}

impl SessionRecorder {
    pub(crate) fn new(jsonl_path: PathBuf) -> Self {
        Self::new_with_capacity(jsonl_path, RECORD_COMMAND_CAPACITY)
    }

    pub(crate) fn buffer(&self, entry: SessionEntry) -> Result<(), SessionStoreError> {
        self.sender()?
            .try_send(RecordCommand::Buffer(entry))
            .map_err(|_| SessionStoreError::ChannelClosed)
    }

    pub(crate) async fn persist(&self) -> Result<(), SessionStoreError> {
        self.send_ack_command(|ack| RecordCommand::Persist { ack })
            .await
    }

    pub(crate) async fn flush(&self) -> Result<(), SessionStoreError> {
        self.send_ack_command(|ack| RecordCommand::Flush { ack })
            .await
    }

    pub(crate) async fn shutdown(self) -> Result<(), SessionStoreError> {
        let Some(running) = self.take_running_state() else {
            return Ok(());
        };

        let (ack_tx, ack_rx) = oneshot::channel();
        running
            .tx
            .send(RecordCommand::Shutdown { ack: ack_tx })
            .await
            .map_err(|_| SessionStoreError::ChannelClosed)?;
        ack_rx
            .await
            .map_err(|_| SessionStoreError::ChannelClosed)??;

        join_worker(running.worker_thread).await
    }

    async fn send_ack_command(
        &self,
        build_command: impl FnOnce(oneshot::Sender<Result<(), SessionStoreError>>) -> RecordCommand,
    ) -> Result<(), SessionStoreError> {
        let (ack_tx, ack_rx) = oneshot::channel();
        self.sender()?
            .send(build_command(ack_tx))
            .await
            .map_err(|_| SessionStoreError::ChannelClosed)?;
        ack_rx.await.map_err(|_| SessionStoreError::ChannelClosed)?
    }

    fn new_with_capacity(jsonl_path: PathBuf, capacity: usize) -> Self {
        let (tx, rx) = mpsc::channel(capacity);
        let worker_path = jsonl_path.clone();
        let worker_thread = spawn_worker_thread(move || run_worker(rx, worker_path));

        Self {
            jsonl_path,
            runtime: Mutex::new(RecorderRuntime {
                state: RecorderState::Running { tx, worker_thread },
            }),
        }
    }

    #[cfg(test)]
    fn new_paused(jsonl_path: PathBuf) -> (Self, oneshot::Sender<()>) {
        let (tx, rx) = mpsc::channel(RECORD_COMMAND_CAPACITY);
        let worker_path = jsonl_path.clone();
        let (start_tx, start_rx) = oneshot::channel();
        let worker_thread = spawn_worker_thread(move || {
            let _ = start_rx.blocking_recv();
            run_worker(rx, worker_path);
        });

        (
            Self {
                jsonl_path,
                runtime: Mutex::new(RecorderRuntime {
                    state: RecorderState::Running { tx, worker_thread },
                }),
            },
            start_tx,
        )
    }

    fn sender(&self) -> Result<mpsc::Sender<RecordCommand>, SessionStoreError> {
        let runtime = self.lock_runtime();
        match &runtime.state {
            RecorderState::Running { tx, .. } => Ok(tx.clone()),
            RecorderState::Shutdown => Err(SessionStoreError::ChannelClosed),
        }
    }

    fn take_running_state(&self) -> Option<RunningRecorder> {
        let mut runtime = self.lock_runtime();
        match mem::replace(&mut runtime.state, RecorderState::Shutdown) {
            RecorderState::Running { tx, worker_thread } => {
                Some(RunningRecorder { tx, worker_thread })
            }
            RecorderState::Shutdown => None,
        }
    }

    fn lock_runtime(&self) -> MutexGuard<'_, RecorderRuntime> {
        self.runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Drop for SessionRecorder {
    fn drop(&mut self) {
        let Some(running) = self.take_running_state() else {
            return;
        };

        let path = self.jsonl_path.clone();
        let (ack_tx, ack_rx) = oneshot::channel();
        let (result_tx, result_rx) = std_mpsc::channel();

        std::thread::spawn(move || {
            let result = match running
                .tx
                .blocking_send(RecordCommand::Shutdown { ack: ack_tx })
            {
                Ok(()) => match ack_rx
                    .blocking_recv()
                    .unwrap_or(Err(SessionStoreError::ChannelClosed))
                {
                    Ok(()) => join_worker_blocking(running.worker_thread),
                    Err(error) => Err(error),
                },
                Err(_) => join_worker_blocking(running.worker_thread),
            };

            let _ = result_tx.send(result);
        });

        match result_rx.recv_timeout(DROP_SHUTDOWN_TIMEOUT) {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                warn!(path = %path.display(), error = %error, "session recorder shutdown failed during drop")
            }
            Err(std_mpsc::RecvTimeoutError::Timeout) => {
                warn!(path = %path.display(), timeout_ms = DROP_SHUTDOWN_TIMEOUT.as_millis(), "session recorder shutdown timed out during drop");
            }
            Err(std_mpsc::RecvTimeoutError::Disconnected) => {
                warn!(path = %path.display(), "session recorder shutdown task disconnected during drop");
            }
        }
    }
}

impl RecorderWorker {
    fn new(jsonl_path: PathBuf) -> Self {
        Self {
            writer: JsonlWriter::new(jsonl_path.clone()),
            jsonl_path,
            pending_entries: Vec::new(),
        }
    }

    fn handle(&mut self, command: RecordCommand) -> bool {
        match command {
            RecordCommand::Buffer(entry) => {
                self.pending_entries.push(entry);
                false
            }
            RecordCommand::Persist { ack } | RecordCommand::Flush { ack } => {
                let _ = ack.send(self.persist_pending());
                false
            }
            RecordCommand::Shutdown { ack } => {
                let _ = ack.send(self.persist_pending());
                true
            }
        }
    }

    fn persist_pending(&mut self) -> Result<(), SessionStoreError> {
        if self.pending_entries.is_empty() {
            return Ok(());
        }

        match self.writer.write_batch(&self.pending_entries) {
            Ok(()) => {
                self.pending_entries.clear();
                Ok(())
            }
            Err(error) => {
                self.reset_writer();
                Err(error)
            }
        }
    }

    fn reset_writer(&mut self) {
        self.writer = JsonlWriter::new(self.jsonl_path.clone());
    }
}

fn run_worker(mut rx: mpsc::Receiver<RecordCommand>, jsonl_path: PathBuf) {
    let mut worker = RecorderWorker::new(jsonl_path);

    while let Some(command) = rx.blocking_recv() {
        if worker.handle(command) {
            return;
        }
    }

    if let Err(error) = worker.persist_pending() {
        warn!(error = %error, "session recorder dropped pending entries after channel closed");
    }
}

fn spawn_worker_thread(run: impl FnOnce() + Send + 'static) -> JoinHandle<()> {
    thread::Builder::new()
        .name("session-recorder".to_string())
        .spawn(run)
        .expect("session recorder worker thread should spawn")
}

async fn join_worker(worker_thread: JoinHandle<()>) -> Result<(), SessionStoreError> {
    tokio::task::spawn_blocking(move || join_worker_blocking(worker_thread))
        .await
        .map_err(|_| SessionStoreError::WorkerPanicked)?
}

fn join_worker_blocking(worker_thread: JoinHandle<()>) -> Result<(), SessionStoreError> {
    worker_thread
        .join()
        .map_err(|_| SessionStoreError::WorkerPanicked)
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use provider_protocol::{ConversationItem, Role};
    use tokio::time::{Duration, timeout};
    use uuid::Uuid;

    use crate::{SessionEntry, SessionEntryKind, SessionHeader, SessionId, jsonl::JsonlLoader};

    use super::SessionRecorder;

    #[tokio::test]
    async fn recorder_delays_file_creation_until_persist() {
        let temp_dir = test_temp_dir("delayed-create");
        let jsonl_path = temp_dir.join("session.jsonl");
        let recorder = SessionRecorder::new(jsonl_path.clone());

        recorder
            .buffer(header_entry())
            .expect("header should buffer before first persist");

        assert!(!jsonl_path.exists());

        recorder
            .persist()
            .await
            .expect("persist should create the file lazily");

        assert!(jsonl_path.exists());
        assert_eq!(
            JsonlLoader::load(&jsonl_path).expect("loader should parse persisted header"),
            vec![header_entry()]
        );
    }

    #[tokio::test]
    async fn recorder_flush_persists_all_buffered_entries_in_order() {
        let temp_dir = test_temp_dir("flush-all");
        let jsonl_path = temp_dir.join("session.jsonl");
        let recorder = SessionRecorder::new(jsonl_path.clone());
        let entries = vec![
            header_entry(),
            item_entry("user-1", "header", Role::User, "hello"),
            item_entry("assistant-1", "user-1", Role::Assistant, "hi"),
        ];

        for entry in &entries {
            recorder
                .buffer(entry.clone())
                .expect("buffer should accept fixture entry");
        }

        recorder
            .flush()
            .await
            .expect("flush should persist all entries");

        assert_eq!(
            JsonlLoader::load(&jsonl_path).expect("loader should parse flushed entries"),
            entries
        );
    }

    #[tokio::test]
    async fn recorder_persist_writes_buffered_batch() {
        let temp_dir = test_temp_dir("persist-batch");
        let jsonl_path = temp_dir.join("session.jsonl");
        let recorder = SessionRecorder::new(jsonl_path.clone());
        let entries = vec![
            header_entry(),
            item_entry("assistant-1", "header", Role::Assistant, "first"),
            item_entry("assistant-2", "assistant-1", Role::Assistant, "second"),
        ];

        for entry in &entries {
            recorder
                .buffer(entry.clone())
                .expect("buffer should accept batched entries");
        }

        recorder
            .persist()
            .await
            .expect("persist should write the buffered batch");

        assert_eq!(
            JsonlLoader::load(&jsonl_path).expect("loader should parse persisted entries"),
            entries
        );
    }

    #[tokio::test]
    async fn recorder_reports_backpressure_when_channel_capacity_is_exhausted() {
        let temp_dir = test_temp_dir("backpressure");
        let jsonl_path = temp_dir.join("session.jsonl");
        let (recorder, start_tx) = SessionRecorder::new_paused(jsonl_path);

        for index in 0..256 {
            recorder
                .buffer(item_entry(
                    &format!("entry-{index}"),
                    if index == 0 { "header" } else { "entry-0" },
                    Role::Assistant,
                    "queued",
                ))
                .expect("buffer should fill the bounded channel");
        }

        let error = recorder
            .buffer(item_entry(
                "overflow",
                "entry-0",
                Role::Assistant,
                "overflow",
            ))
            .expect_err("extra buffer should fail once the channel is full");

        assert!(matches!(error, crate::SessionStoreError::ChannelClosed));

        let _ = start_tx.send(());
        recorder
            .shutdown()
            .await
            .expect("paused worker should drain");
    }

    #[tokio::test]
    async fn recorder_recovers_after_a_transient_io_failure() {
        let temp_dir = test_temp_dir("io-recovery");
        let blocking_path = temp_dir.join("blocking-parent");
        fs::write(&blocking_path, "not a directory").expect("fixture file should exist");
        let jsonl_path = blocking_path.join("session.jsonl");
        let recorder = SessionRecorder::new(jsonl_path.clone());
        let header = header_entry();

        recorder
            .buffer(header.clone())
            .expect("header should buffer before failed persist");

        let error = recorder
            .persist()
            .await
            .expect_err("persist should fail while parent path is a file");
        assert!(matches!(error, crate::SessionStoreError::IoError { .. }));
        assert!(!jsonl_path.exists());

        fs::remove_file(&blocking_path).expect("blocking file should be removable");
        fs::create_dir_all(&blocking_path).expect("parent directory should become writable");

        recorder
            .flush()
            .await
            .expect("flush should retry pending entries");

        assert_eq!(
            JsonlLoader::load(&jsonl_path).expect("loader should parse recovered file"),
            vec![header]
        );
    }

    #[tokio::test]
    async fn recorder_shutdown_drains_pending_entries() {
        let temp_dir = test_temp_dir("shutdown-drain");
        let jsonl_path = temp_dir.join("session.jsonl");
        let recorder = SessionRecorder::new(jsonl_path.clone());
        let entries = vec![
            header_entry(),
            item_entry(
                "assistant-1",
                "header",
                Role::Assistant,
                "saved on shutdown",
            ),
        ];

        for entry in &entries {
            recorder
                .buffer(entry.clone())
                .expect("buffer should accept shutdown fixture");
        }

        recorder
            .shutdown()
            .await
            .expect("shutdown should drain pending entries");

        assert_eq!(
            JsonlLoader::load(&jsonl_path).expect("loader should parse shutdown output"),
            entries
        );
    }

    #[tokio::test]
    async fn recorder_drop_flushes_pending_entries_without_hanging() {
        let temp_dir = test_temp_dir("drop-flush");
        let jsonl_path = temp_dir.join("session.jsonl");
        let entries = vec![
            header_entry(),
            item_entry("assistant-1", "header", Role::Assistant, "drop saves"),
        ];

        timeout(Duration::from_secs(2), async {
            let recorder = SessionRecorder::new(jsonl_path.clone());
            for entry in &entries {
                recorder
                    .buffer(entry.clone())
                    .expect("buffer should accept drop fixture");
            }
            drop(recorder);
        })
        .await
        .expect("drop should complete within the timeout");

        assert_eq!(
            JsonlLoader::load(&jsonl_path).expect("loader should parse drop output"),
            entries
        );
    }

    fn header_entry() -> SessionEntry {
        SessionEntry {
            id: "header".to_string(),
            parent_id: None,
            timestamp: 1_717_514_800_000,
            kind: SessionEntryKind::Header(SessionHeader {
                session_id: fixture_session_id(),
                work_dir: PathBuf::from("/repo"),
                initial_model: "gpt-4.1".to_string(),
                git_head: Some("abc123".to_string()),
                cli_version: Some("0.5.4".to_string()),
            }),
        }
    }

    fn item_entry(id: &str, parent_id: &str, role: Role, text: &str) -> SessionEntry {
        SessionEntry {
            id: id.to_string(),
            parent_id: Some(parent_id.to_string()),
            timestamp: 1_717_514_800_001,
            kind: SessionEntryKind::Item(ConversationItem::text(role, text)),
        }
    }

    fn fixture_session_id() -> SessionId {
        "01914a5c-3c7e-7a2b-8abc-1234567890ab"
            .parse()
            .expect("fixture session id should parse")
    }

    fn test_temp_dir(label: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("hunea-session-recorder-{label}-{}", Uuid::now_v7()));
        fs::create_dir_all(&dir).expect("test temp dir should be creatable");
        dir
    }
}
