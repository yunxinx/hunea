use std::{
    collections::BTreeMap,
    fmt,
    io::{ErrorKind, Read},
    mem,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use agent_client_protocol::schema::{
    CreateTerminalRequest, CreateTerminalResponse, KillTerminalRequest, KillTerminalResponse,
    ReleaseTerminalRequest, ReleaseTerminalResponse, TerminalExitStatus, TerminalId,
    TerminalOutputRequest, TerminalOutputResponse, WaitForTerminalExitRequest,
    WaitForTerminalExitResponse,
};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use tokio::sync::watch;

use super::{AcpSessionEvent, AcpTerminalExitStatus, AcpTerminalSnapshot};

const DEFAULT_TERMINAL_OUTPUT_BYTE_LIMIT: usize = 1024 * 1024;
const TERMINAL_OUTPUT_UPDATE_INTERVAL: Duration = Duration::from_millis(50);
const DEFAULT_TERMINAL_ROWS: u16 = 24;
const DEFAULT_TERMINAL_COLS: u16 = 80;
static NEXT_TERMINAL_MANAGER_ID: AtomicUsize = AtomicUsize::new(1);

/// `AcpTerminalManager` 持有 ACP terminal 进程与输出缓冲。
#[derive(Clone)]
pub(crate) struct AcpTerminalManager {
    inner: Arc<Mutex<AcpTerminalManagerInner>>,
    event_tx: mpsc::Sender<AcpSessionEvent>,
    agent_id: String,
}

struct AcpTerminalManagerInner {
    manager_id: usize,
    next_id: usize,
    terminals: BTreeMap<String, AcpTerminalSession>,
}

impl AcpTerminalManagerInner {
    fn new(manager_id: usize) -> Self {
        Self {
            manager_id,
            next_id: 0,
            terminals: BTreeMap::new(),
        }
    }
}

struct AcpTerminalSession {
    output: AcpTerminalOutputBuffer,
    exit_status: Option<AcpTerminalExitStatus>,
    released: bool,
    exit_tx: watch::Sender<Option<AcpTerminalExitStatus>>,
    exit_rx: watch::Receiver<Option<AcpTerminalExitStatus>>,
    killer: Option<Box<dyn portable_pty::ChildKiller + Send + Sync>>,
    #[cfg(unix)]
    process_group_id: Option<u32>,
    _master: Box<dyn portable_pty::MasterPty + Send>,
    _slave: Option<Box<dyn portable_pty::SlavePty + Send>>,
}

impl AcpTerminalManager {
    pub(crate) fn new(
        agent_id: impl Into<String>,
        event_tx: mpsc::Sender<AcpSessionEvent>,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(AcpTerminalManagerInner::new(
                NEXT_TERMINAL_MANAGER_ID.fetch_add(1, Ordering::Relaxed),
            ))),
            event_tx,
            agent_id: agent_id.into(),
        }
    }

    pub(crate) async fn create(
        &self,
        request: CreateTerminalRequest,
    ) -> Result<CreateTerminalResponse, AcpTerminalError> {
        validate_create_request(&request)?;
        let terminal_id = self.reserve_terminal_id();
        let terminal_id_for_spawn = terminal_id.clone();
        let manager = self.clone();
        tokio::task::spawn_blocking(move || manager.spawn_terminal(terminal_id_for_spawn, request))
            .await
            .map_err(|error| AcpTerminalError::Spawn(error.to_string()))??;
        Ok(CreateTerminalResponse::new(TerminalId::new(terminal_id)))
    }

    pub(crate) fn output(
        &self,
        request: TerminalOutputRequest,
    ) -> Result<TerminalOutputResponse, AcpTerminalError> {
        let snapshot = self
            .snapshot(&request.terminal_id.to_string())
            .ok_or_else(|| AcpTerminalError::NotFound(request.terminal_id.to_string()))?;
        Ok(terminal_output_response_from_snapshot(snapshot))
    }

    pub(crate) async fn wait_for_exit(
        &self,
        request: WaitForTerminalExitRequest,
    ) -> Result<WaitForTerminalExitResponse, AcpTerminalError> {
        let terminal_id = request.terminal_id.to_string();
        let mut exit_rx = {
            let guard = self
                .inner
                .lock()
                .expect("ACP terminal manager lock should not be poisoned");
            let session = guard
                .terminals
                .get(&terminal_id)
                .ok_or_else(|| AcpTerminalError::NotFound(terminal_id.clone()))?;
            if let Some(exit_status) = session.exit_status.clone() {
                return Ok(WaitForTerminalExitResponse::new(sdk_terminal_exit_status(
                    exit_status,
                )));
            }
            session.exit_rx.clone()
        };

        loop {
            if let Some(exit_status) = exit_rx.borrow().clone() {
                return Ok(WaitForTerminalExitResponse::new(sdk_terminal_exit_status(
                    exit_status,
                )));
            }
            exit_rx
                .changed()
                .await
                .map_err(|_| AcpTerminalError::NotFound(terminal_id.clone()))?;
        }
    }

    pub(crate) fn kill(
        &self,
        request: KillTerminalRequest,
    ) -> Result<KillTerminalResponse, AcpTerminalError> {
        let terminal_id = request.terminal_id.to_string();
        let mut guard = self
            .inner
            .lock()
            .expect("ACP terminal manager lock should not be poisoned");
        let session = guard
            .terminals
            .get_mut(&terminal_id)
            .ok_or_else(|| AcpTerminalError::NotFound(terminal_id.clone()))?;
        kill_session_process(session);
        Ok(KillTerminalResponse::new())
    }

    pub(crate) fn release(
        &self,
        request: ReleaseTerminalRequest,
    ) -> Result<ReleaseTerminalResponse, AcpTerminalError> {
        let terminal_id = request.terminal_id.to_string();
        let snapshot = {
            let mut guard = self
                .inner
                .lock()
                .expect("ACP terminal manager lock should not be poisoned");
            let mut session = guard
                .terminals
                .remove(&terminal_id)
                .ok_or_else(|| AcpTerminalError::NotFound(terminal_id.clone()))?;
            session.released = true;
            kill_session_process(&mut session);
            session.snapshot(&terminal_id)
        };
        self.send_snapshot(snapshot);
        Ok(ReleaseTerminalResponse::new())
    }

    pub(crate) fn release_all_for_shutdown(&self) {
        let snapshots = {
            let mut guard = self
                .inner
                .lock()
                .expect("ACP terminal manager lock should not be poisoned");
            mem::take(&mut guard.terminals)
                .into_iter()
                .map(|(terminal_id, mut session)| {
                    session.released = true;
                    kill_session_process(&mut session);
                    session.snapshot(&terminal_id)
                })
                .collect::<Vec<_>>()
        };

        for snapshot in snapshots {
            self.send_snapshot(snapshot);
        }
    }

    fn reserve_terminal_id(&self) -> String {
        let mut guard = self
            .inner
            .lock()
            .expect("ACP terminal manager lock should not be poisoned");
        let id = format!("term-{}-{}", guard.manager_id, guard.next_id);
        guard.next_id = guard.next_id.saturating_add(1);
        id
    }

    fn spawn_terminal(
        &self,
        terminal_id: String,
        request: CreateTerminalRequest,
    ) -> Result<(), AcpTerminalError> {
        let pty = native_pty_system();
        let pair = pty.openpty(PtySize {
            rows: DEFAULT_TERMINAL_ROWS,
            cols: DEFAULT_TERMINAL_COLS,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        let (shell, shell_args) = terminal_shell_invocation(&request.command, &request.args);
        let mut command = CommandBuilder::new(shell);
        for arg in shell_args {
            command.arg(arg);
        }
        if let Some(cwd) = request.cwd.as_ref() {
            command.cwd(cwd);
        }
        for env in &request.env {
            command.env(&env.name, &env.value);
        }

        let child = pair.slave.spawn_command(command)?;
        #[cfg(unix)]
        let process_group_id = child.process_id();
        let killer = child.clone_killer();
        let reader = pair.master.try_clone_reader()?;
        let (exit_tx, exit_rx) = watch::channel(None);
        let session = AcpTerminalSession {
            output: AcpTerminalOutputBuffer::new(output_byte_limit(request.output_byte_limit)),
            exit_status: None,
            released: false,
            exit_tx,
            exit_rx,
            killer: Some(killer),
            #[cfg(unix)]
            process_group_id,
            _master: pair.master,
            _slave: cfg!(windows).then_some(pair.slave),
        };

        {
            let mut guard = self
                .inner
                .lock()
                .expect("ACP terminal manager lock should not be poisoned");
            guard.terminals.insert(terminal_id.clone(), session);
        }
        if let Some(snapshot) = self.snapshot(&terminal_id) {
            self.send_snapshot(snapshot);
        }

        self.spawn_reader(terminal_id.clone(), reader);
        self.spawn_waiter(terminal_id, child);
        Ok(())
    }

    fn spawn_reader(&self, terminal_id: String, mut reader: Box<dyn Read + Send>) {
        let manager = self.clone();
        std::thread::spawn(move || {
            let mut last_sent_at = Instant::now()
                .checked_sub(TERMINAL_OUTPUT_UPDATE_INTERVAL)
                .unwrap_or_else(Instant::now);
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(count) => {
                        let snapshot = manager.push_output(&terminal_id, &buf[..count]);
                        if let Some(snapshot) = snapshot
                            && last_sent_at.elapsed() >= TERMINAL_OUTPUT_UPDATE_INTERVAL
                        {
                            manager.send_snapshot(snapshot);
                            last_sent_at = Instant::now();
                        }
                    }
                    Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
            if let Some(snapshot) = manager.snapshot(&terminal_id) {
                manager.send_snapshot(snapshot);
            }
        });
    }

    fn spawn_waiter(
        &self,
        terminal_id: String,
        mut child: Box<dyn portable_pty::Child + Send + Sync>,
    ) {
        let manager = self.clone();
        std::thread::spawn(move || {
            let exit_status = child
                .wait()
                .map(|status| AcpTerminalExitStatus {
                    exit_code: Some(status.exit_code()),
                    signal: None,
                })
                .unwrap_or_else(|_| AcpTerminalExitStatus {
                    exit_code: None,
                    signal: Some("unknown".to_string()),
                });
            if let Some(snapshot) = manager.finish_terminal(&terminal_id, exit_status) {
                manager.send_snapshot(snapshot);
            }
        });
    }

    fn push_output(&self, terminal_id: &str, bytes: &[u8]) -> Option<AcpTerminalSnapshot> {
        let mut guard = self
            .inner
            .lock()
            .expect("ACP terminal manager lock should not be poisoned");
        let session = guard.terminals.get_mut(terminal_id)?;
        session.output.push_bytes(bytes);
        Some(session.snapshot(terminal_id))
    }

    fn finish_terminal(
        &self,
        terminal_id: &str,
        exit_status: AcpTerminalExitStatus,
    ) -> Option<AcpTerminalSnapshot> {
        let mut guard = self
            .inner
            .lock()
            .expect("ACP terminal manager lock should not be poisoned");
        let session = guard.terminals.get_mut(terminal_id)?;
        session.exit_status = Some(exit_status);
        let _ = session.exit_tx.send(session.exit_status.clone());
        session.killer = None;
        #[cfg(unix)]
        {
            session.process_group_id = None;
        }
        Some(session.snapshot(terminal_id))
    }

    fn snapshot(&self, terminal_id: &str) -> Option<AcpTerminalSnapshot> {
        let guard = self
            .inner
            .lock()
            .expect("ACP terminal manager lock should not be poisoned");
        guard
            .terminals
            .get(terminal_id)
            .map(|session| session.snapshot(terminal_id))
    }

    fn send_snapshot(&self, snapshot: AcpTerminalSnapshot) {
        let _ = self.event_tx.send(AcpSessionEvent::TerminalUpdated {
            agent_id: self.agent_id.clone(),
            snapshot,
        });
    }
}

/// `AcpTerminalOutputBuffer` 将 PTY 字节转换为 TUI 安全文本并按字节上限保留尾部。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AcpTerminalOutputBuffer {
    lines: Vec<String>,
    cursor_col: usize,
    limit: usize,
    truncated: bool,
    pending_utf8: Vec<u8>,
    ansi_state: AnsiState,
}

impl AcpTerminalOutputBuffer {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            lines: vec![String::new()],
            cursor_col: 0,
            limit,
            truncated: false,
            pending_utf8: Vec::new(),
            ansi_state: AnsiState::Ground,
        }
    }

    pub(crate) fn push_bytes(&mut self, bytes: &[u8]) {
        self.pending_utf8.extend_from_slice(bytes);
        loop {
            match std::str::from_utf8(&self.pending_utf8) {
                Ok(valid) => {
                    let text = valid.to_string();
                    self.pending_utf8.clear();
                    self.push_text(&text);
                    break;
                }
                Err(error) => {
                    let valid_up_to = error.valid_up_to();
                    if valid_up_to > 0 {
                        let valid =
                            String::from_utf8_lossy(&self.pending_utf8[..valid_up_to]).to_string();
                        self.pending_utf8.drain(..valid_up_to);
                        self.push_text(&valid);
                    }
                    if let Some(error_len) = error.error_len() {
                        self.pending_utf8.drain(..error_len);
                        self.push_text("\u{fffd}");
                    } else {
                        break;
                    }
                }
            }
        }
    }

    pub(crate) fn snapshot(&self, terminal_id: impl Into<String>) -> AcpTerminalSnapshot {
        AcpTerminalSnapshot {
            terminal_id: terminal_id.into(),
            output: self.output(),
            truncated: self.truncated,
            exit_status: None,
            released: false,
        }
    }

    fn output(&self) -> String {
        self.lines.join("\n")
    }

    fn push_text(&mut self, text: &str) {
        for ch in text.chars() {
            if let Some(printable) = self.ansi_state.feed(ch) {
                self.push_printable(printable);
            }
        }
        self.enforce_limit();
    }

    fn push_printable(&mut self, ch: char) {
        match ch {
            '\r' => self.cursor_col = 0,
            '\n' => {
                self.lines.push(String::new());
                self.cursor_col = 0;
            }
            '\u{8}' => self.cursor_col = self.cursor_col.saturating_sub(1),
            '\t' => {
                let spaces = 4 - (self.cursor_col % 4);
                for _ in 0..spaces {
                    self.write_char(' ');
                }
            }
            ch if ch.is_control() => {}
            ch => self.write_char(ch),
        }
    }

    fn write_char(&mut self, ch: char) {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        let line = self
            .lines
            .last_mut()
            .expect("terminal output buffer should keep one line");
        let char_count = line.chars().count();
        if self.cursor_col < char_count {
            let start = byte_index_for_char(line, self.cursor_col);
            let end = byte_index_for_char(line, self.cursor_col + 1);
            line.replace_range(start..end, &ch.to_string());
        } else {
            for _ in char_count..self.cursor_col {
                line.push(' ');
            }
            line.push(ch);
        }
        self.cursor_col = self.cursor_col.saturating_add(1);
    }

    fn enforce_limit(&mut self) {
        if self.limit == 0 {
            if self.lines.iter().any(|line| !line.is_empty()) || self.lines.len() > 1 {
                self.truncated = true;
            }
            self.lines.clear();
            self.lines.push(String::new());
            self.cursor_col = 0;
            return;
        }

        let output = self.output();
        if output.len() <= self.limit {
            return;
        }

        self.truncated = true;
        let overflow = output.len().saturating_sub(self.limit);
        let mut start = overflow;
        while start < output.len() && !output.is_char_boundary(start) {
            start += 1;
        }
        let retained = output[start..].to_string();
        self.lines = retained.split('\n').map(str::to_string).collect();
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor_col = self
            .lines
            .last()
            .map(|line| line.chars().count())
            .unwrap_or(0);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnsiState {
    Ground,
    Esc,
    Csi,
    Osc,
    OscEsc,
    Charset,
}

impl AnsiState {
    fn feed(&mut self, ch: char) -> Option<char> {
        match *self {
            Self::Ground => {
                if ch == '\u{1b}' {
                    *self = Self::Esc;
                    None
                } else {
                    Some(ch)
                }
            }
            Self::Esc => match ch {
                '[' => {
                    *self = Self::Csi;
                    None
                }
                ']' => {
                    *self = Self::Osc;
                    None
                }
                '(' | ')' | '*' | '+' => {
                    *self = Self::Charset;
                    None
                }
                _ => {
                    *self = Self::Ground;
                    None
                }
            },
            Self::Csi => {
                if ('\u{40}'..='\u{7e}').contains(&ch) {
                    *self = Self::Ground;
                }
                None
            }
            Self::Osc => match ch {
                '\u{7}' => {
                    *self = Self::Ground;
                    None
                }
                '\u{1b}' => {
                    *self = Self::OscEsc;
                    None
                }
                _ => None,
            },
            Self::OscEsc => {
                *self = Self::Ground;
                None
            }
            Self::Charset => {
                *self = Self::Ground;
                None
            }
        }
    }
}

/// `AcpTerminalError` 描述 ACP terminal 方法处理失败。
#[derive(Debug)]
pub(crate) enum AcpTerminalError {
    InvalidRequest(String),
    NotFound(String),
    Spawn(String),
}

impl fmt::Display for AcpTerminalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRequest(message) => write!(f, "{message}"),
            Self::NotFound(id) => write!(f, "ACP terminal not found: {id}"),
            Self::Spawn(message) => write!(f, "spawn ACP terminal: {message}"),
        }
    }
}

impl std::error::Error for AcpTerminalError {}

impl From<anyhow::Error> for AcpTerminalError {
    fn from(value: anyhow::Error) -> Self {
        Self::Spawn(value.to_string())
    }
}

pub(crate) fn validate_create_request(
    request: &CreateTerminalRequest,
) -> Result<(), AcpTerminalError> {
    if request.command.trim().is_empty() {
        return Err(AcpTerminalError::InvalidRequest(
            "terminal/create command must not be empty".to_string(),
        ));
    }
    if let Some(cwd) = request.cwd.as_ref()
        && !cwd.is_absolute()
    {
        return Err(AcpTerminalError::InvalidRequest(format!(
            "terminal/create cwd must be absolute: {}",
            cwd.display()
        )));
    }
    Ok(())
}

fn output_byte_limit(limit: Option<u64>) -> usize {
    limit
        .and_then(|limit| usize::try_from(limit).ok())
        .unwrap_or(DEFAULT_TERMINAL_OUTPUT_BYTE_LIMIT)
}

fn terminal_shell_invocation(command: &str, args: &[String]) -> (&'static str, Vec<String>) {
    #[cfg(windows)]
    {
        (
            "cmd.exe",
            vec![
                "/S".to_string(),
                "/C".to_string(),
                terminal_shell_command_line(command, args),
            ],
        )
    }

    #[cfg(not(windows))]
    {
        (
            "sh",
            vec!["-c".to_string(), terminal_shell_command_line(command, args)],
        )
    }
}

fn terminal_shell_command_line(command: &str, args: &[String]) -> String {
    let command = command.trim();
    let mut line = if args.is_empty() {
        command.to_string()
    } else {
        shell_quote_arg(command)
    };
    for arg in args {
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(&shell_quote_arg(arg));
    }
    line
}

#[cfg(not(windows))]
fn shell_quote_arg(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/' | ':'))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(windows)]
fn shell_quote_arg(value: &str) -> String {
    if value.is_empty() {
        return "\"\"".to_string();
    }
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/' | ':'))
    {
        return value.to_string();
    }
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn terminal_output_response_from_snapshot(snapshot: AcpTerminalSnapshot) -> TerminalOutputResponse {
    TerminalOutputResponse::new(snapshot.output, snapshot.truncated)
        .exit_status(snapshot.exit_status.map(sdk_terminal_exit_status))
}

fn sdk_terminal_exit_status(status: AcpTerminalExitStatus) -> TerminalExitStatus {
    TerminalExitStatus::new()
        .exit_code(status.exit_code)
        .signal(status.signal)
}

fn kill_session_process(session: &mut AcpTerminalSession) {
    #[cfg(unix)]
    if let Some(process_group_id) = session.process_group_id.take()
        && let Ok(process_group_id) = libc::pid_t::try_from(process_group_id)
    {
        // portable-pty 在 Unix 上会将 PTY 子进程设为新的 session leader，
        // 因此 PID 可作为 PGID，用于清理 shell/REPL 派生出的子进程。
        unsafe {
            let _ = libc::kill(-process_group_id, libc::SIGKILL);
        }
    }
    if let Some(mut killer) = session.killer.take() {
        let _ = killer.kill();
    }
}

impl AcpTerminalSession {
    fn snapshot(&self, terminal_id: &str) -> AcpTerminalSnapshot {
        let mut snapshot = self.output.snapshot(terminal_id);
        snapshot.exit_status = self.exit_status.clone();
        snapshot.released = self.released;
        snapshot
    }
}

fn byte_index_for_char(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map(|(index, _)| index)
        .unwrap_or(text.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_command_line_keeps_command_only_as_shell_line() {
        assert_eq!(
            terminal_shell_command_line("cd /tmp && cargo check", &[]),
            "cd /tmp && cargo check"
        );
    }

    #[test]
    fn terminal_managers_allocate_distinct_terminal_id_namespaces() {
        let (first_tx, _first_rx) = mpsc::channel();
        let (second_tx, _second_rx) = mpsc::channel();
        let first = AcpTerminalManager::new("fake", first_tx);
        let second = AcpTerminalManager::new("fake", second_tx);

        assert_ne!(first.reserve_terminal_id(), second.reserve_terminal_id());
    }

    #[cfg(not(windows))]
    #[test]
    fn shell_command_line_quotes_command_path_when_args_are_present() {
        assert_eq!(
            terminal_shell_command_line(
                "/tmp/tool dir/run",
                &["arg with spaces".to_string(), "plain".to_string()],
            ),
            "'/tmp/tool dir/run' 'arg with spaces' plain"
        );
    }

    #[cfg(windows)]
    #[test]
    fn shell_command_line_quotes_command_path_when_args_are_present() {
        assert_eq!(
            terminal_shell_command_line(
                "C:\\Program Files\\Tool\\run.exe",
                &["arg with spaces".to_string(), "plain".to_string()],
            ),
            "\"C:\\Program Files\\Tool\\run.exe\" \"arg with spaces\" plain"
        );
    }
}
