use std::{
    env,
    ffi::OsString,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;

use serde::Deserialize;
use serde_json::json;
use tokio::{
    fs::OpenOptions,
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
};
use tokio_util::sync::CancellationToken;

use crate::{
    Tool, ToolCall, ToolDefinition, ToolExecutionContext, ToolExecutionFuture, ToolKind,
    ToolPermissionPolicy, ToolProgress, ToolResult, ToolTerminalExitStatus, ToolTerminalSnapshot,
};

const BASH_TOOL_NAME: &str = "bash";
const DEFAULT_MAX_LINES: usize = 2_000;
const DEFAULT_MAX_BYTES: usize = 50 * 1024;
const MAX_ROLLING_BYTES: usize = DEFAULT_MAX_BYTES * 2;
const READ_CHUNK_SIZE: usize = 8 * 1024;
const OUTPUT_UPDATE_THROTTLE: Duration = Duration::from_millis(100);
const IO_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);
const TOOL_CALL_INTERRUPTED: &str = "Tool call interrupted";

/// `bash_tool` 创建 workspace shell 命令执行工具。
pub fn bash_tool(root: impl AsRef<Path>) -> impl Tool + 'static {
    BashTool {
        root: root.as_ref().to_path_buf(),
    }
}

#[derive(Clone)]
struct BashTool {
    root: PathBuf,
}

impl std::fmt::Debug for BashTool {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BashTool")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

impl Tool for BashTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(BASH_TOOL_NAME)
            .with_label("Shell:")
            .with_kind(ToolKind::Execute)
            .with_description(
                "Execute a shell command in the current workspace. Returns merged stdout and stderr. Output is truncated to the last 2000 lines or 50KB, with full output saved to a temp file when truncated.",
            )
            .with_input_schema(json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Shell command to execute"
                    },
                    "timeout": {
                        "type": "number",
                        "minimum": 0.001,
                        "description": "Optional timeout in seconds"
                    },
                    "description": {
                        "type": "string",
                        "description": "Optional description explaining why this command should be run"
                    },
                    "workdir": {
                        "type": "string",
                        "description": "Optional workspace-relative or workspace-contained absolute working directory"
                    }
                },
                "required": ["command"],
                "additionalProperties": false
            }))
            .with_permission_policy(ToolPermissionPolicy::Ask)
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        cancellation: &'a CancellationToken,
    ) -> ToolExecutionFuture<'a> {
        let context = ToolExecutionContext::new(cancellation);
        self.execute_with_context(call, context)
    }

    fn execute_with_context<'a>(
        &'a self,
        call: ToolCall,
        context: ToolExecutionContext<'a>,
    ) -> ToolExecutionFuture<'a> {
        let root = self.root.clone();
        Box::pin(async move { execute_bash(root, call, context).await })
    }
}

#[derive(Debug, Deserialize)]
struct BashArguments {
    command: String,
    timeout: Option<f64>,
    workdir: Option<String>,
}

#[derive(Debug, Clone)]
struct ShellConfig {
    program: PathBuf,
    args: Vec<OsString>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandEndReason {
    Exited,
    TimedOut,
    Cancelled,
}

#[derive(Debug)]
struct CommandExecutionOutcome {
    exit_status: Option<std::process::ExitStatus>,
    end_reason: CommandEndReason,
    output: OutputSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OutputSnapshot {
    content: String,
    truncated: bool,
    truncated_by: Option<&'static str>,
    total_lines: usize,
    total_bytes: usize,
    output_lines: usize,
    output_bytes: usize,
    full_output_path: Option<PathBuf>,
}

async fn execute_bash(
    root: PathBuf,
    call: ToolCall,
    context: ToolExecutionContext<'_>,
) -> ToolResult {
    if context.cancellation().is_cancelled() {
        return ToolResult::error(call.call_id, TOOL_CALL_INTERRUPTED);
    }

    let arguments = match serde_json::from_value::<BashArguments>(call.arguments) {
        Ok(arguments) => arguments,
        Err(error) => {
            return ToolResult::error(call.call_id, format!("bash arguments are invalid: {error}"));
        }
    };
    let command = arguments.command.trim();
    if command.is_empty() {
        return ToolResult::error(call.call_id, "'command' is required");
    }
    let command = command.to_string();

    let cwd = match resolve_workdir(&root, arguments.workdir.as_deref()).await {
        Ok(cwd) => cwd,
        Err(message) => return ToolResult::error(call.call_id, message),
    };
    let timeout = match timeout_duration(arguments.timeout) {
        Ok(timeout) => timeout,
        Err(message) => return ToolResult::error(call.call_id, message),
    };
    let shell = match resolve_shell() {
        Ok(shell) => shell,
        Err(message) => return ToolResult::error(call.call_id, message),
    };

    let terminal_id = call.call_id.clone();
    emit_terminal_snapshot(
        &context,
        TerminalSnapshotData {
            terminal_id: &terminal_id,
            command: &command,
            cwd: &cwd,
            output: "",
            truncated: false,
            exit_status: None,
            released: false,
        },
    );

    let started_at = Instant::now();
    let outcome =
        match run_shell_command(&shell, &cwd, &command, timeout, &terminal_id, &context).await {
            Ok(outcome) => outcome,
            Err(message) => return ToolResult::error(call.call_id, message),
        };
    let duration_ms = u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX);

    let exit_status = terminal_exit_status(outcome.exit_status.as_ref());
    emit_terminal_snapshot(
        &context,
        TerminalSnapshotData {
            terminal_id: &terminal_id,
            command: &command,
            cwd: &cwd,
            output: &outcome.output.content,
            truncated: outcome.output.truncated,
            exit_status: exit_status.clone(),
            released: true,
        },
    );

    let mut details = output_details(&outcome.output, outcome.exit_status.as_ref());
    details["duration_ms"] = json!(duration_ms);
    details["timed_out"] = json!(outcome.end_reason == CommandEndReason::TimedOut);
    details["cancelled"] = json!(outcome.end_reason == CommandEndReason::Cancelled);
    details["cwd"] = json!(cwd.display().to_string());

    let display_output_text = format_display_output(&outcome.output, "(no output)");
    let mut output_text = format_model_output(&outcome.output, "(no output)");
    let is_error = match outcome.end_reason {
        CommandEndReason::Exited => match outcome.exit_status.and_then(|status| status.code()) {
            Some(0) => false,
            Some(code) => {
                append_status(
                    &mut output_text,
                    &format!("Command exited with code {code}"),
                );
                true
            }
            None => {
                append_status(&mut output_text, "Command terminated by signal");
                true
            }
        },
        CommandEndReason::TimedOut => {
            let timeout = arguments.timeout.unwrap_or_default();
            append_status(
                &mut output_text,
                &format!(
                    "Command timed out after {} seconds",
                    format_seconds(timeout)
                ),
            );
            true
        }
        CommandEndReason::Cancelled => {
            append_status(&mut output_text, "Command aborted");
            true
        }
    };

    let mut result = if is_error {
        ToolResult::error(call.call_id, output_text)
    } else {
        ToolResult::success(call.call_id, output_text)
    };
    result.display_content = Some(display_output_text);
    result.details = Some(details);
    result
}

async fn resolve_workdir(root: &Path, requested: Option<&str>) -> Result<PathBuf, String> {
    let root = tokio::fs::canonicalize(root)
        .await
        .map_err(|error| format!("workspace root is unavailable: {error}"))?;
    let candidate = match requested.map(str::trim).filter(|value| !value.is_empty()) {
        Some(requested) => {
            let requested_path = Path::new(requested);
            if requested_path.is_absolute() {
                requested_path.to_path_buf()
            } else {
                root.join(requested_path)
            }
        }
        None => root.clone(),
    };
    let candidate = tokio::fs::canonicalize(&candidate)
        .await
        .map_err(|error| format!("workdir not found: {}: {error}", candidate.display()))?;
    if !candidate.starts_with(&root) {
        return Err(format!(
            "workdir is outside workspace: {}",
            candidate.display()
        ));
    }
    let metadata = tokio::fs::metadata(&candidate)
        .await
        .map_err(|error| format!("stat failed for workdir '{}': {error}", candidate.display()))?;
    if !metadata.is_dir() {
        return Err(format!(
            "workdir is not a directory: {}",
            candidate.display()
        ));
    }
    Ok(candidate)
}

fn timeout_duration(timeout: Option<f64>) -> Result<Option<Duration>, String> {
    let Some(timeout) = timeout else {
        return Ok(None);
    };
    if !timeout.is_finite() || timeout <= 0.0 {
        return Err("'timeout' must be a positive number of seconds".to_string());
    }
    Ok(Some(Duration::from_secs_f64(timeout)))
}

fn resolve_shell() -> Result<ShellConfig, String> {
    #[cfg(windows)]
    {
        resolve_windows_shell()
    }
    #[cfg(not(windows))]
    {
        resolve_unix_shell()
    }
}

#[cfg(not(windows))]
fn resolve_unix_shell() -> Result<ShellConfig, String> {
    if Path::new("/bin/bash").is_file() {
        return Ok(ShellConfig {
            program: PathBuf::from("/bin/bash"),
            args: vec![OsString::from("-c")],
        });
    }
    if let Some(path) = find_on_path("bash") {
        return Ok(ShellConfig {
            program: path,
            args: vec![OsString::from("-c")],
        });
    }
    Ok(ShellConfig {
        program: PathBuf::from("/bin/sh"),
        args: vec![OsString::from("-c")],
    })
}

#[cfg(windows)]
fn resolve_windows_shell() -> Result<ShellConfig, String> {
    let mut candidates = Vec::new();
    if let Some(program_files) = env::var_os("ProgramFiles") {
        candidates.push(PathBuf::from(program_files).join("Git\\bin\\bash.exe"));
    }
    if let Some(program_files_x86) = env::var_os("ProgramFiles(x86)") {
        candidates.push(PathBuf::from(program_files_x86).join("Git\\bin\\bash.exe"));
    }
    for candidate in candidates {
        if candidate.is_file() {
            return Ok(ShellConfig {
                program: candidate,
                args: vec![OsString::from("-c")],
            });
        }
    }
    if let Some(path) = find_on_path("bash.exe").or_else(|| find_on_path("bash")) {
        return Ok(ShellConfig {
            program: path,
            args: vec![OsString::from("-c")],
        });
    }
    Err("No bash shell found. Install Git Bash or add bash.exe to PATH.".to_string())
}

fn find_on_path(binary_name: &str) -> Option<PathBuf> {
    let paths = env::var_os("PATH")?;
    env::split_paths(&paths)
        .map(|directory| directory.join(binary_name))
        .find(|candidate| candidate.is_file())
}

async fn run_shell_command(
    shell: &ShellConfig,
    cwd: &Path,
    command: &str,
    timeout: Option<Duration>,
    terminal_id: &str,
    context: &ToolExecutionContext<'_>,
) -> Result<CommandExecutionOutcome, String> {
    let mut process = Command::new(&shell.program);
    process.args(&shell.args);
    process.arg(command);
    process.current_dir(cwd);
    process.stdin(Stdio::null());
    process.stdout(Stdio::piped());
    process.stderr(Stdio::piped());
    process.kill_on_drop(true);
    #[cfg(unix)]
    process.process_group(0);

    let mut child = process
        .spawn()
        .map_err(|error| format!("spawn shell command failed: {error}"))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "stdout pipe was unexpectedly unavailable".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "stderr pipe was unexpectedly unavailable".to_string())?;
    let (chunk_sender, mut chunk_receiver) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(read_pipe(stdout, chunk_sender.clone()));
    tokio::spawn(read_pipe(stderr, chunk_sender));

    let mut output = OutputAccumulator::new("hunea-bash");
    let mut output_closed = false;
    let mut exit_status = None;
    let mut end_reason = CommandEndReason::Exited;
    let mut timeout_sleep = timeout.map(tokio::time::sleep).map(Box::pin);
    let mut drain_sleep: Option<std::pin::Pin<Box<tokio::time::Sleep>>> = None;
    let mut last_update = std::time::Instant::now()
        .checked_sub(OUTPUT_UPDATE_THROTTLE)
        .unwrap_or_else(std::time::Instant::now);

    loop {
        if exit_status.is_some() && output_closed {
            break;
        }

        tokio::select! {
            maybe_chunk = chunk_receiver.recv(), if !output_closed => {
                if let Some(chunk) = maybe_chunk {
                    output.append(&chunk).await?;
                    if last_update.elapsed() >= OUTPUT_UPDATE_THROTTLE {
                        let snapshot = output.snapshot(false);
                        emit_terminal_snapshot(
                            context,
                            TerminalSnapshotData {
                                terminal_id,
                                command,
                                cwd,
                                output: &snapshot.content,
                                truncated: snapshot.truncated,
                                exit_status: None,
                                released: false,
                            },
                        );
                        last_update = std::time::Instant::now();
                    }
                } else {
                    output_closed = true;
                }
            }
            status = child.wait(), if exit_status.is_none() => {
                exit_status = Some(status.map_err(|error| format!("wait for shell command failed: {error}"))?);
                drain_sleep = Some(Box::pin(tokio::time::sleep(IO_DRAIN_TIMEOUT)));
            }
            _ = context.cancellation().cancelled(), if exit_status.is_none() && end_reason == CommandEndReason::Exited => {
                end_reason = CommandEndReason::Cancelled;
                kill_process_tree(&mut child).await;
            }
            _ = async {
                if let Some(timeout_sleep) = timeout_sleep.as_mut() {
                    timeout_sleep.as_mut().await;
                } else {
                    std::future::pending::<()>().await;
                }
            }, if exit_status.is_none() && end_reason == CommandEndReason::Exited && timeout_sleep.is_some() => {
                end_reason = CommandEndReason::TimedOut;
                kill_process_tree(&mut child).await;
            }
            _ = async {
                if let Some(drain_sleep) = drain_sleep.as_mut() {
                    drain_sleep.as_mut().await;
                } else {
                    std::future::pending::<()>().await;
                }
            }, if exit_status.is_some() && !output_closed && drain_sleep.is_some() => {
                output_closed = true;
            }
        }
    }

    output.finish().await?;
    Ok(CommandExecutionOutcome {
        exit_status,
        end_reason,
        output: output.snapshot(true),
    })
}

async fn read_pipe<R>(mut reader: R, sender: tokio::sync::mpsc::UnboundedSender<Vec<u8>>)
where
    R: AsyncRead + Unpin,
{
    let mut buffer = [0u8; READ_CHUNK_SIZE];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) => break,
            Ok(count) => {
                if sender.send(buffer[..count].to_vec()).is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

async fn kill_process_tree(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    {
        if let Some(pid) = child.id().and_then(|pid| i32::try_from(pid).ok()) {
            // SAFETY: `pid` 来自已启动的 child，并且 child 已通过
            // `process_group(0)` 进入独立 process group；负 pid 只用于杀掉
            // 该 process group。
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
    }

    #[cfg(windows)]
    {
        if let Some(pid) = child.id() {
            let _ = Command::new("taskkill")
                .args(["/F", "/T", "/PID", &pid.to_string()])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
        }
    }

    let _ = child.start_kill();
}

struct OutputAccumulator {
    max_lines: usize,
    max_bytes: usize,
    temp_file_prefix: &'static str,
    prefix_text: String,
    tail_text: String,
    tail_bytes: usize,
    pending_utf8: Vec<u8>,
    total_bytes: usize,
    completed_lines: usize,
    has_open_line: bool,
    temp_file_path: Option<PathBuf>,
    temp_file: Option<tokio::fs::File>,
}

impl OutputAccumulator {
    fn new(temp_file_prefix: &'static str) -> Self {
        Self {
            max_lines: DEFAULT_MAX_LINES,
            max_bytes: DEFAULT_MAX_BYTES,
            temp_file_prefix,
            prefix_text: String::new(),
            tail_text: String::new(),
            tail_bytes: 0,
            pending_utf8: Vec::new(),
            total_bytes: 0,
            completed_lines: 0,
            has_open_line: false,
            temp_file_path: None,
            temp_file: None,
        }
    }

    async fn append(&mut self, bytes: &[u8]) -> Result<(), String> {
        if bytes.is_empty() {
            return Ok(());
        }
        let text = self.decode_complete_utf8(bytes);
        self.append_text(&text).await
    }

    async fn append_text(&mut self, text: &str) -> Result<(), String> {
        if text.is_empty() {
            return Ok(());
        }

        self.observe_text(text);
        if self.temp_file.is_some() || self.should_use_temp_file() {
            self.ensure_temp_file().await?;
            if let Some(file) = &mut self.temp_file {
                file.write_all(text.as_bytes())
                    .await
                    .map_err(|error| format!("write full bash output failed: {error}"))?;
            }
        } else {
            self.prefix_text.push_str(text);
        }

        self.tail_text.push_str(text);
        self.tail_bytes += text.len();
        self.trim_tail();
        Ok(())
    }

    async fn finish(&mut self) -> Result<(), String> {
        let remaining_text = self.decode_pending_utf8();
        self.append_text(&remaining_text).await?;
        if self.should_use_temp_file() {
            self.ensure_temp_file().await?;
        }
        if let Some(file) = &mut self.temp_file {
            file.flush()
                .await
                .map_err(|error| format!("flush full bash output failed: {error}"))?;
        }
        Ok(())
    }

    fn decode_complete_utf8(&mut self, bytes: &[u8]) -> String {
        self.pending_utf8.extend_from_slice(bytes);
        let mut decoded = String::new();

        loop {
            match std::str::from_utf8(&self.pending_utf8) {
                Ok(text) => {
                    decoded.push_str(text);
                    self.pending_utf8.clear();
                    break;
                }
                Err(error) => {
                    let valid_up_to = error.valid_up_to();
                    if valid_up_to > 0 {
                        let text = std::str::from_utf8(&self.pending_utf8[..valid_up_to])
                            .expect("valid_up_to should mark valid UTF-8 prefix");
                        decoded.push_str(text);
                        self.pending_utf8.drain(..valid_up_to);
                        continue;
                    }

                    if let Some(error_len) = error.error_len() {
                        decoded.push(char::REPLACEMENT_CHARACTER);
                        self.pending_utf8.drain(..error_len);
                    } else {
                        break;
                    }
                }
            }
        }

        sanitize_output(&decoded)
    }

    fn decode_pending_utf8(&mut self) -> String {
        if self.pending_utf8.is_empty() {
            return String::new();
        }
        let text = sanitize_output(&String::from_utf8_lossy(&self.pending_utf8));
        self.pending_utf8.clear();
        text
    }

    fn snapshot(&self, persist_if_truncated: bool) -> OutputSnapshot {
        let total_lines = self.total_lines();
        let mut truncated = total_lines > self.max_lines || self.total_bytes > self.max_bytes;
        let truncation = truncate_tail(&self.tail_text, self.max_lines, self.max_bytes);
        if persist_if_truncated && truncated && self.temp_file_path.is_none() {
            truncated = false;
        }
        OutputSnapshot {
            content: if truncated {
                truncation.content
            } else {
                self.tail_text.clone()
            },
            truncated,
            truncated_by: truncated.then_some(truncation.truncated_by),
            total_lines,
            total_bytes: self.total_bytes,
            output_lines: if truncated {
                truncation.output_lines
            } else {
                total_lines
            },
            output_bytes: if truncated {
                truncation.output_bytes
            } else {
                self.total_bytes
            },
            full_output_path: self.temp_file_path.clone(),
        }
    }

    async fn ensure_temp_file(&mut self) -> Result<(), String> {
        if self.temp_file.is_some() {
            return Ok(());
        }
        let (path, mut file) = create_temp_output_file(self.temp_file_prefix).await?;
        if !self.prefix_text.is_empty() {
            file.write_all(self.prefix_text.as_bytes())
                .await
                .map_err(|error| format!("write full bash output failed: {error}"))?;
            self.prefix_text.clear();
        }
        self.temp_file_path = Some(path);
        self.temp_file = Some(file);
        Ok(())
    }

    fn observe_text(&mut self, text: &str) {
        self.total_bytes += text.len();
        let newline_count = text
            .as_bytes()
            .iter()
            .filter(|byte| **byte == b'\n')
            .count();
        self.completed_lines += newline_count;
        self.has_open_line = !text.ends_with('\n');
        if newline_count == 0 && !text.is_empty() {
            self.has_open_line = true;
        }
    }

    fn total_lines(&self) -> usize {
        self.completed_lines + usize::from(self.has_open_line)
    }

    fn should_use_temp_file(&self) -> bool {
        self.total_lines() > self.max_lines || self.total_bytes > self.max_bytes
    }

    fn trim_tail(&mut self) {
        if self.tail_bytes <= MAX_ROLLING_BYTES {
            return;
        }
        let mut start = self.tail_text.len().saturating_sub(MAX_ROLLING_BYTES);
        while start < self.tail_text.len() && !self.tail_text.is_char_boundary(start) {
            start += 1;
        }
        self.tail_text = self.tail_text[start..].to_string();
        self.tail_bytes = self.tail_text.len();
    }
}

#[derive(Debug)]
struct TailTruncation {
    content: String,
    truncated_by: &'static str,
    output_lines: usize,
    output_bytes: usize,
}

fn truncate_tail(text: &str, max_lines: usize, max_bytes: usize) -> TailTruncation {
    let lines = text.lines().collect::<Vec<_>>();
    let total_lines = lines.len();
    let mut selected = Vec::new();
    let mut selected_bytes = 0usize;
    let mut truncated_by = "lines";

    for line in lines.iter().rev().take(max_lines) {
        let line_bytes = line.len() + usize::from(!selected.is_empty());
        if selected_bytes + line_bytes > max_bytes {
            truncated_by = "bytes";
            break;
        }
        selected.push(*line);
        selected_bytes += line_bytes;
    }
    selected.reverse();

    if selected.is_empty() && lines.last().is_some() {
        truncated_by = "bytes";
        let last = lines.last().copied().unwrap_or_default();
        let start = byte_tail_start(last, max_bytes);
        selected.push(&last[start..]);
        selected_bytes = selected[0].len();
    }

    let content = selected.join("\n");
    TailTruncation {
        content,
        truncated_by,
        output_lines: selected.len().min(total_lines),
        output_bytes: selected_bytes,
    }
}

fn byte_tail_start(text: &str, max_bytes: usize) -> usize {
    let mut start = text.len().saturating_sub(max_bytes);
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    start
}

async fn create_temp_output_file(prefix: &str) -> Result<(PathBuf, tokio::fs::File), String> {
    for attempt in 0..16 {
        let path = temp_output_path(prefix, attempt);
        match open_new_private_file(&path).await {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("create full bash output failed: {error}")),
        }
    }

    Err("create full bash output failed: temp file name collision".to_string())
}

async fn open_new_private_file(path: &Path) -> std::io::Result<tokio::fs::File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        options.mode(0o600);
    }
    options.open(path).await
}

fn temp_output_path(prefix: &str, attempt: usize) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    env::temp_dir().join(format!(
        "{prefix}-{}-{stamp}-{attempt}.log",
        std::process::id()
    ))
}

fn sanitize_output(text: &str) -> String {
    text.chars()
        .filter(|character| {
            matches!(*character, '\n' | '\r' | '\t')
                || (!character.is_control() && !is_unicode_format_character(*character))
        })
        .collect::<String>()
        .replace('\r', "")
}

fn is_unicode_format_character(character: char) -> bool {
    matches!(character as u32, 0xfff9..=0xfffb)
}

fn output_details(
    output: &OutputSnapshot,
    exit_status: Option<&std::process::ExitStatus>,
) -> serde_json::Value {
    json!({
        "execution_kind": "command",
        "exit_code": exit_status.and_then(std::process::ExitStatus::code),
        "truncated": output.truncated,
        "truncated_by": output.truncated_by,
        "total_lines": output.total_lines,
        "total_bytes": output.total_bytes,
        "output_lines": output.output_lines,
        "output_bytes": output.output_bytes,
        "max_lines": DEFAULT_MAX_LINES,
        "max_bytes": DEFAULT_MAX_BYTES,
        "full_output_path": output.full_output_path.as_ref().map(|path| path.display().to_string()),
    })
}

fn format_display_output(output: &OutputSnapshot, empty_text: &str) -> String {
    if output.content.is_empty() {
        empty_text.to_string()
    } else {
        output.content.clone()
    }
}

fn format_model_output(output: &OutputSnapshot, empty_text: &str) -> String {
    let mut text = format_display_output(output, empty_text);
    if output.truncated {
        let full_output = output
            .full_output_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "unavailable".to_string());
        let status = match output.truncated_by {
            Some("bytes") => format!(
                "[Showing last {} bytes of {} bytes total. Full output: {full_output}]",
                output.output_bytes, output.total_bytes
            ),
            _ => {
                let start_line = output
                    .total_lines
                    .saturating_sub(output.output_lines)
                    .saturating_add(1);
                format!(
                    "[Showing lines {start_line}-{} of {}. Full output: {full_output}]",
                    output.total_lines, output.total_lines
                )
            }
        };
        append_status(&mut text, &status);
    }
    text
}

fn append_status(text: &mut String, status: &str) {
    if !text.is_empty() {
        text.push_str("\n\n");
    }
    text.push_str(status);
}

fn terminal_exit_status(
    exit_status: Option<&std::process::ExitStatus>,
) -> Option<ToolTerminalExitStatus> {
    let exit_status = exit_status?;
    Some(ToolTerminalExitStatus {
        exit_code: exit_status.code().and_then(|code| u32::try_from(code).ok()),
        signal: exit_signal(exit_status),
    })
}

#[cfg(unix)]
fn exit_signal(exit_status: &std::process::ExitStatus) -> Option<String> {
    exit_status
        .signal()
        .map(|signal| format!("signal {signal}"))
}

#[cfg(not(unix))]
fn exit_signal(_exit_status: &std::process::ExitStatus) -> Option<String> {
    None
}

struct TerminalSnapshotData<'a> {
    terminal_id: &'a str,
    command: &'a str,
    cwd: &'a Path,
    output: &'a str,
    truncated: bool,
    exit_status: Option<ToolTerminalExitStatus>,
    released: bool,
}

fn emit_terminal_snapshot(context: &ToolExecutionContext<'_>, data: TerminalSnapshotData<'_>) {
    context.emit(ToolProgress::TerminalUpdated {
        snapshot: ToolTerminalSnapshot {
            terminal_id: data.terminal_id.to_string(),
            command: Some(data.command.to_string()),
            cwd: Some(data.cwd.display().to_string()),
            output: data.output.to_string(),
            truncated: data.truncated,
            exit_status: data.exit_status,
            released: data.released,
        },
    });
}

fn format_seconds(seconds: f64) -> String {
    if seconds.fract() == 0.0 {
        format!("{seconds:.0}")
    } else {
        seconds.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::OutputAccumulator;

    #[tokio::test]
    async fn output_accumulator_preserves_utf8_split_across_chunks() {
        let mut output = OutputAccumulator::new("hunea-bash-test");
        let text = "before 你 after\n";
        let split_inside_multibyte_character = "before ".len() + 1;

        output
            .append(&text.as_bytes()[..split_inside_multibyte_character])
            .await
            .expect("append first chunk");
        output
            .append(&text.as_bytes()[split_inside_multibyte_character..])
            .await
            .expect("append second chunk");
        output.finish().await.expect("finish output");

        assert_eq!(output.snapshot(true).content, text);
    }
}
