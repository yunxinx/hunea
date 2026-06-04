use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
};

use tracing::warn;

use crate::{SessionEntry, SessionStoreError};

/// append-only JSONL writer。
#[allow(dead_code)]
pub(crate) struct JsonlWriter {
    path: PathBuf,
    file: Option<BufWriter<File>>,
}

#[allow(dead_code)]
impl JsonlWriter {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path, file: None }
    }

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

        Ok(self.file.as_mut().expect("writer should initialize file"))
    }
}

/// JSONL loader。
#[allow(dead_code)]
pub(crate) struct JsonlLoader;

#[allow(dead_code)]
impl JsonlLoader {
    pub(crate) fn load(path: &Path) -> Result<Vec<SessionEntry>, SessionStoreError> {
        let file = File::open(path).map_err(io_error)?;
        let mut reader = BufReader::new(file);
        let mut loaded_entries = Vec::new();
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
                        loaded_entries.push(entry);
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

        validate_parent_links(&loaded_entries, &seen_ids)?;

        Ok(loaded_entries)
    }
}

#[allow(dead_code)]
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

#[allow(dead_code)]
fn io_error(source: std::io::Error) -> SessionStoreError {
    SessionStoreError::IoError { source }
}
