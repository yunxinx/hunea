//! 外部阻塞 I/O 的同步 runner 边界。
//!
//! TUI runner 本身是同步循环，没有可复用的 tokio executor handle；这里用专用
//! OS worker 承载系统剪贴板等阻塞 I/O。不要把这个模式复制到已经处于 tokio
//! async 上下文的代码里；async 侧应优先使用项目既有 runtime 边界或
//! `spawn_blocking`。

use std::{
    io,
    panic::{AssertUnwindSafe, catch_unwind},
    process::Command,
    sync::{
        Arc, mpsc,
        mpsc::{SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
};

use arboard::Clipboard;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use color_eyre::eyre::Result;

use crate::{AppEvent, ExternalEditorLaunch, Model};

use super::{
    apply_model_event_without_effect,
    terminal::{TerminalSession, TuiTerminal},
};

type SystemClipboardResult = std::result::Result<(), String>;

const CLIPBOARD_JOB_QUEUE_CAPACITY: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ExternalIoEvent {
    SelectionSystemClipboardCopied,
    SelectionSystemClipboardFailed { text: String },
}

pub(super) struct ExternalIoRuntime {
    clipboard_events_sender: mpsc::Sender<ExternalIoEvent>,
    clipboard_events_receiver: mpsc::Receiver<ExternalIoEvent>,
    pending_clipboard_writes: usize,
    clipboard_worker: ClipboardWorker,
}

struct SystemClipboardWriter {
    write: Arc<dyn Fn(&str) -> SystemClipboardResult + Send + Sync + 'static>,
}

struct ClipboardWorker {
    jobs_sender: Option<SyncSender<ClipboardJob>>,
    worker_thread: Option<JoinHandle<()>>,
}

struct ClipboardJob {
    text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CopySelectionStartResult {
    Queued,
    QueueFull,
    WorkerStopped,
}

impl ExternalIoRuntime {
    pub(super) fn new() -> Self {
        let (clipboard_events_sender, clipboard_events_receiver) = mpsc::channel();
        let clipboard_worker = ClipboardWorker::start(
            clipboard_events_sender.clone(),
            SystemClipboardWriter::new(copy_selection_to_system_clipboard),
        );
        Self {
            clipboard_events_sender,
            clipboard_events_receiver,
            pending_clipboard_writes: 0,
            clipboard_worker,
        }
    }

    #[cfg(test)]
    fn with_system_clipboard_writer(
        writer: impl Fn(&str) -> SystemClipboardResult + Send + Sync + 'static,
    ) -> Self {
        let (clipboard_events_sender, clipboard_events_receiver) = mpsc::channel();
        let clipboard_worker = ClipboardWorker::start(
            clipboard_events_sender.clone(),
            SystemClipboardWriter::new(writer),
        );
        Self {
            clipboard_events_sender,
            clipboard_events_receiver,
            pending_clipboard_writes: 0,
            clipboard_worker,
        }
    }

    pub(super) fn start_copy_selection(&mut self, text: String) -> CopySelectionStartResult {
        match self.clipboard_worker.submit(text) {
            Ok(()) => {
                self.pending_clipboard_writes = self.pending_clipboard_writes.saturating_add(1);
                CopySelectionStartResult::Queued
            }
            Err(ClipboardSubmitError::QueueFull) => CopySelectionStartResult::QueueFull,
            Err(ClipboardSubmitError::WorkerStopped { text }) => {
                if self
                    .clipboard_events_sender
                    .send(ExternalIoEvent::SelectionSystemClipboardFailed { text })
                    .is_ok()
                {
                    self.pending_clipboard_writes = self.pending_clipboard_writes.saturating_add(1);
                    CopySelectionStartResult::Queued
                } else {
                    CopySelectionStartResult::WorkerStopped
                }
            }
        }
    }

    pub(super) const fn has_pending_work(&self) -> bool {
        self.pending_clipboard_writes > 0
    }

    pub(super) fn drain_events(&mut self) -> Vec<ExternalIoEvent> {
        let events = self
            .clipboard_events_receiver
            .try_iter()
            .collect::<Vec<_>>();
        self.pending_clipboard_writes = self.pending_clipboard_writes.saturating_sub(events.len());
        events
    }

    pub(super) fn shutdown_and_drain_events(&mut self) -> Vec<ExternalIoEvent> {
        self.clipboard_worker.shutdown();
        self.drain_events()
    }
}

enum ClipboardSubmitError {
    QueueFull,
    WorkerStopped { text: String },
}

impl Default for ExternalIoRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemClipboardWriter {
    fn new(writer: impl Fn(&str) -> SystemClipboardResult + Send + Sync + 'static) -> Self {
        Self {
            write: Arc::new(writer),
        }
    }

    fn write(&self, text: &str) -> SystemClipboardResult {
        (self.write)(text)
    }
}

impl ClipboardWorker {
    fn start(events_sender: mpsc::Sender<ExternalIoEvent>, writer: SystemClipboardWriter) -> Self {
        let (jobs_sender, jobs_receiver) =
            mpsc::sync_channel::<ClipboardJob>(CLIPBOARD_JOB_QUEUE_CAPACITY);
        let worker_thread = thread::spawn(move || {
            for job in jobs_receiver {
                let write_result = catch_unwind(AssertUnwindSafe(|| writer.write(&job.text)));
                let event = match write_result {
                    Ok(Ok(())) => ExternalIoEvent::SelectionSystemClipboardCopied,
                    Ok(Err(_)) | Err(_) => {
                        ExternalIoEvent::SelectionSystemClipboardFailed { text: job.text }
                    }
                };
                if events_sender.send(event).is_err() {
                    break;
                }
            }
        });
        Self {
            jobs_sender: Some(jobs_sender),
            worker_thread: Some(worker_thread),
        }
    }

    fn submit(&self, text: String) -> std::result::Result<(), ClipboardSubmitError> {
        let Some(jobs_sender) = self.jobs_sender.as_ref() else {
            return Err(ClipboardSubmitError::WorkerStopped { text });
        };
        jobs_sender
            .try_send(ClipboardJob { text })
            .map_err(|error| match error {
                TrySendError::Full(_job) => ClipboardSubmitError::QueueFull,
                TrySendError::Disconnected(job) => {
                    ClipboardSubmitError::WorkerStopped { text: job.text }
                }
            })
    }

    fn shutdown(&mut self) {
        self.jobs_sender.take();
        if let Some(worker_thread) = self.worker_thread.take() {
            let _ = worker_thread.join();
        }
    }
}

impl Drop for ClipboardWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub(super) fn run_external_editor_effect(
    terminal: &mut TuiTerminal,
    model: &mut Model,
    launch: ExternalEditorLaunch,
) -> Result<()> {
    TerminalSession::suspend(terminal)?;
    let failed = run_external_editor_command(&launch.command).is_err();
    TerminalSession::resume(terminal)?;

    let area = terminal.size()?;
    apply_model_event_without_effect(
        model,
        AppEvent::Resized {
            width: area.width,
            height: area.height,
        },
        "external editor terminal resize",
    );
    apply_model_event_without_effect(
        model,
        AppEvent::ExternalEditorFinished {
            draft_path: launch.draft_path,
            original_draft: launch.original_draft,
            failed,
        },
        "external editor finished",
    );
    Ok(())
}

pub(super) fn run_copy_selection_effect(
    model: &mut Model,
    external_io: &mut ExternalIoRuntime,
    text: String,
) -> Result<()> {
    match external_io.start_copy_selection(text) {
        CopySelectionStartResult::Queued => {}
        CopySelectionStartResult::QueueFull | CopySelectionStartResult::WorkerStopped => {
            apply_model_event_without_effect(
                model,
                AppEvent::SelectionCopyCompleted { success: false },
                "selection copy queue rejected",
            );
        }
    }
    Ok(())
}

pub(super) fn apply_external_io_event(
    terminal: &mut TuiTerminal,
    model: &mut Model,
    event: ExternalIoEvent,
) -> Result<()> {
    let success = match event {
        ExternalIoEvent::SelectionSystemClipboardCopied => true,
        ExternalIoEvent::SelectionSystemClipboardFailed { text } => {
            copy_selection_to_terminal_clipboard(terminal, &text).is_ok()
        }
    };
    apply_model_event_without_effect(
        model,
        AppEvent::SelectionCopyCompleted { success },
        "selection copy completed",
    );
    Ok(())
}

fn run_external_editor_command(command: &[String]) -> io::Result<()> {
    if command.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "external editor command is empty",
        ));
    }

    let status = Command::new(&command[0]).args(&command[1..]).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(
            "external editor exited with a failure status",
        ))
    }
}

fn copy_selection_to_system_clipboard(text: &str) -> SystemClipboardResult {
    let mut clipboard = Clipboard::new().map_err(|error| error.to_string())?;
    clipboard.set_text(text).map_err(|error| error.to_string())
}

fn copy_selection_to_terminal_clipboard(terminal: &mut TuiTerminal, text: &str) -> io::Result<()> {
    copy_selection_to_terminal_clipboard_writer(terminal.backend_mut(), text)
}

fn copy_selection_to_terminal_clipboard_writer(
    writer: &mut impl io::Write,
    text: &str,
) -> io::Result<()> {
    let encoded = BASE64_STANDARD.encode(text.as_bytes());
    let sequence = format!("\u{1b}]52;c;{encoded}\u{7}");
    writer.write_all(sequence.as_bytes())?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
            mpsc,
        },
        thread,
        time::{Duration, Instant},
    };

    use super::*;

    #[test]
    fn copy_selection_system_clipboard_write_runs_on_background_thread() {
        let main_thread = thread::current().id();
        let (thread_id_sender, thread_id_receiver) = mpsc::channel();
        let mut external_io = ExternalIoRuntime::with_system_clipboard_writer(move |_text| {
            thread_id_sender
                .send(thread::current().id())
                .expect("test should receive the writer thread id");
            Ok(())
        });

        external_io.start_copy_selection("alpha".to_string());

        let writer_thread = thread_id_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("background clipboard writer should run");
        assert_ne!(
            writer_thread, main_thread,
            "system clipboard I/O must not run on the runner thread"
        );
        assert!(external_io.has_pending_work());
        assert_eq!(
            drain_external_io_events_until(&mut external_io, 1),
            vec![ExternalIoEvent::SelectionSystemClipboardCopied]
        );
        assert!(!external_io.has_pending_work());
    }

    #[test]
    fn copy_selection_reuses_one_background_worker_for_multiple_writes() {
        let (thread_id_sender, thread_id_receiver) = mpsc::channel();
        let mut external_io = ExternalIoRuntime::with_system_clipboard_writer(move |_text| {
            thread_id_sender
                .send(thread::current().id())
                .expect("test should receive every writer thread id");
            Ok(())
        });

        external_io.start_copy_selection("alpha".to_string());
        external_io.start_copy_selection("beta".to_string());

        let events = drain_external_io_events_until(&mut external_io, 2);
        let worker_threads = (0..2)
            .map(|_| {
                thread_id_receiver
                    .recv_timeout(Duration::from_secs(1))
                    .expect("worker should report each clipboard write")
            })
            .collect::<Vec<_>>();

        assert_eq!(
            events,
            vec![
                ExternalIoEvent::SelectionSystemClipboardCopied,
                ExternalIoEvent::SelectionSystemClipboardCopied,
            ]
        );
        assert_eq!(
            worker_threads[0], worker_threads[1],
            "clipboard writes should use one long-lived worker instead of spawning one OS thread per copy"
        );
        assert!(!external_io.has_pending_work());
    }

    #[test]
    fn copy_selection_effect_returns_before_system_clipboard_write_finishes() {
        let (release_sender, release_receiver) = mpsc::channel();
        let release_receiver = Arc::new(Mutex::new(release_receiver));
        let writer_release_receiver = Arc::clone(&release_receiver);
        let watchdog_release_sender = release_sender.clone();
        let mut external_io = ExternalIoRuntime::with_system_clipboard_writer(move |_text| {
            writer_release_receiver
                .lock()
                .expect("test release receiver should not be poisoned")
                .recv()
                .expect("test should release the background writer");
            Ok(())
        });
        thread::spawn(move || {
            thread::sleep(Duration::from_secs(1));
            let _ = watchdog_release_sender.send(());
        });

        let started_at = Instant::now();
        external_io.start_copy_selection("alpha".to_string());

        assert!(
            started_at.elapsed() < Duration::from_millis(500),
            "starting a copy should enqueue work instead of waiting for clipboard I/O"
        );
        assert!(external_io.has_pending_work());

        release_sender
            .send(())
            .expect("test should release the background writer");
        let deadline = Instant::now() + Duration::from_secs(1);
        let events = loop {
            let events = external_io.drain_events();
            if !events.is_empty() {
                break events;
            }
            assert!(
                Instant::now() < deadline,
                "background clipboard writer should complete after release"
            );
            thread::sleep(Duration::from_millis(5));
        };

        assert_eq!(
            events,
            vec![ExternalIoEvent::SelectionSystemClipboardCopied]
        );
    }

    #[test]
    fn copy_selection_rejects_when_clipboard_job_queue_is_full() {
        let (writer_started_sender, writer_started_receiver) = mpsc::channel();
        let (release_sender, release_receiver) = mpsc::channel();
        let release_receiver = Arc::new(Mutex::new(release_receiver));
        let writer_release_receiver = Arc::clone(&release_receiver);
        let mut external_io = ExternalIoRuntime::with_system_clipboard_writer(move |_text| {
            writer_started_sender
                .send(())
                .expect("test should observe each clipboard write start");
            writer_release_receiver
                .lock()
                .expect("test release receiver should not be poisoned")
                .recv()
                .expect("test should release every queued clipboard write");
            Ok(())
        });

        assert_eq!(
            external_io.start_copy_selection("in-flight".to_string()),
            CopySelectionStartResult::Queued
        );
        writer_started_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("first clipboard write should start and block");

        for index in 0..CLIPBOARD_JOB_QUEUE_CAPACITY {
            assert_eq!(
                external_io.start_copy_selection(format!("queued-{index}")),
                CopySelectionStartResult::Queued
            );
        }
        assert_eq!(
            external_io.pending_clipboard_writes,
            CLIPBOARD_JOB_QUEUE_CAPACITY + 1
        );

        assert_eq!(
            external_io.start_copy_selection("overflow".to_string()),
            CopySelectionStartResult::QueueFull
        );
        assert_eq!(
            external_io.pending_clipboard_writes,
            CLIPBOARD_JOB_QUEUE_CAPACITY + 1,
            "rejected copy requests must not keep the runner polling forever"
        );

        for _ in 0..=CLIPBOARD_JOB_QUEUE_CAPACITY {
            release_sender
                .send(())
                .expect("test should release the accepted clipboard writes");
        }
        assert_eq!(
            drain_external_io_events_until(&mut external_io, CLIPBOARD_JOB_QUEUE_CAPACITY + 1),
            vec![ExternalIoEvent::SelectionSystemClipboardCopied; CLIPBOARD_JOB_QUEUE_CAPACITY + 1]
        );
        assert!(!external_io.has_pending_work());
    }

    #[test]
    fn failed_system_clipboard_write_preserves_text_for_terminal_fallback() {
        let mut external_io = ExternalIoRuntime::with_system_clipboard_writer(|_text| {
            Err("clipboard unavailable".to_string())
        });

        external_io.start_copy_selection("alpha".to_string());
        let deadline = Instant::now() + Duration::from_secs(1);
        let events = loop {
            let events = external_io.drain_events();
            if !events.is_empty() {
                break events;
            }
            assert!(
                Instant::now() < deadline,
                "background clipboard writer should report failure"
            );
            thread::sleep(Duration::from_millis(5));
        };

        assert_eq!(
            events,
            vec![ExternalIoEvent::SelectionSystemClipboardFailed {
                text: "alpha".to_string()
            }]
        );
        assert!(
            !external_io.has_pending_work(),
            "failed clipboard writes should not leave the runner in a permanent background-poll state"
        );
    }

    #[test]
    fn panicking_system_clipboard_write_reports_failure_and_keeps_worker_alive() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let writer_attempts = Arc::clone(&attempts);
        let mut external_io = ExternalIoRuntime::with_system_clipboard_writer(move |_text| {
            if writer_attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                panic!("clipboard backend panicked");
            }
            Ok(())
        });

        external_io.start_copy_selection("alpha".to_string());
        external_io.start_copy_selection("beta".to_string());

        assert_eq!(
            drain_external_io_events_until(&mut external_io, 2),
            vec![
                ExternalIoEvent::SelectionSystemClipboardFailed {
                    text: "alpha".to_string()
                },
                ExternalIoEvent::SelectionSystemClipboardCopied,
            ]
        );
        assert!(!external_io.has_pending_work());
    }

    #[test]
    fn shutdown_waits_for_queued_clipboard_writes_and_drains_completion_events() {
        let (release_sender, release_receiver) = mpsc::channel();
        let release_receiver = Arc::new(Mutex::new(release_receiver));
        let writer_release_receiver = Arc::clone(&release_receiver);
        let mut external_io = ExternalIoRuntime::with_system_clipboard_writer(move |_text| {
            writer_release_receiver
                .lock()
                .expect("test release receiver should not be poisoned")
                .recv()
                .expect("test should release the queued clipboard write");
            Ok(())
        });

        external_io.start_copy_selection("alpha".to_string());
        assert!(external_io.has_pending_work());
        release_sender
            .send(())
            .expect("test should release the background writer");

        assert_eq!(
            external_io.shutdown_and_drain_events(),
            vec![ExternalIoEvent::SelectionSystemClipboardCopied]
        );
        assert!(!external_io.has_pending_work());
    }

    #[test]
    fn terminal_clipboard_writer_emits_osc52_sequence() {
        let mut output = Vec::new();

        copy_selection_to_terminal_clipboard_writer(&mut output, "alpha")
            .expect("OSC52 write should succeed");

        assert_eq!(output, b"\x1b]52;c;YWxwaGE=\x07");
    }

    fn drain_external_io_events_until(
        external_io: &mut ExternalIoRuntime,
        expected_count: usize,
    ) -> Vec<ExternalIoEvent> {
        let deadline = Instant::now() + Duration::from_secs(1);
        let mut events = Vec::new();
        while events.len() < expected_count {
            events.extend(external_io.drain_events());
            assert!(
                Instant::now() < deadline,
                "background clipboard worker should report all requested events"
            );
            if events.len() < expected_count {
                thread::sleep(Duration::from_millis(5));
            }
        }
        events
    }
}
