use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    str::FromStr,
};

use provider_protocol::{ConversationItem, ConversationItemValidationError};
use runtime_domain::{paths::hunea_config_dir, session::TranscriptReplayItem};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::{Uuid, Version};

pub(crate) mod jsonl;
pub(crate) mod message_history;
pub(crate) mod meta_derive;
pub(crate) mod metadata;
pub(crate) mod recorder;
mod store;
pub(crate) mod util;

pub use store::{InMemorySessionStore, LocalSessionStore, SessionStore};

/// 短 entry id 固定为 8 个 hex 字符。
const SHORT_ENTRY_ID_HEX_LEN: usize = 8;

/// 短 entry id 碰撞后的重试次数上限。
///
/// 超过这个阈值后直接回退到完整 UUID，避免在热点时间窗口里反复生成相同短 id。
const ENTRY_ID_RETRY_LIMIT: usize = 100;

const SESSION_TITLE_FALLBACK_CHAR_LIMIT: usize = 50;
const SESSION_MESSAGE_PREVIEW_CHAR_LIMIT: usize = 256;

/// `SessionIdParseError` 描述 session id 解析失败原因。
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SessionIdParseError {
    #[error("session id is not a valid UUID: {0}")]
    InvalidUuid(String),
    #[error("session id must use UUIDv7")]
    UnsupportedVersion,
}

/// `SessionId` 是 session 的稳定标识。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionId(Uuid);

impl SessionId {
    /// 生成一个新的 UUIDv7 session id。
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    fn from_uuid(uuid: Uuid) -> Result<Self, SessionIdParseError> {
        if uuid.get_version() == Some(Version::SortRand) {
            Ok(Self(uuid))
        } else {
            Err(SessionIdParseError::UnsupportedVersion)
        }
    }

    fn timestamp_millis(&self) -> u64 {
        // UUIDv7 将 Unix 毫秒时间戳放在最高 48 bit；直接读取可避免在文件名热路径保留 panic 点。
        (self.0.as_u128() >> 80) as u64
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for SessionId {
    type Err = SessionIdParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // 这里只需要“是不是合法 UUID”这一层语义，不向上暴露 `uuid` crate 的详细诊断。
        let uuid =
            Uuid::try_parse(s).map_err(|_| SessionIdParseError::InvalidUuid(s.to_string()))?;
        Self::from_uuid(uuid)
    }
}

impl Serialize for SessionId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for SessionId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

/// session 初始化快照。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionHeader {
    pub session_id: SessionId,
    pub work_dir: PathBuf,
    #[serde(default)]
    pub session_name: Option<String>,
    pub initial_model: String,
    pub git_head: Option<String>,
    pub cli_version: Option<String>,
}

/// `ProjectDir` 表示一个 canonical project 工作目录。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProjectDir(PathBuf);

impl ProjectDir {
    /// 从工作目录路径构造 project 目录标识。
    #[must_use]
    pub fn from_work_dir(work_dir: &Path) -> Self {
        Self(
            work_dir
                .canonicalize()
                .unwrap_or_else(|_| work_dir.to_path_buf()),
        )
    }

    pub(crate) fn from_stored_path(path: PathBuf) -> Self {
        Self(path)
    }

    /// 返回 canonical path。
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }

    /// 返回 SQLite project_dir key 使用的 canonical string。
    #[must_use]
    pub fn canonical_string(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }

    /// 返回适合 UI 展示的路径 display adapter。
    pub fn display(&self) -> std::path::Display<'_> {
        self.0.display()
    }

    /// 返回 project session 文件目录名。
    #[must_use]
    pub fn encoded_session_dir(&self) -> String {
        let canonical_text = self.0.to_string_lossy();

        #[cfg(windows)]
        let encoded = canonical_text
            .strip_prefix(r"\\?\")
            .unwrap_or(canonical_text.as_ref())
            .replace(['\\', ':'], "-");

        #[cfg(not(windows))]
        let encoded = canonical_text
            .strip_prefix('/')
            .unwrap_or(canonical_text.as_ref())
            .replace('/', "-");

        format!("--{encoded}--")
    }
}

/// 会话配置快照。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigSnapshot {
    pub provider_id: String,
    pub model: String,
    pub system_prompt: Option<String>,
}

/// session 列表与恢复所需的元数据快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMeta {
    pub session_id: SessionId,
    pub project_dir: ProjectDir,
    pub title: String,
    pub preview: Option<String>,
    pub first_user_preview: Option<String>,
    pub last_assistant_preview: Option<String>,
    pub total_tokens: u64,
    pub model: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub git_head: Option<String>,
    pub jsonl_path: PathBuf,
    pub size_bytes: Option<u64>,
}

/// session 列表查询选项。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SessionListOptions {
    /// 是否在返回列表前扫描 JSONL 并修复 SQLite 元数据索引。
    pub repair: bool,
}

/// 解析后的 provider-visible 条目，保留其在 session 树中的 entry id。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSessionItem {
    pub entry_id: String,
    pub item: ConversationItem,
}

/// 恢复 session 时返回的完整状态。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ResolvedSessionState {
    pub items: Vec<ResolvedSessionItem>,
    pub transcript: Vec<TranscriptReplayItem>,
    pub latest_config: Option<ConfigSnapshot>,
}

mod session_tree;

pub use session_tree::{
    SessionBranchTreeSnapshot, SessionBranchTreeSnapshotNode, SessionTreeSnapshot,
    SessionTreeSnapshotBranchChoice, SessionTreeSnapshotRow, SessionTreeSnapshotRowKind, resolve,
    resolve_state, session_branch_preview_snapshot, session_branch_tree_snapshot,
    session_tree_snapshot, session_tree_snapshot_for_leaf,
};

/// session 持久化条目类型。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum SessionEntryKind {
    Header(SessionHeader),
    Item(ConversationItem),
    Compaction {
        summary: String,
        first_kept_entry_id: String,
        tokens_before: u64,
    },
    BranchSummary {
        from_id: String,
        summary: String,
    },
    ConfigChange(ConfigSnapshot),
    TranscriptReplay(TranscriptReplayItem),
    Leaf {
        target_id: Option<String>,
    },
}

/// JSONL 中的单行持久化单元。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionEntry {
    pub id: String,
    pub parent_id: Option<String>,
    pub timestamp: i64,
    pub kind: SessionEntryKind,
}

/// session-store 对外暴露的错误语义。
#[derive(Debug, Error)]
pub enum SessionStoreError {
    #[error("failed to access session storage: {source}")]
    IoError {
        #[source]
        source: std::io::Error,
    },
    #[error("failed to serialize session entry `{entry_id}` for persistence: {source}")]
    SerializeEntry {
        entry_id: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("failed to parse persisted session entry at line {line}: {source}")]
    CorruptedEntry {
        line: usize,
        #[source]
        source: serde_json::Error,
    },
    #[error("entry references missing parent `{parent_id}`")]
    DanglingParent { parent_id: String },
    #[error("duplicate entry id `{id}` in session")]
    DuplicateId { id: String },
    #[error("invalid conversation item: {source}")]
    InvalidConversationItem {
        #[source]
        source: ConversationItemValidationError,
    },
    #[error("session `{session_id}` does not exist")]
    SessionNotFound { session_id: SessionId },
    #[error("session store configuration is invalid: {message}")]
    ConfigurationError { message: String },
    #[error("session metadata index is corrupt: {message}")]
    CorruptIndex { message: String },
    #[error("session is missing a header entry: {message}")]
    MissingHeader { message: String },
    #[error("failed to access session metadata sqlite index: {source}")]
    SqliteError {
        #[source]
        source: rusqlite::Error,
    },
    #[error("session metadata index task panicked")]
    MetadataTaskPanicked,
    #[error("session writer channel closed")]
    ChannelClosed,
    #[error("session writer queue is full")]
    QueueFull,
    #[error("session writer worker panicked")]
    WorkerPanicked,
    #[error("failed to resolve session history: {source}")]
    ResolveFailed {
        #[source]
        source: ResolveError,
    },
}

/// `ResolveError` 描述 tree resolve 失败原因。
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ResolveError {
    #[error("leaf `{0}` was not found")]
    LeafNotFound(String),
    #[error("duplicate entry id `{0}` in session")]
    DuplicateId(String),
    #[error("entry references missing parent `{0}`")]
    DanglingParent(String),
    #[error("entry graph contains a parent cycle")]
    CycleDetected,
    #[error("compaction target `{0}` is invalid")]
    InvalidCompactionTarget(String),
    #[error("entry `{0}` cannot be projected as a session tree row")]
    InvalidTreeRow(String),
}

/// 生成 session 内唯一 entry id。
#[must_use]
pub fn generate_entry_id(existing_ids: &HashSet<String>) -> String {
    generate_entry_id_with(existing_ids, Uuid::now_v7)
}

fn generate_entry_id_with(
    existing_ids: &HashSet<String>,
    mut next_uuid: impl FnMut() -> Uuid,
) -> String {
    for _ in 0..ENTRY_ID_RETRY_LIMIT {
        let candidate = short_entry_id(next_uuid());
        if !existing_ids.contains(&candidate) {
            return candidate;
        }
    }

    loop {
        let candidate = next_uuid().to_string();
        if !existing_ids.contains(&candidate) {
            return candidate;
        }
    }
}

fn short_entry_id(uuid: Uuid) -> String {
    // UUIDv7 的高位承载 Unix 毫秒时间戳；短 id 若截取前缀会在同一时间窗口内系统性碰撞。
    // 这里显式取低 32 bit，等价于 simple 格式尾部 8 个 hex 字符，避免把时间前缀误当成离散 id。
    format!(
        "{:0width$x}",
        uuid.as_u128() as u32,
        width = SHORT_ENTRY_ID_HEX_LEN
    )
}

/// 返回 hunea 配置根目录。
#[must_use]
pub fn hunea_dir() -> Option<PathBuf> {
    hunea_config_dir()
}

/// 生成 session JSONL 文件名。
#[must_use]
pub fn session_filename(session_id: &SessionId) -> String {
    let timestamp = format_filename_timestamp_millis(session_id.timestamp_millis());
    format!("{timestamp}_{session_id}.jsonl")
}

fn format_filename_timestamp_millis(timestamp_ms: u64) -> String {
    let seconds = (timestamp_ms / 1_000).min(i64::MAX as u64) as i64;
    let days_since_epoch = seconds.div_euclid(86_400);
    let seconds_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days_since_epoch);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;

    format!("{year:04}-{month:02}-{day:02}T{hour:02}-{minute:02}-{second:02}")
}

fn civil_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    let shifted_days = days_since_epoch + 719_468;
    let era = if shifted_days >= 0 {
        shifted_days / 146_097
    } else {
        (shifted_days - 146_096) / 146_097
    };
    let day_of_era = shifted_days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_piece = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_piece + 2) / 5 + 1;
    let month = month_piece + if month_piece < 10 { 3 } else { -9 };
    let year = year + if month <= 2 { 1 } else { 0 };

    (year, month, day)
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashSet,
        path::{Path, PathBuf},
    };

    use provider_protocol::{ContentBlock, ConversationItem, Role};
    use runtime_domain::{
        paths::hunea_config_dir,
        session::{
            RuntimeTerminalSnapshot, RuntimeToolActivity, RuntimeToolActivityContent,
            RuntimeToolActivityRawValue, RuntimeToolActivityStatus, RuntimeToolKind,
            TranscriptReplayItem,
        },
    };
    use serde_json::json;
    use uuid::Uuid;

    use super::{
        ConfigSnapshot, ProjectDir, SHORT_ENTRY_ID_HEX_LEN, SessionEntry, SessionEntryKind,
        SessionHeader, SessionId, generate_entry_id, generate_entry_id_with, hunea_dir,
        session_filename, short_entry_id,
    };

    #[test]
    fn session_id_new_is_time_ordered() {
        let first = SessionId::new();
        let second = SessionId::new();

        assert!(
            first < second,
            "UUIDv7 session ids should preserve creation order"
        );
    }

    #[test]
    fn session_id_display_and_parse_roundtrip() {
        let session_id = SessionId::new();
        let encoded = session_id.to_string();

        let decoded: SessionId = encoded.parse().expect("session id should parse");

        assert_eq!(decoded, session_id);
    }

    #[test]
    fn session_entry_kind_uses_tagged_payload_serde() {
        let session_id = SessionId::new();
        let entry = SessionEntry {
            id: "entry-1".to_string(),
            parent_id: None,
            timestamp: 1_717_514_800_000,
            kind: SessionEntryKind::Header(SessionHeader {
                session_id: session_id.clone(),
                work_dir: PathBuf::from("/repo"),
                session_name: None,
                initial_model: "gpt-4.1".to_string(),
                git_head: Some("abc123".to_string()),
                cli_version: Some("0.5.1".to_string()),
            }),
        };

        let value = serde_json::to_value(&entry).expect("entry should serialize");

        assert_eq!(value["kind"]["type"], json!("header"));
        assert_eq!(value["kind"]["payload"]["session_id"], json!(session_id));

        let decoded: SessionEntry =
            serde_json::from_value(value).expect("entry should deserialize");
        assert_eq!(decoded, entry);
    }

    #[test]
    fn session_entry_kind_item_roundtrips() {
        let entry = SessionEntry {
            id: "entry-2".to_string(),
            parent_id: Some("entry-1".to_string()),
            timestamp: 1_717_514_800_100,
            kind: SessionEntryKind::Item(ConversationItem::Message {
                role: Role::User,
                content: vec![ContentBlock::Text("hello".to_string())],
            }),
        };

        let decoded: SessionEntry = serde_json::from_str(
            &serde_json::to_string(&entry).expect("item entry should serialize"),
        )
        .expect("item entry should deserialize");

        assert_eq!(decoded, entry);
    }

    #[test]
    fn session_entry_kind_all_variants_roundtrip() {
        let variants = [
            SessionEntryKind::Compaction {
                summary: "summary".to_string(),
                first_kept_entry_id: "entry-3".to_string(),
                tokens_before: 128,
            },
            SessionEntryKind::BranchSummary {
                from_id: "entry-2".to_string(),
                summary: "alternate branch".to_string(),
            },
            SessionEntryKind::ConfigChange(ConfigSnapshot {
                provider_id: "local".to_string(),
                model: "gpt-4.1-mini".to_string(),
                system_prompt: Some("be terse".to_string()),
            }),
            SessionEntryKind::TranscriptReplay(TranscriptReplayItem::ToolActivity {
                activity: sample_tool_activity("call-1", "first"),
            }),
            SessionEntryKind::TranscriptReplay(TranscriptReplayItem::TerminalSnapshot {
                snapshot: RuntimeTerminalSnapshot {
                    terminal_id: "call-1".to_string(),
                    command: Some("cargo test".to_string()),
                    cwd: Some("/repo".to_string()),
                    output: "running tests".to_string(),
                    truncated: false,
                    exit_status: None,
                    released: true,
                },
            }),
            SessionEntryKind::Leaf {
                target_id: Some("entry-5".to_string()),
            },
        ];

        for kind in variants {
            let entry = SessionEntry {
                id: "entry-x".to_string(),
                parent_id: None,
                timestamp: 1_717_514_800_200,
                kind,
            };

            let decoded: SessionEntry = serde_json::from_str(
                &serde_json::to_string(&entry).expect("entry should serialize"),
            )
            .expect("entry should deserialize");

            assert_eq!(decoded, entry);
        }
    }

    #[test]
    fn generate_entry_id_returns_short_unique_value() {
        let entry_id = generate_entry_id(&HashSet::new());

        assert_eq!(entry_id.len(), SHORT_ENTRY_ID_HEX_LEN);
        assert!(
            entry_id.chars().all(|ch| ch.is_ascii_hexdigit()),
            "entry id should use hex characters"
        );
    }

    #[test]
    fn generate_entry_id_retries_until_short_id_is_unique() {
        let colliding_uuid = test_uuid("00000000-0000-7000-8000-0000deadbeef");
        let unique_uuid = test_uuid("00000000-0000-7001-8001-0000cafebabe");
        let existing_ids = HashSet::from([short_entry_id(colliding_uuid)]);
        let mut uuids = vec![colliding_uuid, unique_uuid].into_iter();

        let entry_id = generate_entry_id_with(&existing_ids, || {
            uuids.next().expect("test should provide enough UUIDs")
        });

        assert_eq!(entry_id, "cafebabe");
    }

    #[test]
    fn generate_entry_id_falls_back_to_full_uuid_after_retry_limit() {
        let mut uuids = (0..=100u16)
            .map(|index| test_uuid_with_shared_short_suffix(index, "deadbeef"))
            .collect::<Vec<_>>()
            .into_iter();
        let existing_ids = HashSet::from(["deadbeef".to_string()]);

        let entry_id = generate_entry_id_with(&existing_ids, || {
            uuids.next().expect("test should provide enough UUIDs")
        });

        assert_eq!(
            entry_id, "00000000-0000-7064-8064-0000deadbeef",
            "fallback should return the full UUID once the short id keeps colliding"
        );
    }

    #[test]
    fn generate_entry_id_avoids_existing_ids_in_normal_path() {
        let existing_ids = HashSet::from([generate_entry_id(&HashSet::new())]);

        let next_id = generate_entry_id(&existing_ids);

        assert!(!existing_ids.contains(&next_id));
    }

    #[test]
    #[cfg(not(windows))]
    fn project_dir_encoded_session_dir_preserves_spaces_on_unix() {
        assert_eq!(
            ProjectDir::from_work_dir(Path::new("/home/user/my project")).encoded_session_dir(),
            "--home-user-my project--"
        );
    }

    #[test]
    #[cfg(not(windows))]
    fn project_dir_formats_unix_paths() {
        let project_dir = ProjectDir::from_work_dir(Path::new("/home/user/project"));

        assert_eq!(project_dir.canonical_string(), "/home/user/project");
        assert_eq!(project_dir.as_path(), Path::new("/home/user/project"));
        assert_eq!(project_dir.encoded_session_dir(), "--home-user-project--");
    }

    #[test]
    #[cfg(windows)]
    fn project_dir_formats_windows_paths() {
        let project_dir = ProjectDir::from_work_dir(Path::new(r"C:\Users\User\project"));

        assert_eq!(project_dir.canonical_string(), r"C:\Users\User\project");
        assert_eq!(project_dir.as_path(), Path::new(r"C:\Users\User\project"));
        assert_eq!(
            project_dir.encoded_session_dir(),
            "--C-Users-User-project--"
        );
    }

    #[test]
    fn hunea_dir_matches_runtime_domain_directory() {
        assert_eq!(hunea_dir(), hunea_config_dir());
    }

    #[test]
    fn session_filename_uses_timestamp_and_uuid() {
        let session_id = SessionId::new();
        let filename = session_filename(&session_id);
        let session_id_text = session_id.to_string();

        assert!(filename.ends_with(&format!("_{session_id_text}.jsonl")));
        assert!(filename.contains('T'));
        assert!(!filename.contains(':'));
    }

    #[test]
    fn session_filename_does_not_depend_on_uuid_timestamp_adapter() {
        let session_id = SessionId(test_uuid("00000000-0000-4000-8000-000000000001"));
        let filename = session_filename(&session_id);

        assert_eq!(
            filename,
            "1970-01-01T00-00-00_00000000-0000-4000-8000-000000000001.jsonl"
        );
    }

    fn test_uuid(input: &str) -> Uuid {
        Uuid::try_parse(input).expect("test UUID should parse")
    }

    fn test_uuid_with_shared_short_suffix(index: u16, suffix: &str) -> Uuid {
        test_uuid(&format!(
            "00000000-0000-7{index:03x}-8{index:03x}-0000{suffix}"
        ))
    }

    #[test]
    fn serialize_entry_error_mentions_write_side_context() {
        let source = serde_json::from_str::<serde_json::Value>("{")
            .expect_err("fixture JSON should be invalid");
        let error = super::SessionStoreError::SerializeEntry {
            entry_id: "entry-42".to_string(),
            source,
        };

        let message = error.to_string();

        assert!(message.contains("serialize session entry `entry-42`"));
        assert!(!message.contains("line 0"));
    }

    fn sample_tool_activity(activity_id: &str, text: &str) -> RuntimeToolActivity {
        RuntimeToolActivity {
            activity_id: activity_id.to_string(),
            title: format!("Write {text}"),
            kind: RuntimeToolKind::Write,
            status: RuntimeToolActivityStatus::Completed,
            content: vec![RuntimeToolActivityContent::Diff {
                path: "src/lib.rs".to_string(),
                old_text: Some("old".to_string()),
                new_text: text.to_string(),
                is_truncated: false,
            }],
            locations: Vec::new(),
            raw_input: Some(RuntimeToolActivityRawValue::from(
                serde_json::json!({"path":"src/lib.rs"}),
            )),
            raw_output: Some(RuntimeToolActivityRawValue::tool_result(
                text.to_string(),
                None,
            )),
        }
    }
}
