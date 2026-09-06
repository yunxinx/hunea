use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::{AgentId, AgentTurnId};
use crate::session::{RuntimePermissionRequest, RuntimeTarget};

/// 一次 Agent launch batch 允许的最大 child 数量。
pub const AGENT_LAUNCH_BATCH_LIMIT: usize = 8;

/// launch boundary 冻结的 Agent title 最大终端显示宽度。
pub const AGENT_TITLE_MAX_DISPLAY_WIDTH: usize = 64;

const AGENT_OUTCOME_SUMMARY_MAX_DISPLAY_WIDTH: usize = 240;
const AGENT_OBJECTIVE_SUMMARY_MAX_DISPLAY_WIDTH: usize = 240;
const TRUNCATION_MARKER: &str = "...";

macro_rules! redacted_identity {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(u64);

        impl $name {
            /// 从 host 分配的数值创建 identity。
            pub const fn new(value: u64) -> Self {
                Self(value)
            }

            /// 返回持久化与 runtime correlation 使用的稳定数值。
            pub const fn get(self) -> u64 {
                self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(stringify!($name))
            }
        }
    };
}

redacted_identity!(
    AgentLaunchGroupId,
    "`AgentLaunchGroupId` 标识一次显式 typed batch launch。"
);
redacted_identity!(
    AgentRuntimeGeneration,
    "`AgentRuntimeGeneration` 标识 active Agent plugin/runtime generation。"
);
redacted_identity!(
    AgentObservationId,
    "`AgentObservationId` 标识一个 owner-bound Agent projection observer。"
);
redacted_identity!(
    AgentProjectionRevision,
    "`AgentProjectionRevision` 标识同 generation 内单调递增的 projection revision。"
);

/// `AgentObjective` 保存 child provider request 使用的完整任务描述。
///
/// 该类型故意不实现 serde：完整 objective 只允许停留在 request assembly/control plane，
/// 持久化与 projection 必须使用 [`AgentObjectiveSummary`]。
#[derive(Clone, PartialEq, Eq)]
pub struct AgentObjective(String);

impl AgentObjective {
    /// 创建非空的 provider-facing child objective。
    pub fn new(content: impl Into<String>) -> Result<Self, AgentLaunchInputError> {
        let content = content.into();
        if content.trim().is_empty() {
            return Err(AgentLaunchInputError::EmptyObjective);
        }
        Ok(Self(content))
    }

    /// 返回 provider request assembly 所需的 objective 正文。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for AgentObjective {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentObjective")
            .field("content_chars", &self.0.chars().count())
            .finish()
    }
}

/// `AgentInstructions` 保存只供 child request assembly 消费的 control-only instructions。
#[derive(Clone, PartialEq, Eq)]
pub struct AgentInstructions(String);

impl AgentInstructions {
    /// 创建 control-only instructions；空正文表示没有额外控制指令。
    pub fn new(content: impl Into<String>) -> Self {
        Self(content.into())
    }

    /// 只在 provider request assembly boundary 暴露 instructions 正文。
    pub fn expose_for_request_assembly(&self) -> &str {
        &self.0
    }

    /// 将 control-only instructions 追加到 provider-visible 文本，不产生 transcript 文本。
    pub fn append_to_provider_text(&self, text: impl Into<String>) -> String {
        let text = text.into();
        if text.trim().is_empty() {
            return self.0.clone();
        }
        format!("{text}\n\n{}", self.0)
    }

    /// 判断是否没有额外 direct instructions。
    pub fn is_empty(&self) -> bool {
        self.0.trim().is_empty()
    }
}

impl fmt::Debug for AgentInstructions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentInstructions")
            .field("content_chars", &self.0.chars().count())
            .finish()
    }
}

/// `AgentTitle` 是 launch boundary 解析并冻结的 delivery-safe 单行标题。
#[derive(Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct AgentTitle(String);

impl AgentTitle {
    /// 从显式 `display_title` 或 objective 的首个非空 logical content 解析标题。
    pub fn resolve(
        objective: &AgentObjective,
        display_title: Option<&str>,
    ) -> Result<Self, AgentLaunchInputError> {
        let source = match display_title {
            Some(title) => {
                if title.trim().is_empty() {
                    return Err(AgentLaunchInputError::EmptyDisplayTitle);
                }
                title
            }
            None => first_non_empty_logical_content(objective.as_str())
                .ok_or(AgentLaunchInputError::MissingTitleContent)?,
        };

        Self::from_source(source)
    }

    /// 返回所有 product surface 共享的 frozen title。
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn from_source(source: &str) -> Result<Self, AgentLaunchInputError> {
        if source.chars().any(is_terminal_control) {
            return Err(AgentLaunchInputError::TitleContainsTerminalControl);
        }
        let normalized = collapse_whitespace(source);
        if normalized.is_empty() {
            return Err(AgentLaunchInputError::MissingTitleContent);
        }
        Ok(Self(truncate_display_width(
            &normalized,
            AGENT_TITLE_MAX_DISPLAY_WIDTH,
        )))
    }

    fn deserialize_frozen(source: String) -> Result<Self, AgentLaunchInputError> {
        let title = Self::from_source(&source)?;
        if title.0 != source {
            return Err(AgentLaunchInputError::NonCanonicalFrozenTitle);
        }
        Ok(title)
    }
}

impl fmt::Debug for AgentTitle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentTitle")
            .field("content_chars", &self.0.chars().count())
            .field("display_width", &UnicodeWidthStr::width(self.0.as_str()))
            .finish()
    }
}

impl<'de> Deserialize<'de> for AgentTitle {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let source = String::deserialize(deserializer)?;
        Self::deserialize_frozen(source).map_err(serde::de::Error::custom)
    }
}

/// `AgentOutcomeSummary` 是 terminal outcome 可持久化的脱敏单行摘要。
#[derive(Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct AgentOutcomeSummary(String);

impl AgentOutcomeSummary {
    /// 创建 delivery-safe closed summary。
    pub fn new(content: impl Into<String>) -> Result<Self, AgentLaunchInputError> {
        let content = content.into();
        if content.chars().any(is_terminal_control) {
            return Err(AgentLaunchInputError::OutcomeSummaryContainsTerminalControl);
        }
        let normalized = collapse_whitespace(&content);
        if normalized.is_empty() {
            return Err(AgentLaunchInputError::EmptyOutcomeSummary);
        }
        Ok(Self(truncate_display_width(
            &normalized,
            AGENT_OUTCOME_SUMMARY_MAX_DISPLAY_WIDTH,
        )))
    }

    /// 返回 TUI 可展示的 closed summary。
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn deserialize_frozen(source: String) -> Result<Self, AgentLaunchInputError> {
        let summary = Self::new(source.clone())?;
        if summary.0 != source {
            return Err(AgentLaunchInputError::NonCanonicalOutcomeSummary);
        }
        Ok(summary)
    }
}

impl fmt::Debug for AgentOutcomeSummary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentOutcomeSummary")
            .field("content_chars", &self.0.chars().count())
            .finish()
    }
}

impl<'de> Deserialize<'de> for AgentOutcomeSummary {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let source = String::deserialize(deserializer)?;
        Self::deserialize_frozen(source).map_err(serde::de::Error::custom)
    }
}

/// `AgentObjectiveSummary` 是 launch fact 与 product projection 使用的脱敏单行摘要。
///
/// 摘要只取 objective 的第一条 logical content，再做 whitespace 规范化和显示宽度截断；
/// 因此多行 delivery 内容与 control-only instructions 不会进入 replay。
#[derive(Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct AgentObjectiveSummary(String);

impl AgentObjectiveSummary {
    /// 从完整 objective 解析 delivery-safe 摘要。
    pub fn from_objective(objective: &AgentObjective) -> Result<Self, AgentLaunchInputError> {
        let content = first_non_empty_logical_content(objective.as_str())
            .ok_or(AgentLaunchInputError::EmptyObjectiveSummary)?;
        Self::new(content)
    }

    /// 创建 delivery-safe 单行摘要。
    pub fn new(content: impl Into<String>) -> Result<Self, AgentLaunchInputError> {
        let content = content.into();
        if content.chars().any(is_terminal_control) {
            return Err(AgentLaunchInputError::ObjectiveSummaryContainsTerminalControl);
        }
        let normalized = collapse_whitespace(&content);
        if normalized.is_empty() {
            return Err(AgentLaunchInputError::EmptyObjectiveSummary);
        }
        Ok(Self(truncate_display_width(
            &normalized,
            AGENT_OBJECTIVE_SUMMARY_MAX_DISPLAY_WIDTH,
        )))
    }

    /// 返回 TUI/replay 可展示的摘要正文。
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn deserialize_frozen(source: String) -> Result<Self, AgentLaunchInputError> {
        let summary = Self::new(source.clone())?;
        if summary.0 != source {
            return Err(AgentLaunchInputError::NonCanonicalObjectiveSummary);
        }
        Ok(summary)
    }
}

impl Default for AgentObjectiveSummary {
    fn default() -> Self {
        Self("Child Agent objective unavailable".to_string())
    }
}

impl fmt::Debug for AgentObjectiveSummary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentObjectiveSummary")
            .field("content_chars", &self.0.chars().count())
            .field("display_width", &UnicodeWidthStr::width(self.0.as_str()))
            .finish()
    }
}

impl<'de> Deserialize<'de> for AgentObjectiveSummary {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let source = String::deserialize(deserializer)?;
        Self::deserialize_frozen(source).map_err(serde::de::Error::custom)
    }
}

/// 一次 child Agent launch request；control 与 delivery 保持独立 ownership。
#[derive(Clone, PartialEq, Eq)]
pub struct AgentLaunchRequest {
    objective: AgentObjective,
    title: AgentTitle,
    instructions: AgentInstructions,
}

impl AgentLaunchRequest {
    /// 创建 request，并在边界冻结 resolved title。
    pub fn new(
        objective: AgentObjective,
        display_title: Option<&str>,
        instructions: AgentInstructions,
    ) -> Result<Self, AgentLaunchInputError> {
        let title = AgentTitle::resolve(&objective, display_title)?;
        Ok(Self {
            objective,
            title,
            instructions,
        })
    }

    /// 返回只供 child provider request assembly 消费的完整 objective。
    pub fn objective(&self) -> &AgentObjective {
        &self.objective
    }

    /// 返回 launch 时冻结的 title。
    pub fn title(&self) -> &AgentTitle {
        &self.title
    }

    /// 返回 control-only instructions wrapper。
    pub fn instructions(&self) -> &AgentInstructions {
        &self.instructions
    }
}

impl fmt::Debug for AgentLaunchRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentLaunchRequest")
            .field("objective", &self.objective)
            .field("title", &self.title)
            .field("instructions", &self.instructions)
            .finish()
    }
}

/// 一个显式 launch operation 的原子 request 集合。
#[derive(Clone, PartialEq, Eq)]
pub struct AgentLaunchBatch {
    requests: Vec<AgentLaunchRequest>,
}

impl AgentLaunchBatch {
    /// 创建非空且不超过固定 host 上限的 launch batch。
    pub fn new(requests: Vec<AgentLaunchRequest>) -> Result<Self, AgentLaunchInputError> {
        if requests.is_empty() {
            return Err(AgentLaunchInputError::EmptyBatch);
        }
        if requests.len() > AGENT_LAUNCH_BATCH_LIMIT {
            return Err(AgentLaunchInputError::BatchTooLarge);
        }
        Ok(Self { requests })
    }

    /// 返回 batch 中按调用顺序排列的 request。
    pub fn requests(&self) -> &[AgentLaunchRequest] {
        &self.requests
    }

    /// 消费 batch 并返回 request ownership。
    pub fn into_requests(self) -> Vec<AgentLaunchRequest> {
        self.requests
    }
}

impl fmt::Debug for AgentLaunchBatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentLaunchBatch")
            .field("request_count", &self.requests.len())
            .finish()
    }
}

/// launch input 无法形成安全、确定的 child request。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum AgentLaunchInputError {
    #[error("Agent objective must not be empty")]
    EmptyObjective,
    #[error("Agent display title must not be empty")]
    EmptyDisplayTitle,
    #[error("Agent objective does not contain title content")]
    MissingTitleContent,
    #[error("Agent title contains a terminal control character")]
    TitleContainsTerminalControl,
    #[error("Persisted Agent title is not canonical")]
    NonCanonicalFrozenTitle,
    #[error("Agent outcome summary must not be empty")]
    EmptyOutcomeSummary,
    #[error("Agent outcome summary contains a terminal control character")]
    OutcomeSummaryContainsTerminalControl,
    #[error("Persisted Agent outcome summary is not canonical")]
    NonCanonicalOutcomeSummary,
    #[error("Agent objective summary must not be empty")]
    EmptyObjectiveSummary,
    #[error("Agent objective summary contains a terminal control character")]
    ObjectiveSummaryContainsTerminalControl,
    #[error("Persisted Agent objective summary is not canonical")]
    NonCanonicalObjectiveSummary,
    #[error("Agent launch batch must contain at least one request")]
    EmptyBatch,
    #[error("Agent launch batch exceeds the host limit")]
    BatchTooLarge,
}

/// immutable launch fact 中的单个 child identity 与 frozen title。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentLaunchChildSnapshot {
    pub agent_id: AgentId,
    pub title: AgentTitle,
    /// 对用户交付安全的 objective 摘要；不包含 control-only instructions。
    #[serde(default)]
    pub objective: AgentObjectiveSummary,
}

/// 一次 committed launch operation 的 immutable durable fact。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentLaunchSnapshot {
    pub group_id: AgentLaunchGroupId,
    pub parent_agent_id: AgentId,
    #[serde(default = "default_turn_id")]
    pub parent_turn_id: AgentTurnId,
    pub children: Vec<AgentLaunchChildSnapshot>,
    pub occurred_at_ms: i64,
}

/// child Agent 的唯一 terminal product outcome。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentOutcome {
    Completed,
    Failed,
    Cancelled,
}

/// child Agent terminal outcome 的 durable fact。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentOutcomeSnapshot {
    pub agent_id: AgentId,
    pub title: AgentTitle,
    #[serde(default)]
    pub group_id: Option<AgentLaunchGroupId>,
    #[serde(default)]
    pub parent_agent_id: Option<AgentId>,
    #[serde(default)]
    pub parent_turn_id: Option<AgentTurnId>,
    pub outcome: AgentOutcome,
    pub occurred_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<AgentOutcomeSummary>,
}

fn default_turn_id() -> AgentTurnId {
    AgentTurnId::new(0)
}

/// 一个 child terminal outcome 的 delivery-safe completion projection。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentChildCompletion {
    pub agent_id: AgentId,
    pub title: AgentTitle,
    pub outcome: AgentOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<AgentOutcomeSummary>,
}

/// 一次 launch group 的稳定 completion aggregate。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentGroupCompletion {
    pub group_id: AgentLaunchGroupId,
    pub parent_agent_id: AgentId,
    pub children: Vec<AgentChildCompletion>,
    pub occurred_at_ms: i64,
}

/// launch transaction 提交后立即返回给 host tool 的 typed receipt。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentLaunchReceipt {
    pub group_id: AgentLaunchGroupId,
    pub parent_agent_id: AgentId,
    pub children: Vec<AgentLaunchChildSnapshot>,
}

/// TUI 可观察的 child Agent product lifecycle state。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentProjectionStatus {
    Pending,
    Working,
    WaitingPermission,
    Completed,
    Failed,
    Cancelled,
    Stopping,
    CleanupBlocked,
}

/// child Agent 最新的 delivery-safe 活动摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentActivitySummary {
    Preparing,
    Thinking,
    Retrying { summary: String },
    UsingTool { title: String },
    WaitingPermission { summary: String },
    Idle,
}

/// `/agents` overview 中固定单行 row 的纯 product projection。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentOverviewRow {
    pub agent_id: AgentId,
    pub title: AgentTitle,
    pub status: AgentProjectionStatus,
    pub latest_activity: AgentActivitySummary,
    pub elapsed_ms: Option<u64>,
    pub tool_uses: Option<usize>,
    pub token_usage: Option<usize>,
}

/// 一个 Agent observer 首次收到的一致 overview snapshot。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentOverviewSnapshot {
    pub observation_id: AgentObservationId,
    pub generation: AgentRuntimeGeneration,
    pub revision: AgentProjectionRevision,
    pub rows: Vec<AgentOverviewRow>,
}

/// overview 增量的 stable operation。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentOverviewDeltaKind {
    Upsert(AgentOverviewRow),
    Remove { agent_id: AgentId },
}

/// 同 generation 内严格递增的 overview delta。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentOverviewDelta {
    pub observation_id: AgentObservationId,
    pub generation: AgentRuntimeGeneration,
    pub revision: AgentProjectionRevision,
    pub kind: AgentOverviewDeltaKind,
}

/// child permission response 的完整 authoritative target。
#[derive(Clone, PartialEq, Eq)]
pub struct AgentPermissionTarget {
    pub agent_id: AgentId,
    pub turn_id: AgentTurnId,
    pub generation: AgentRuntimeGeneration,
    pub runtime_target: RuntimeTarget,
    pub request_id: String,
}

impl fmt::Debug for AgentPermissionTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentPermissionTarget")
            .field("has_agent_id", &(self.agent_id.get() != 0))
            .field("has_turn_id", &(self.turn_id.get() != 0))
            .field("has_generation", &(self.generation.get() != 0))
            .field("has_runtime_target", &true)
            .field("has_request_id", &!self.request_id.is_empty())
            .finish()
    }
}

/// preview 中 permission request 的 mutation state。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentPermissionState {
    Pending,
    Submitted,
}

/// preview 当前可交互的 FIFO-head permission projection。
#[derive(Clone, PartialEq, Eq)]
pub struct AgentPermissionRequest {
    pub target: AgentPermissionTarget,
    pub request: RuntimePermissionRequest,
    pub state: AgentPermissionState,
    pub occurred_at_ms: i64,
}

impl fmt::Debug for AgentPermissionRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentPermissionRequest")
            .field("target", &self.target)
            .field("state", &self.state)
            .field("option_count", &self.request.options.len())
            .field("has_title", &self.request.title.is_some())
            .field("has_tool_activity", &self.request.tool_activity.is_some())
            .finish()
    }
}

/// full child transcript 中一条 committed、delivery-safe display item。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentTranscriptItem {
    User { content: String },
    Assistant { content: String },
    Tool { title: String, content: String },
}

/// Enter 打开的 child transcript 一致 snapshot。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTranscriptSnapshot {
    pub observation_id: AgentObservationId,
    pub generation: AgentRuntimeGeneration,
    pub revision: AgentProjectionRevision,
    pub agent_id: AgentId,
    pub title: AgentTitle,
    pub status: AgentProjectionStatus,
    pub items: Vec<AgentTranscriptItem>,
}

/// Space 打开的 quick preview 一致 snapshot。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentPreviewSnapshot {
    pub generation: AgentRuntimeGeneration,
    pub revision: AgentProjectionRevision,
    pub agent_id: AgentId,
    pub title: AgentTitle,
    pub status: AgentProjectionStatus,
    pub latest_activity: AgentActivitySummary,
    pub elapsed_ms: Option<u64>,
    pub latest_committed_answer: Option<String>,
    pub permission: Option<AgentPermissionRequest>,
}

fn first_non_empty_logical_content(content: &str) -> Option<&str> {
    content
        .split(['\n', '\r', '\u{2028}', '\u{2029}'])
        .find(|line| !line.trim().is_empty())
}

fn is_terminal_control(character: char) -> bool {
    matches!(character, '\u{0000}'..='\u{001f}' | '\u{007f}'..='\u{009f}' | '\u{2028}' | '\u{2029}')
}

fn collapse_whitespace(content: &str) -> String {
    content.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_display_width(content: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(content) <= max_width {
        return content.to_string();
    }

    let marker_width = UnicodeWidthStr::width(TRUNCATION_MARKER);
    let content_width = max_width.saturating_sub(marker_width);
    let mut truncated = String::new();
    let mut width: usize = 0;
    for grapheme in UnicodeSegmentation::graphemes(content, true) {
        let grapheme_width = UnicodeWidthStr::width(grapheme);
        if width.saturating_add(grapheme_width) > content_width {
            break;
        }
        truncated.push_str(grapheme);
        width = width.saturating_add(grapheme_width);
    }
    truncated.push_str(TRUNCATION_MARKER);
    truncated
}

#[cfg(test)]
mod tests {
    use unicode_segmentation::UnicodeSegmentation;
    use unicode_width::UnicodeWidthStr;

    use super::*;

    fn objective(content: &str) -> AgentObjective {
        AgentObjective::new(content).expect("fixture objective should be valid")
    }

    #[test]
    fn explicit_title_wins_and_is_normalized_once() {
        let title = AgentTitle::resolve(&objective("fallback"), Some("  concise   title  "))
            .expect("explicit title should resolve");

        assert_eq!(title.as_str(), "concise title");
    }

    #[test]
    fn objective_fallback_uses_first_non_empty_logical_content() {
        let title = AgentTitle::resolve(&objective("\n  写一首俳句  \nsecond line"), None)
            .expect("objective should resolve");

        assert_eq!(title.as_str(), "写一首俳句");
    }

    #[test]
    fn title_rejects_terminal_controls_without_echoing_them() {
        for source in ["line\nbreak", "tab\tinside", "escape\u{001b}[31m"] {
            let error = AgentTitle::resolve(&objective("fallback"), Some(source))
                .expect_err("control input must be rejected");
            assert_eq!(error, AgentLaunchInputError::TitleContainsTerminalControl);
            assert!(!error.to_string().contains(source));
        }
    }

    #[test]
    fn title_truncation_preserves_cjk_emoji_and_combining_graphemes() {
        for grapheme in ["界", "e\u{301}", "👨‍👩‍👧‍👦"] {
            let source = grapheme.repeat(80);
            let title = AgentTitle::resolve(&objective("fallback"), Some(&source))
                .expect("unicode title should resolve");

            assert!(UnicodeWidthStr::width(title.as_str()) <= AGENT_TITLE_MAX_DISPLAY_WIDTH);
            assert!(title.as_str().ends_with(TRUNCATION_MARKER));
            assert!(
                UnicodeSegmentation::graphemes(
                    title
                        .as_str()
                        .strip_suffix(TRUNCATION_MARKER)
                        .expect("truncated title should contain marker"),
                    true,
                )
                .all(|candidate| candidate == grapheme),
                "split grapheme {grapheme:?} in {:?}",
                title.as_str()
            );
        }
    }

    #[test]
    fn narrow_width_truncation_never_splits_a_grapheme() {
        assert_eq!(truncate_display_width("e\u{301}x", 4), "e\u{301}x");
        assert_eq!(truncate_display_width("e\u{301}xxxx", 4), "e\u{301}...");
        assert_eq!(
            UnicodeWidthStr::width(truncate_display_width("界界", 4).as_str()),
            4
        );
    }

    #[test]
    fn launch_debug_omits_objective_title_and_instructions() {
        let request = AgentLaunchRequest::new(
            objective("delivery secret"),
            Some("title secret"),
            AgentInstructions::new("control secret"),
        )
        .expect("request should resolve");
        let debug = format!("{request:?}");

        for secret in ["delivery secret", "title secret", "control secret"] {
            assert!(!debug.contains(secret), "leaked {secret}");
        }
    }

    #[test]
    fn title_serde_roundtrip_rejects_noncanonical_values() {
        let title =
            AgentTitle::resolve(&objective("写一首俳句"), None).expect("title should resolve");
        let json = serde_json::to_string(&title).expect("title should serialize");

        assert_eq!(serde_json::from_str::<AgentTitle>(&json).unwrap(), title);
        assert!(serde_json::from_str::<AgentTitle>("\"  not canonical  \"").is_err());
        assert!(serde_json::from_str::<AgentTitle>("\"line\\nbreak\"").is_err());
    }

    #[test]
    fn batch_enforces_explicit_host_limit_without_leaking_requests() {
        let request = || {
            AgentLaunchRequest::new(
                objective("objective"),
                None,
                AgentInstructions::new("instructions"),
            )
            .expect("request should resolve")
        };

        assert_eq!(
            AgentLaunchBatch::new(Vec::new()),
            Err(AgentLaunchInputError::EmptyBatch)
        );
        assert_eq!(
            AgentLaunchBatch::new((0..=AGENT_LAUNCH_BATCH_LIMIT).map(|_| request()).collect()),
            Err(AgentLaunchInputError::BatchTooLarge)
        );
        let batch = AgentLaunchBatch::new(vec![request()]).expect("batch should be valid");
        assert_eq!(
            format!("{batch:?}"),
            "AgentLaunchBatch { request_count: 1 }"
        );
    }

    #[test]
    fn objective_summary_only_keeps_first_logical_line_and_is_canonical() {
        let objective = objective("  first line with delivery text  \nsecond line secret");
        let summary = AgentObjectiveSummary::from_objective(&objective)
            .expect("objective should produce a summary");
        assert_eq!(summary.as_str(), "first line with delivery text");
        let encoded = serde_json::to_string(&summary).expect("summary should serialize");
        assert_eq!(
            serde_json::from_str::<AgentObjectiveSummary>(&encoded).unwrap(),
            summary
        );
        assert!(serde_json::from_str::<AgentObjectiveSummary>("\" first line \"").is_err());
    }

    #[test]
    fn objective_debug_and_summary_debug_do_not_echo_delivery_body() {
        let objective = objective("private delivery body");
        let summary = AgentObjectiveSummary::from_objective(&objective).unwrap();
        assert!(!format!("{objective:?}").contains("private delivery body"));
        assert!(!format!("{summary:?}").contains("private delivery body"));
    }

    #[test]
    fn replay_facts_without_new_child_metadata_still_restore() {
        let old_launch = serde_json::json!({
            "group_id": 7,
            "parent_agent_id": 1,
            "children": [{"agent_id": 8, "title": "write a haiku"}],
            "occurred_at_ms": 10
        });
        let launch: AgentLaunchSnapshot =
            serde_json::from_value(old_launch).expect("old launch fact should restore");
        assert_eq!(launch.parent_turn_id, AgentTurnId::new(0));
        assert_eq!(
            launch.children[0].objective.as_str(),
            "Child Agent objective unavailable"
        );

        let old_outcome = serde_json::json!({
            "agent_id": 8,
            "title": "write a haiku",
            "outcome": "completed",
            "occurred_at_ms": 20
        });
        let outcome: AgentOutcomeSnapshot =
            serde_json::from_value(old_outcome).expect("old outcome fact should restore");
        assert_eq!(outcome.group_id, None);
        assert_eq!(outcome.parent_agent_id, None);
        assert_eq!(outcome.parent_turn_id, None);
    }

    #[test]
    fn permission_debug_omits_routing_and_option_bodies() {
        let request = AgentPermissionRequest {
            target: AgentPermissionTarget {
                agent_id: AgentId::new(41),
                turn_id: AgentTurnId::new(42),
                generation: AgentRuntimeGeneration::new(43),
                runtime_target: RuntimeTarget::provider("secret-provider", "secret-model"),
                request_id: "secret-request".to_string(),
            },
            request: RuntimePermissionRequest::new(
                "secret-request",
                Some("secret-title".to_string()),
                vec![crate::session::RuntimePermissionOption::new(
                    "secret-option",
                    "secret-name",
                    crate::session::RuntimePermissionOptionKind::AllowOnce,
                )],
            ),
            state: AgentPermissionState::Pending,
            occurred_at_ms: 1,
        };
        let debug = format!("{request:?}");

        for secret in [
            "41",
            "42",
            "43",
            "secret-provider",
            "secret-model",
            "secret-request",
            "secret-option",
            "secret-name",
            "secret-title",
        ] {
            assert!(!debug.contains(secret), "leaked {secret}");
        }
    }
}
