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

/// settled child 的自动销毁阈值（毫秒）：终态定格 20s 后由 runtime 的 drain 清扫走
/// 完整 delete 路径（10s Just finished 过渡 + 10s Completed 滞留）。消费方
/// （runtime 清扫与 UI 侧销毁唤醒 deadline）共用该值，保证唤醒与清扫窗口一致。
pub const SETTLED_CHILD_AUTO_DESTROY_AFTER_MS: i64 = 20_000;

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

/// `AgentInstructions` 保存 host 在 launch 边界注入的身份指令，只经
/// `AgentTurnRequest` 的 direct instructions 通道进入 provider request assembly。
#[derive(Clone, PartialEq, Eq)]
pub struct AgentInstructions(String);

impl AgentInstructions {
    /// 创建 control-only instructions；空正文表示没有额外控制指令。
    pub fn new(content: impl Into<String>) -> Self {
        Self(content.into())
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

/// `AgentChildMessage` 保存 parent 在 child turn 边界投递的 followup 消息正文。
///
/// 正文会进入 child transcript 与 provider request（用户/模型可见），因此构造边界
/// 即完成非空与 terminal-control 校验；完整正文不参与任何持久化摘要。
#[derive(Clone, PartialEq, Eq)]
pub struct AgentChildMessage(String);

impl AgentChildMessage {
    /// 创建非空且不含 terminal control 的 delivery-safe 消息。
    pub fn new(content: impl Into<String>) -> Result<Self, AgentLaunchInputError> {
        let content = content.into();
        if content.trim().is_empty() {
            return Err(AgentLaunchInputError::EmptyChildMessage);
        }
        if content.chars().any(is_terminal_control) {
            return Err(AgentLaunchInputError::ChildMessageContainsTerminalControl);
        }
        Ok(Self(content))
    }

    /// 返回 child followup turn 使用的消息正文。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for AgentChildMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentChildMessage")
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

/// 一次 child Agent launch request；objective 承载全部 caller 可控输入。
///
/// host 身份指令不在 request 内携带：launch 边界在 request assembly 处恒注入
/// `AgentInstructions`，caller 没有覆盖通道。
#[derive(Clone, PartialEq, Eq)]
pub struct AgentLaunchRequest {
    objective: AgentObjective,
    title: AgentTitle,
}

impl AgentLaunchRequest {
    /// 创建 request，并在边界冻结 resolved title。
    pub fn new(
        objective: AgentObjective,
        display_title: Option<&str>,
    ) -> Result<Self, AgentLaunchInputError> {
        let title = AgentTitle::resolve(&objective, display_title)?;
        Ok(Self { objective, title })
    }

    /// 返回只供 child provider request assembly 消费的完整 objective。
    pub fn objective(&self) -> &AgentObjective {
        &self.objective
    }

    /// 返回 launch 时冻结的 title。
    pub fn title(&self) -> &AgentTitle {
        &self.title
    }
}

impl fmt::Debug for AgentLaunchRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentLaunchRequest")
            .field("objective", &self.objective)
            .field("title", &self.title)
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

/// typed child Agent input（launch request 与 followup 消息）无法形成安全、确定的
/// child request。
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
    #[error("Agent message must not be empty")]
    EmptyChildMessage,
    #[error("Agent message contains a terminal control character")]
    ChildMessageContainsTerminalControl,
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
    /// terminal 定格的累计 elapsed（毫秒）；resume 恢复的投影没有计时起点。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<AgentOutcomeSummary>,
}

fn default_turn_id() -> AgentTurnId {
    AgentTurnId::new(0)
}

/// 一个 child terminal outcome 的 delivery-safe completion projection。
///
/// 该结构是父 Agent tool result 的数据面：`report` 是完整的 committed assistant 正文
/// （按字符上限截断并显式标注）。240 列单行摘要不进入该面——TUI/面板消费的是
/// projection snapshot（`AgentOutcomeSnapshot::summary`），不是 completion。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentChildCompletion {
    pub agent_id: AgentId,
    pub title: AgentTitle,
    pub outcome: AgentOutcome,
    /// 完整 committed assistant 报告；reasoning-only 收尾（无任何非空正文 item）时为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<String>,
    /// 终态定格的 token usage。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<usize>,
    /// 终态定格的工具调用次数。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_uses: Option<usize>,
    /// 终态定格的累计耗时（人类可读档位，如 `16s` / `2m 05s` / `1h 05m`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<String>,
    /// `report` 超出字符上限被截断时为 `true`。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

impl fmt::Debug for AgentChildCompletion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentChildCompletion")
            .field("agent_id", &self.agent_id)
            .field("title", &self.title)
            .field("outcome", &self.outcome)
            // 报告正文只进入 tool result，不进入诊断输出。
            .field(
                "report_chars",
                &self.report.as_ref().map(|report| report.chars().count()),
            )
            .field("tokens", &self.tokens)
            .field("tool_uses", &self.tool_uses)
            .field("duration", &self.duration)
            .field("truncated", &self.truncated)
            .finish()
    }
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
    /// 当前 terminal 周期的定格时刻（unix ms）；未进入终态时为 `None`。
    pub settled_at_ms: Option<i64>,
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

/// `AgentObservationRequestId` 标识一次由调用方发起的 Agent observation 请求。
///
/// 对齐 `SessionLoadRequestId` 模式：由 TUI/调用方单调分配，runtime 只原样回显，
/// 不复用 session load 的语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AgentObservationRequestId(u64);

impl AgentObservationRequestId {
    /// 从调用方维护的单调序列创建请求标识。
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// 返回调用方分配的原始数值。
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// 一次 per-agent observation 的聚合 delivery 视图。
///
/// transcript 与 quick preview 共用同一 observation 与 revision；TUI 的 transcript surface 与
/// quick preview 都从同一 observation 消费，不建立第二个 observer。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentViewSnapshot {
    pub observation_id: AgentObservationId,
    pub generation: AgentRuntimeGeneration,
    pub revision: AgentProjectionRevision,
    pub transcript: AgentTranscriptSnapshot,
    pub preview: AgentPreviewSnapshot,
}

/// child permission queue 的 delivery-safe 投影。
///
/// `Some` 表示该 agent 的 FIFO head 变化（Pending/Submitted），`None` 表示收敛或清空。
/// 它独立于 observation 交付，保证 attention 事实在未打开任何 surface 时也能到达 TUI。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentPermissionUpdate {
    pub agent_id: AgentId,
    pub generation: AgentRuntimeGeneration,
    pub request: Option<AgentPermissionRequest>,
}

/// observation 请求的 closed 拒绝分类；不携带 raw 错误正文。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentObservationRejection {
    UnknownAgent,
    StaleGeneration,
    Duplicate,
}

/// Agent projection port 的 closed 事件集合。
///
/// 只包含 delivery-safe 投影事实；instructions、provider prompt、raw tool payload/result、
/// raw error 与 streaming partial 不得进入任何 snapshot/delta。其中 document fact
/// （`AgentLaunchFact`/`AgentOutcomeFact`）只携带 typed frozen snapshot，与 observation 无关：
/// 即使没有任何 surface 存活，已 commit 的 launch/outcome 事实也要到达 runtime event 边界。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentProjectionEvent {
    AgentsOverviewSnapshotLoaded {
        request_id: AgentObservationRequestId,
        snapshot: AgentOverviewSnapshot,
    },
    AgentsOverviewUpdated {
        delta: AgentOverviewDelta,
    },
    AgentViewSnapshotLoaded {
        request_id: AgentObservationRequestId,
        snapshot: AgentViewSnapshot,
    },
    /// 存活的 per-agent observation 在 revision 严格递增时收到的整快照更新。
    AgentViewUpdated {
        snapshot: AgentViewSnapshot,
    },
    AgentPermissionUpdated {
        update: AgentPermissionUpdate,
    },
    AgentObservationRejected {
        request_id: AgentObservationRequestId,
        reason: AgentObservationRejection,
    },
    /// 一次 committed typed batch launch 的 document fact；与 durable launch fact 同源同序。
    AgentLaunchFact {
        snapshot: AgentLaunchSnapshot,
    },
    /// 一个 child terminal outcome 的 document fact；durable append 成功（或确认无需
    /// 持久化）后才交付，重试成功时交付的是同一 frozen snapshot。
    AgentOutcomeFact {
        snapshot: AgentOutcomeSnapshot,
    },
    /// outcome 持久化尚未收敛时的下次重试计划：`retry_not_before_ms` 之前 orchestrator
    /// 不会重试 durable append（失败后的退避 gate）。UI 侧据此登记兜底唤醒 deadline，
    /// 保证空闲应用（没有用户输入驱动 drain）时重试仍会被触发；事件只携带重试时刻，
    /// 不携带错误正文。
    AgentPersistRetryScheduled {
        retry_not_before_ms: i64,
    },
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
    fn launch_debug_omits_objective_and_title_bodies() {
        let request = AgentLaunchRequest::new(objective("delivery secret"), Some("title secret"))
            .expect("request should resolve");
        let debug = format!("{request:?}");

        for secret in ["delivery secret", "title secret"] {
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
    fn outcome_snapshot_keeps_legacy_replay_compatibility() {
        let title =
            AgentTitle::resolve(&objective("写一首俳句"), None).expect("title should resolve");
        let snapshot = AgentOutcomeSnapshot {
            agent_id: AgentId::new(2),
            title,
            group_id: Some(AgentLaunchGroupId::new(7)),
            parent_agent_id: Some(AgentId::MAIN),
            parent_turn_id: Some(AgentTurnId::new(9)),
            outcome: AgentOutcome::Completed,
            occurred_at_ms: 43,
            duration_ms: None,
            summary: None,
        };

        // duration_ms 缺省可省略：旧 replay JSON 反序列化为 None，
        // 新序列化 None 时同样不写该字段（wire 形态与旧格式一致）。
        let json = serde_json::to_string(&snapshot).expect("snapshot should serialize");
        assert!(!json.contains("duration_ms"));
        assert_eq!(
            serde_json::from_str::<AgentOutcomeSnapshot>(&json).unwrap(),
            snapshot
        );

        let timed = AgentOutcomeSnapshot {
            duration_ms: Some(125_000),
            ..snapshot
        };
        let timed_json = serde_json::to_string(&timed).expect("timed snapshot should serialize");
        assert_eq!(
            serde_json::from_str::<AgentOutcomeSnapshot>(&timed_json).unwrap(),
            timed
        );
    }

    #[test]
    fn batch_enforces_explicit_host_limit_without_leaking_requests() {
        let request = || {
            AgentLaunchRequest::new(objective("objective"), None).expect("request should resolve")
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
    fn child_message_rejects_empty_and_controls_without_echoing_them() {
        for source in ["", "   \t "] {
            assert_eq!(
                AgentChildMessage::new(source),
                Err(AgentLaunchInputError::EmptyChildMessage)
            );
        }
        for source in ["line\nbreak", "tab\tinside", "escape\u{001b}[31m"] {
            let error = AgentChildMessage::new(source).expect_err("control input must be rejected");
            assert_eq!(
                error,
                AgentLaunchInputError::ChildMessageContainsTerminalControl
            );
            assert!(!error.to_string().contains(source));
        }
        let message = AgentChildMessage::new("  refine the report  ")
            .expect("plain message should be accepted");
        assert_eq!(message.as_str(), "  refine the report  ");
    }

    #[test]
    fn child_message_debug_omits_body() {
        let message =
            AgentChildMessage::new("PRIVATE_MESSAGE_BODY").expect("message should construct");
        let debug = format!("{message:?}");
        assert!(debug.contains("content_chars"));
        assert!(!debug.contains("PRIVATE_MESSAGE_BODY"));
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
    fn completion_without_envelope_fields_still_restores() {
        // 旧会话/旧回执没有 report/metrics 字段：反序列化必须得到空信封而不是报错；
        // 已删除的 legacy `summary` 键作为未知字段被容忍，不阻断恢复。
        let old_completion = serde_json::json!({
            "agent_id": 8,
            "title": "write a haiku",
            "outcome": "completed",
            "summary": "committed report"
        });
        let completion: AgentChildCompletion =
            serde_json::from_value(old_completion).expect("old completion should restore");
        assert_eq!(completion.report, None);
        assert_eq!(completion.tokens, None);
        assert_eq!(completion.tool_uses, None);
        assert_eq!(completion.duration, None);
        assert!(!completion.truncated);
    }

    #[test]
    fn completion_debug_omits_report_body() {
        let completion = AgentChildCompletion {
            agent_id: AgentId::new(8),
            title: AgentTitle::resolve(&objective("objective"), Some("title"))
                .expect("title should resolve"),
            outcome: AgentOutcome::Completed,
            report: Some("PRIVATE_REPORT_BODY".to_string()),
            tokens: Some(1200),
            tool_uses: Some(8),
            duration: Some("42s".to_string()),
            truncated: false,
        };
        let debug = format!("{completion:?}");

        assert!(!debug.contains("PRIVATE_REPORT_BODY"));
        assert!(debug.contains("report_chars"));
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

    fn permission_update_fixture() -> AgentPermissionUpdate {
        AgentPermissionUpdate {
            agent_id: AgentId::new(41),
            generation: AgentRuntimeGeneration::new(43),
            request: Some(AgentPermissionRequest {
                target: AgentPermissionTarget {
                    agent_id: AgentId::new(41),
                    turn_id: AgentTurnId::new(42),
                    generation: AgentRuntimeGeneration::new(43),
                    runtime_target: RuntimeTarget::provider("secret-provider", "secret-model"),
                    request_id: "secret-request".to_string(),
                },
                request: RuntimePermissionRequest::new(
                    "secret-request",
                    Some("secret permission body".to_string()),
                    vec![crate::session::RuntimePermissionOption::new(
                        "secret-option-id",
                        "secret option body",
                        crate::session::RuntimePermissionOptionKind::AllowOnce,
                    )],
                ),
                state: AgentPermissionState::Pending,
                occurred_at_ms: 1,
            }),
        }
    }

    #[test]
    fn permission_update_debug_omits_permission_bodies() {
        let debug = format!("{:?}", permission_update_fixture());

        for secret in [
            "secret-provider",
            "secret-model",
            "secret-request",
            "secret-option-id",
            "secret permission body",
            "secret option body",
        ] {
            assert!(!debug.contains(secret), "leaked {secret}");
        }
    }

    #[test]
    fn projection_events_echo_request_id_and_classify_rejections() {
        let request_id = AgentObservationRequestId::new(7);
        assert_eq!(request_id.get(), 7);

        let loaded = AgentProjectionEvent::AgentsOverviewSnapshotLoaded {
            request_id,
            snapshot: AgentOverviewSnapshot {
                observation_id: AgentObservationId::new(1),
                generation: AgentRuntimeGeneration::new(2),
                revision: AgentProjectionRevision::new(3),
                rows: Vec::new(),
            },
        };
        let AgentProjectionEvent::AgentsOverviewSnapshotLoaded {
            request_id: echoed, ..
        } = &loaded
        else {
            panic!("overview snapshot event should carry the caller request id");
        };
        assert_eq!(*echoed, request_id);

        let rejected = AgentProjectionEvent::AgentObservationRejected {
            request_id,
            reason: AgentObservationRejection::UnknownAgent,
        };
        let AgentProjectionEvent::AgentObservationRejected { reason, .. } = &rejected else {
            panic!("observation rejection should stay typed");
        };
        assert_eq!(*reason, AgentObservationRejection::UnknownAgent);
        assert!(!format!("{rejected:?}").contains("error"));
    }

    #[test]
    fn view_snapshot_binds_transcript_and_preview_to_one_observation() {
        let observation_id = AgentObservationId::new(9);
        let generation = AgentRuntimeGeneration::new(2);
        let revision = AgentProjectionRevision::new(4);
        let agent_id = AgentId::new(41);
        let snapshot = AgentViewSnapshot {
            observation_id,
            generation,
            revision,
            transcript: AgentTranscriptSnapshot {
                observation_id,
                generation,
                revision,
                agent_id,
                title: AgentTitle::resolve(&objective("committed task"), None)
                    .expect("title should resolve"),
                status: AgentProjectionStatus::Completed,
                items: vec![
                    AgentTranscriptItem::User {
                        content: "committed task".to_string(),
                    },
                    AgentTranscriptItem::Assistant {
                        content: "committed answer".to_string(),
                    },
                ],
            },
            preview: AgentPreviewSnapshot {
                generation,
                revision,
                agent_id,
                title: AgentTitle::resolve(&objective("committed task"), None)
                    .expect("title should resolve"),
                status: AgentProjectionStatus::Completed,
                latest_activity: AgentActivitySummary::Idle,
                elapsed_ms: Some(10),
                latest_committed_answer: Some("committed answer".to_string()),
                permission: None,
            },
        };

        assert_eq!(snapshot.transcript.observation_id, observation_id);
        assert_eq!(snapshot.preview.revision, revision);
        assert_eq!(
            snapshot.transcript.items.last(),
            Some(&AgentTranscriptItem::Assistant {
                content: "committed answer".to_string()
            })
        );
    }
}
