//! 外部阻塞 I/O 的同步 runner 边界。
//!
//! TUI runner 本身是同步循环，没有可复用的 tokio executor handle；这里用专用
//! OS worker 承载系统剪贴板等阻塞 I/O。不要把这个模式复制到已经处于 tokio
//! async 上下文的代码里；async 侧应优先使用项目既有 runtime 边界或
//! `spawn_blocking`。

use std::{
    cell::Cell,
    io,
    panic::{self, AssertUnwindSafe, catch_unwind},
    process::Command,
    sync::{
        Once, mpsc,
        mpsc::{SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
};

use arboard::Clipboard;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use color_eyre::eyre::Result;

use crate::{AppEffect, AppEvent, ExternalEditorLaunch, Model};

use super::{
    apply_model_event_without_effect,
    terminal::{TerminalSession, TuiTerminal},
};

type SystemClipboardResult<T = ()> = std::result::Result<T, String>;

const CLIPBOARD_JOB_QUEUE_CAPACITY: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ExternalIoEvent {
    SelectionCopiedToSystemClipboard,
    SelectionSystemClipboardFailed { text: String },
    ClipboardWorkerPanicked { text: String },
    ClipboardWorkerStopped { text: String },
}

pub(super) struct ExternalIoRuntime {
    clipboard_events_sender: mpsc::Sender<ExternalIoEvent>,
    clipboard_events_receiver: mpsc::Receiver<ExternalIoEvent>,
    pending_clipboard_writes: usize,
    clipboard_worker: ClipboardWorker,
}

struct SystemClipboardWriter {
    connection: Option<Box<dyn SystemClipboardConnection + Send>>,
    connection_factory:
        Box<dyn FnMut() -> SystemClipboardResult<Box<dyn SystemClipboardConnection + Send>> + Send>,
}

trait SystemClipboardConnection {
    fn set_text(&mut self, text: &str) -> SystemClipboardResult;
}

struct ArboardClipboardConnection {
    clipboard: Clipboard,
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
            SystemClipboardWriter::system(),
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
            SystemClipboardWriter::from_write_fn_for_test(writer),
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
                    .send(ExternalIoEvent::ClipboardWorkerStopped { text })
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
        let events = self.drain_events();
        self.pending_clipboard_writes = 0;
        events
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
    fn system() -> Self {
        Self::with_connection_factory(|| {
            let clipboard = Clipboard::new().map_err(|error| error.to_string())?;
            Ok(Box::new(ArboardClipboardConnection { clipboard }))
        })
    }

    fn with_connection_factory(
        connection_factory: impl FnMut() -> SystemClipboardResult<
            Box<dyn SystemClipboardConnection + Send>,
        > + Send
        + 'static,
    ) -> Self {
        Self {
            connection: None,
            connection_factory: Box::new(connection_factory),
        }
    }

    #[cfg(test)]
    fn from_write_fn_for_test(
        writer: impl Fn(&str) -> SystemClipboardResult + Send + Sync + 'static,
    ) -> Self {
        let writer: std::sync::Arc<dyn Fn(&str) -> SystemClipboardResult + Send + Sync + 'static> =
            std::sync::Arc::new(writer);
        Self::with_connection_factory(move || {
            Ok(Box::new(FunctionClipboardConnection {
                write: std::sync::Arc::clone(&writer),
            }))
        })
    }

    #[cfg(test)]
    fn with_connection_factory_for_test(
        connection_factory: impl FnMut() -> SystemClipboardResult<
            Box<dyn SystemClipboardConnection + Send>,
        > + Send
        + 'static,
    ) -> Self {
        Self::with_connection_factory(connection_factory)
    }

    fn write(&mut self, text: &str) -> SystemClipboardResult {
        if self.connection.is_none() {
            self.connection = Some((self.connection_factory)()?);
        }

        let Some(connection) = self.connection.as_mut() else {
            return Err("system clipboard connection is unavailable".to_string());
        };
        match connection.set_text(text) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.connection.take();
                Err(error)
            }
        }
    }
}

impl SystemClipboardConnection for ArboardClipboardConnection {
    fn set_text(&mut self, text: &str) -> SystemClipboardResult {
        self.clipboard
            .set_text(text)
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
struct FunctionClipboardConnection {
    write: std::sync::Arc<dyn Fn(&str) -> SystemClipboardResult + Send + Sync + 'static>,
}

#[cfg(test)]
impl SystemClipboardConnection for FunctionClipboardConnection {
    fn set_text(&mut self, text: &str) -> SystemClipboardResult {
        (self.write)(text)
    }
}

impl ClipboardWorker {
    fn start(
        events_sender: mpsc::Sender<ExternalIoEvent>,
        mut writer: SystemClipboardWriter,
    ) -> Self {
        let (jobs_sender, jobs_receiver) =
            mpsc::sync_channel::<ClipboardJob>(CLIPBOARD_JOB_QUEUE_CAPACITY);
        let worker_thread = thread::spawn(move || {
            let mut can_write_system_clipboard = true;
            while let Ok(job) = jobs_receiver.recv() {
                let event = if can_write_system_clipboard {
                    match catch_clipboard_worker_panic(|| writer.write(&job.text)) {
                        Ok(Ok(())) => ExternalIoEvent::SelectionCopiedToSystemClipboard,
                        Ok(Err(_)) => {
                            ExternalIoEvent::SelectionSystemClipboardFailed { text: job.text }
                        }
                        Err(ClipboardWorkerPanic) => {
                            can_write_system_clipboard = false;
                            ExternalIoEvent::ClipboardWorkerPanicked { text: job.text }
                        }
                    }
                } else {
                    ExternalIoEvent::ClipboardWorkerStopped { text: job.text }
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

    fn submit(&mut self, text: String) -> std::result::Result<(), ClipboardSubmitError> {
        self.reap_finished_thread();
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
        self.reap_finished_thread();
        if self.worker_thread.is_some() {
            self.worker_thread.take();
        }
    }

    fn reap_finished_thread(&mut self) {
        let Some(worker_thread) = self.worker_thread.as_ref() else {
            return;
        };
        if worker_thread.is_finished() {
            if let Some(worker_thread) = self.worker_thread.take() {
                let _ = worker_thread.join();
            }
            self.jobs_sender.take();
        }
    }
}

impl Drop for ClipboardWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct ClipboardWorkerPanic;

thread_local! {
    static SUPPRESS_CLIPBOARD_WORKER_PANIC_HOOK: Cell<bool> = const { Cell::new(false) };
}

static INSTALL_CLIPBOARD_WORKER_PANIC_HOOK: Once = Once::new();

struct ClipboardWorkerPanicHookGuard {
    previous_suppression_state: bool,
}

impl ClipboardWorkerPanicHookGuard {
    fn enter() -> Self {
        install_clipboard_worker_panic_hook();
        let previous_suppression_state = SUPPRESS_CLIPBOARD_WORKER_PANIC_HOOK.with(|flag| {
            let previous_suppression_state = flag.get();
            flag.set(true);
            previous_suppression_state
        });
        Self {
            previous_suppression_state,
        }
    }
}

impl Drop for ClipboardWorkerPanicHookGuard {
    fn drop(&mut self) {
        SUPPRESS_CLIPBOARD_WORKER_PANIC_HOOK.with(|flag| flag.set(self.previous_suppression_state));
    }
}

fn install_clipboard_worker_panic_hook() {
    INSTALL_CLIPBOARD_WORKER_PANIC_HOOK.call_once(|| {
        let previous_hook = panic::take_hook();
        panic::set_hook(Box::new(move |panic_info| {
            // `catch_unwind` 之前会先运行 panic hook；只静默 clipboard worker
            // 已标记的异常边界，避免默认 hook 把 panic 文本写进 TUI 终端。
            let suppress_hook = SUPPRESS_CLIPBOARD_WORKER_PANIC_HOOK.with(|flag| flag.get());
            if !suppress_hook {
                previous_hook(panic_info);
            }
        }));
    });
}

fn catch_clipboard_worker_panic(
    write: impl FnOnce() -> SystemClipboardResult,
) -> std::result::Result<SystemClipboardResult, ClipboardWorkerPanic> {
    let _panic_hook_guard = ClipboardWorkerPanicHookGuard::enter();
    match catch_unwind(AssertUnwindSafe(write)) {
        Ok(result) => Ok(result),
        Err(panic_payload) => {
            // panic payload 的 Drop 也允许 panic；这里不需要读取 payload，避免在
            // 已经处理的异常边界上触发第二次 unwinding。
            std::mem::forget(panic_payload);
            Err(ClipboardWorkerPanic)
        }
    }
}

pub(super) fn run_external_editor_effect(
    terminal: &mut TuiTerminal,
    model: &mut Model,
    launch: ExternalEditorLaunch,
) -> Result<Option<AppEffect>> {
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
    Ok(model.update(AppEvent::ExternalEditorFinished {
        draft_path: launch.draft_path,
        original_draft: launch.original_draft,
        failed,
    }))
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
        ExternalIoEvent::SelectionCopiedToSystemClipboard => true,
        ExternalIoEvent::SelectionSystemClipboardFailed { text }
        | ExternalIoEvent::ClipboardWorkerPanicked { text }
        | ExternalIoEvent::ClipboardWorkerStopped { text } => {
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
            vec![ExternalIoEvent::SelectionCopiedToSystemClipboard]
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
                ExternalIoEvent::SelectionCopiedToSystemClipboard,
                ExternalIoEvent::SelectionCopiedToSystemClipboard,
            ]
        );
        assert_eq!(
            worker_threads[0], worker_threads[1],
            "clipboard writes should use one long-lived worker instead of spawning one OS thread per copy"
        );
        assert!(!external_io.has_pending_work());
    }

    #[test]
    fn system_clipboard_writer_reuses_one_connection_for_multiple_writes() {
        let connection_count = Arc::new(AtomicUsize::new(0));
        let writes = Arc::new(Mutex::new(Vec::new()));
        let factory_connection_count = Arc::clone(&connection_count);
        let factory_writes = Arc::clone(&writes);
        let mut writer = SystemClipboardWriter::with_connection_factory_for_test(move || {
            factory_connection_count.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(RecordingClipboardConnection {
                writes: Arc::clone(&factory_writes),
                fail_first_write: false,
            }))
        });

        assert_eq!(writer.write("alpha"), Ok(()));
        assert_eq!(writer.write("beta"), Ok(()));

        assert_eq!(
            connection_count.load(Ordering::SeqCst),
            1,
            "system clipboard writer should keep one clipboard connection across writes"
        );
        assert_eq!(
            writes
                .lock()
                .expect("writes lock should not poison")
                .as_slice(),
            ["alpha".to_string(), "beta".to_string()]
        );
    }

    #[test]
    fn system_clipboard_writer_recreates_connection_after_write_failure() {
        let connection_count = Arc::new(AtomicUsize::new(0));
        let writes = Arc::new(Mutex::new(Vec::new()));
        let factory_connection_count = Arc::clone(&connection_count);
        let factory_writes = Arc::clone(&writes);
        let mut writer = SystemClipboardWriter::with_connection_factory_for_test(move || {
            let connection_index = factory_connection_count.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(RecordingClipboardConnection {
                writes: Arc::clone(&factory_writes),
                fail_first_write: connection_index == 0,
            }))
        });

        assert_eq!(
            writer.write("alpha"),
            Err("injected clipboard failure".to_string())
        );
        assert_eq!(writer.write("beta"), Ok(()));

        assert_eq!(
            connection_count.load(Ordering::SeqCst),
            2,
            "failed clipboard connection should be discarded before the next write"
        );
        assert_eq!(
            writes
                .lock()
                .expect("writes lock should not poison")
                .as_slice(),
            ["beta".to_string()]
        );
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
            vec![ExternalIoEvent::SelectionCopiedToSystemClipboard]
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
            vec![
                ExternalIoEvent::SelectionCopiedToSystemClipboard;
                CLIPBOARD_JOB_QUEUE_CAPACITY + 1
            ]
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
    fn panicking_system_clipboard_write_stops_worker_and_fails_queued_writes() {
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
                ExternalIoEvent::ClipboardWorkerPanicked {
                    text: "alpha".to_string()
                },
                ExternalIoEvent::ClipboardWorkerStopped {
                    text: "beta".to_string()
                },
            ]
        );
        assert!(!external_io.has_pending_work());

        assert_eq!(
            external_io.start_copy_selection("gamma".to_string()),
            CopySelectionStartResult::Queued
        );
        assert_eq!(
            drain_external_io_events_until(&mut external_io, 1),
            vec![ExternalIoEvent::ClipboardWorkerStopped {
                text: "gamma".to_string()
            }]
        );
    }

    #[test]
    fn shutdown_detaches_blocked_clipboard_writer_and_drains_ready_events() {
        let (release_sender, release_receiver) = mpsc::channel::<()>();
        let release_receiver = Arc::new(Mutex::new(release_receiver));
        let writer_release_receiver = Arc::clone(&release_receiver);
        let mut external_io = ExternalIoRuntime::with_system_clipboard_writer(move |_text| {
            writer_release_receiver
                .lock()
                .expect("test release receiver should not be poisoned")
                .recv()
                .ok();
            Ok(())
        });

        external_io.start_copy_selection("alpha".to_string());
        assert!(external_io.has_pending_work());

        let started_at = Instant::now();
        let shutdown_events = external_io.shutdown_and_drain_events();

        assert_eq!(
            shutdown_events,
            Vec::new(),
            "shutdown should only drain events that were ready before shutdown"
        );
        assert!(!external_io.has_pending_work());
        assert!(
            started_at.elapsed() < Duration::from_millis(500),
            "TUI shutdown must not wait for a blocked system clipboard backend"
        );

        drop(release_sender);
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

    struct RecordingClipboardConnection {
        writes: Arc<Mutex<Vec<String>>>,
        fail_first_write: bool,
    }

    impl SystemClipboardConnection for RecordingClipboardConnection {
        fn set_text(&mut self, text: &str) -> SystemClipboardResult {
            if self.fail_first_write {
                self.fail_first_write = false;
                return Err("injected clipboard failure".to_string());
            }
            self.writes
                .lock()
                .expect("writes lock should not poison")
                .push(text.to_string());
            Ok(())
        }
    }
}
