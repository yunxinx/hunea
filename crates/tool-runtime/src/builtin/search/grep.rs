use std::{
    collections::VecDeque,
    io,
    path::{Path, PathBuf},
    process::Stdio,
};

use grep_regex::RegexMatcherBuilder;
use grep_searcher::{
    BinaryDetection, Searcher, SearcherBuilder, Sink, SinkContext, SinkFinish, SinkMatch,
};
use serde::Deserialize;
use serde_json::json;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
    task,
    task::JoinError,
};
use tokio_util::sync::CancellationToken;

use crate::{
    Tool, ToolCall, ToolDefinition, ToolExecutionContext, ToolExecutionFuture, ToolKind,
    ToolPermissionPolicy, ToolResult,
};

use super::super::workspace_file::{
    workspace::resolve_read_path, workspace_access::local_workspace_access,
};
use super::{
    error::SearchToolError,
    ripgrep::{
        ManagedRipgrepConfig, RipgrepCommand, RipgrepCommandPlan, managed_ripgrep_fallback_for,
        resolve_ripgrep_command_plan,
    },
    search_fallback::{
        GREP_MAX_LINE_CHARS, SEARCH_MAX_OUTPUT_BYTES, TOOL_CALL_INTERRUPTED,
        VCS_DIRECTORIES_TO_EXCLUDE, build_search_walker, collect_capped_stderr, compile_glob,
        format_bytes, model_search_path, path_has_vcs_component, path_matches_glob,
        path_text_has_vcs_component, search_relative_path, stderr_task_output,
        truncate_head_by_bytes, truncate_line,
    },
};

const GREP_TOOL_NAME: &str = "grep";
const DEFAULT_MATCH_LIMIT: usize = 100;
const MAX_MATCH_LIMIT: usize = 1_000;
const MAX_CONTEXT_LINES: usize = 20;

/// `grep_tool` 创建递归内容搜索工具。
pub fn grep_tool(root: impl AsRef<Path>) -> impl Tool + 'static {
    grep_tool_with_config(
        root,
        ManagedRipgrepConfig::default(),
        PathBuf::from(".hunea"),
    )
}

pub(crate) fn grep_tool_with_config(
    root: impl AsRef<Path>,
    managed_ripgrep: ManagedRipgrepConfig,
    managed_root: PathBuf,
) -> impl Tool + 'static {
    GrepTool {
        root: root.as_ref().to_path_buf(),
        managed_ripgrep,
        managed_root,
    }
}

#[derive(Clone)]
struct GrepTool {
    root: PathBuf,
    managed_ripgrep: ManagedRipgrepConfig,
    managed_root: PathBuf,
}

impl std::fmt::Debug for GrepTool {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GrepTool")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

impl Tool for GrepTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(GREP_TOOL_NAME)
            .with_label("Grep")
            .with_kind(ToolKind::Search)
            .with_description(
                "Search file contents recursively under an existing relative or absolute path. Relative paths resolve from the current working directory. Returns file paths, 1-based line numbers, matching text, and truncation notes when match, line, or byte limits are reached. Respects ignore files and searches hidden files.",
            )
            .with_input_schema(json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Regex or literal pattern to search for"
                    },
                    "path": {
                        "type": "string",
                        "description": "Existing relative or absolute file or directory path; relative paths resolve from the current working directory and default to it"
                    },
                    "glob": {
                        "type": "string",
                        "description": "Optional glob filter relative to the search path, for example \"*.rs\" or \"crates/**/Cargo.toml\""
                    },
                    "ignore_case": {
                        "type": "boolean",
                        "description": "Match case-insensitively"
                    },
                    "literal": {
                        "type": "boolean",
                        "description": "Treat pattern as plain text instead of regex"
                    },
                    "context": {
                        "type": "integer",
                        "minimum": 0,
                        "maximum": MAX_CONTEXT_LINES,
                        "description": "Number of context lines before and after each match"
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": MAX_MATCH_LIMIT,
                        "description": "Maximum number of matching lines to return"
                    }
                },
                "required": ["pattern"],
                "additionalProperties": false
            }))
            .with_permission_policy(ToolPermissionPolicy::Always)
            .with_prompt_guidelines(
                "Uses ripgrep internally but handles permissions and .gitignore — prefer over running rg in bash.",
            )
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        cancellation: &'a CancellationToken,
    ) -> ToolExecutionFuture<'a> {
        self.execute_with_context(call, ToolExecutionContext::new(cancellation))
    }

    fn execute_with_context<'a>(
        &'a self,
        call: ToolCall,
        context: ToolExecutionContext<'a>,
    ) -> ToolExecutionFuture<'a> {
        let root = self.root.clone();
        let managed_ripgrep = self.managed_ripgrep.clone();
        let managed_root = self.managed_root.clone();
        Box::pin(
            async move { execute_grep(root, managed_ripgrep, managed_root, call, context).await },
        )
    }
}

#[derive(Debug, Deserialize)]
struct GrepArguments {
    pattern: String,
    path: Option<String>,
    glob: Option<String>,
    ignore_case: Option<bool>,
    literal: Option<bool>,
    context: Option<usize>,
    limit: Option<usize>,
}

#[derive(Debug, Clone)]
struct NormalizedGrepArguments {
    pattern: String,
    requested_path: String,
    glob: Option<String>,
    ignore_case: bool,
    literal: bool,
    context: usize,
    limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GrepOutcome {
    content: String,
    total_matches: usize,
    shown_matches: usize,
    truncated: bool,
    match_truncated: bool,
    byte_truncated: bool,
    lines_truncated: bool,
    backend: &'static str,
}

#[derive(Debug, Clone)]
struct GrepExecutionPlan {
    root: PathBuf,
    search_path: PathBuf,
    search_path_is_file: bool,
    arguments: NormalizedGrepArguments,
    external_command: RipgrepCommandPlan,
    /// primary 非managed 时可能存在的 managed 二进制候选，用于 primary 执行失败后重试，
    /// 避免 PATH 上坏的 rg 短路已安装的 managed rg。
    managed_fallback: Option<RipgrepCommand>,
}

async fn execute_grep(
    root: PathBuf,
    managed_ripgrep: ManagedRipgrepConfig,
    managed_root: PathBuf,
    call: ToolCall,
    context: ToolExecutionContext<'_>,
) -> ToolResult {
    if context.cancellation().is_cancelled() {
        return ToolResult::error(call.call_id, TOOL_CALL_INTERRUPTED);
    }
    let call_id = call.call_id;
    let cancellation = context.cancellation().clone();
    let plan = match task::spawn_blocking(move || {
        build_grep_execution_plan(
            root,
            managed_ripgrep,
            managed_root,
            call.arguments,
            &cancellation,
        )
    })
    .await
    {
        Ok(Ok(plan)) => plan,
        Ok(Err(error)) => return ToolResult::error(call_id, error.to_string()),
        Err(error) => return join_error_result(call_id, error),
    };

    let GrepExecutionPlan {
        root,
        search_path,
        search_path_is_file,
        arguments,
        external_command,
        managed_fallback,
    } = plan;

    if let RipgrepCommandPlan::Ready(command) = external_command
        && let Ok(outcome) = run_external_grep(
            &command,
            &root,
            &search_path,
            search_path_is_file,
            &arguments,
            &context,
        )
        .await
    {
        return grep_result(call_id, outcome);
    }

    // primary（system/bundled）执行失败时，尝试 managed 二进制重试，
    // 避免 PATH 上坏的 rg 短路已安装的 managed rg。
    if let Some(command) = managed_fallback
        && let Ok(outcome) = run_external_grep(
            &command,
            &root,
            &search_path,
            search_path_is_file,
            &arguments,
            &context,
        )
        .await
    {
        return grep_result(call_id, outcome);
    }

    let fallback_call_id = call_id.clone();
    let cancellation = context.cancellation().clone();
    match task::spawn_blocking(move || rust_grep(&root, &search_path, &arguments, &cancellation))
        .await
    {
        Ok(Ok(outcome)) => grep_result(call_id, outcome),
        Ok(Err(error)) => ToolResult::error(call_id, error.to_string()),
        Err(error) => join_error_result(fallback_call_id, error),
    }
}

fn build_grep_execution_plan(
    root: PathBuf,
    managed_ripgrep: ManagedRipgrepConfig,
    managed_root: PathBuf,
    arguments: serde_json::Value,
    cancellation: &CancellationToken,
) -> Result<GrepExecutionPlan, SearchToolError> {
    if cancellation.is_cancelled() {
        return Err(SearchToolError::Interrupted);
    }
    let arguments = parse_arguments(arguments)?;
    let access = local_workspace_access();
    let root = match access.as_ref().canonicalize(&root) {
        Ok(root) => root,
        Err(source) => return Err(SearchToolError::WorkspaceRoot { path: root, source }),
    };
    let search_path = match resolve_read_path(access.as_ref(), &root, &arguments.requested_path) {
        Ok(path) => path,
        Err(source) => return Err(SearchToolError::WorkspacePath { source }),
    };
    let search_path_is_file = search_path.is_file();

    let external_command = resolve_ripgrep_command_plan(&managed_ripgrep, &managed_root);
    let managed_fallback =
        managed_ripgrep_fallback_for(&external_command, &managed_ripgrep, &managed_root);
    Ok(GrepExecutionPlan {
        root,
        search_path,
        search_path_is_file,
        arguments,
        external_command,
        managed_fallback,
    })
}

fn parse_arguments(value: serde_json::Value) -> Result<NormalizedGrepArguments, SearchToolError> {
    let arguments = serde_json::from_value::<GrepArguments>(value).map_err(|source| {
        SearchToolError::InvalidArguments {
            tool: GREP_TOOL_NAME,
            source,
        }
    })?;
    let pattern = arguments.pattern.trim();
    if pattern.is_empty() {
        return Err(SearchToolError::MissingPattern);
    }
    Ok(NormalizedGrepArguments {
        pattern: pattern.to_string(),
        requested_path: arguments.path.unwrap_or_else(|| ".".to_string()),
        glob: arguments
            .glob
            .map(|glob| glob.trim().to_string())
            .filter(|glob| !glob.is_empty()),
        ignore_case: arguments.ignore_case.unwrap_or(false),
        literal: arguments.literal.unwrap_or(false),
        context: arguments.context.unwrap_or(0).min(MAX_CONTEXT_LINES),
        limit: arguments
            .limit
            .unwrap_or(DEFAULT_MATCH_LIMIT)
            .clamp(1, MAX_MATCH_LIMIT),
    })
}

async fn run_external_grep(
    command: &RipgrepCommand,
    root: &Path,
    search_path: &Path,
    search_path_is_file: bool,
    arguments: &NormalizedGrepArguments,
    context: &ToolExecutionContext<'_>,
) -> Result<GrepOutcome, SearchToolError> {
    let (command_cwd, command_target) = if search_path_is_file {
        let parent = search_path
            .parent()
            .ok_or_else(|| SearchToolError::PathIo {
                operation: "resolve grep file search parent",
                path: search_path.to_path_buf(),
                source: io::Error::new(io::ErrorKind::InvalidInput, "file has no parent directory"),
            })?;
        let target = search_path
            .file_name()
            .ok_or_else(|| SearchToolError::PathIo {
                operation: "resolve grep file search name",
                path: search_path.to_path_buf(),
                source: io::Error::new(io::ErrorKind::InvalidInput, "file has no name"),
            })?;
        (parent.to_path_buf(), target.to_string_lossy().into_owned())
    } else {
        (search_path.to_path_buf(), ".".to_string())
    };

    let mut args = vec![
        "--json".to_string(),
        "--line-number".to_string(),
        "--color=never".to_string(),
        "--sort".to_string(),
        "path".to_string(),
        "--hidden".to_string(),
    ];
    for directory in VCS_DIRECTORIES_TO_EXCLUDE {
        args.push("--glob".to_string());
        args.push(format!("!{directory}"));
    }
    if arguments.ignore_case {
        args.push("--ignore-case".to_string());
    }
    if arguments.literal {
        args.push("--fixed-strings".to_string());
    }
    if arguments.context > 0 {
        args.push("--context".to_string());
        args.push(arguments.context.to_string());
    }
    if let Some(glob) = arguments.glob.as_ref() {
        args.push("--glob".to_string());
        args.push(glob.clone());
    }
    args.push("--".to_string());
    args.push(arguments.pattern.clone());
    args.push(command_target);

    let mut process = Command::new(&command.path);
    process.args(&args);
    process.current_dir(&command_cwd);
    process.stdin(Stdio::null());
    process.stdout(Stdio::piped());
    process.stderr(Stdio::piped());
    process.kill_on_drop(true);

    let mut child = process
        .spawn()
        .map_err(|source| SearchToolError::RipgrepSpawn { source })?;
    let stdout = child
        .stdout
        .take()
        .ok_or(SearchToolError::RipgrepStdoutUnavailable)?;
    let stderr_task = child
        .stderr
        .take()
        .map(|stderr| tokio::spawn(collect_capped_stderr(stderr)));
    let mut lines = BufReader::new(stdout).lines();
    let mut matches = Vec::new();
    let mut observed_matches = 0usize;
    let mut match_truncated = false;
    let mut killed_due_to_limit = false;

    loop {
        let line = tokio::select! {
            _ = context.cancellation().cancelled() => {
                let _ = child.start_kill();
                return Err(SearchToolError::Interrupted);
            }
            line = lines.next_line() => {
                line.map_err(|source| SearchToolError::RipgrepOutputRead { source })?
            }
        };
        let Some(line) = line else {
            break;
        };
        let Some(raw_match) = parse_rg_match_event(&line) else {
            continue;
        };
        if path_text_has_vcs_component(&raw_match.path) {
            continue;
        }
        let candidate_path = external_match_path(&command_cwd, &raw_match.path);
        let match_event = ExternalGrepMatch {
            model_path: model_search_path(root, &candidate_path),
            candidate_path,
            line_number: raw_match.line_number,
            line_text: raw_match.line_text,
        };
        if observed_matches >= arguments.limit {
            match_truncated = true;
            killed_due_to_limit = true;
            observed_matches += 1;
            let _ = child.start_kill();
            break;
        }
        observed_matches += 1;
        matches.push(match_event);
    }

    let status = child
        .wait()
        .await
        .map_err(|source| SearchToolError::RipgrepWait { source })?;
    let stderr = stderr_task_output(stderr_task).await;
    if !killed_due_to_limit && !matches!(status.code(), Some(0) | Some(1)) {
        return Err(SearchToolError::RipgrepFailed {
            stderr: stderr.trim().to_string(),
        });
    }

    let (lines, lines_truncated) = format_external_grep_matches(arguments, &matches).await;
    let formatted = format_grep_content(lines, matches.len(), match_truncated, lines_truncated);
    Ok(GrepOutcome {
        content: formatted.content,
        total_matches: observed_matches,
        shown_matches: matches.len(),
        truncated: match_truncated || formatted.byte_truncated || formatted.lines_truncated,
        match_truncated,
        byte_truncated: formatted.byte_truncated,
        lines_truncated: formatted.lines_truncated,
        backend: command.backend.as_str(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExternalGrepMatch {
    candidate_path: PathBuf,
    model_path: String,
    line_number: usize,
    line_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RawExternalGrepMatch {
    path: String,
    line_number: usize,
    line_text: String,
}

#[derive(Debug, Deserialize)]
struct RipgrepJsonEvent {
    #[serde(rename = "type")]
    event_type: String,
    data: Option<RipgrepMatchData>,
}

#[derive(Debug, Deserialize)]
struct RipgrepMatchData {
    path: RipgrepText,
    line_number: usize,
    lines: RipgrepText,
}

#[derive(Debug, Deserialize)]
struct RipgrepText {
    text: String,
}

fn parse_rg_match_event(line: &str) -> Option<RawExternalGrepMatch> {
    let event = serde_json::from_str::<RipgrepJsonEvent>(line).ok()?;
    if event.event_type != "match" {
        return None;
    }
    let data = event.data?;
    Some(RawExternalGrepMatch {
        path: data.path.text,
        line_number: data.line_number,
        line_text: sanitize_rg_line_text(&data.lines.text),
    })
}

fn sanitize_rg_line_text(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "")
        .trim_end_matches('\n')
        .to_string()
}

async fn format_external_grep_matches(
    arguments: &NormalizedGrepArguments,
    matches: &[ExternalGrepMatch],
) -> (Vec<String>, bool) {
    let mut output = Vec::with_capacity(matches.len());
    let mut lines_truncated = false;
    for match_event in matches {
        if arguments.context == 0 {
            let (line, was_truncated) = truncate_line(&match_event.line_text, GREP_MAX_LINE_CHARS);
            lines_truncated |= was_truncated;
            output.push(format!(
                "{}:{}:{}",
                match_event.model_path, match_event.line_number, line
            ));
            continue;
        }

        let Ok(content) = tokio::fs::read_to_string(&match_event.candidate_path).await else {
            let (line, was_truncated) = truncate_line(&match_event.line_text, GREP_MAX_LINE_CHARS);
            lines_truncated |= was_truncated;
            output.push(format!(
                "{}:{}:{}",
                match_event.model_path, match_event.line_number, line
            ));
            continue;
        };
        let lines = content.lines().collect::<Vec<_>>();
        let match_index = match_event.line_number.saturating_sub(1);
        if match_index >= lines.len() {
            let (line, was_truncated) = truncate_line(&match_event.line_text, GREP_MAX_LINE_CHARS);
            lines_truncated |= was_truncated;
            output.push(format!(
                "{}:{}:{}",
                match_event.model_path, match_event.line_number, line
            ));
            continue;
        }
        let (block, was_truncated) = format_match_block(
            &match_event.model_path,
            &lines,
            match_index,
            arguments.context,
        );
        lines_truncated |= was_truncated;
        output.push(block);
    }
    (output, lines_truncated)
}

fn external_match_path(command_cwd: &Path, path: &str) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        command_cwd.join(path)
    }
}

/// Searcher 报告的一行（匹配行或 context 行），文本已经过 lossy 转换和截断。
#[derive(Debug, Clone, PartialEq, Eq)]
struct RustGrepLine {
    line_number: u64,
    text: String,
    was_truncated: bool,
}

/// 一个等待补齐 after-context 的匹配块；`after_left` 归零或文件结束时完成为 block 字符串。
#[derive(Debug, Clone)]
struct PendingRustGrepBlock {
    /// 每行记录分隔符：匹配行用 `:`，context 行用 `-`。
    lines: Vec<(char, RustGrepLine)>,
    match_line: u64,
    after_left: usize,
}

/// 文件级 Sink：把 Searcher 的 byte-oriented 事件重建成现有 fallback 的
/// `path:line:text` / `path-line:text` block 格式。
///
/// 行文本进入时立即做 lossy UTF-8 转换并去掉末尾 `\n`/`\r`，坏字节只会变成
/// `U+FFFD`，不会让整个文件静默消失；context 窗口通过 `recent_lines` 和
/// `pending` 两个有界缓冲重建，不重新读取整文件。
struct RustGrepFileSink<'a> {
    model_path: String,
    context: usize,
    /// 本文件还可展示的匹配数（全局 limit 减去之前文件已观察到的匹配）。
    remaining_matches: usize,
    cancellation: &'a CancellationToken,
    recent_lines: VecDeque<RustGrepLine>,
    pending: VecDeque<PendingRustGrepBlock>,
    completed_blocks: Vec<String>,
    shown_matches: usize,
    observed_matches: usize,
    lines_truncated: bool,
    limit_hit: bool,
    binary_detected: bool,
}

impl<'a> RustGrepFileSink<'a> {
    fn new(
        model_path: String,
        context: usize,
        remaining_matches: usize,
        cancellation: &'a CancellationToken,
    ) -> Self {
        Self {
            model_path,
            context,
            remaining_matches,
            cancellation,
            recent_lines: VecDeque::new(),
            pending: VecDeque::new(),
            completed_blocks: Vec::new(),
            shown_matches: 0,
            observed_matches: 0,
            lines_truncated: false,
            limit_hit: false,
            binary_detected: false,
        }
    }

    fn check_cancelled(&self) -> Result<bool, io::Error> {
        if self.cancellation.is_cancelled() {
            Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"))
        } else {
            Ok(true)
        }
    }

    fn line_from_bytes(&mut self, bytes: &[u8], line_number: Option<u64>) -> RustGrepLine {
        let mut text = String::from_utf8_lossy(bytes).into_owned();
        if text.ends_with('\n') {
            text.pop();
            if text.ends_with('\r') {
                text.pop();
            }
        }
        let (text, was_truncated) = truncate_line(&text, GREP_MAX_LINE_CHARS);
        RustGrepLine {
            line_number: line_number.unwrap_or(0),
            text,
            was_truncated,
        }
    }

    fn push_recent(&mut self, line: RustGrepLine) {
        if self.context == 0 {
            return;
        }
        self.recent_lines.push_back(line);
        while self.recent_lines.len() > self.context {
            self.recent_lines.pop_front();
        }
    }

    /// 把一行按 after-context 喂给仍需要它的 pending block；行号单调递增，
    /// 因此只有行号大于 block 匹配行的行才可能属于它的 after 窗口。
    fn feed_after_context(&mut self, line: &RustGrepLine) {
        for block in self.pending.iter_mut() {
            if block.after_left > 0 && line.line_number > block.match_line {
                block.lines.push(('-', line.clone()));
                block.after_left -= 1;
            }
        }
        self.complete_ready_blocks();
    }

    fn complete_ready_blocks(&mut self) {
        while self
            .pending
            .front()
            .is_some_and(|block| block.after_left == 0)
        {
            let block = self.pending.pop_front().expect("front block checked");
            self.commit_block(block);
        }
    }

    /// 文件结束或 group 边界时强制完成剩余 pending，EOF 截断的 after-context
    /// 以现有 block 的 `min(match + context + 1, lines)` 语义截断。
    fn flush_pending(&mut self) {
        while let Some(block) = self.pending.pop_front() {
            self.commit_block(block);
        }
    }

    fn commit_block(&mut self, block: PendingRustGrepBlock) {
        self.lines_truncated |= block.lines.iter().any(|(_, line)| line.was_truncated);
        self.completed_blocks.push(self.format_block(block));
    }

    fn format_block(&self, block: PendingRustGrepBlock) -> String {
        block
            .lines
            .iter()
            .map(|(separator, line)| {
                format!(
                    "{}{separator}{}:{}",
                    self.model_path, line.line_number, line.text
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl Sink for RustGrepFileSink<'_> {
    type Error = io::Error;

    fn begin(&mut self, _searcher: &Searcher) -> Result<bool, Self::Error> {
        self.check_cancelled()
    }

    fn matched(&mut self, _searcher: &Searcher, mat: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        self.check_cancelled()?;
        let line = self.line_from_bytes(mat.bytes(), mat.line_number());
        self.feed_after_context(&line);
        if self.remaining_matches == 0 {
            // 第一个超额匹配：记录它并继续消费当前已展示 block 的
            // after-context；排空 pending 后再停止，避免 limit 截断 context。
            if !self.limit_hit {
                self.observed_matches += 1;
            }
            self.limit_hit = true;
            return Ok(!self.pending.is_empty());
        }
        self.remaining_matches -= 1;
        self.observed_matches += 1;
        self.shown_matches += 1;
        let mut block_lines = Vec::with_capacity(self.context.saturating_add(1));
        block_lines.extend(self.recent_lines.iter().map(|recent| ('-', recent.clone())));
        block_lines.push((':', line.clone()));
        self.pending.push_back(PendingRustGrepBlock {
            lines: block_lines,
            match_line: line.line_number,
            after_left: self.context,
        });
        self.push_recent(line);
        self.complete_ready_blocks();
        Ok(true)
    }

    fn context(
        &mut self,
        _searcher: &Searcher,
        ctx: &SinkContext<'_>,
    ) -> Result<bool, Self::Error> {
        self.check_cancelled()?;
        let line = self.line_from_bytes(ctx.bytes(), ctx.line_number());
        self.feed_after_context(&line);
        self.push_recent(line);
        Ok(!(self.limit_hit && self.pending.is_empty()))
    }

    fn context_break(&mut self, _searcher: &Searcher) -> Result<bool, Self::Error> {
        self.check_cancelled()?;
        // group 边界意味着此前的 pending 已收齐 after-context；清空 recent，
        // 避免 before-context 泄漏到下一组。
        self.flush_pending();
        self.recent_lines.clear();
        Ok(!self.limit_hit)
    }

    fn binary_data(&mut self, _searcher: &Searcher, _offset: u64) -> Result<bool, Self::Error> {
        // 含 NUL 的文件整体丢弃，不产生任何普通 `path:line:text` 输出。
        self.binary_detected = true;
        self.pending.clear();
        self.completed_blocks.clear();
        self.recent_lines.clear();
        self.shown_matches = 0;
        self.observed_matches = 0;
        self.lines_truncated = false;
        Ok(false)
    }

    fn finish(&mut self, _searcher: &Searcher, _: &SinkFinish) -> Result<(), Self::Error> {
        if !self.binary_detected {
            self.flush_pending();
        }
        Ok(())
    }
}

fn rust_grep(
    root: &Path,
    search_path: &Path,
    arguments: &NormalizedGrepArguments,
    cancellation: &CancellationToken,
) -> Result<GrepOutcome, SearchToolError> {
    if cancellation.is_cancelled() {
        return Err(SearchToolError::Interrupted);
    }
    let mut matcher_builder = RegexMatcherBuilder::new();
    matcher_builder
        .multi_line(true)
        .case_insensitive(arguments.ignore_case)
        .fixed_strings(arguments.literal)
        // CRLF 也是行边界；保持 Searcher 使用 LF 分隔符即可流式按行处理，
        // 不需要额外引入 matcher 的 direct dependency。
        .crlf(true)
        .line_terminator(Some(b'\n'))
        .dot_matches_new_line(false)
        .ban_byte(Some(b'\x00'));
    let matcher = matcher_builder
        .build(&arguments.pattern)
        .map_err(|source| SearchToolError::InvalidRegex { source })?;
    let mut searcher = SearcherBuilder::new()
        .line_number(true)
        .before_context(arguments.context)
        .after_context(arguments.context)
        .binary_detection(BinaryDetection::quit(b'\x00'))
        .bom_sniffing(true)
        .build();
    let glob = arguments.glob.as_deref().map(compile_glob).transpose()?;
    let mut matches = Vec::new();
    let mut observed_matches = 0usize;
    let mut match_truncated = false;
    let mut lines_truncated = false;
    let search_root_is_file = search_path.is_file();

    'walk: for entry in build_search_walker(search_path, true) {
        if cancellation.is_cancelled() {
            return Err(SearchToolError::Interrupted);
        }
        let entry = entry.map_err(|source| SearchToolError::WalkSearchPath { source })?;
        let path = entry.path();
        let search_relative = search_relative_path(search_path, path);
        if path_has_vcs_component(search_relative) {
            continue;
        }
        let file_type = entry
            .file_type()
            .ok_or_else(|| SearchToolError::FileTypeUnavailable {
                path: path.to_path_buf(),
            })?;
        if !file_type.is_file() {
            continue;
        }
        if let Some(glob) = glob.as_ref()
            && !path_matches_glob(search_path, search_root_is_file, path, glob)
        {
            continue;
        }

        let mut sink = RustGrepFileSink::new(
            model_search_path(root, path),
            arguments.context,
            arguments.limit.saturating_sub(observed_matches),
            cancellation,
        );
        match searcher.search_path(&matcher, path, &mut sink) {
            Ok(()) => {}
            Err(_) if cancellation.is_cancelled() => return Err(SearchToolError::Interrupted),
            // 不可读文件按旧 fallback 语义跳过，不升级为整次 grep 失败。
            Err(_) => continue,
        }
        if sink.binary_detected {
            continue;
        }
        observed_matches += sink.observed_matches;
        match_truncated |= sink.limit_hit;
        lines_truncated |= sink.lines_truncated;
        matches.extend(sink.completed_blocks);
        if sink.limit_hit {
            break 'walk;
        }
    }

    let shown_matches = matches.len();
    let formatted = format_grep_content(matches, shown_matches, match_truncated, lines_truncated);
    Ok(GrepOutcome {
        content: formatted.content,
        total_matches: observed_matches,
        shown_matches,
        truncated: match_truncated || formatted.byte_truncated || formatted.lines_truncated,
        match_truncated,
        byte_truncated: formatted.byte_truncated,
        lines_truncated: formatted.lines_truncated,
        backend: "rust_fallback",
    })
}

fn format_match_block(
    model_path: &str,
    lines: &[&str],
    match_index: usize,
    context: usize,
) -> (String, bool) {
    if context == 0 {
        let (line, was_truncated) = truncate_line(lines[match_index], GREP_MAX_LINE_CHARS);
        return (
            format!("{}:{}:{}", model_path, match_index + 1, line),
            was_truncated,
        );
    }
    let start = match_index.saturating_sub(context);
    let end = (match_index + context + 1).min(lines.len());
    let mut was_any_line_truncated = false;
    let content = (start..end)
        .map(|index| {
            let separator = if index == match_index { ":" } else { "-" };
            let (line, was_truncated) = truncate_line(lines[index], GREP_MAX_LINE_CHARS);
            was_any_line_truncated |= was_truncated;
            format!("{}{separator}{}:{}", model_path, index + 1, line)
        })
        .collect::<Vec<_>>()
        .join("\n");
    (content, was_any_line_truncated)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FormattedGrepContent {
    content: String,
    byte_truncated: bool,
    lines_truncated: bool,
}

fn format_grep_content(
    lines: Vec<String>,
    shown_matches: usize,
    match_truncated: bool,
    lines_truncated: bool,
) -> FormattedGrepContent {
    if shown_matches == 0 {
        return FormattedGrepContent {
            content: "No matches found.".to_string(),
            byte_truncated: false,
            lines_truncated,
        };
    }
    let raw_content = lines.join("\n");
    let truncation = truncate_head_by_bytes(raw_content, SEARCH_MAX_OUTPUT_BYTES);
    let mut content = truncation.content;
    let mut notices = Vec::new();
    if match_truncated {
        notices.push(format!(
            "{shown_matches} matches limit reached. Use limit={} for more, or refine pattern",
            shown_matches.saturating_mul(2).max(shown_matches + 1)
        ));
    }
    if truncation.is_truncated {
        notices.push(format!(
            "{} limit reached",
            format_bytes(SEARCH_MAX_OUTPUT_BYTES)
        ));
    }
    if lines_truncated {
        notices.push(format!(
            "some lines truncated to {GREP_MAX_LINE_CHARS} chars. Use read tool to see full lines"
        ));
    }
    if !notices.is_empty() {
        content.push_str(&format!("\n\n[{}]", notices.join(". ")));
    }
    FormattedGrepContent {
        content,
        byte_truncated: truncation.is_truncated,
        lines_truncated,
    }
}

fn grep_result(call_id: String, outcome: GrepOutcome) -> ToolResult {
    ToolResult::success(call_id, outcome.content).with_details(json!({
        "backend": outcome.backend,
        "total_matches": outcome.total_matches,
        "shown_matches": outcome.shown_matches,
        "truncated": outcome.truncated,
        "match_truncated": outcome.match_truncated,
        "byte_truncated": outcome.byte_truncated,
        "lines_truncated": outcome.lines_truncated,
    }))
}

fn join_error_result(call_id: String, error: JoinError) -> ToolResult {
    ToolResult::error(
        call_id,
        SearchToolError::JoinTask {
            operation: "grep",
            source: error,
        }
        .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    use tokio_util::sync::CancellationToken;

    use super::super::ripgrep::RipgrepBackend;
    use super::*;

    #[test]
    fn parse_arguments_preserves_grep_deserialization_error() {
        let result = parse_arguments(json!({
            "pattern": 12,
        }));

        assert!(matches!(
            result,
            Err(SearchToolError::InvalidArguments { tool: "grep", .. })
        ));
    }

    #[test]
    fn rust_grep_preserves_invalid_regex_error() {
        let root = temp_root("rust-grep-invalid-regex");
        let arguments = NormalizedGrepArguments {
            pattern: "[".to_string(),
            requested_path: ".".to_string(),
            glob: None,
            ignore_case: false,
            literal: false,
            context: 0,
            limit: 10,
        };

        let result = rust_grep(&root, &root, &arguments, &CancellationToken::new());

        assert!(matches!(result, Err(SearchToolError::InvalidRegex { .. })));
        cleanup(&root);
    }

    #[test]
    fn grep_execution_plan_resolves_workspace_paths_before_async_execution() {
        let root = temp_root("grep-execution-plan-paths");
        let source_dir = root.join("src");
        fs::create_dir_all(&source_dir).expect("source directory should exist");
        let cancellation = CancellationToken::new();

        let plan = build_grep_execution_plan(
            root.clone(),
            ManagedRipgrepConfig::default(),
            root.clone(),
            json!({
                "pattern": "needle",
                "path": "src",
            }),
            &cancellation,
        )
        .expect("grep execution plan should resolve search paths");

        assert_eq!(
            plan.root,
            fs::canonicalize(&root).expect("root should canonicalize")
        );
        assert_eq!(
            plan.search_path,
            fs::canonicalize(&source_dir).expect("source dir should canonicalize")
        );
        assert_eq!(plan.arguments.pattern, "needle");
        cleanup(&root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn external_grep_uses_dot_path_for_workspace_root() {
        let root = temp_root("external-grep-root-path");
        let script = write_executable(
            &root,
            "fake-rg",
            r#"#!/bin/sh
last=""
for arg in "$@"; do
  last="$arg"
done
if [ "$last" != "." ]; then
  printf 'expected root path ".", got "%s"\n' "$last" >&2
  exit 2
fi
printf '{"type":"match","data":{"path":{"text":"src/lib.rs"},"line_number":1,"lines":{"text":"needle\\n"}}}\n'
"#,
        );
        let command = RipgrepCommand {
            path: script,
            backend: RipgrepBackend::SystemPath,
        };
        let arguments = NormalizedGrepArguments {
            pattern: "needle".to_string(),
            requested_path: ".".to_string(),
            glob: None,
            ignore_case: false,
            literal: false,
            context: 0,
            limit: 10,
        };

        let outcome = run_external_grep(
            &command,
            &root,
            &root,
            false,
            &arguments,
            &ToolExecutionContext::new(&CancellationToken::new()),
        )
        .await
        .expect("root search should pass . to rg");

        assert_eq!(outcome.content, "src/lib.rs:1:needle");
        cleanup(&root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn external_grep_restores_external_directory_matches_to_absolute_model_paths() {
        let workspace = temp_root("external-grep-workspace");
        let external = temp_root("external-grep-directory");
        fs::create_dir_all(external.join("src")).expect("external source directory should exist");
        fs::write(external.join("src/lib.rs"), "needle\n")
            .expect("external source file should exist");
        let script = write_executable(
            &workspace,
            "fake-rg-external-directory",
            r#"#!/bin/sh
last=""
for arg in "$@"; do
  last="$arg"
done
if [ "$last" != "." ] || [ ! -f "src/lib.rs" ]; then
  printf 'expected external directory cwd with target "."\n' >&2
  exit 2
fi
printf '{"type":"match","data":{"path":{"text":"src/lib.rs"},"line_number":1,"lines":{"text":"needle\\n"}}}\n'
"#,
        );
        let command = RipgrepCommand {
            path: script,
            backend: RipgrepBackend::SystemPath,
        };
        let arguments = NormalizedGrepArguments {
            pattern: "needle".to_string(),
            requested_path: external.display().to_string(),
            glob: Some("src/*.rs".to_string()),
            ignore_case: false,
            literal: true,
            context: 0,
            limit: 10,
        };

        let outcome = run_external_grep(
            &command,
            &workspace,
            &external,
            false,
            &arguments,
            &ToolExecutionContext::new(&CancellationToken::new()),
        )
        .await
        .expect("external directory search should restore the rg path");

        assert_eq!(
            outcome.content,
            format!("{}:1:needle", external.join("src/lib.rs").display())
        );
        cleanup(&external);
        cleanup(&workspace);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn external_grep_uses_file_parent_for_context_reads_and_absolute_model_paths() {
        let workspace = temp_root("external-grep-file-workspace");
        let external = temp_root("external-grep-file-directory");
        let search_file = external.join("needle.txt");
        fs::write(&search_file, "before\nneedle\nafter\n")
            .expect("external search file should exist");
        let script = write_executable(
            &workspace,
            "fake-rg-external-file",
            r#"#!/bin/sh
last=""
for arg in "$@"; do
  last="$arg"
done
if [ "$last" != "needle.txt" ] || [ ! -f "$last" ]; then
  printf 'expected file parent cwd with target "needle.txt"\n' >&2
  exit 2
fi
printf '{"type":"match","data":{"path":{"text":"needle.txt"},"line_number":2,"lines":{"text":"needle\\n"}}}\n'
"#,
        );
        let command = RipgrepCommand {
            path: script,
            backend: RipgrepBackend::SystemPath,
        };
        let arguments = NormalizedGrepArguments {
            pattern: "needle".to_string(),
            requested_path: search_file.display().to_string(),
            glob: Some("needle*.txt".to_string()),
            ignore_case: false,
            literal: true,
            context: 1,
            limit: 10,
        };

        let outcome = run_external_grep(
            &command,
            &workspace,
            &search_file,
            true,
            &arguments,
            &ToolExecutionContext::new(&CancellationToken::new()),
        )
        .await
        .expect("external file search should restore its candidate path");

        let prefix = search_file.display();
        assert!(outcome.content.contains(&format!("{prefix}-1:before")));
        assert!(outcome.content.contains(&format!("{prefix}:2:needle")));
        assert!(outcome.content.contains(&format!("{prefix}-3:after")));
        cleanup(&external);
        cleanup(&workspace);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn external_grep_stops_after_limit_when_streaming_json_matches() {
        let root = temp_root("external-grep-limit");
        let script = write_executable(
            &root,
            "fake-rg",
            r#"#!/bin/sh
i=1
while [ "$i" -le 500 ]; do
  printf '{"type":"match","data":{"path":{"text":"src/lib.rs"},"line_number":%s,"lines":{"text":"needle-%s\\n"}}}\n' "$i" "$i"
  i=$((i + 1))
done
"#,
        );
        let command = RipgrepCommand {
            path: script,
            backend: RipgrepBackend::SystemPath,
        };
        let arguments = NormalizedGrepArguments {
            pattern: "needle".to_string(),
            requested_path: ".".to_string(),
            glob: None,
            ignore_case: false,
            literal: false,
            context: 0,
            limit: 3,
        };

        let outcome = run_external_grep(
            &command,
            &root,
            &root,
            false,
            &arguments,
            &ToolExecutionContext::new(&CancellationToken::new()),
        )
        .await
        .expect("fake rg should succeed");

        assert_eq!(outcome.shown_matches, 3);
        assert!(outcome.match_truncated);
        assert!(outcome.total_matches <= 4);
        assert!(outcome.content.contains("src/lib.rs:1:needle-1"));
        assert!(!outcome.content.contains("needle-4"));
        cleanup(&root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn external_grep_excludes_vcs_directories_but_keeps_hidden_files() {
        let root = temp_root("external-grep-vcs");
        let script = write_executable(
            &root,
            "fake-rg",
            r#"#!/bin/sh
exclude_vcs=0
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--glob" ] && [ "${2:-}" = "!.git" ]; then
    exclude_vcs=1
  fi
  shift
done
printf '{"type":"match","data":{"path":{"text":".hidden.rs"},"line_number":1,"lines":{"text":"needle hidden\\n"}}}\n'
if [ "$exclude_vcs" -eq 0 ]; then
  printf '{"type":"match","data":{"path":{"text":".git/config"},"line_number":1,"lines":{"text":"needle vcs\\n"}}}\n'
fi
"#,
        );
        let command = RipgrepCommand {
            path: script,
            backend: RipgrepBackend::SystemPath,
        };
        let arguments = NormalizedGrepArguments {
            pattern: "needle".to_string(),
            requested_path: ".".to_string(),
            glob: None,
            ignore_case: false,
            literal: false,
            context: 0,
            limit: 10,
        };

        let outcome = run_external_grep(
            &command,
            &root,
            &root,
            false,
            &arguments,
            &ToolExecutionContext::new(&CancellationToken::new()),
        )
        .await
        .expect("fake rg should succeed");

        assert!(outcome.content.contains(".hidden.rs:1:needle hidden"));
        assert!(!outcome.content.contains(".git/config"));
        cleanup(&root);
    }

    #[test]
    fn rust_grep_matches_utf16le_bom_file_with_line_numbers_and_context() {
        let root = temp_root("rust-grep-utf16le");
        write_bytes(
            &root,
            "utf16.txt",
            &utf16le_with_bom("alpha\nbeta\nneedle here\nomega\ngamma\n"),
        );
        let arguments = NormalizedGrepArguments {
            pattern: "needle".to_string(),
            requested_path: ".".to_string(),
            glob: None,
            ignore_case: false,
            literal: false,
            context: 1,
            limit: 10,
        };

        let outcome = rust_grep(&root, &root, &arguments, &CancellationToken::new())
            .expect("utf-16 search should succeed");

        assert_eq!(outcome.total_matches, 1);
        assert!(!outcome.truncated);
        assert!(
            outcome.content.contains("utf16.txt:3:needle here"),
            "unexpected content: {}",
            outcome.content
        );
        assert!(outcome.content.contains("utf16.txt-2:beta"));
        assert!(outcome.content.contains("utf16.txt-4:omega"));
        cleanup(&root);
    }

    #[test]
    fn rust_grep_lossy_decodes_invalid_utf8_with_literal_pattern() {
        let root = temp_root("rust-grep-bad-utf8-literal");
        write_bytes(&root, "bad.txt", b"first \xFF line\nsecond\n");
        let arguments = NormalizedGrepArguments {
            pattern: "line".to_string(),
            requested_path: ".".to_string(),
            glob: None,
            ignore_case: false,
            literal: true,
            context: 0,
            limit: 10,
        };

        let outcome = rust_grep(&root, &root, &arguments, &CancellationToken::new())
            .expect("invalid utf-8 search should not fail");

        assert_eq!(outcome.total_matches, 1);
        assert!(!outcome.content.contains("No matches found."));
        assert!(
            outcome.content.contains("bad.txt:1:first \u{FFFD} line"),
            "unexpected content: {}",
            outcome.content
        );
        cleanup(&root);
    }

    #[test]
    fn rust_grep_lossy_decodes_invalid_utf8_with_ignore_case() {
        let root = temp_root("rust-grep-bad-utf8-case");
        write_bytes(&root, "bad.txt", b"NEEDLE \xFF haystack\nplain\n");
        let arguments = NormalizedGrepArguments {
            pattern: "needle".to_string(),
            requested_path: ".".to_string(),
            glob: None,
            ignore_case: true,
            literal: false,
            context: 0,
            limit: 10,
        };

        let outcome = rust_grep(&root, &root, &arguments, &CancellationToken::new())
            .expect("invalid utf-8 search should not fail");

        assert_eq!(outcome.total_matches, 1);
        assert!(
            outcome
                .content
                .contains("bad.txt:1:NEEDLE \u{FFFD} haystack"),
            "unexpected content: {}",
            outcome.content
        );
        cleanup(&root);
    }

    #[test]
    fn rust_grep_literal_pattern_matches_meta_characters_literally() {
        let root = temp_root("rust-grep-literal-meta");
        write_bytes(&root, "lit.txt", b"a.c\nabc\n");
        let arguments = NormalizedGrepArguments {
            pattern: "a.c".to_string(),
            requested_path: ".".to_string(),
            glob: None,
            ignore_case: false,
            literal: true,
            context: 0,
            limit: 10,
        };

        let outcome = rust_grep(&root, &root, &arguments, &CancellationToken::new())
            .expect("literal search should succeed");

        assert_eq!(outcome.total_matches, 1);
        assert!(outcome.content.contains("lit.txt:1:a.c"));
        assert!(!outcome.content.contains("lit.txt:2:abc"));
        cleanup(&root);
    }

    #[test]
    fn rust_grep_skips_nul_binary_files_but_keeps_searching_later_text_files() {
        let root = temp_root("rust-grep-binary-skip");
        write_bytes(&root, "bin.dat", b"needle\x00binary payload\n");
        write_bytes(&root, "text.txt", b"line one\nneedle in text\n");
        let arguments = NormalizedGrepArguments {
            pattern: "needle".to_string(),
            requested_path: ".".to_string(),
            glob: None,
            ignore_case: false,
            literal: false,
            context: 0,
            limit: 10,
        };

        let outcome = rust_grep(&root, &root, &arguments, &CancellationToken::new())
            .expect("binary skip search should succeed");

        assert_eq!(outcome.total_matches, 1);
        assert!(outcome.content.contains("text.txt:2:needle in text"));
        assert!(!outcome.content.contains("bin.dat"));
        cleanup(&root);
    }

    #[test]
    fn rust_grep_finds_match_at_end_of_large_multi_buffer_file() {
        let root = temp_root("rust-grep-large-file");
        let mut content = String::from("first needle\n");
        for _ in 0..2_000 {
            content.push_str("filler line of padding for the large file test\n");
        }
        content.push_str("final needle\n");
        write_bytes(&root, "large.txt", content.as_bytes());
        let arguments = NormalizedGrepArguments {
            pattern: "needle".to_string(),
            requested_path: ".".to_string(),
            glob: None,
            ignore_case: false,
            literal: false,
            context: 0,
            limit: 10,
        };

        let outcome = rust_grep(&root, &root, &arguments, &CancellationToken::new())
            .expect("large file search should succeed");

        assert_eq!(outcome.total_matches, 2);
        assert!(outcome.content.contains("large.txt:1:first needle"));
        assert!(outcome.content.contains("large.txt:2002:final needle"));
        cleanup(&root);
    }

    #[test]
    fn rust_grep_context_blocks_preserve_separators_and_line_numbers() {
        let root = temp_root("rust-grep-context");
        write_bytes(
            &root,
            "ctx.txt",
            b"one\ntwo needle\nthree\nfour\nfive\nsix needle\nseven\neight\n",
        );
        let arguments = NormalizedGrepArguments {
            pattern: "needle".to_string(),
            requested_path: ".".to_string(),
            glob: None,
            ignore_case: false,
            literal: false,
            context: 1,
            limit: 10,
        };

        let outcome = rust_grep(&root, &root, &arguments, &CancellationToken::new())
            .expect("context search should succeed");

        assert_eq!(outcome.total_matches, 2);
        assert!(outcome.content.contains("ctx.txt-1:one"));
        assert!(outcome.content.contains("ctx.txt:2:two needle"));
        assert!(outcome.content.contains("ctx.txt-3:three"));
        assert!(outcome.content.contains("ctx.txt-5:five"));
        assert!(outcome.content.contains("ctx.txt:6:six needle"));
        assert!(outcome.content.contains("ctx.txt-7:seven"));
        cleanup(&root);
    }

    #[test]
    fn rust_grep_overlapping_context_blocks_share_context_lines() {
        let root = temp_root("rust-grep-overlap");
        write_bytes(
            &root,
            "ov.txt",
            b"one\ntwo\nthree needle\nfour needle\nfive\nsix\n",
        );
        let arguments = NormalizedGrepArguments {
            pattern: "needle".to_string(),
            requested_path: ".".to_string(),
            glob: None,
            ignore_case: false,
            literal: false,
            context: 1,
            limit: 10,
        };

        let outcome = rust_grep(&root, &root, &arguments, &CancellationToken::new())
            .expect("overlap context search should succeed");

        assert_eq!(outcome.total_matches, 2);
        assert!(outcome.content.contains("ov.txt-2:two"));
        assert!(outcome.content.contains("ov.txt:3:three needle"));
        assert!(outcome.content.contains("ov.txt-4:four needle"));
        assert!(outcome.content.contains("ov.txt-3:three needle"));
        assert!(outcome.content.contains("ov.txt:4:four needle"));
        assert!(outcome.content.contains("ov.txt-5:five"));
        cleanup(&root);
    }

    #[test]
    fn rust_grep_eof_context_is_truncated_by_end_of_file() {
        let root = temp_root("rust-grep-eof-context");
        write_bytes(&root, "eof.txt", b"one\ntwo\nthree needle\n");
        let arguments = NormalizedGrepArguments {
            pattern: "needle".to_string(),
            requested_path: ".".to_string(),
            glob: None,
            ignore_case: false,
            literal: false,
            context: 2,
            limit: 10,
        };

        let outcome = rust_grep(&root, &root, &arguments, &CancellationToken::new())
            .expect("eof context search should succeed");

        assert_eq!(outcome.total_matches, 1);
        assert!(outcome.content.contains("eof.txt-1:one"));
        assert!(outcome.content.contains("eof.txt-2:two"));
        assert!(outcome.content.contains("eof.txt:3:three needle"));
        assert!(!outcome.content.contains("eof.txt-4:"));
        cleanup(&root);
    }

    #[test]
    fn rust_grep_limit_truncation_reports_limit_plus_one_total() {
        let root = temp_root("rust-grep-limit");
        let content = (1..=10)
            .map(|index| format!("line {index} needle"))
            .collect::<Vec<_>>()
            .join("\n");
        write_bytes(&root, "many.txt", content.as_bytes());
        let arguments = NormalizedGrepArguments {
            pattern: "needle".to_string(),
            requested_path: ".".to_string(),
            glob: None,
            ignore_case: false,
            literal: false,
            context: 0,
            limit: 3,
        };

        let outcome = rust_grep(&root, &root, &arguments, &CancellationToken::new())
            .expect("limit search should succeed");

        assert_eq!(outcome.shown_matches, 3);
        assert_eq!(outcome.total_matches, 4);
        assert!(outcome.match_truncated);
        assert!(outcome.truncated);
        assert!(outcome.content.contains("many.txt:1:line 1 needle"));
        assert!(outcome.content.contains("many.txt:3:line 3 needle"));
        assert!(!outcome.content.contains("many.txt:4:line 4 needle"));
        cleanup(&root);
    }

    #[test]
    fn rust_grep_limit_preserves_after_context_for_last_shown_match() {
        let root = temp_root("rust-grep-limit-context");
        write_bytes(
            &root,
            "limit-context.txt",
            b"first needle\nextra needle\nanother extra needle\ntail context\nfinal line\n",
        );
        let arguments = NormalizedGrepArguments {
            pattern: "needle".to_string(),
            requested_path: ".".to_string(),
            glob: None,
            ignore_case: false,
            literal: false,
            context: 3,
            limit: 1,
        };

        let outcome = rust_grep(&root, &root, &arguments, &CancellationToken::new())
            .expect("limit context search should succeed");

        assert_eq!(outcome.shown_matches, 1);
        assert_eq!(outcome.total_matches, 2);
        assert!(outcome.match_truncated);
        assert!(outcome.content.contains("limit-context.txt:1:first needle"));
        assert!(outcome.content.contains("limit-context.txt-2:extra needle"));
        assert!(
            outcome
                .content
                .contains("limit-context.txt-3:another extra needle")
        );
        assert!(outcome.content.contains("limit-context.txt-4:tail context"));
        assert!(!outcome.content.contains("limit-context.txt-5:final line"));
        cleanup(&root);
    }

    #[test]
    fn rust_grep_truncates_long_lines_and_reports_lines_truncated() {
        let root = temp_root("rust-grep-line-truncation");
        let long_prefix = "x".repeat(GREP_MAX_LINE_CHARS + 20);
        write_bytes(
            &root,
            "long.txt",
            format!("{long_prefix} needle\n").as_bytes(),
        );
        let arguments = NormalizedGrepArguments {
            pattern: "needle".to_string(),
            requested_path: ".".to_string(),
            glob: None,
            ignore_case: false,
            literal: false,
            context: 0,
            limit: 10,
        };

        let outcome = rust_grep(&root, &root, &arguments, &CancellationToken::new())
            .expect("long line search should succeed");

        assert!(outcome.lines_truncated);
        assert!(outcome.truncated);
        assert!(
            outcome.content.contains(&format!(
                "long.txt:1:{}...",
                "x".repeat(GREP_MAX_LINE_CHARS)
            )),
            "unexpected content: {}",
            outcome.content
        );
        cleanup(&root);
    }

    #[test]
    fn rust_grep_anchors_match_whole_lines_not_whole_file() {
        let root = temp_root("rust-grep-anchors");
        write_bytes(&root, "anchors.txt", b"needle\nx needle\ny\nneedle z\n");
        write_bytes(
            &root,
            "anchors-crlf.txt",
            b"needle\r\nx needle\r\ny\r\nneedle z\r\n",
        );
        let arguments = NormalizedGrepArguments {
            pattern: "^needle$".to_string(),
            requested_path: ".".to_string(),
            glob: None,
            ignore_case: false,
            literal: false,
            context: 0,
            limit: 10,
        };

        let outcome = rust_grep(&root, &root, &arguments, &CancellationToken::new())
            .expect("anchored search should succeed");

        assert_eq!(outcome.total_matches, 2);
        assert!(outcome.content.contains("anchors.txt:1:needle"));
        assert!(!outcome.content.contains("anchors.txt:2"));
        assert!(!outcome.content.contains("anchors.txt:4"));
        assert!(outcome.content.contains("anchors-crlf.txt:1:needle"));
        assert!(!outcome.content.contains("anchors-crlf.txt:2"));
        assert!(!outcome.content.contains("anchors-crlf.txt:4"));
        cleanup(&root);
    }

    #[cfg(unix)]
    fn write_executable(root: &Path, name: &str, content: &str) -> PathBuf {
        let path = root.join(name);
        {
            use std::io::Write;

            let mut file = fs::File::create(&path).expect("create fake executable");
            file.write_all(content.as_bytes())
                .expect("write fake executable");
            file.sync_all().expect("sync fake executable");
        }
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
            .expect("chmod fake executable");
        path
    }

    fn temp_root(prefix: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("hunea-{prefix}-{}-{stamp}", std::process::id()));
        fs::create_dir_all(&root).expect("create temp root");
        root
    }

    fn write_bytes(root: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = root.join(name);
        {
            use std::io::Write;

            let mut file = fs::File::create(&path).expect("create test file");
            file.write_all(bytes).expect("write test file");
        }
        path
    }

    fn utf16le_with_bom(text: &str) -> Vec<u8> {
        let mut bytes = vec![0xFF, 0xFE];
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        bytes
    }

    fn cleanup(path: &Path) {
        let _ = fs::remove_dir_all(path);
    }
}
