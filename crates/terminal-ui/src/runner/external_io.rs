use std::{
    io,
    process::Command,
    sync::{Arc, mpsc},
    thread,
};

use arboard::Clipboard;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use color_eyre::eyre::Result;

use crate::{AppEvent, ExternalEditorLaunch, Model};

use super::terminal::{TerminalSession, TuiTerminal};

type SystemClipboardResult = std::result::Result<(), String>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ExternalIoEvent {
    SelectionSystemClipboardCopied,
    SelectionSystemClipboardFailed { text: String },
}

pub(super) struct ExternalIoRuntime {
    clipboard_events_sender: mpsc::Sender<ExternalIoEvent>,
    clipboard_events_receiver: mpsc::Receiver<ExternalIoEvent>,
    pending_clipboard_writes: usize,
    system_clipboard_writer: SystemClipboardWriter,
}

#[derive(Clone)]
struct SystemClipboardWriter {
    write: Arc<dyn Fn(&str) -> SystemClipboardResult + Send + Sync + 'static>,
}

impl ExternalIoRuntime {
    pub(super) fn new() -> Self {
        let (clipboard_events_sender, clipboard_events_receiver) = mpsc::channel();
        Self {
            clipboard_events_sender,
            clipboard_events_receiver,
            pending_clipboard_writes: 0,
            system_clipboard_writer: SystemClipboardWriter::new(copy_selection_to_system_clipboard),
        }
    }

    #[cfg(test)]
    fn with_system_clipboard_writer(
        writer: impl Fn(&str) -> SystemClipboardResult + Send + Sync + 'static,
    ) -> Self {
        let mut runtime = Self::new();
        runtime.system_clipboard_writer = SystemClipboardWriter::new(writer);
        runtime
    }

    pub(super) fn start_copy_selection(&mut self, text: String) {
        self.pending_clipboard_writes = self.pending_clipboard_writes.saturating_add(1);
        let sender = self.clipboard_events_sender.clone();
        let writer = self.system_clipboard_writer.clone();
        thread::spawn(move || {
            let event = match writer.write(&text) {
                Ok(()) => ExternalIoEvent::SelectionSystemClipboardCopied,
                Err(_) => ExternalIoEvent::SelectionSystemClipboardFailed { text },
            };
            let _ = sender.send(event);
        });
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

pub(super) fn run_external_editor_effect(
    terminal: &mut TuiTerminal,
    model: &mut Model,
    launch: ExternalEditorLaunch,
) -> Result<()> {
    TerminalSession::suspend(terminal)?;
    let failed = run_external_editor_command(&launch.command).is_err();
    TerminalSession::resume(terminal)?;

    let area = terminal.size()?;
    let _ = model.update(AppEvent::Resized {
        width: area.width,
        height: area.height,
    });
    let _ = model.update(AppEvent::ExternalEditorFinished {
        draft_path: launch.draft_path,
        original_draft: launch.original_draft,
        failed,
    });
    Ok(())
}

pub(super) fn run_copy_selection_effect(
    external_io: &mut ExternalIoRuntime,
    text: String,
) -> Result<()> {
    external_io.start_copy_selection(text);
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
    let _ = model.update(AppEvent::SelectionCopyCompleted { success });
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
        sync::{Arc, Mutex, mpsc},
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
            external_io.drain_events(),
            vec![ExternalIoEvent::SelectionSystemClipboardCopied]
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
    }

    #[test]
    fn terminal_clipboard_writer_emits_osc52_sequence() {
        let mut output = Vec::new();

        copy_selection_to_terminal_clipboard_writer(&mut output, "alpha")
            .expect("OSC52 write should succeed");

        assert_eq!(output, b"\x1b]52;c;YWxwaGE=\x07");
    }
}
