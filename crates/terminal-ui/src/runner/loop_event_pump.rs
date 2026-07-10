use std::{
    collections::VecDeque,
    io,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crossterm::event::{Event, EventStream, KeyCode, MouseEventKind};
use futures_util::{Stream, StreamExt};
use tokio::sync::mpsc as tokio_mpsc;

use super::input::TerminalInputCoalescing;

const MAX_READY_TERMINAL_EVENTS_PER_FRAME: usize = 4096;
const PAGE_SCROLL_BURST_QUIET_PERIOD: Duration = Duration::from_millis(24);
const PAGE_SCROLL_BURST_MAX_READ_PERIOD: Duration = Duration::from_millis(96);

#[derive(Debug)]
pub(super) enum LoopEvent {
    Terminal(Event),
    TerminalInputFailed(io::Error),
    BackgroundReady,
}

struct LoopEventWakeState {
    sender: mpsc::Sender<LoopEvent>,
    is_pending: AtomicBool,
}

/// `LoopEventWaker` 让后台 producer 通知 TUI runner 重新 drain 自身 receiver。
#[derive(Clone)]
pub struct LoopEventWaker {
    state: Arc<LoopEventWakeState>,
}

impl LoopEventWaker {
    /// 合并重复 wake，并在 event pump 已关闭时安静返回。
    pub fn wake(&self) {
        if self.state.is_pending.swap(true, Ordering::AcqRel) {
            return;
        }
        if self.state.sender.send(LoopEvent::BackgroundReady).is_err() {
            self.state.is_pending.store(false, Ordering::Release);
        }
    }

    fn mark_delivered(&self) {
        self.state.is_pending.store(false, Ordering::Release);
    }

    #[cfg(test)]
    pub(super) fn disconnected_for_test() -> Self {
        let (sender, receiver) = mpsc::channel();
        drop(receiver);
        Self {
            state: Arc::new(LoopEventWakeState {
                sender,
                is_pending: AtomicBool::new(false),
            }),
        }
    }
}

pub(super) struct LoopEventPump {
    receiver: mpsc::Receiver<LoopEvent>,
    pending_events: VecDeque<LoopEvent>,
    waker: LoopEventWaker,
    input_control_sender: Option<tokio_mpsc::UnboundedSender<TerminalInputCommand>>,
    input_thread: Option<JoinHandle<()>>,
}

impl LoopEventPump {
    pub(super) fn start() -> io::Result<Self> {
        Self::start_with_source_factory(EventStream::new)
    }

    pub(super) fn waker(&self) -> LoopEventWaker {
        self.waker.clone()
    }

    pub(super) fn wait(&mut self, timeout: Option<Duration>) -> io::Result<Option<LoopEvent>> {
        let event = match self.pending_events.pop_front() {
            Some(event) => Some(event),
            None => receive_loop_event(&self.receiver, timeout)?,
        };
        if matches!(event, Some(LoopEvent::BackgroundReady)) {
            self.waker.mark_delivered();
        }
        Ok(event)
    }

    pub(super) fn collect_terminal_burst(
        &mut self,
        first_event: Event,
        options: TerminalInputCoalescing,
    ) -> io::Result<Vec<Event>> {
        let mut events = vec![first_event];
        let mut page_scroll_burst = PageScrollBurstRead::default();
        page_scroll_burst.observe(events.last().expect("first event is present"), options);

        while events.len() < MAX_READY_TERMINAL_EVENTS_PER_FRAME {
            let timeout = page_scroll_burst.poll_duration();
            let next_event = match self.pending_events.pop_front() {
                Some(event) => Some(event),
                None => receive_loop_event(&self.receiver, Some(timeout))?,
            };
            match next_event {
                Some(LoopEvent::Terminal(event)) => {
                    page_scroll_burst.observe(&event, options);
                    events.push(event);
                }
                Some(LoopEvent::TerminalInputFailed(error)) => return Err(error),
                Some(event @ LoopEvent::BackgroundReady) => {
                    self.pending_events.push_front(event);
                    break;
                }
                None => break,
            }
        }

        Ok(events)
    }

    pub(super) fn pause_terminal_input(&mut self) -> io::Result<()> {
        self.send_input_command(TerminalInputCommand::Pause)?;
        self.discard_queued_terminal_input()
    }

    pub(super) fn resume_terminal_input(&mut self) -> io::Result<()> {
        self.send_input_command(TerminalInputCommand::Resume)
    }

    fn send_input_command(
        &self,
        command: fn(mpsc::SyncSender<io::Result<()>>) -> TerminalInputCommand,
    ) -> io::Result<()> {
        let sender = self.input_control_sender.as_ref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::BrokenPipe,
                "terminal input thread is unavailable",
            )
        })?;
        let (ack_sender, ack_receiver) = mpsc::sync_channel(1);
        sender.send(command(ack_sender)).map_err(|_| {
            io::Error::new(io::ErrorKind::BrokenPipe, "terminal input thread stopped")
        })?;
        ack_receiver.recv().map_err(|_| {
            io::Error::new(
                io::ErrorKind::BrokenPipe,
                "terminal input acknowledgement channel closed",
            )
        })?
    }

    fn discard_queued_terminal_input(&mut self) -> io::Result<()> {
        self.pending_events
            .retain(|event| !matches!(event, LoopEvent::Terminal(_)));
        loop {
            match self.receiver.try_recv() {
                Ok(LoopEvent::Terminal(_)) => {}
                Ok(event) => self.pending_events.push_back(event),
                Err(mpsc::TryRecvError::Empty) => return Ok(()),
                Err(mpsc::TryRecvError::Disconnected) => return Err(loop_channel_closed()),
            }
        }
    }

    fn start_with_source_factory<F, S>(mut source_factory: F) -> io::Result<Self>
    where
        F: FnMut() -> S + Send + 'static,
        S: Stream<Item = io::Result<Event>> + Send + 'static,
    {
        let (event_sender, receiver) = mpsc::channel();
        let (input_control_sender, input_control_receiver) = tokio_mpsc::unbounded_channel();
        let (startup_sender, startup_receiver) = mpsc::sync_channel(1);
        let waker = LoopEventWaker {
            state: Arc::new(LoopEventWakeState {
                sender: event_sender.clone(),
                is_pending: AtomicBool::new(false),
            }),
        };
        let input_thread = thread::Builder::new()
            .name("hunea-terminal-input".to_string())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread().build() {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = startup_sender.send(Err(io::Error::other(format!(
                            "start terminal input runtime: {error}"
                        ))));
                        return;
                    }
                };
                let initial_source = source_factory();
                if startup_sender.send(Ok(())).is_err() {
                    return;
                }
                runtime.block_on(run_terminal_input_loop(
                    source_factory,
                    initial_source,
                    input_control_receiver,
                    event_sender,
                ));
            })?;
        startup_receiver.recv().map_err(|_| {
            io::Error::new(
                io::ErrorKind::BrokenPipe,
                "terminal input thread stopped during startup",
            )
        })??;

        Ok(Self {
            receiver,
            pending_events: VecDeque::new(),
            waker,
            input_control_sender: Some(input_control_sender),
            input_thread: Some(input_thread),
        })
    }

    #[cfg(test)]
    pub(super) fn channel_for_test() -> (Self, mpsc::Sender<LoopEvent>) {
        let (sender, receiver) = mpsc::channel();
        let waker = LoopEventWaker {
            state: Arc::new(LoopEventWakeState {
                sender: sender.clone(),
                is_pending: AtomicBool::new(false),
            }),
        };
        (
            Self {
                receiver,
                pending_events: VecDeque::new(),
                waker,
                input_control_sender: None,
                input_thread: None,
            },
            sender,
        )
    }

    #[cfg(test)]
    fn start_with_source_factory_for_test<F, S>(source_factory: F) -> io::Result<Self>
    where
        F: FnMut() -> S + Send + 'static,
        S: Stream<Item = io::Result<Event>> + Send + 'static,
    {
        Self::start_with_source_factory(source_factory)
    }
}

impl Drop for LoopEventPump {
    fn drop(&mut self) {
        let Some(input_thread) = self.input_thread.take() else {
            return;
        };
        if let Some(sender) = self.input_control_sender.take() {
            let (ack_sender, ack_receiver) = mpsc::sync_channel(1);
            if sender
                .send(TerminalInputCommand::Shutdown(ack_sender))
                .is_ok()
            {
                let _ = ack_receiver.recv();
            }
        }
        let _ = input_thread.join();
    }
}

type TerminalEventSource = Pin<Box<dyn Stream<Item = io::Result<Event>> + Send>>;

enum TerminalInputCommand {
    Pause(mpsc::SyncSender<io::Result<()>>),
    Resume(mpsc::SyncSender<io::Result<()>>),
    Shutdown(mpsc::SyncSender<io::Result<()>>),
}

enum TerminalInputAction {
    Command(Option<TerminalInputCommand>),
    Event(Option<io::Result<Event>>),
}

async fn run_terminal_input_loop<F, S>(
    mut source_factory: F,
    initial_source: S,
    mut control_receiver: tokio_mpsc::UnboundedReceiver<TerminalInputCommand>,
    event_sender: mpsc::Sender<LoopEvent>,
) where
    F: FnMut() -> S,
    S: Stream<Item = io::Result<Event>> + Send + 'static,
{
    let mut source: Option<TerminalEventSource> = Some(Box::pin(initial_source));
    loop {
        let action = match source.as_mut() {
            Some(active_source) => tokio::select! {
                command = control_receiver.recv() => TerminalInputAction::Command(command),
                event = active_source.next() => TerminalInputAction::Event(event),
            },
            None => TerminalInputAction::Command(control_receiver.recv().await),
        };

        match action {
            TerminalInputAction::Command(Some(TerminalInputCommand::Pause(ack))) => {
                source = None;
                let _ = ack.send(Ok(()));
            }
            TerminalInputAction::Command(Some(TerminalInputCommand::Resume(ack))) => {
                if source.is_none() {
                    source = Some(Box::pin(source_factory()));
                }
                let _ = ack.send(Ok(()));
            }
            TerminalInputAction::Command(Some(TerminalInputCommand::Shutdown(ack))) => {
                drop(source.take());
                let _ = ack.send(Ok(()));
                break;
            }
            TerminalInputAction::Command(None) => break,
            TerminalInputAction::Event(Some(Ok(event))) => {
                if event_sender.send(LoopEvent::Terminal(event)).is_err() {
                    break;
                }
            }
            TerminalInputAction::Event(Some(Err(error))) => {
                let _ = event_sender.send(LoopEvent::TerminalInputFailed(error));
                break;
            }
            TerminalInputAction::Event(None) => {
                let _ = event_sender.send(LoopEvent::TerminalInputFailed(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "terminal input stream ended",
                )));
                break;
            }
        }
    }
}

fn receive_loop_event(
    receiver: &mpsc::Receiver<LoopEvent>,
    timeout: Option<Duration>,
) -> io::Result<Option<LoopEvent>> {
    match timeout {
        None => receiver.recv().map(Some).map_err(|_| loop_channel_closed()),
        Some(duration) => match receiver.recv_timeout(duration) {
            Ok(event) => Ok(Some(event)),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(loop_channel_closed()),
        },
    }
}

fn loop_channel_closed() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "TUI loop event channel closed")
}

#[derive(Debug, Default)]
struct PageScrollBurstRead {
    started_at: Option<Instant>,
    quiet_deadline: Option<Instant>,
}

impl PageScrollBurstRead {
    fn observe(&mut self, event: &Event, options: TerminalInputCoalescing) {
        if !options.has_page_scroll_burst_coalescing || !is_page_scroll_event(event) {
            self.clear();
            return;
        }

        let now = Instant::now();
        let started_at = *self.started_at.get_or_insert(now);
        let max_deadline = started_at + PAGE_SCROLL_BURST_MAX_READ_PERIOD;
        self.quiet_deadline = Some((now + PAGE_SCROLL_BURST_QUIET_PERIOD).min(max_deadline));
    }

    fn poll_duration(&self) -> Duration {
        self.quiet_deadline
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
            .unwrap_or(Duration::ZERO)
    }

    fn clear(&mut self) {
        self.started_at = None;
        self.quiet_deadline = None;
    }
}

fn is_page_scroll_event(event: &Event) -> bool {
    match event {
        Event::Mouse(mouse) => matches!(
            mouse.kind,
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
        ),
        Event::Key(key) => {
            key.modifiers.is_empty() && matches!(key.code, KeyCode::Up | KeyCode::Down)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io,
        pin::Pin,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        task::{Context, Poll},
        thread,
        time::{Duration, Instant},
    };

    use crossterm::event::{Event, KeyCode, KeyEvent};
    use futures_util::Stream;

    use super::*;
    use crate::runner::input::TerminalInputCoalescing;

    #[test]
    fn background_wake_releases_an_indefinite_wait() {
        let (mut pump, _event_sender) = LoopEventPump::channel_for_test();
        let waker = pump.waker();
        let wake_thread = thread::spawn(move || waker.wake());

        let event = pump.wait(None).expect("blocking loop wait should wake");

        wake_thread.join().expect("wake thread should finish");
        assert!(matches!(event, Some(LoopEvent::BackgroundReady)));
    }

    #[test]
    fn background_wakes_are_coalesced_until_the_marker_is_delivered() {
        let (mut pump, _event_sender) = LoopEventPump::channel_for_test();
        let waker = pump.waker();

        waker.wake();
        waker.wake();

        assert!(matches!(
            pump.wait(Some(Duration::ZERO))
                .expect("queued wake should be readable"),
            Some(LoopEvent::BackgroundReady)
        ));
        assert!(
            pump.wait(Some(Duration::ZERO))
                .expect("empty queue should be readable")
                .is_none(),
            "duplicate wake should not leave a second marker"
        );

        waker.wake();
        assert!(matches!(
            pump.wait(Some(Duration::ZERO))
                .expect("wake after delivery should be readable"),
            Some(LoopEvent::BackgroundReady)
        ));
    }

    #[test]
    fn timed_wait_returns_none_without_periodic_wake() {
        let (mut pump, _event_sender) = LoopEventPump::channel_for_test();

        assert!(
            pump.wait(Some(Duration::from_millis(1)))
                .expect("timeout should not fail")
                .is_none()
        );
    }

    #[test]
    fn background_wake_interrupts_terminal_scroll_burst_wait() {
        let (mut pump, event_sender) = LoopEventPump::channel_for_test();
        let waker = pump.waker();
        let first = Event::Key(KeyEvent::from(KeyCode::Up));
        event_sender
            .send(LoopEvent::Terminal(first.clone()))
            .expect("test terminal event should queue");
        waker.wake();

        let first = match pump
            .wait(Some(Duration::ZERO))
            .expect("first terminal event should be readable")
        {
            Some(LoopEvent::Terminal(event)) => event,
            other => panic!("expected terminal event, got {other:?}"),
        };
        let started_at = Instant::now();
        let events = pump
            .collect_terminal_burst(
                first,
                TerminalInputCoalescing {
                    has_page_scroll_burst_coalescing: true,
                },
            )
            .expect("terminal burst collection should succeed");

        assert_eq!(events, vec![Event::Key(KeyEvent::from(KeyCode::Up))]);
        assert!(
            started_at.elapsed() < Duration::from_millis(24),
            "background wake should end the quiet-period wait immediately"
        );
        assert!(matches!(
            pump.wait(Some(Duration::ZERO))
                .expect("preserved wake should be readable"),
            Some(LoopEvent::BackgroundReady)
        ));
    }

    #[test]
    fn terminal_burst_preserves_event_order() {
        let (mut pump, event_sender) = LoopEventPump::channel_for_test();
        let first = Event::Key(KeyEvent::from(KeyCode::Char('a')));
        let second = Event::Key(KeyEvent::from(KeyCode::Char('b')));
        event_sender
            .send(LoopEvent::Terminal(first.clone()))
            .expect("first terminal event should queue");
        event_sender
            .send(LoopEvent::Terminal(second.clone()))
            .expect("second terminal event should queue");

        let first = match pump
            .wait(Some(Duration::ZERO))
            .expect("first terminal event should be readable")
        {
            Some(LoopEvent::Terminal(event)) => event,
            other => panic!("expected terminal event, got {other:?}"),
        };

        assert_eq!(
            pump.collect_terminal_burst(first, TerminalInputCoalescing::default())
                .expect("ready terminal events should collect"),
            vec![
                Event::Key(KeyEvent::from(KeyCode::Char('a'))),
                Event::Key(KeyEvent::from(KeyCode::Char('b'))),
            ]
        );
    }

    #[test]
    fn terminal_input_eof_becomes_a_named_loop_error() {
        let mut pump =
            LoopEventPump::start_with_source_factory_for_test(futures_util::stream::empty)
                .expect("fake terminal input thread should start");

        let event = pump
            .wait(Some(Duration::from_secs(1)))
            .expect("terminal input EOF should be delivered");

        assert!(matches!(
            event,
            Some(LoopEvent::TerminalInputFailed(error))
                if error.kind() == io::ErrorKind::UnexpectedEof
                    && error.to_string() == "terminal input stream ended"
        ));
    }

    #[test]
    fn pausing_and_resuming_replaces_the_terminal_event_source() {
        let created_sources = Arc::new(AtomicUsize::new(0));
        let dropped_sources = Arc::new(AtomicUsize::new(0));
        let factory_created_sources = Arc::clone(&created_sources);
        let factory_dropped_sources = Arc::clone(&dropped_sources);
        let mut pump = LoopEventPump::start_with_source_factory_for_test(move || {
            factory_created_sources.fetch_add(1, Ordering::SeqCst);
            CountingPendingStream {
                dropped_sources: Arc::clone(&factory_dropped_sources),
            }
        })
        .expect("fake terminal input thread should start");

        assert_eq!(created_sources.load(Ordering::SeqCst), 1);

        pump.pause_terminal_input()
            .expect("terminal input should pause");
        assert_eq!(dropped_sources.load(Ordering::SeqCst), 1);

        pump.resume_terminal_input()
            .expect("terminal input should resume");
        assert_eq!(created_sources.load(Ordering::SeqCst), 2);

        drop(pump);
        assert_eq!(dropped_sources.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn pausing_discards_queued_terminal_input_but_preserves_background_wake() {
        let (mut pump, event_sender) = LoopEventPump::channel_for_test();
        event_sender
            .send(LoopEvent::Terminal(Event::Key(KeyEvent::from(
                KeyCode::Char('x'),
            ))))
            .expect("test terminal event should queue");
        pump.waker().wake();

        pump.discard_queued_terminal_input()
            .expect("queued terminal input should be discardable");

        assert!(matches!(
            pump.wait(Some(Duration::ZERO))
                .expect("background wake should remain readable"),
            Some(LoopEvent::BackgroundReady)
        ));
        assert!(
            pump.wait(Some(Duration::ZERO))
                .expect("terminal queue should be empty")
                .is_none()
        );
    }

    struct CountingPendingStream {
        dropped_sources: Arc<AtomicUsize>,
    }

    impl Stream for CountingPendingStream {
        type Item = io::Result<Event>;

        fn poll_next(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            Poll::Pending
        }
    }

    impl Drop for CountingPendingStream {
        fn drop(&mut self) {
            self.dropped_sources.fetch_add(1, Ordering::SeqCst);
        }
    }
}
