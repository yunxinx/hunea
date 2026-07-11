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
use tokio::sync::{Notify, mpsc as tokio_mpsc};

use super::input::TerminalInputCoalescing;

const MAX_READY_TERMINAL_EVENTS_PER_FRAME: usize = 4096;
const LOOP_EVENT_CHANNEL_CAPACITY: usize = 1024;
const PAGE_SCROLL_BURST_QUIET_PERIOD: Duration = Duration::from_millis(24);
const PAGE_SCROLL_BURST_MAX_READ_PERIOD: Duration = Duration::from_millis(96);

#[derive(Debug)]
pub(super) enum LoopEvent {
    Terminal(Event),
    TerminalInputFailed(io::Error),
    BackgroundReady,
}

struct LoopEventWakeState {
    sender: mpsc::SyncSender<LoopEvent>,
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
        match self.state.sender.try_send(LoopEvent::BackgroundReady) {
            Ok(()) => {}
            Err(mpsc::TrySendError::Full(_) | mpsc::TrySendError::Disconnected(_)) => {
                self.state.is_pending.store(false, Ordering::Release);
            }
        }
    }

    fn mark_delivered(&self) {
        self.state.is_pending.store(false, Ordering::Release);
    }

    #[cfg(test)]
    pub(super) fn disconnected_for_test() -> Self {
        let (sender, receiver) = mpsc::sync_channel(1);
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
    capacity_available: Arc<Notify>,
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
            None => self.receive(timeout)?,
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
                None => self.receive(Some(timeout))?,
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
                Ok(LoopEvent::Terminal(_)) => self.capacity_available.notify_one(),
                Ok(event) => {
                    self.capacity_available.notify_one();
                    self.pending_events.push_back(event);
                }
                Err(mpsc::TryRecvError::Empty) => return Ok(()),
                Err(mpsc::TryRecvError::Disconnected) => return Err(loop_channel_closed()),
            }
        }
    }

    fn receive(&self, timeout: Option<Duration>) -> io::Result<Option<LoopEvent>> {
        let event = receive_loop_event(&self.receiver, timeout)?;
        if event.is_some() {
            self.capacity_available.notify_one();
        }
        Ok(event)
    }

    fn start_with_source_factory<F, S>(source_factory: F) -> io::Result<Self>
    where
        F: FnMut() -> S + Send + 'static,
        S: Stream<Item = io::Result<Event>> + Send + 'static,
    {
        Self::start_with_source_factory_and_capacity(source_factory, LOOP_EVENT_CHANNEL_CAPACITY)
    }

    fn start_with_source_factory_and_capacity<F, S>(
        mut source_factory: F,
        channel_capacity: usize,
    ) -> io::Result<Self>
    where
        F: FnMut() -> S + Send + 'static,
        S: Stream<Item = io::Result<Event>> + Send + 'static,
    {
        let (event_sender, receiver) = mpsc::sync_channel(channel_capacity);
        let (input_control_sender, input_control_receiver) = tokio_mpsc::unbounded_channel();
        let (startup_sender, startup_receiver) = mpsc::sync_channel(1);
        let capacity_available = Arc::new(Notify::new());
        let input_capacity_available = Arc::clone(&capacity_available);
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
                    input_capacity_available,
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
            capacity_available,
            waker,
            input_control_sender: Some(input_control_sender),
            input_thread: Some(input_thread),
        })
    }

    #[cfg(test)]
    pub(super) fn channel_for_test() -> (Self, mpsc::SyncSender<LoopEvent>) {
        Self::channel_for_test_with_capacity(LOOP_EVENT_CHANNEL_CAPACITY)
    }

    #[cfg(test)]
    fn channel_for_test_with_capacity(capacity: usize) -> (Self, mpsc::SyncSender<LoopEvent>) {
        let (sender, receiver) = mpsc::sync_channel(capacity);
        let capacity_available = Arc::new(Notify::new());
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
                capacity_available,
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

    #[cfg(test)]
    fn start_with_source_factory_and_capacity_for_test<F, S>(
        source_factory: F,
        channel_capacity: usize,
    ) -> io::Result<Self>
    where
        F: FnMut() -> S + Send + 'static,
        S: Stream<Item = io::Result<Event>> + Send + 'static,
    {
        Self::start_with_source_factory_and_capacity(source_factory, channel_capacity)
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
    CapacityAvailable,
}

async fn run_terminal_input_loop<F, S>(
    mut source_factory: F,
    initial_source: S,
    mut control_receiver: tokio_mpsc::UnboundedReceiver<TerminalInputCommand>,
    event_sender: mpsc::SyncSender<LoopEvent>,
    capacity_available: Arc<Notify>,
) where
    F: FnMut() -> S,
    S: Stream<Item = io::Result<Event>> + Send + 'static,
{
    let mut source: Option<TerminalEventSource> = Some(Box::pin(initial_source));
    let mut pending_event = None;
    loop {
        let action = if pending_event.is_some() {
            tokio::select! {
                command = control_receiver.recv() => TerminalInputAction::Command(command),
                () = capacity_available.notified() => TerminalInputAction::CapacityAvailable,
            }
        } else {
            match source.as_mut() {
                Some(active_source) => tokio::select! {
                    command = control_receiver.recv() => TerminalInputAction::Command(command),
                    event = active_source.next() => TerminalInputAction::Event(event),
                },
                None => TerminalInputAction::Command(control_receiver.recv().await),
            }
        };

        match action {
            TerminalInputAction::Command(Some(TerminalInputCommand::Pause(ack))) => {
                pending_event = None;
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
                if !try_queue_loop_event(
                    &event_sender,
                    LoopEvent::Terminal(event),
                    &mut pending_event,
                ) {
                    break;
                }
            }
            TerminalInputAction::Event(Some(Err(error))) => {
                source = None;
                if !try_queue_loop_event(
                    &event_sender,
                    LoopEvent::TerminalInputFailed(error),
                    &mut pending_event,
                ) {
                    break;
                }
            }
            TerminalInputAction::Event(None) => {
                source = None;
                if !try_queue_loop_event(
                    &event_sender,
                    LoopEvent::TerminalInputFailed(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "terminal input stream ended",
                    )),
                    &mut pending_event,
                ) {
                    break;
                }
            }
            TerminalInputAction::CapacityAvailable => {
                let Some(event) = pending_event.take() else {
                    continue;
                };
                if !try_queue_loop_event(&event_sender, event, &mut pending_event) {
                    break;
                }
            }
        }
    }
}

fn try_queue_loop_event(
    sender: &mpsc::SyncSender<LoopEvent>,
    event: LoopEvent,
    pending_event: &mut Option<LoopEvent>,
) -> bool {
    match sender.try_send(event) {
        Ok(()) => true,
        Err(mpsc::TrySendError::Full(event)) => {
            *pending_event = Some(event);
            true
        }
        Err(mpsc::TrySendError::Disconnected(_)) => false,
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
            atomic::{AtomicBool, AtomicUsize, Ordering},
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
    fn background_wake_returns_immediately_when_loop_queue_is_full() {
        let (mut pump, event_sender) = LoopEventPump::channel_for_test_with_capacity(1);
        event_sender
            .send(LoopEvent::Terminal(Event::Key(KeyEvent::from(
                KeyCode::Char('x'),
            ))))
            .expect("test terminal event should fill queue");

        let waker = pump.waker();
        let (finished_sender, finished_receiver) = mpsc::sync_channel(1);
        thread::spawn(move || {
            waker.wake();
            let _ = finished_sender.send(());
        });
        finished_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("full queue must not block a background producer");
        assert!(matches!(
            pump.wait(Some(Duration::ZERO))
                .expect("queued terminal event should remain readable"),
            Some(LoopEvent::Terminal(_))
        ));
        pump.waker().wake();
        assert!(matches!(
            pump.wait(Some(Duration::ZERO))
                .expect("wake should retry after queue capacity returns"),
            Some(LoopEvent::BackgroundReady)
        ));
    }

    #[test]
    fn full_terminal_queue_stops_polling_source_until_capacity_returns() {
        let poll_count = Arc::new(AtomicUsize::new(0));
        let factory_poll_count = Arc::clone(&poll_count);
        let mut pump = LoopEventPump::start_with_source_factory_and_capacity_for_test(
            move || AlwaysReadyKeyStream {
                poll_count: Arc::clone(&factory_poll_count),
                was_dropped: Arc::new(AtomicBool::new(false)),
            },
            2,
        )
        .expect("bounded input pump should start");

        thread::sleep(Duration::from_millis(20));
        assert_eq!(
            poll_count.load(Ordering::SeqCst),
            3,
            "two queued events plus one pending event is the bounded maximum"
        );

        assert!(matches!(
            pump.wait(Some(Duration::from_secs(1)))
                .expect("first queued event should be readable"),
            Some(LoopEvent::Terminal(_))
        ));
        let deadline = Instant::now() + Duration::from_secs(1);
        while poll_count.load(Ordering::SeqCst) == 3 && Instant::now() < deadline {
            thread::yield_now();
        }
        assert_eq!(poll_count.load(Ordering::SeqCst), 4);
    }

    #[test]
    fn pausing_full_terminal_queue_preempts_capacity_wait() {
        let was_dropped = Arc::new(AtomicBool::new(false));
        let factory_was_dropped = Arc::clone(&was_dropped);
        let mut pump = LoopEventPump::start_with_source_factory_and_capacity_for_test(
            move || AlwaysReadyKeyStream {
                poll_count: Arc::new(AtomicUsize::new(0)),
                was_dropped: Arc::clone(&factory_was_dropped),
            },
            1,
        )
        .expect("bounded input pump should start");

        thread::sleep(Duration::from_millis(20));
        pump.pause_terminal_input()
            .expect("pause should preempt a pending full-queue event");

        assert!(was_dropped.load(Ordering::SeqCst));
    }

    #[test]
    fn dropping_full_terminal_queue_preempts_capacity_wait() {
        let (finished_sender, finished_receiver) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let pump = LoopEventPump::start_with_source_factory_and_capacity_for_test(
                move || AlwaysReadyKeyStream {
                    poll_count: Arc::new(AtomicUsize::new(0)),
                    was_dropped: Arc::new(AtomicBool::new(false)),
                },
                1,
            )
            .expect("bounded input pump should start");
            thread::sleep(Duration::from_millis(20));
            drop(pump);
            let _ = finished_sender.send(());
        });

        finished_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("pump drop should not block behind a full terminal queue");
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

    struct AlwaysReadyKeyStream {
        poll_count: Arc<AtomicUsize>,
        was_dropped: Arc<AtomicBool>,
    }

    impl Stream for AlwaysReadyKeyStream {
        type Item = io::Result<Event>;

        fn poll_next(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            self.poll_count.fetch_add(1, Ordering::SeqCst);
            Poll::Ready(Some(Ok(Event::Key(KeyEvent::from(KeyCode::Char('x'))))))
        }
    }

    impl Drop for AlwaysReadyKeyStream {
        fn drop(&mut self) {
            self.was_dropped.store(true, Ordering::SeqCst);
        }
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
