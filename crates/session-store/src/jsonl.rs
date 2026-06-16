use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
};

use tracing::warn;

use crate::{SessionEntry, SessionStoreError};

/// append-only JSONL writer。
pub(crate) struct JsonlWriter {
    path: PathBuf,
    file: Option<BufWriter<File>>,
}

impl JsonlWriter {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path, file: None }
    }

    #[cfg(test)]
    pub(crate) fn write(&mut self, entry: &SessionEntry) -> Result<(), SessionStoreError> {
        self.write_batch(std::slice::from_ref(entry))
    }

    pub(crate) fn write_batch(
        &mut self,
        entries: &[SessionEntry],
    ) -> Result<(), SessionStoreError> {
        if entries.is_empty() {
            return Ok(());
        }

        let file = self.file_mut()?;

        for entry in entries {
            let serialized = serde_json::to_string(entry).map_err(|source| {
                SessionStoreError::SerializeEntry {
                    entry_id: entry.id.clone(),
                    source,
                }
            })?;
            file.write_all(serialized.as_bytes()).map_err(io_error)?;
            file.write_all(b"\n").map_err(io_error)?;
        }

        file.flush().map_err(io_error)?;
        file.get_ref().sync_all().map_err(io_error)
    }

    #[cfg(test)]
    pub(crate) fn file_exists(&self) -> bool {
        self.path.exists()
    }

    fn file_mut(&mut self) -> Result<&mut BufWriter<File>, SessionStoreError> {
        if self.file.is_none() {
            if let Some(parent_dir) = self.path.parent() {
                fs::create_dir_all(parent_dir).map_err(io_error)?;
            }

            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
                .map_err(io_error)?;
            self.file = Some(BufWriter::new(file));
        }

        self.file
            .as_mut()
            .ok_or_else(|| SessionStoreError::CorruptIndex {
                message: "session JSONL writer did not initialize its file".to_string(),
            })
    }
}

/// JSONL loader。
pub(crate) struct JsonlLoader;

impl JsonlLoader {
    pub(crate) fn load(path: &Path) -> Result<Vec<SessionEntry>, SessionStoreError> {
        let mut loaded_entries = Vec::new();
        let seen_ids = scan_entries(path, |entry| {
            loaded_entries.push(entry);
            Ok(())
        })?;

        validate_parent_links(&loaded_entries, &seen_ids)?;

        Ok(loaded_entries)
    }

    pub(crate) fn scan(
        path: &Path,
        mut visit: impl FnMut(SessionEntry) -> Result<(), SessionStoreError>,
    ) -> Result<(), SessionStoreError> {
        scan_entries(path, &mut visit).map(|_| ())
    }
}

fn scan_entries(
    path: &Path,
    mut on_entry: impl FnMut(SessionEntry) -> Result<(), SessionStoreError>,
) -> Result<HashSet<String>, SessionStoreError> {
    let file = File::open(path).map_err(io_error)?;
    let mut reader = BufReader::new(file);
    let mut seen_ids = HashSet::new();
    let mut line_number = 1;
    let mut line_bytes = Vec::new();

    loop {
        line_bytes.clear();
        let bytes_read = reader
            .read_until(b'\n', &mut line_bytes)
            .map_err(io_error)?;
        if bytes_read == 0 {
            break;
        }

        let has_newline = line_bytes.last() == Some(&b'\n');
        if has_newline {
            line_bytes.pop();
            if line_bytes.last() == Some(&b'\r') {
                line_bytes.pop();
            }
        }

        if line_bytes.is_empty() {
            line_number += 1;
            continue;
        }

        let line = match std::str::from_utf8(&line_bytes) {
            Ok(line) => line,
            Err(_) if !has_newline => break,
            Err(error) => {
                warn!(
                    line = line_number,
                    error = %error,
                    "skipping session entry line with invalid UTF-8"
                );
                line_number += 1;
                continue;
            }
        };

        match serde_json::from_str::<SessionEntry>(line) {
            Ok(entry) => {
                if seen_ids.insert(entry.id.clone()) {
                    on_entry(entry)?;
                } else {
                    warn!(
                        id = %entry.id,
                        line = line_number,
                        "duplicate session entry id detected; keeping first occurrence"
                    );
                }
            }
            Err(_) if !has_newline => break,
            Err(error) => {
                warn!(
                    line = line_number,
                    error = %error,
                    "skipping corrupted session entry line"
                );
            }
        }

        line_number += 1;
    }

    Ok(seen_ids)
}

fn validate_parent_links(
    entries: &[SessionEntry],
    seen_ids: &HashSet<String>,
) -> Result<(), SessionStoreError> {
    for entry in entries {
        if let Some(parent_id) = &entry.parent_id
            && !seen_ids.contains(parent_id)
        {
            return Err(SessionStoreError::DanglingParent {
                parent_id: parent_id.clone(),
            });
        }
    }

    Ok(())
}

fn io_error(source: std::io::Error) -> SessionStoreError {
    SessionStoreError::IoError { source }
}

#[cfg(test)]
mod tests {
    use std::{fs, io::Write, path::PathBuf};

    use provider_protocol::{ContentBlock, ConversationItem, Role, ToolCall};
    use uuid::Uuid;

    use super::{JsonlLoader, JsonlWriter};
    use crate::{ConfigSnapshot, SessionEntry, SessionEntryKind, SessionHeader, SessionId};

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
    fn test_temp_dir(label: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("hunea-session-store-{label}-{}", Uuid::now_v7()));
        fs::create_dir_all(&dir).expect("test temp dir should be creatable");
        dir
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
}
