use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    str::FromStr,
};

use provider_protocol::{ConversationItem, ConversationItemValidationError};
use runtime_domain::{paths::hunea_config_dir, session::TranscriptReplayItem};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::{Timestamp, Uuid, Version};

pub(crate) mod jsonl;
pub(crate) mod metadata;
pub(crate) mod recorder;
mod store;

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

    fn timestamp(&self) -> Timestamp {
        self.0
            .get_timestamp()
            .expect("SessionId should always contain a UUIDv7 timestamp")
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
    pub project_dir: String,
    pub title: String,
    pub preview: Option<String>,
    pub first_user_preview: Option<String>,
    pub last_assistant_preview: Option<String>,
    pub total_tokens: u64,
    pub model: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub git_head: Option<String>,
    pub work_dir: PathBuf,
    pub jsonl_path: PathBuf,
    pub size_bytes: Option<u64>,
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
    #[error("session metadata index is inconsistent: {message}")]
    IndexInconsistent { message: String },
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

/// 编码 project 目录名。
pub fn encode_project_dir(cwd: &Path) -> String {
    let canonical_path = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let canonical_text = canonical_path.to_string_lossy();

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

/// 返回 hunea 配置根目录。
pub fn hunea_dir() -> Option<PathBuf> {
    hunea_config_dir()
}

/// 生成 session JSONL 文件名。
pub fn session_filename(session_id: &SessionId) -> String {
    let timestamp = format_filename_timestamp(session_id.timestamp());
    format!("{timestamp}_{session_id}.jsonl")
}

fn format_filename_timestamp(timestamp: Timestamp) -> String {
    let (seconds, _) = timestamp.to_unix();
    let seconds = seconds as i64;
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
        fs,
        io::Write,
        path::{Path, PathBuf},
    };

    use provider_protocol::{ContentBlock, ConversationItem, Role, ToolCall};
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
        ConfigSnapshot, SHORT_ENTRY_ID_HEX_LEN, SessionEntry, SessionEntryKind, SessionHeader,
        SessionId, encode_project_dir, generate_entry_id, generate_entry_id_with, hunea_dir,
        jsonl::{JsonlLoader, JsonlWriter},
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
    fn encode_project_dir_preserves_spaces_on_unix() {
        assert_eq!(
            encode_project_dir(Path::new("/home/user/my project")),
            "--home-user-my project--"
        );
    }

    #[test]
    #[cfg(not(windows))]
    fn encode_project_dir_formats_unix_paths() {
        assert_eq!(
            encode_project_dir(Path::new("/home/user/project")),
            "--home-user-project--"
        );
    }

    #[test]
    #[cfg(windows)]
    fn encode_project_dir_formats_windows_paths() {
        assert_eq!(
            encode_project_dir(Path::new(r"C:\Users\User\project")),
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
    fn jsonl_writer_delays_file_creation_until_first_write() {
        let temp_dir = test_temp_dir("delayed-create");
        let jsonl_path = temp_dir.join("session.jsonl");
        let mut writer = JsonlWriter::new(jsonl_path.clone());

        assert!(!writer.file_exists());
        assert!(!jsonl_path.exists());

        writer
            .write(&sample_entries()[0])
            .expect("first write should create the JSONL file");

        assert!(writer.file_exists());
        assert!(jsonl_path.exists());
    }

    #[test]
    fn jsonl_writer_and_loader_roundtrip_entries_in_write_order() {
        let temp_dir = test_temp_dir("roundtrip");
        let jsonl_path = temp_dir.join("session.jsonl");
        let entries = sample_entries();
        let mut writer = JsonlWriter::new(jsonl_path.clone());

        writer
            .write_batch(&entries)
            .expect("batch write should persist all entries");

        let loaded = JsonlLoader::load(&jsonl_path).expect("loader should parse persisted entries");

        assert_eq!(loaded, entries);
    }

    #[test]
    fn jsonl_loader_skips_truncated_last_line() {
        let temp_dir = test_temp_dir("truncated-last-line");
        let jsonl_path = temp_dir.join("session.jsonl");
        let entries = sample_entries();
        let mut writer = JsonlWriter::new(jsonl_path.clone());

        writer
            .write_batch(&entries[..2])
            .expect("seed entries should persist");
        fs::OpenOptions::new()
            .append(true)
            .open(&jsonl_path)
            .expect("jsonl file should exist")
            .write_all(br#"{"id":"partial""#)
            .expect("partial line should append");

        let loaded = JsonlLoader::load(&jsonl_path).expect("loader should ignore truncated tail");

        assert_eq!(loaded, entries[..2].to_vec());
    }

    #[test]
    fn jsonl_loader_skips_invalid_middle_line_and_keeps_other_entries() {
        let temp_dir = test_temp_dir("invalid-middle-line");
        let jsonl_path = temp_dir.join("session.jsonl");
        let entries = sample_entries();
        let header_json = serde_json::to_string(&entries[0]).expect("header should serialize");
        let user_json = serde_json::to_string(&entries[1]).expect("user entry should serialize");

        fs::write(
            &jsonl_path,
            format!("{header_json}\n{{not-json}}\n{user_json}\n"),
        )
        .expect("fixture file should be writable");

        let loaded = JsonlLoader::load(&jsonl_path).expect("loader should skip invalid middle row");

        assert_eq!(loaded, entries[..2].to_vec());
    }

    #[test]
    fn jsonl_loader_skips_invalid_utf8_middle_line_and_keeps_other_entries() {
        let temp_dir = test_temp_dir("invalid-utf8-middle-line");
        let jsonl_path = temp_dir.join("session.jsonl");
        let entries = sample_entries();
        let header_json = serde_json::to_string(&entries[0]).expect("header should serialize");
        let user_json = serde_json::to_string(&entries[1]).expect("user entry should serialize");
        let mut file = fs::File::create(&jsonl_path).expect("fixture file should be creatable");

        file.write_all(header_json.as_bytes())
            .expect("header should write");
        file.write_all(b"\n").expect("header newline should write");
        file.write_all(b"{\"id\":\"broken\",\xff}\n")
            .expect("invalid utf8 line should write");
        file.write_all(user_json.as_bytes())
            .expect("user entry should write");
        file.write_all(b"\n").expect("user newline should write");

        let loaded =
            JsonlLoader::load(&jsonl_path).expect("loader should skip invalid UTF-8 middle row");

        assert_eq!(loaded, entries[..2].to_vec());
    }

    #[test]
    fn jsonl_loader_skips_truncated_invalid_utf8_tail() {
        let temp_dir = test_temp_dir("truncated-invalid-utf8-tail");
        let jsonl_path = temp_dir.join("session.jsonl");
        let entries = sample_entries();
        let mut writer = JsonlWriter::new(jsonl_path.clone());

        writer
            .write_batch(&entries[..2])
            .expect("seed entries should persist");
        fs::OpenOptions::new()
            .append(true)
            .open(&jsonl_path)
            .expect("jsonl file should exist")
            .write_all(b"{\"id\":\"partial\",\xff")
            .expect("truncated invalid utf8 tail should append");

        let loaded =
            JsonlLoader::load(&jsonl_path).expect("loader should ignore truncated invalid UTF-8");

        assert_eq!(loaded, entries[..2].to_vec());
    }

    #[test]
    fn jsonl_loader_keeps_first_entry_when_ids_repeat() {
        let temp_dir = test_temp_dir("duplicate-id");
        let jsonl_path = temp_dir.join("session.jsonl");
        let mut entries = sample_entries();
        let mut duplicate_entry = entries[2].clone();
        duplicate_entry.kind = SessionEntryKind::Item(ConversationItem::text(
            Role::Assistant,
            "different body should be ignored",
        ));
        entries.push(duplicate_entry);
        let mut writer = JsonlWriter::new(jsonl_path.clone());

        writer
            .write_batch(&entries)
            .expect("duplicate fixture should persist");

        let loaded =
            JsonlLoader::load(&jsonl_path).expect("loader should deduplicate repeated ids");

        assert_eq!(loaded.len(), sample_entries().len());
        assert_eq!(loaded[2], sample_entries()[2]);
    }

    #[test]
    fn jsonl_loader_rejects_dangling_parent_reference() {
        let temp_dir = test_temp_dir("dangling-parent");
        let jsonl_path = temp_dir.join("session.jsonl");
        let mut entries = sample_entries();
        entries[1].parent_id = Some("missing-parent".to_string());
        let mut writer = JsonlWriter::new(jsonl_path.clone());

        writer
            .write_batch(&entries[..2])
            .expect("fixture entries should persist");

        let error = JsonlLoader::load(&jsonl_path).expect_err("dangling parent should fail");

        assert!(matches!(
            error,
            super::SessionStoreError::DanglingParent { ref parent_id }
                if parent_id == "missing-parent"
        ));
    }

    #[test]
    fn jsonl_writer_and_loader_roundtrip_large_entry_payload() {
        let temp_dir = test_temp_dir("large-entry");
        let jsonl_path = temp_dir.join("session.jsonl");
        let large_text = "x".repeat(12 * 1024);
        let large_entry = SessionEntry {
            id: "large-user".to_string(),
            parent_id: Some("header".to_string()),
            timestamp: 1_717_514_800_099,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::User, large_text)),
        };
        let mut entries = sample_entries();
        entries.push(large_entry);
        let mut writer = JsonlWriter::new(jsonl_path.clone());

        writer
            .write_batch(&entries)
            .expect("large entry batch should persist");

        let loaded = JsonlLoader::load(&jsonl_path).expect("loader should parse large payload");

        assert_eq!(loaded, entries);
    }

    #[test]
    fn resolve_returns_linear_history_items_in_order() {
        let entries = linear_history_entries();

        let resolved = super::resolve(&entries, "user-2").expect("linear history should resolve");

        assert_eq!(
            resolved,
            vec![
                ConversationItem::text(Role::User, "hello"),
                ConversationItem::text(Role::Assistant, "hi"),
                ConversationItem::text(Role::User, "follow up"),
            ]
        );
    }

    #[test]
    fn resolve_state_returns_explicit_transcript_replay_items() {
        let session_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ab"
            .parse()
            .expect("fixture session id should parse");
        let expected_activity = sample_tool_activity("call-1", "final");
        let expected_snapshot = RuntimeTerminalSnapshot {
            terminal_id: "call-1".to_string(),
            command: Some("cargo test".to_string()),
            cwd: Some("/repo".to_string()),
            output: "test output".to_string(),
            truncated: false,
            exit_status: None,
            released: true,
        };
        let entries = vec![
            SessionEntry {
                id: "header".to_string(),
                parent_id: None,
                timestamp: 1_717_514_800_000,
                kind: SessionEntryKind::Header(SessionHeader {
                    session_id,
                    work_dir: PathBuf::from("/repo"),
                    session_name: None,
                    initial_model: "qwen3".to_string(),
                    git_head: None,
                    cli_version: None,
                }),
            },
            SessionEntry {
                id: "assistant-1".to_string(),
                parent_id: Some("header".to_string()),
                timestamp: 1_717_514_800_001,
                kind: SessionEntryKind::Item(ConversationItem::assistant_with_tool_calls(
                    "editing".to_string(),
                    vec![ToolCall::new(
                        "call-1",
                        "write_file",
                        r#"{"path":"src/lib.rs"}"#,
                    )],
                )),
            },
            SessionEntry {
                id: "tool-1".to_string(),
                parent_id: Some("assistant-1".to_string()),
                timestamp: 1_717_514_800_002,
                kind: SessionEntryKind::Item(ConversationItem::tool_result(
                    "call-1",
                    vec![ContentBlock::Text("plain provider output".to_string())],
                    false,
                )),
            },
            SessionEntry {
                id: "replay-start".to_string(),
                parent_id: Some("tool-1".to_string()),
                timestamp: 1_717_514_800_003,
                kind: SessionEntryKind::TranscriptReplay(TranscriptReplayItem::ToolActivity {
                    activity: sample_tool_activity("call-1", "started"),
                }),
            },
            SessionEntry {
                id: "replay-final".to_string(),
                parent_id: Some("replay-start".to_string()),
                timestamp: 1_717_514_800_004,
                kind: SessionEntryKind::TranscriptReplay(TranscriptReplayItem::ToolActivity {
                    activity: expected_activity.clone(),
                }),
            },
            SessionEntry {
                id: "terminal-final".to_string(),
                parent_id: Some("replay-final".to_string()),
                timestamp: 1_717_514_800_005,
                kind: SessionEntryKind::TranscriptReplay(TranscriptReplayItem::TerminalSnapshot {
                    snapshot: expected_snapshot.clone(),
                }),
            },
        ];

        let resolved =
            super::resolve_state(&entries, "terminal-final").expect("state should resolve");

        assert_eq!(
            resolved
                .items
                .iter()
                .map(|item| item.item.text_content())
                .collect::<Vec<_>>(),
            vec!["editing", "plain provider output"]
        );
        assert_eq!(
            resolved.transcript,
            vec![
                TranscriptReplayItem::ToolActivity {
                    activity: expected_activity
                },
                TranscriptReplayItem::TerminalSnapshot {
                    snapshot: expected_snapshot
                },
            ]
        );
    }

    #[test]
    fn resolve_returns_branch_specific_history() {
        let entries = branching_entries();

        let left_branch =
            super::resolve(&entries, "assistant-b").expect("left branch should resolve");
        let right_branch =
            super::resolve(&entries, "assistant-c").expect("right branch should resolve");

        assert_eq!(
            left_branch,
            vec![
                ConversationItem::text(Role::User, "hello"),
                ConversationItem::text(Role::Assistant, "branch-b"),
            ]
        );
        assert_eq!(
            right_branch,
            vec![
                ConversationItem::text(Role::User, "hello"),
                ConversationItem::text(Role::Assistant, "branch-c"),
            ]
        );
    }

    #[test]
    fn resolve_keeps_following_appended_entries_on_selected_branch() {
        let entries = branching_with_append_entries();

        let resolved =
            super::resolve(&entries, "user-d").expect("appended branch history should resolve");

        assert_eq!(
            resolved,
            vec![
                ConversationItem::text(Role::User, "hello"),
                ConversationItem::text(Role::Assistant, "branch-c"),
                ConversationItem::text(Role::User, "branch follow-up"),
            ]
        );
    }

    #[test]
    fn session_tree_snapshot_marks_active_path_and_user_rewind_prefill() {
        let entries = linear_history_entries();

        let snapshot = super::session_tree_snapshot(&entries).expect("linear tree should snapshot");

        assert_eq!(snapshot.current_row_id.as_deref(), Some("user-2"));
        assert!(snapshot.active_row_ids.contains("user-1"));
        assert!(snapshot.active_row_ids.contains("assistant-1"));
        assert!(snapshot.active_row_ids.contains("user-2"));
        let user = snapshot
            .rows
            .iter()
            .find(|row| row.id == "user-2")
            .expect("user-2 should exist");
        assert_eq!(user.kind, super::SessionTreeSnapshotRowKind::User);
        assert_eq!(user.rewind_target_id.as_deref(), Some("assistant-1"));
        assert_eq!(user.rewind_prefill.as_deref(), Some("follow up"));
        let assistant = snapshot
            .rows
            .iter()
            .find(|row| row.id == "assistant-1")
            .expect("assistant-1 should exist");
        assert_eq!(assistant.rewind_target_id.as_deref(), Some("assistant-1"));
        assert_eq!(assistant.rewind_prefill, None);
    }

    #[test]
    fn session_tree_snapshot_allows_rewind_only_after_single_tool_call_result() {
        let entries = assistant_tool_batch_entries(&["call-1"], &["call-1"], true);

        let snapshot =
            super::session_tree_snapshot(&entries).expect("single tool batch should snapshot");
        let assistant = snapshot_row(&snapshot, "assistant-1");
        let tool = snapshot_row(&snapshot, "tool-1");

        assert_eq!(
            assistant.rewind_target_id, None,
            "assistant tool-call rows are attached to the following tool results"
        );
        assert_eq!(
            tool.rewind_target_id.as_deref(),
            Some("tool-1"),
            "the only tool result closes the provider-visible batch and is rewindable"
        );
    }

    #[test]
    fn session_tree_snapshot_allows_rewind_only_after_final_tool_call_result() {
        let entries = assistant_tool_batch_entries(
            &["call-1", "call-2", "call-3"],
            &["call-1", "call-2", "call-3"],
            true,
        );

        let snapshot =
            super::session_tree_snapshot(&entries).expect("multi tool batch should snapshot");
        let assistant = snapshot_row(&snapshot, "assistant-1");
        let first_tool = snapshot_row(&snapshot, "tool-1");
        let second_tool = snapshot_row(&snapshot, "tool-2");
        let final_tool = snapshot_row(&snapshot, "tool-3");

        assert_eq!(
            assistant.rewind_target_id, None,
            "assistant tool-call rows must not be independently rewindable"
        );
        assert_eq!(
            first_tool.rewind_target_id, None,
            "intermediate tool results leave unresolved provider tool calls"
        );
        assert_eq!(
            second_tool.rewind_target_id, None,
            "intermediate tool results leave unresolved provider tool calls"
        );
        assert_eq!(
            final_tool.rewind_target_id.as_deref(),
            Some("tool-3"),
            "only the final tool result that resolves every call in the batch is rewindable"
        );
    }

    #[test]
    fn session_tree_snapshot_does_not_rewind_incomplete_tool_call_batch() {
        let entries = assistant_tool_batch_entries(&["call-1", "call-2"], &["call-1"], true);

        let snapshot =
            super::session_tree_snapshot(&entries).expect("incomplete tool batch should snapshot");
        let assistant = snapshot_row(&snapshot, "assistant-1");
        let tool = snapshot_row(&snapshot, "tool-1");

        assert_eq!(
            assistant.rewind_target_id, None,
            "assistant tool-call rows are not safe restore targets without all results"
        );
        assert_eq!(
            tool.rewind_target_id, None,
            "a partial tool-result batch would still leave unresolved provider tool calls"
        );
    }

    #[test]
    fn session_tree_snapshot_maps_reasoning_rewind_to_following_assistant() {
        let entries = vec![
            SessionEntry {
                id: "user-1".to_string(),
                parent_id: None,
                timestamp: 1,
                kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "hello")),
            },
            SessionEntry {
                id: "reasoning-1".to_string(),
                parent_id: Some("user-1".to_string()),
                timestamp: 2,
                kind: SessionEntryKind::Item(ConversationItem::Reasoning {
                    content: "thinking".to_string(),
                    summary: None,
                    encrypted: None,
                }),
            },
            SessionEntry {
                id: "assistant-1".to_string(),
                parent_id: Some("reasoning-1".to_string()),
                timestamp: 3,
                kind: SessionEntryKind::Item(ConversationItem::text(Role::Assistant, "answer")),
            },
            SessionEntry {
                id: "assistant-replay".to_string(),
                parent_id: Some("assistant-1".to_string()),
                timestamp: 4,
                kind: SessionEntryKind::TranscriptReplay(TranscriptReplayItem::Message {
                    role: runtime_domain::session::TranscriptReplayRole::Assistant,
                    content: "answer".to_string(),
                }),
            },
        ];

        let snapshot =
            super::session_tree_snapshot(&entries).expect("reasoning tree should snapshot");
        let reasoning = snapshot
            .rows
            .iter()
            .find(|row| row.id == "reasoning-1")
            .expect("reasoning row should remain visible");
        let assistant = snapshot
            .rows
            .iter()
            .find(|row| row.id == "assistant-1")
            .expect("assistant row should exist");

        assert_eq!(reasoning.kind, super::SessionTreeSnapshotRowKind::Reasoning);
        assert_eq!(
            reasoning.rewind_target_id, assistant.rewind_target_id,
            "reasoning should rewind to its owning assistant turn, not to itself"
        );
        assert_eq!(
            reasoning.rewind_target_id.as_deref(),
            Some("assistant-replay"),
            "reasoning should reuse the assistant row's final restore target"
        );
    }

    #[test]
    fn session_tree_snapshot_marks_trailing_reasoning_as_not_rewindable() {
        let entries = vec![
            SessionEntry {
                id: "user-1".to_string(),
                parent_id: None,
                timestamp: 1,
                kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "hello")),
            },
            SessionEntry {
                id: "reasoning-1".to_string(),
                parent_id: Some("user-1".to_string()),
                timestamp: 2,
                kind: SessionEntryKind::Item(ConversationItem::Reasoning {
                    content: "thinking".to_string(),
                    summary: None,
                    encrypted: None,
                }),
            },
        ];

        let snapshot =
            super::session_tree_snapshot(&entries).expect("reasoning tree should snapshot");
        let reasoning = snapshot
            .rows
            .iter()
            .find(|row| row.id == "reasoning-1")
            .expect("reasoning row should remain visible");

        assert_eq!(reasoning.kind, super::SessionTreeSnapshotRowKind::Reasoning);
        assert_eq!(
            reasoning.rewind_target_id, None,
            "trailing reasoning without an assistant answer should be visible but not rewindable"
        );
    }

    #[test]
    fn session_tree_snapshot_projects_only_logical_rows_without_replay_duplicates() {
        let session_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ab"
            .parse()
            .expect("fixture session id should parse");
        let entries = vec![
            SessionEntry {
                id: "header".to_string(),
                parent_id: None,
                timestamp: 1_717_514_800_000,
                kind: SessionEntryKind::Header(SessionHeader {
                    session_id,
                    work_dir: PathBuf::from("/repo"),
                    session_name: None,
                    initial_model: "qwen3".to_string(),
                    git_head: None,
                    cli_version: None,
                }),
            },
            SessionEntry {
                id: "user-1".to_string(),
                parent_id: Some("header".to_string()),
                timestamp: 1_717_514_800_001,
                kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "hello")),
            },
            SessionEntry {
                id: "user-replay".to_string(),
                parent_id: Some("user-1".to_string()),
                timestamp: 1_717_514_800_002,
                kind: SessionEntryKind::TranscriptReplay(TranscriptReplayItem::Message {
                    role: runtime_domain::session::TranscriptReplayRole::User,
                    content: "hello".to_string(),
                }),
            },
            SessionEntry {
                id: "assistant-1".to_string(),
                parent_id: Some("user-replay".to_string()),
                timestamp: 1_717_514_800_003,
                kind: SessionEntryKind::Item(ConversationItem::text(Role::Assistant, "answer")),
            },
            SessionEntry {
                id: "assistant-replay".to_string(),
                parent_id: Some("assistant-1".to_string()),
                timestamp: 1_717_514_800_004,
                kind: SessionEntryKind::TranscriptReplay(TranscriptReplayItem::Message {
                    role: runtime_domain::session::TranscriptReplayRole::Assistant,
                    content: "answer".to_string(),
                }),
            },
            SessionEntry {
                id: "config-1".to_string(),
                parent_id: Some("assistant-replay".to_string()),
                timestamp: 1_717_514_800_005,
                kind: SessionEntryKind::ConfigChange(ConfigSnapshot {
                    provider_id: "local".to_string(),
                    model: "qwen3".to_string(),
                    system_prompt: None,
                }),
            },
            SessionEntry {
                id: "leaf-1".to_string(),
                parent_id: Some("config-1".to_string()),
                timestamp: 1_717_514_800_006,
                kind: SessionEntryKind::Leaf {
                    target_id: Some("assistant-1".to_string()),
                },
            },
        ];

        let snapshot = super::session_tree_snapshot(&entries).expect("replay tree should snapshot");

        assert_eq!(
            snapshot
                .rows
                .iter()
                .map(|row| row.id.as_str())
                .collect::<Vec<_>>(),
            vec!["user-1", "assistant-1"],
            "tree projection should show one logical row per user-visible message only"
        );
        assert_eq!(
            snapshot
                .rows
                .iter()
                .map(|row| row.preview_content.as_str())
                .collect::<Vec<_>>(),
            vec!["hello", "answer"],
            "provider items and transcript replay records with the same visible content must not duplicate"
        );
    }

    #[test]
    fn session_tree_snapshot_prefers_assistant_replay_content_for_preview() {
        let collapsed_hint = "… +26 lines (ctrl + t to view transcript)";
        let full_content = "assistant full line 1\nassistant full line 2";
        let entries = vec![
            SessionEntry {
                id: "user-1".to_string(),
                parent_id: None,
                timestamp: 1,
                kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "hello")),
            },
            SessionEntry {
                id: "assistant-1".to_string(),
                parent_id: Some("user-1".to_string()),
                timestamp: 2,
                kind: SessionEntryKind::Item(ConversationItem::text(
                    Role::Assistant,
                    collapsed_hint,
                )),
            },
            SessionEntry {
                id: "assistant-replay".to_string(),
                parent_id: Some("assistant-1".to_string()),
                timestamp: 3,
                kind: SessionEntryKind::TranscriptReplay(TranscriptReplayItem::Message {
                    role: runtime_domain::session::TranscriptReplayRole::Assistant,
                    content: full_content.to_string(),
                }),
            },
        ];

        let snapshot =
            super::session_tree_snapshot(&entries).expect("assistant replay tree should snapshot");
        let assistant = snapshot
            .rows
            .iter()
            .find(|row| row.id == "assistant-1")
            .expect("assistant row should exist");

        assert_eq!(assistant.preview_content, full_content);
        assert!(
            !assistant.summary.contains("ctrl + t"),
            "tree preview summary should be derived from full replay content, not collapsed UI text"
        );
    }

    #[test]
    fn session_tree_snapshot_prefers_tool_replay_content_for_preview() {
        let collapsed_hint = "… +8 lines (ctrl + t to view transcript)";
        let entries = vec![
            SessionEntry {
                id: "assistant-1".to_string(),
                parent_id: None,
                timestamp: 1,
                kind: SessionEntryKind::Item(ConversationItem::assistant_with_tool_calls(
                    "checking".to_string(),
                    vec![ToolCall::new(
                        "call-1",
                        "read_file",
                        r#"{"path":"src/lib.rs"}"#,
                    )],
                )),
            },
            SessionEntry {
                id: "tool-1".to_string(),
                parent_id: Some("assistant-1".to_string()),
                timestamp: 2,
                kind: SessionEntryKind::Item(ConversationItem::tool_result(
                    "call-1",
                    vec![ContentBlock::Text(collapsed_hint.to_string())],
                    false,
                )),
            },
            SessionEntry {
                id: "tool-replay".to_string(),
                parent_id: Some("tool-1".to_string()),
                timestamp: 3,
                kind: SessionEntryKind::TranscriptReplay(TranscriptReplayItem::ToolActivity {
                    activity: sample_tool_activity("call-1", "full provider output"),
                }),
            },
        ];

        let snapshot =
            super::session_tree_snapshot(&entries).expect("tool replay tree should snapshot");
        let tool = snapshot
            .rows
            .iter()
            .find(|row| row.id == "tool-1")
            .expect("tool row should exist");

        assert_eq!(tool.preview_content, "full provider output");
        assert!(
            !tool.summary.contains("ctrl + t"),
            "tool preview summary should be derived from replay output, not collapsed UI text"
        );
    }

    #[test]
    fn session_tree_snapshot_projects_assistant_tool_calls_into_debug_preview_replay() {
        let entries = vec![
            SessionEntry {
                id: "user-1".to_string(),
                parent_id: None,
                timestamp: 1,
                kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "inspect")),
            },
            SessionEntry {
                id: "assistant-1".to_string(),
                parent_id: Some("user-1".to_string()),
                timestamp: 2,
                kind: SessionEntryKind::Item(ConversationItem::assistant_with_tool_calls(
                    "I will inspect the file.".to_string(),
                    vec![ToolCall::new(
                        "call-1",
                        "read_file",
                        r#"{"path":"Cargo.toml","limit":20}"#,
                    )],
                )),
            },
        ];

        let snapshot = super::session_tree_snapshot(&entries)
            .expect("assistant tool call tree should snapshot");
        let assistant = snapshot
            .rows
            .iter()
            .find(|row| row.id == "assistant-1")
            .expect("assistant row should exist");

        assert_eq!(assistant.preview_replay_items.len(), 1);
        assert!(matches!(
            &assistant.preview_replay_items[0],
            TranscriptReplayItem::Message {
                role: runtime_domain::session::TranscriptReplayRole::Assistant,
                content,
            } if content.contains("I will inspect the file.")
                && content.contains("Tool call `read_file` (call-1)")
                && content.contains("```json")
                && content.contains("\"path\": \"Cargo.toml\"")
                && content.contains("\"limit\": 20")
        ));
    }

    #[test]
    fn session_tree_snapshot_projects_tool_activity_into_debug_preview_replay() {
        let activity = RuntimeToolActivity {
            activity_id: "call-1".to_string(),
            title: "Run cargo test".to_string(),
            kind: RuntimeToolKind::Execute,
            status: RuntimeToolActivityStatus::Completed,
            content: Vec::new(),
            locations: Vec::new(),
            raw_input: Some(r#"{"command":"cargo test"}"#.into()),
            raw_output: Some(
                (1..=8)
                    .map(|line| format!("test output line {line}"))
                    .collect::<Vec<_>>()
                    .join("\n")
                    .into(),
            ),
        };
        let entries = vec![
            SessionEntry {
                id: "assistant-1".to_string(),
                parent_id: None,
                timestamp: 1,
                kind: SessionEntryKind::Item(ConversationItem::assistant_with_tool_calls(
                    "running tests".to_string(),
                    vec![ToolCall::new(
                        "call-1",
                        "bash",
                        r#"{"command":"cargo test"}"#,
                    )],
                )),
            },
            SessionEntry {
                id: "tool-1".to_string(),
                parent_id: Some("assistant-1".to_string()),
                timestamp: 2,
                kind: SessionEntryKind::Item(ConversationItem::tool_result(
                    "call-1",
                    vec![ContentBlock::Text("compact result".to_string())],
                    false,
                )),
            },
            SessionEntry {
                id: "tool-replay".to_string(),
                parent_id: Some("tool-1".to_string()),
                timestamp: 3,
                kind: SessionEntryKind::TranscriptReplay(TranscriptReplayItem::ToolActivity {
                    activity: activity.clone(),
                }),
            },
        ];

        let snapshot =
            super::session_tree_snapshot(&entries).expect("tool activity tree should snapshot");
        let tool = snapshot
            .rows
            .iter()
            .find(|row| row.id == "tool-1")
            .expect("tool row should exist");

        assert_eq!(
            tool.preview_replay_items,
            vec![TranscriptReplayItem::ToolActivity { activity }]
        );
    }

    #[test]
    fn session_tree_snapshot_keeps_linear_logical_rows_flat() {
        let entries = linear_history_entries();

        let snapshot = super::session_tree_snapshot(&entries).expect("linear tree should snapshot");

        assert_eq!(
            snapshot
                .rows
                .iter()
                .map(|row| (row.id.as_str(), row.display_depth))
                .collect::<Vec<_>>(),
            vec![("user-1", 0), ("assistant-1", 0), ("user-2", 0)],
            "linear visible history should not inherit physical parent-chain depth as visual indent"
        );
    }

    #[test]
    fn session_tree_snapshot_lists_only_current_leaf_path_rows() {
        let entries = branching_with_append_entries();

        let snapshot =
            super::session_tree_snapshot(&entries).expect("branch path tree should snapshot");

        assert_eq!(
            snapshot
                .rows
                .iter()
                .map(|row| row.id.as_str())
                .collect::<Vec<_>>(),
            vec!["user-a", "assistant-c", "user-d"],
            "path tree must not include messages exclusive to sibling branches"
        );
        assert_eq!(snapshot.current_row_id.as_deref(), Some("user-d"));
    }

    #[test]
    fn session_tree_snapshot_lists_branch_choices_at_fork_parent() {
        let entries = branching_with_append_entries();

        let snapshot =
            super::session_tree_snapshot(&entries).expect("branch choices should snapshot");
        let branch_parent = snapshot
            .rows
            .iter()
            .find(|row| row.id == "user-a")
            .expect("fork parent should be visible on the active path");

        assert_eq!(
            branch_parent
                .branch_choices
                .iter()
                .map(|branch| {
                    (
                        branch.branch.branch_row_id.as_str(),
                        branch.branch.subtree_leaf_id.as_str(),
                        branch.branch.latest_row_id.as_str(),
                        branch.branch.display_summary.as_str(),
                        branch.branch.kind,
                        branch.branch.is_current,
                        branch.branch.message_count,
                        branch.branch.branch_created_at_ms,
                        branch.branch.latest_updated_at_ms,
                    )
                })
                .collect::<Vec<_>>(),
            vec![
                (
                    "assistant-b",
                    "assistant-b",
                    "assistant-b",
                    "branch-b",
                    super::SessionTreeSnapshotRowKind::Assistant,
                    false,
                    1,
                    1_717_514_800_002,
                    1_717_514_800_002,
                ),
                (
                    "assistant-c",
                    "user-d",
                    "user-d",
                    "branch follow-up",
                    super::SessionTreeSnapshotRowKind::User,
                    true,
                    2,
                    1_717_514_800_003,
                    1_717_514_800_004,
                ),
            ],
            "branch picker choices should summarize each sibling branch by its subtree leaf"
        );
    }

    #[test]
    fn session_branch_preview_snapshot_starts_at_fork_parent() {
        let entries = branching_after_visible_context_entries();

        let preview = super::session_branch_preview_snapshot(&entries, "assistant-b")
            .expect("branch preview should snapshot");

        assert_eq!(
            preview
                .rows
                .iter()
                .map(|row| (row.id.as_str(), row.display_depth))
                .collect::<Vec<_>>(),
            vec![("user-a", 0), ("assistant-b", 1)],
            "branch preview should skip visible ancestors before the fork point"
        );
        assert_eq!(preview.current_row_id.as_deref(), Some("assistant-b"));
    }

    #[test]
    fn session_tree_snapshot_for_hypothetical_leaf_matches_after_switch() {
        let entries = branching_with_append_entries();

        let preview = super::session_tree_snapshot_for_leaf(&entries, "assistant-b")
            .expect("hypothetical branch should snapshot");
        let mut switched_entries = entries.clone();
        switched_entries.push(SessionEntry {
            id: "leaf-switch".to_string(),
            parent_id: Some("user-d".to_string()),
            timestamp: 1_717_514_800_005,
            kind: SessionEntryKind::Leaf {
                target_id: Some("assistant-b".to_string()),
            },
        });
        let switched = super::session_tree_snapshot(&switched_entries)
            .expect("switched branch should snapshot");

        assert_eq!(
            preview
                .rows
                .iter()
                .map(|row| row.id.as_str())
                .collect::<Vec<_>>(),
            switched
                .rows
                .iter()
                .map(|row| row.id.as_str())
                .collect::<Vec<_>>(),
            "preview path must match the committed path after switching to that branch"
        );
        assert_eq!(preview.current_row_id, switched.current_row_id);
    }

    #[test]
    fn session_branch_tree_snapshot_lists_branch_roots_with_tree_parents() {
        let entries = nested_branch_tree_entries();

        let snapshot = super::session_branch_tree_snapshot(&entries)
            .expect("branch tree should snapshot nested branches");

        assert_eq!(snapshot.nodes.len(), 5);
        assert_eq!(snapshot.total_message_count, 7);
        assert_eq!(
            snapshot.current_branch_row_id.as_deref(),
            Some("user-b-alt")
        );

        let root = branch_tree_node(&snapshot, "user-root");
        assert_eq!(root.parent_branch_row_id, None);
        assert_eq!(root.branch.message_count, 7);

        let alpha = branch_tree_node(&snapshot, "assistant-a");
        assert_eq!(alpha.parent_branch_row_id.as_deref(), Some("user-root"));
        assert_eq!(alpha.branch.subtree_leaf_id, "assistant-a");
        assert_eq!(alpha.branch.message_count, 1);

        let beta = branch_tree_node(&snapshot, "assistant-b");
        assert_eq!(beta.parent_branch_row_id.as_deref(), Some("user-root"));
        assert_eq!(beta.branch.message_count, 5);

        let follow = branch_tree_node(&snapshot, "user-b-follow");
        assert_eq!(follow.parent_branch_row_id.as_deref(), Some("assistant-b"));
        assert_eq!(follow.branch.message_count, 2);
        assert!(!follow.branch.is_current);

        let alternate = branch_tree_node(&snapshot, "user-b-alt");
        assert_eq!(
            alternate.parent_branch_row_id.as_deref(),
            Some("assistant-b")
        );
        assert_eq!(alternate.branch.message_count, 2);
        assert!(alternate.branch.is_current);
        assert_eq!(alternate.branch.display_summary, "alt answer");
    }

    #[test]
    fn session_tree_snapshot_indents_true_sibling_branches() {
        let entries = branching_entries();

        let snapshot =
            super::session_tree_snapshot(&entries).expect("branching tree should snapshot");

        assert_eq!(
            snapshot
                .rows
                .iter()
                .map(|row| (row.id.as_str(), row.display_depth))
                .collect::<Vec<_>>(),
            vec![("user-a", 0), ("assistant-c", 1)],
            "path tree should keep current branch indent while omitting sibling-only rows"
        );
    }

    #[test]
    fn session_tree_snapshot_indents_rewinded_user_branch_under_outer_assistant() {
        let entries = nested_rewind_user_branch_entries();
        let snapshot =
            super::session_tree_snapshot(&entries).expect("nested rewind tree should snapshot");

        assert_eq!(
            snapshot
                .rows
                .iter()
                .map(|row| (row.id.as_str(), row.display_depth))
                .collect::<Vec<_>>(),
            vec![
                ("user-root", 0),
                ("reason-root", 0),
                ("assistant-root", 0),
                ("user-a", 1),
                ("reason-a", 1),
                ("assistant-a", 1),
                ("user-c", 2),
                ("reason-c", 2),
                ("assistant-c", 2),
            ],
            "path tree should preserve nested branch depth without listing inactive sibling paths"
        );
    }

    #[test]
    fn session_tree_snapshot_indents_each_rewind_branch_progressively_through_config_chain() {
        let entries = nested_config_rewind_chain_entries();
        let snapshot = super::session_tree_snapshot(&entries)
            .expect("nested config rewind tree should snapshot");

        assert_eq!(
            snapshot
                .rows
                .iter()
                .map(|row| (row.id.as_str(), row.display_depth))
                .collect::<Vec<_>>(),
            vec![
                ("user-root", 0),
                ("reason-root", 0),
                ("assistant-root", 0),
                ("user-branch-4", 3),
                ("reason-branch-4", 3),
                ("assistant-branch-4", 3),
            ],
            "path tree should keep the active rewind branch's full computed depth"
        );
    }

    #[test]
    fn session_tree_snapshot_keeps_linear_follow_up_after_branch_at_branch_depth() {
        let entries = branching_with_append_entries();
        let snapshot =
            super::session_tree_snapshot(&entries).expect("branch append tree should snapshot");

        assert_eq!(
            snapshot
                .rows
                .iter()
                .map(|row| (row.id.as_str(), row.display_depth))
                .collect::<Vec<_>>(),
            vec![("user-a", 0), ("assistant-c", 1), ("user-d", 1)],
            "linear follow-up after a selected branch should stay at the branch depth"
        );
    }

    #[test]
    fn resolve_follows_requested_leaf_entry_target() {
        let entries = entries_with_trailing_leaf_override();

        let resolved = super::resolve(&entries, "leaf-1")
            .expect("leaf entry should redirect canonical history");

        assert_eq!(resolved, vec![ConversationItem::text(Role::User, "hello")]);
    }

    #[test]
    fn resolve_keeps_explicit_non_leaf_selection_when_a_trailing_leaf_exists() {
        let entries = entries_with_trailing_leaf_override();

        let resolved = super::resolve(&entries, "assistant-c")
            .expect("leaf override should redirect canonical history");

        assert_eq!(
            resolved,
            vec![
                ConversationItem::text(Role::User, "hello"),
                ConversationItem::text(Role::Assistant, "branch-c"),
            ]
        );
    }

    #[test]
    fn resolve_replaces_compacted_history_with_summary_and_kept_tail() {
        let entries = entries_with_compaction();

        let resolved = super::resolve(&entries, "assistant-d")
            .expect("compacted history should resolve to summary plus kept tail");

        assert_eq!(
            resolved,
            vec![
                ConversationItem::system(vec![
                    ContentBlock::Text("compacted summary".to_string(),)
                ]),
                ConversationItem::text(Role::Assistant, "keep me"),
                ConversationItem::text(Role::Assistant, "after compaction"),
            ]
        );
    }

    #[test]
    fn resolve_uses_latest_compaction_boundary() {
        let entries = entries_with_multiple_compactions();

        let resolved =
            super::resolve(&entries, "assistant-f").expect("latest compaction should win");

        assert_eq!(
            resolved,
            vec![
                ConversationItem::system(vec![ContentBlock::Text("latest summary".to_string(),)]),
                ConversationItem::text(Role::Assistant, "second keep"),
                ConversationItem::text(Role::Assistant, "after latest compaction"),
            ]
        );
    }

    #[test]
    fn resolve_uses_latest_non_leaf_entry_when_requested_leaf_resets_target() {
        let entries = entries_with_trailing_leaf_reset();

        let resolved = super::resolve(&entries, "leaf-reset")
            .expect("leaf reset should fall back to the latest concrete entry");

        assert_eq!(
            resolved,
            vec![
                ConversationItem::text(Role::User, "hello"),
                ConversationItem::text(Role::Assistant, "branch-c"),
            ]
        );
    }

    #[test]
    fn resolve_returns_empty_history_for_header_only_session() {
        let entries = header_only_entries();

        let resolved = super::resolve(&entries, "header").expect("header-only session resolves");

        assert!(resolved.is_empty());
    }

    #[test]
    fn resolve_skips_config_and_branch_summary_entries() {
        let entries = entries_with_non_history_metadata();

        let resolved = super::resolve(&entries, "assistant-c")
            .expect("non-history metadata should not appear in canonical history");

        assert_eq!(
            resolved,
            vec![
                ConversationItem::text(Role::User, "hello"),
                ConversationItem::text(Role::Assistant, "final reply"),
            ]
        );
    }

    #[test]
    fn resolve_reports_missing_parent_on_selected_path() {
        let entries = entries_with_dangling_parent();

        let error = super::resolve(&entries, "assistant-1")
            .expect_err("dangling parent should fail resolve");

        assert_eq!(
            error,
            super::ResolveError::DanglingParent("missing".to_string())
        );
    }

    #[test]
    fn resolve_reports_cycle_on_selected_path() {
        let entries = entries_with_cycle();

        let error = super::resolve(&entries, "assistant-b").expect_err("cycle should fail resolve");

        assert_eq!(error, super::ResolveError::CycleDetected);
    }

    #[test]
    fn resolve_reports_missing_leaf_target_from_requested_leaf_entry() {
        let entries = entries_with_missing_leaf_target();

        let error = super::resolve(&entries, "leaf-missing")
            .expect_err("missing leaf target should fail resolve");

        assert_eq!(
            error,
            super::ResolveError::LeafNotFound("missing-target".to_string())
        );
    }

    #[test]
    fn resolve_reports_invalid_compaction_target() {
        let entries = entries_with_invalid_compaction_target();

        let error = super::resolve(&entries, "assistant-d")
            .expect_err("unknown compaction target should fail resolve");

        assert_eq!(
            error,
            super::ResolveError::InvalidCompactionTarget("missing-target".to_string())
        );
    }

    #[test]
    fn resolve_reports_duplicate_entry_id() {
        let entries = entries_with_duplicate_id();

        let error =
            super::resolve(&entries, "assistant-1").expect_err("duplicate id should fail resolve");

        assert_eq!(
            error,
            super::ResolveError::DuplicateId("assistant-1".to_string())
        );
    }

    #[test]
    fn resolve_rejects_compaction_target_that_is_not_an_item() {
        let entries = entries_with_non_item_compaction_target();

        let error = super::resolve(&entries, "assistant-d")
            .expect_err("non-item compaction target should fail resolve");

        assert_eq!(
            error,
            super::ResolveError::InvalidCompactionTarget("config-1".to_string())
        );
    }

    #[test]
    fn resolve_handles_large_linear_history() {
        let entries = long_linear_history_entries(1_000);

        let resolved = super::resolve(&entries, "assistant-999")
            .expect("large linear history should resolve successfully");

        assert_eq!(resolved.len(), 1_000);
        assert_eq!(
            resolved.first(),
            Some(&ConversationItem::text(Role::Assistant, "message-0"))
        );
        assert_eq!(
            resolved.last(),
            Some(&ConversationItem::text(Role::Assistant, "message-999"))
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

    fn test_temp_dir(label: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("hunea-session-store-{label}-{}", Uuid::now_v7()));
        fs::create_dir_all(&dir).expect("test temp dir should be creatable");
        dir
    }

    fn snapshot_row<'a>(
        snapshot: &'a super::SessionTreeSnapshot,
        row_id: &str,
    ) -> &'a super::SessionTreeSnapshotRow {
        snapshot
            .rows
            .iter()
            .find(|row| row.id == row_id)
            .unwrap_or_else(|| panic!("{row_id} should exist in tree snapshot"))
    }

    fn branch_tree_node<'a>(
        snapshot: &'a super::SessionBranchTreeSnapshot,
        branch_row_id: &str,
    ) -> &'a super::SessionBranchTreeSnapshotNode {
        snapshot
            .nodes
            .iter()
            .find(|node| node.branch.branch_row_id == branch_row_id)
            .unwrap_or_else(|| panic!("{branch_row_id} should exist in branch tree snapshot"))
    }

    fn assistant_tool_batch_entries(
        call_ids: &[&str],
        result_call_ids: &[&str],
        include_replay_before_results: bool,
    ) -> Vec<SessionEntry> {
        let mut entries = vec![
            SessionEntry {
                id: "user-1".to_string(),
                parent_id: None,
                timestamp: 1,
                kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "read files")),
            },
            SessionEntry {
                id: "assistant-1".to_string(),
                parent_id: Some("user-1".to_string()),
                timestamp: 2,
                kind: SessionEntryKind::Item(ConversationItem::assistant_with_tool_calls(
                    "reading".to_string(),
                    call_ids
                        .iter()
                        .map(|call_id| ToolCall::new(*call_id, "read", "{}"))
                        .collect(),
                )),
            },
        ];

        let mut parent_id = "assistant-1".to_string();
        let mut timestamp = 3;
        if include_replay_before_results {
            for call_id in call_ids {
                let replay_id = format!("replay-{call_id}");
                entries.push(SessionEntry {
                    id: replay_id.clone(),
                    parent_id: Some(parent_id),
                    timestamp,
                    kind: SessionEntryKind::TranscriptReplay(TranscriptReplayItem::ToolActivity {
                        activity: sample_tool_activity(call_id, "completed"),
                    }),
                });
                parent_id = replay_id;
                timestamp += 1;
            }
        }

        for (index, call_id) in result_call_ids.iter().enumerate() {
            let tool_id = format!("tool-{}", index + 1);
            entries.push(SessionEntry {
                id: tool_id.clone(),
                parent_id: Some(parent_id),
                timestamp,
                kind: SessionEntryKind::Item(ConversationItem::tool_result(
                    *call_id,
                    vec![ContentBlock::Text(format!("result {}", index + 1))],
                    false,
                )),
            });
            parent_id = tool_id;
            timestamp += 1;
        }

        entries
    }

    fn nested_branch_tree_entries() -> Vec<SessionEntry> {
        vec![
            SessionEntry {
                id: "header".to_string(),
                parent_id: None,
                timestamp: 1_717_514_800_000,
                kind: SessionEntryKind::Header(SessionHeader {
                    session_id: "01914a5c-3c7e-7a2b-8abc-1234567890ab"
                        .parse()
                        .expect("fixture session id should parse"),
                    work_dir: PathBuf::from("/repo"),
                    session_name: None,
                    initial_model: "qwen3".to_string(),
                    git_head: None,
                    cli_version: None,
                }),
            },
            SessionEntry {
                id: "user-root".to_string(),
                parent_id: Some("header".to_string()),
                timestamp: 1_717_514_800_001,
                kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "root question")),
            },
            SessionEntry {
                id: "assistant-a".to_string(),
                parent_id: Some("user-root".to_string()),
                timestamp: 1_717_514_800_002,
                kind: SessionEntryKind::Item(ConversationItem::text(Role::Assistant, "alpha")),
            },
            SessionEntry {
                id: "assistant-b".to_string(),
                parent_id: Some("user-root".to_string()),
                timestamp: 1_717_514_800_003,
                kind: SessionEntryKind::Item(ConversationItem::text(Role::Assistant, "beta")),
            },
            SessionEntry {
                id: "user-b-follow".to_string(),
                parent_id: Some("assistant-b".to_string()),
                timestamp: 1_717_514_800_004,
                kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "follow")),
            },
            SessionEntry {
                id: "assistant-b-follow".to_string(),
                parent_id: Some("user-b-follow".to_string()),
                timestamp: 1_717_514_800_005,
                kind: SessionEntryKind::Item(ConversationItem::text(
                    Role::Assistant,
                    "follow answer",
                )),
            },
            SessionEntry {
                id: "user-b-alt".to_string(),
                parent_id: Some("assistant-b".to_string()),
                timestamp: 1_717_514_800_006,
                kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "alt")),
            },
            SessionEntry {
                id: "assistant-b-alt".to_string(),
                parent_id: Some("user-b-alt".to_string()),
                timestamp: 1_717_514_800_007,
                kind: SessionEntryKind::Item(ConversationItem::text(Role::Assistant, "alt answer")),
            },
        ]
    }

    fn sample_entries() -> Vec<SessionEntry> {
        let session_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ab"
            .parse()
            .expect("fixture session id should parse");

        vec![
            SessionEntry {
                id: "header".to_string(),
                parent_id: None,
                timestamp: 1_717_514_800_000,
                kind: SessionEntryKind::Header(SessionHeader {
                    session_id,
                    work_dir: PathBuf::from("/repo"),
                    session_name: None,
                    initial_model: "gpt-4.1".to_string(),
                    git_head: Some("abc123".to_string()),
                    cli_version: Some("0.5.2".to_string()),
                }),
            },
            SessionEntry {
                id: "user-1".to_string(),
                parent_id: Some("header".to_string()),
                timestamp: 1_717_514_800_001,
                kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "hello")),
            },
            SessionEntry {
                id: "assistant-1".to_string(),
                parent_id: Some("user-1".to_string()),
                timestamp: 1_717_514_800_002,
                kind: SessionEntryKind::Item(ConversationItem::assistant_with_tool_calls(
                    "working".to_string(),
                    vec![ToolCall::new(
                        "call-1",
                        "read_file",
                        r#"{"path":"Cargo.toml"}"#,
                    )],
                )),
            },
            SessionEntry {
                id: "tool-1".to_string(),
                parent_id: Some("assistant-1".to_string()),
                timestamp: 1_717_514_800_003,
                kind: SessionEntryKind::Item(ConversationItem::tool_result(
                    "call-1",
                    vec![ContentBlock::Text("done".to_string())],
                    false,
                )),
            },
            SessionEntry {
                id: "reasoning-1".to_string(),
                parent_id: Some("tool-1".to_string()),
                timestamp: 1_717_514_800_004,
                kind: SessionEntryKind::Item(ConversationItem::Reasoning {
                    content: "internal chain".to_string(),
                    summary: Some("brief".to_string()),
                    encrypted: Some("cipher".to_string()),
                }),
            },
            SessionEntry {
                id: "compaction-1".to_string(),
                parent_id: Some("reasoning-1".to_string()),
                timestamp: 1_717_514_800_005,
                kind: SessionEntryKind::Compaction {
                    summary: "summary".to_string(),
                    first_kept_entry_id: "tool-1".to_string(),
                    tokens_before: 128,
                },
            },
            SessionEntry {
                id: "branch-1".to_string(),
                parent_id: Some("assistant-1".to_string()),
                timestamp: 1_717_514_800_006,
                kind: SessionEntryKind::BranchSummary {
                    from_id: "assistant-1".to_string(),
                    summary: "alternate".to_string(),
                },
            },
            SessionEntry {
                id: "config-1".to_string(),
                parent_id: Some("branch-1".to_string()),
                timestamp: 1_717_514_800_007,
                kind: SessionEntryKind::ConfigChange(ConfigSnapshot {
                    provider_id: "local".to_string(),
                    model: "gpt-4.1-mini".to_string(),
                    system_prompt: Some("be terse".to_string()),
                }),
            },
            SessionEntry {
                id: "leaf-1".to_string(),
                parent_id: Some("config-1".to_string()),
                timestamp: 1_717_514_800_008,
                kind: SessionEntryKind::Leaf {
                    target_id: Some("tool-1".to_string()),
                },
            },
        ]
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

    fn linear_history_entries() -> Vec<SessionEntry> {
        let session_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ab"
            .parse()
            .expect("fixture session id should parse");

        vec![
            SessionEntry {
                id: "header".to_string(),
                parent_id: None,
                timestamp: 1_717_514_800_000,
                kind: SessionEntryKind::Header(SessionHeader {
                    session_id,
                    work_dir: PathBuf::from("/repo"),
                    session_name: None,
                    initial_model: "gpt-4.1".to_string(),
                    git_head: Some("abc123".to_string()),
                    cli_version: Some("0.5.2".to_string()),
                }),
            },
            SessionEntry {
                id: "user-1".to_string(),
                parent_id: Some("header".to_string()),
                timestamp: 1_717_514_800_001,
                kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "hello")),
            },
            SessionEntry {
                id: "assistant-1".to_string(),
                parent_id: Some("user-1".to_string()),
                timestamp: 1_717_514_800_002,
                kind: SessionEntryKind::Item(ConversationItem::text(Role::Assistant, "hi")),
            },
            SessionEntry {
                id: "user-2".to_string(),
                parent_id: Some("assistant-1".to_string()),
                timestamp: 1_717_514_800_003,
                kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "follow up")),
            },
        ]
    }

    fn branching_entries() -> Vec<SessionEntry> {
        let session_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ab"
            .parse()
            .expect("fixture session id should parse");

        vec![
            SessionEntry {
                id: "header".to_string(),
                parent_id: None,
                timestamp: 1_717_514_800_000,
                kind: SessionEntryKind::Header(SessionHeader {
                    session_id,
                    work_dir: PathBuf::from("/repo"),
                    session_name: None,
                    initial_model: "gpt-4.1".to_string(),
                    git_head: Some("abc123".to_string()),
                    cli_version: Some("0.5.2".to_string()),
                }),
            },
            SessionEntry {
                id: "user-a".to_string(),
                parent_id: Some("header".to_string()),
                timestamp: 1_717_514_800_001,
                kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "hello")),
            },
            SessionEntry {
                id: "assistant-b".to_string(),
                parent_id: Some("user-a".to_string()),
                timestamp: 1_717_514_800_002,
                kind: SessionEntryKind::Item(ConversationItem::text(Role::Assistant, "branch-b")),
            },
            SessionEntry {
                id: "assistant-c".to_string(),
                parent_id: Some("user-a".to_string()),
                timestamp: 1_717_514_800_003,
                kind: SessionEntryKind::Item(ConversationItem::text(Role::Assistant, "branch-c")),
            },
        ]
    }

    fn branching_with_append_entries() -> Vec<SessionEntry> {
        let mut entries = branching_entries();
        entries.push(SessionEntry {
            id: "user-d".to_string(),
            parent_id: Some("assistant-c".to_string()),
            timestamp: 1_717_514_800_004,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "branch follow-up")),
        });
        entries
    }

    fn branching_after_visible_context_entries() -> Vec<SessionEntry> {
        let session_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ab"
            .parse()
            .expect("fixture session id should parse");

        vec![
            SessionEntry {
                id: "header".to_string(),
                parent_id: None,
                timestamp: 1_717_514_800_000,
                kind: SessionEntryKind::Header(SessionHeader {
                    session_id,
                    work_dir: PathBuf::from("/repo"),
                    session_name: None,
                    initial_model: "gpt-4.1".to_string(),
                    git_head: Some("abc123".to_string()),
                    cli_version: Some("0.5.2".to_string()),
                }),
            },
            SessionEntry {
                id: "user-context".to_string(),
                parent_id: Some("header".to_string()),
                timestamp: 1_717_514_800_001,
                kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "context")),
            },
            SessionEntry {
                id: "assistant-context".to_string(),
                parent_id: Some("user-context".to_string()),
                timestamp: 1_717_514_800_002,
                kind: SessionEntryKind::Item(ConversationItem::text(Role::Assistant, "not shown")),
            },
            SessionEntry {
                id: "user-a".to_string(),
                parent_id: Some("assistant-context".to_string()),
                timestamp: 1_717_514_800_003,
                kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "hello")),
            },
            SessionEntry {
                id: "assistant-b".to_string(),
                parent_id: Some("user-a".to_string()),
                timestamp: 1_717_514_800_004,
                kind: SessionEntryKind::Item(ConversationItem::text(Role::Assistant, "branch-b")),
            },
            SessionEntry {
                id: "assistant-c".to_string(),
                parent_id: Some("user-a".to_string()),
                timestamp: 1_717_514_800_005,
                kind: SessionEntryKind::Item(ConversationItem::text(Role::Assistant, "branch-c")),
            },
            SessionEntry {
                id: "user-d".to_string(),
                parent_id: Some("assistant-c".to_string()),
                timestamp: 1_717_514_800_006,
                kind: SessionEntryKind::Item(ConversationItem::text(
                    Role::User,
                    "branch follow-up",
                )),
            },
        ]
    }

    fn nested_rewind_user_branch_entries() -> Vec<SessionEntry> {
        let session_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ab"
            .parse()
            .expect("fixture session id should parse");

        vec![
            SessionEntry {
                id: "header".to_string(),
                parent_id: None,
                timestamp: 1_717_514_800_000,
                kind: SessionEntryKind::Header(SessionHeader {
                    session_id,
                    work_dir: PathBuf::from("/repo"),
                    session_name: None,
                    initial_model: "gpt-4.1".to_string(),
                    git_head: Some("abc123".to_string()),
                    cli_version: Some("0.5.2".to_string()),
                }),
            },
            SessionEntry {
                id: "user-root".to_string(),
                parent_id: Some("header".to_string()),
                timestamp: 1_717_514_800_001,
                kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "你好哦")),
            },
            SessionEntry {
                id: "reason-root".to_string(),
                parent_id: Some("user-root".to_string()),
                timestamp: 1_717_514_800_002,
                kind: SessionEntryKind::Item(ConversationItem::Reasoning {
                    content: "think root".to_string(),
                    summary: None,
                    encrypted: None,
                }),
            },
            SessionEntry {
                id: "assistant-root".to_string(),
                parent_id: Some("reason-root".to_string()),
                timestamp: 1_717_514_800_003,
                kind: SessionEntryKind::Item(ConversationItem::text(Role::Assistant, "root reply")),
            },
            SessionEntry {
                id: "user-inactive".to_string(),
                parent_id: Some("assistant-root".to_string()),
                timestamp: 1_717_514_800_004,
                kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "skipped branch")),
            },
            SessionEntry {
                id: "user-a".to_string(),
                parent_id: Some("assistant-root".to_string()),
                timestamp: 1_717_514_800_005,
                kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "你是谁")),
            },
            SessionEntry {
                id: "reason-a".to_string(),
                parent_id: Some("user-a".to_string()),
                timestamp: 1_717_514_800_005,
                kind: SessionEntryKind::Item(ConversationItem::Reasoning {
                    content: "think a".to_string(),
                    summary: None,
                    encrypted: None,
                }),
            },
            SessionEntry {
                id: "assistant-a".to_string(),
                parent_id: Some("reason-a".to_string()),
                timestamp: 1_717_514_800_006,
                kind: SessionEntryKind::Item(ConversationItem::text(Role::Assistant, "你好！")),
            },
            SessionEntry {
                id: "user-b".to_string(),
                parent_id: Some("assistant-a".to_string()),
                timestamp: 1_717_514_800_007,
                kind: SessionEntryKind::Item(ConversationItem::text(
                    Role::User,
                    "linear follow up",
                )),
            },
            SessionEntry {
                id: "reason-b".to_string(),
                parent_id: Some("user-b".to_string()),
                timestamp: 1_717_514_800_008,
                kind: SessionEntryKind::Item(ConversationItem::Reasoning {
                    content: "think b".to_string(),
                    summary: None,
                    encrypted: None,
                }),
            },
            SessionEntry {
                id: "assistant-b".to_string(),
                parent_id: Some("reason-b".to_string()),
                timestamp: 1_717_514_800_009,
                kind: SessionEntryKind::Item(ConversationItem::text(
                    Role::Assistant,
                    "linear reply",
                )),
            },
            SessionEntry {
                id: "leaf-1".to_string(),
                parent_id: Some("assistant-b".to_string()),
                timestamp: 1_717_514_800_010,
                kind: SessionEntryKind::Leaf {
                    target_id: Some("assistant-a".to_string()),
                },
            },
            SessionEntry {
                id: "user-c".to_string(),
                parent_id: Some("assistant-a".to_string()),
                timestamp: 1_717_514_800_011,
                kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "你能做什么")),
            },
            SessionEntry {
                id: "reason-c".to_string(),
                parent_id: Some("user-c".to_string()),
                timestamp: 1_717_514_800_012,
                kind: SessionEntryKind::Item(ConversationItem::Reasoning {
                    content: "think c".to_string(),
                    summary: None,
                    encrypted: None,
                }),
            },
            SessionEntry {
                id: "assistant-c".to_string(),
                parent_id: Some("reason-c".to_string()),
                timestamp: 1_717_514_800_013,
                kind: SessionEntryKind::Item(ConversationItem::text(
                    Role::Assistant,
                    "nested tail",
                )),
            },
        ]
    }

    fn nested_config_rewind_chain_entries() -> Vec<SessionEntry> {
        let session_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ab"
            .parse()
            .expect("fixture session id should parse");

        let mut timestamp = 1_717_514_800_000i64;
        let mut next_timestamp = || {
            let value = timestamp;
            timestamp += 1;
            value
        };

        let config_snapshot = || ConfigSnapshot {
            provider_id: "opencode".to_string(),
            model: "gpt-4.1".to_string(),
            system_prompt: None,
        };

        // 复刻真实 session：rewind 时新建的 ConfigChange 总是挂到上一次 ConfigChange，
        // 形成隐藏的 fork 链——这是触发本次 bug 的关键拓扑。
        let mut entries = vec![
            SessionEntry {
                id: "header".to_string(),
                parent_id: None,
                timestamp: next_timestamp(),
                kind: SessionEntryKind::Header(SessionHeader {
                    session_id,
                    work_dir: PathBuf::from("/repo"),
                    session_name: None,
                    initial_model: "gpt-4.1".to_string(),
                    git_head: Some("abc123".to_string()),
                    cli_version: Some("0.6.0".to_string()),
                }),
            },
            SessionEntry {
                id: "config-root".to_string(),
                parent_id: Some("header".to_string()),
                timestamp: next_timestamp(),
                kind: SessionEntryKind::ConfigChange(config_snapshot()),
            },
        ];

        let push_user_assistant_chain = |entries: &mut Vec<SessionEntry>,
                                         ts: &mut dyn FnMut() -> i64,
                                         slug: &str,
                                         parent: &str|
         -> String {
            let user_id = format!("user-{slug}");
            entries.push(SessionEntry {
                id: user_id.clone(),
                parent_id: Some(parent.to_string()),
                timestamp: ts(),
                kind: SessionEntryKind::Item(ConversationItem::text(
                    Role::User,
                    format!("question {slug}"),
                )),
            });
            let reason_id = format!("reason-{slug}");
            entries.push(SessionEntry {
                id: reason_id.clone(),
                parent_id: Some(user_id),
                timestamp: ts(),
                kind: SessionEntryKind::Item(ConversationItem::Reasoning {
                    content: format!("think {slug}"),
                    summary: None,
                    encrypted: None,
                }),
            });
            let assistant_id = format!("assistant-{slug}");
            entries.push(SessionEntry {
                id: assistant_id.clone(),
                parent_id: Some(reason_id),
                timestamp: ts(),
                kind: SessionEntryKind::Item(ConversationItem::text(
                    Role::Assistant,
                    format!("answer {slug}"),
                )),
            });
            assistant_id
        };

        let root_assistant_id =
            push_user_assistant_chain(&mut entries, &mut next_timestamp, "root", "config-root");

        entries.push(SessionEntry {
            id: "tr-root".to_string(),
            parent_id: Some(root_assistant_id),
            timestamp: next_timestamp(),
            kind: SessionEntryKind::TranscriptReplay(TranscriptReplayItem::Message {
                role: runtime_domain::session::TranscriptReplayRole::Assistant,
                content: "answer root".to_string(),
            }),
        });

        let mut config_chain_parent = "tr-root".to_string();
        for branch_index in 1..=4 {
            let config_id = format!("config-{branch_index}");
            entries.push(SessionEntry {
                id: config_id.clone(),
                parent_id: Some(config_chain_parent.clone()),
                timestamp: next_timestamp(),
                kind: SessionEntryKind::ConfigChange(config_snapshot()),
            });
            push_user_assistant_chain(
                &mut entries,
                &mut next_timestamp,
                &format!("branch-{branch_index}"),
                &config_id,
            );
            config_chain_parent = config_id;
        }

        entries
    }

    fn entries_with_trailing_leaf_override() -> Vec<SessionEntry> {
        let mut entries = branching_entries();
        entries.push(SessionEntry {
            id: "leaf-1".to_string(),
            parent_id: Some("assistant-c".to_string()),
            timestamp: 1_717_514_800_004,
            kind: SessionEntryKind::Leaf {
                target_id: Some("user-a".to_string()),
            },
        });
        entries
    }

    fn entries_with_compaction() -> Vec<SessionEntry> {
        let session_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ab"
            .parse()
            .expect("fixture session id should parse");

        vec![
            SessionEntry {
                id: "header".to_string(),
                parent_id: None,
                timestamp: 1_717_514_800_000,
                kind: SessionEntryKind::Header(SessionHeader {
                    session_id,
                    work_dir: PathBuf::from("/repo"),
                    session_name: None,
                    initial_model: "gpt-4.1".to_string(),
                    git_head: Some("abc123".to_string()),
                    cli_version: Some("0.5.2".to_string()),
                }),
            },
            SessionEntry {
                id: "user-a".to_string(),
                parent_id: Some("header".to_string()),
                timestamp: 1_717_514_800_001,
                kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "drop me")),
            },
            SessionEntry {
                id: "assistant-b".to_string(),
                parent_id: Some("user-a".to_string()),
                timestamp: 1_717_514_800_002,
                kind: SessionEntryKind::Item(ConversationItem::text(
                    Role::Assistant,
                    "drop me too",
                )),
            },
            SessionEntry {
                id: "assistant-c".to_string(),
                parent_id: Some("assistant-b".to_string()),
                timestamp: 1_717_514_800_003,
                kind: SessionEntryKind::Item(ConversationItem::text(Role::Assistant, "keep me")),
            },
            SessionEntry {
                id: "compaction-1".to_string(),
                parent_id: Some("assistant-c".to_string()),
                timestamp: 1_717_514_800_004,
                kind: SessionEntryKind::Compaction {
                    summary: "compacted summary".to_string(),
                    first_kept_entry_id: "assistant-c".to_string(),
                    tokens_before: 64,
                },
            },
            SessionEntry {
                id: "assistant-d".to_string(),
                parent_id: Some("compaction-1".to_string()),
                timestamp: 1_717_514_800_005,
                kind: SessionEntryKind::Item(ConversationItem::text(
                    Role::Assistant,
                    "after compaction",
                )),
            },
        ]
    }

    fn entries_with_multiple_compactions() -> Vec<SessionEntry> {
        let mut entries = entries_with_compaction();
        entries.push(SessionEntry {
            id: "assistant-e".to_string(),
            parent_id: Some("assistant-d".to_string()),
            timestamp: 1_717_514_800_006,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::Assistant, "second keep")),
        });
        entries.push(SessionEntry {
            id: "compaction-2".to_string(),
            parent_id: Some("assistant-e".to_string()),
            timestamp: 1_717_514_800_007,
            kind: SessionEntryKind::Compaction {
                summary: "latest summary".to_string(),
                first_kept_entry_id: "assistant-e".to_string(),
                tokens_before: 96,
            },
        });
        entries.push(SessionEntry {
            id: "assistant-f".to_string(),
            parent_id: Some("compaction-2".to_string()),
            timestamp: 1_717_514_800_008,
            kind: SessionEntryKind::Item(ConversationItem::text(
                Role::Assistant,
                "after latest compaction",
            )),
        });
        entries
    }

    fn entries_with_trailing_leaf_reset() -> Vec<SessionEntry> {
        let mut entries = branching_entries();
        entries.push(SessionEntry {
            id: "leaf-reset".to_string(),
            parent_id: Some("assistant-c".to_string()),
            timestamp: 1_717_514_800_004,
            kind: SessionEntryKind::Leaf { target_id: None },
        });
        entries
    }

    fn header_only_entries() -> Vec<SessionEntry> {
        let session_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ab"
            .parse()
            .expect("fixture session id should parse");

        vec![SessionEntry {
            id: "header".to_string(),
            parent_id: None,
            timestamp: 1_717_514_800_000,
            kind: SessionEntryKind::Header(SessionHeader {
                session_id,
                work_dir: PathBuf::from("/repo"),
                session_name: None,
                initial_model: "gpt-4.1".to_string(),
                git_head: Some("abc123".to_string()),
                cli_version: Some("0.5.2".to_string()),
            }),
        }]
    }

    fn entries_with_non_history_metadata() -> Vec<SessionEntry> {
        let mut entries = branching_entries();
        entries.truncate(2);
        entries.push(SessionEntry {
            id: "branch-summary".to_string(),
            parent_id: Some("user-a".to_string()),
            timestamp: 1_717_514_800_002,
            kind: SessionEntryKind::BranchSummary {
                from_id: "user-a".to_string(),
                summary: "alternate".to_string(),
            },
        });
        entries.push(SessionEntry {
            id: "config-change".to_string(),
            parent_id: Some("branch-summary".to_string()),
            timestamp: 1_717_514_800_003,
            kind: SessionEntryKind::ConfigChange(ConfigSnapshot {
                provider_id: "local".to_string(),
                model: "gpt-4.1-mini".to_string(),
                system_prompt: Some("be terse".to_string()),
            }),
        });
        entries.push(SessionEntry {
            id: "assistant-c".to_string(),
            parent_id: Some("config-change".to_string()),
            timestamp: 1_717_514_800_004,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::Assistant, "final reply")),
        });
        entries
    }

    fn entries_with_dangling_parent() -> Vec<SessionEntry> {
        let mut entries = linear_history_entries();
        entries[2].parent_id = Some("missing".to_string());
        entries.truncate(3);
        entries
    }

    fn entries_with_cycle() -> Vec<SessionEntry> {
        let mut entries = branching_entries();
        entries[1].parent_id = Some("assistant-b".to_string());
        entries[2].parent_id = Some("user-a".to_string());
        entries.truncate(3);
        entries
    }

    fn entries_with_missing_leaf_target() -> Vec<SessionEntry> {
        let mut entries = branching_entries();
        entries.push(SessionEntry {
            id: "leaf-missing".to_string(),
            parent_id: Some("assistant-c".to_string()),
            timestamp: 1_717_514_800_004,
            kind: SessionEntryKind::Leaf {
                target_id: Some("missing-target".to_string()),
            },
        });
        entries
    }

    fn entries_with_invalid_compaction_target() -> Vec<SessionEntry> {
        let mut entries = entries_with_compaction();
        entries[4].kind = SessionEntryKind::Compaction {
            summary: "compacted summary".to_string(),
            first_kept_entry_id: "missing-target".to_string(),
            tokens_before: 64,
        };
        entries
    }

    fn entries_with_duplicate_id() -> Vec<SessionEntry> {
        let mut entries = linear_history_entries();
        entries.push(SessionEntry {
            id: "assistant-1".to_string(),
            parent_id: Some("user-2".to_string()),
            timestamp: 1_717_514_800_004,
            kind: SessionEntryKind::Item(ConversationItem::text(
                Role::Assistant,
                "shadowed duplicate",
            )),
        });
        entries
    }

    fn entries_with_non_item_compaction_target() -> Vec<SessionEntry> {
        let session_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ab"
            .parse()
            .expect("fixture session id should parse");

        vec![
            SessionEntry {
                id: "header".to_string(),
                parent_id: None,
                timestamp: 1_717_514_800_000,
                kind: SessionEntryKind::Header(SessionHeader {
                    session_id,
                    work_dir: PathBuf::from("/repo"),
                    session_name: None,
                    initial_model: "gpt-4.1".to_string(),
                    git_head: Some("abc123".to_string()),
                    cli_version: Some("0.5.2".to_string()),
                }),
            },
            SessionEntry {
                id: "user-a".to_string(),
                parent_id: Some("header".to_string()),
                timestamp: 1_717_514_800_001,
                kind: SessionEntryKind::Item(ConversationItem::text(Role::User, "hello")),
            },
            SessionEntry {
                id: "config-1".to_string(),
                parent_id: Some("user-a".to_string()),
                timestamp: 1_717_514_800_002,
                kind: SessionEntryKind::ConfigChange(ConfigSnapshot {
                    provider_id: "local".to_string(),
                    model: "gpt-4.1-mini".to_string(),
                    system_prompt: Some("be terse".to_string()),
                }),
            },
            SessionEntry {
                id: "compaction-1".to_string(),
                parent_id: Some("config-1".to_string()),
                timestamp: 1_717_514_800_003,
                kind: SessionEntryKind::Compaction {
                    summary: "summary".to_string(),
                    first_kept_entry_id: "config-1".to_string(),
                    tokens_before: 32,
                },
            },
            SessionEntry {
                id: "assistant-d".to_string(),
                parent_id: Some("compaction-1".to_string()),
                timestamp: 1_717_514_800_004,
                kind: SessionEntryKind::Item(ConversationItem::text(
                    Role::Assistant,
                    "after compaction",
                )),
            },
        ]
    }

    fn long_linear_history_entries(item_count: usize) -> Vec<SessionEntry> {
        let session_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ab"
            .parse()
            .expect("fixture session id should parse");
        let mut entries = Vec::with_capacity(item_count + 1);
        entries.push(SessionEntry {
            id: "header".to_string(),
            parent_id: None,
            timestamp: 1_717_514_800_000,
            kind: SessionEntryKind::Header(SessionHeader {
                session_id,
                work_dir: PathBuf::from("/repo"),
                session_name: None,
                initial_model: "gpt-4.1".to_string(),
                git_head: Some("abc123".to_string()),
                cli_version: Some("0.5.2".to_string()),
            }),
        });

        let mut parent_id = "header".to_string();
        for index in 0..item_count {
            let id = format!("assistant-{index}");
            entries.push(SessionEntry {
                id: id.clone(),
                parent_id: Some(parent_id),
                timestamp: 1_717_514_800_001 + index as i64,
                kind: SessionEntryKind::Item(ConversationItem::text(
                    Role::Assistant,
                    format!("message-{index}"),
                )),
            });
            parent_id = id;
        }

        entries
    }
}
