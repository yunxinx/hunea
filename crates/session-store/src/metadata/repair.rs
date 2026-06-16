use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use rusqlite::{Connection, params};

use crate::{
    ProjectDir, SessionMeta, SessionStoreError, jsonl::JsonlLoader, meta_derive::SessionMetaDeriver,
};

use super::{checked_i64, io_error, sqlite_error, upsert_session_row};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SessionFileFingerprint {
    pub(super) file_size: u64,
    pub(super) modified_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DiscoveredSessionFile {
    pub(super) path: PathBuf,
    pub(super) fingerprint: SessionFileFingerprint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct IndexedProjectFile {
    pub(super) session_id: String,
    pub(super) jsonl_path: PathBuf,
    pub(super) fingerprint: Option<SessionFileFingerprint>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ExtractedSessionMeta {
    pub(super) meta: SessionMeta,
    pub(super) fingerprint: SessionFileFingerprint,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct RepairPlan {
    pub(super) stale_session_ids: Vec<String>,
    pub(super) files_to_refresh: Vec<DiscoveredSessionFile>,
}

fn upsert_repair_state(
    conn: &Connection,
    session_id: &str,
    jsonl_path: &Path,
    fingerprint: &SessionFileFingerprint,
) -> Result<(), SessionStoreError> {
    conn.execute(
        "
        INSERT INTO session_repair_state (
            session_id,
            jsonl_path,
            file_size,
            modified_at_ms
        ) VALUES (?1, ?2, ?3, ?4)
        ON CONFLICT(session_id) DO UPDATE SET
            jsonl_path = excluded.jsonl_path,
            file_size = excluded.file_size,
            modified_at_ms = excluded.modified_at_ms
        ",
        params![
            session_id,
            jsonl_path.to_string_lossy(),
            checked_i64(
                fingerprint.file_size,
                &format!("session `{session_id}` file_size")
            )?,
            fingerprint.modified_at_ms,
        ],
    )
    .map_err(sqlite_error)?;

    Ok(())
}

pub(super) fn rebuild_index_from_jsonl(
    conn: &Connection,
    sessions_dir: &Path,
) -> Result<usize, SessionStoreError> {
    conn.execute("DELETE FROM session_repair_state", [])
        .map_err(sqlite_error)?;
    conn.execute("DELETE FROM sessions", [])
        .map_err(sqlite_error)?;

    let mut processed = 0;
    for discovered_file in collect_jsonl_files(sessions_dir)? {
        let extracted = extract_session_meta(&discovered_file)?;
        upsert_session_row(conn, &extracted.meta)?;
        upsert_repair_state(
            conn,
            &extracted.meta.session_id.to_string(),
            &extracted.meta.jsonl_path,
            &extracted.fingerprint,
        )?;
        processed += 1;
    }

    Ok(processed)
}

pub(super) fn repair_project_from_jsonl(
    conn: &Connection,
    sessions_dir: &Path,
    project_dir: &ProjectDir,
) -> Result<(), SessionStoreError> {
    let project_dir_key = project_dir.canonical_string();
    let project_sessions_dir = sessions_dir.join(project_dir.encoded_session_dir());
    let discovered_files = collect_jsonl_files(&project_sessions_dir)?;
    let indexed_files = load_indexed_project_files(conn, &project_dir_key)?;
    let repair_plan = build_repair_plan(&discovered_files, &indexed_files);
    let indexed_by_path: BTreeMap<PathBuf, IndexedProjectFile> = indexed_files
        .into_iter()
        .map(|indexed| (indexed.jsonl_path.clone(), indexed))
        .collect();

    for session_id in repair_plan.stale_session_ids {
        delete_session_rows(conn, &session_id)?;
    }

    for discovered_file in repair_plan.files_to_refresh {
        let extracted = extract_session_meta(&discovered_file)?;
        if let Some(existing) = indexed_by_path.get(&discovered_file.path)
            && existing.session_id != extracted.meta.session_id.to_string()
        {
            delete_session_rows(conn, &existing.session_id)?;
        }
        upsert_session_row(conn, &extracted.meta)?;
        upsert_repair_state(
            conn,
            &extracted.meta.session_id.to_string(),
            &extracted.meta.jsonl_path,
            &extracted.fingerprint,
        )?;
    }

    Ok(())
}

fn delete_session_rows(conn: &Connection, session_id: &str) -> Result<(), SessionStoreError> {
    conn.execute(
        "DELETE FROM session_repair_state WHERE session_id = ?1",
        params![session_id],
    )
    .map_err(sqlite_error)?;
    conn.execute(
        "DELETE FROM sessions WHERE session_id = ?1",
        params![session_id],
    )
    .map_err(sqlite_error)?;
    Ok(())
}

fn load_indexed_project_files(
    conn: &Connection,
    project_dir: &str,
) -> Result<Vec<IndexedProjectFile>, SessionStoreError> {
    let mut statement = conn
        .prepare(
            "
            SELECT
                s.session_id,
                s.jsonl_path,
                r.file_size,
                r.modified_at_ms
            FROM sessions s
            LEFT JOIN session_repair_state r ON r.session_id = s.session_id
            WHERE s.project_dir = ?1
            ",
        )
        .map_err(sqlite_error)?;
    let rows = statement
        .query_map(params![project_dir], |row| {
            let file_size = row.get::<_, Option<i64>>(2)?;
            let modified_at_ms = row.get::<_, Option<i64>>(3)?;
            Ok(IndexedProjectFile {
                session_id: row.get(0)?,
                jsonl_path: PathBuf::from(row.get::<_, String>(1)?),
                fingerprint: match (file_size, modified_at_ms) {
                    (Some(file_size), Some(modified_at_ms)) => Some(SessionFileFingerprint {
                        file_size: u64::try_from(file_size).map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                2,
                                rusqlite::types::Type::Integer,
                                Box::new(error),
                            )
                        })?,
                        modified_at_ms,
                    }),
                    _ => None,
                },
            })
        })
        .map_err(sqlite_error)?;

    let mut indexed_files = Vec::new();
    for row in rows {
        indexed_files.push(row.map_err(sqlite_error)?);
    }

    Ok(indexed_files)
}

pub(super) fn build_repair_plan(
    discovered_files: &[DiscoveredSessionFile],
    indexed_files: &[IndexedProjectFile],
) -> RepairPlan {
    let discovered_by_path: BTreeMap<&Path, &DiscoveredSessionFile> = discovered_files
        .iter()
        .map(|file| (file.path.as_path(), file))
        .collect();
    let indexed_by_path: BTreeMap<&Path, &IndexedProjectFile> = indexed_files
        .iter()
        .map(|indexed| (indexed.jsonl_path.as_path(), indexed))
        .collect();

    let stale_session_ids = indexed_files
        .iter()
        .filter(|indexed| !discovered_by_path.contains_key(indexed.jsonl_path.as_path()))
        .map(|indexed| indexed.session_id.clone())
        .collect();
    let files_to_refresh = discovered_files
        .iter()
        .filter(
            |discovered| match indexed_by_path.get(discovered.path.as_path()) {
                Some(indexed) => indexed.fingerprint.as_ref() != Some(&discovered.fingerprint),
                None => true,
            },
        )
        .cloned()
        .collect();

    RepairPlan {
        stale_session_ids,
        files_to_refresh,
    }
}

fn extract_session_meta(
    discovered_file: &DiscoveredSessionFile,
) -> Result<ExtractedSessionMeta, SessionStoreError> {
    let mut deriver = SessionMetaDeriver::default();

    JsonlLoader::scan(&discovered_file.path, |entry| {
        deriver.observe(&entry);
        Ok(())
    })?;

    let meta = deriver.finish(
        discovered_file.path.clone(),
        Some(discovered_file.fingerprint.file_size),
        format!(
            "session file `{}` is missing a header entry",
            discovered_file.path.display()
        ),
    )?;

    Ok(ExtractedSessionMeta {
        meta,
        fingerprint: discovered_file.fingerprint.clone(),
    })
}

fn collect_jsonl_files(directory: &Path) -> Result<Vec<DiscoveredSessionFile>, SessionStoreError> {
    let mut files = Vec::new();
    collect_jsonl_files_into(directory, &mut files)?;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

fn collect_jsonl_files_into(
    directory: &Path,
    files: &mut Vec<DiscoveredSessionFile>,
) -> Result<(), SessionStoreError> {
    let read_dir = match fs::read_dir(directory) {
        Ok(read_dir) => read_dir,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(io_error(error)),
    };

    for entry in read_dir {
        let entry = entry.map_err(io_error)?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(io_error)?;
        if file_type.is_dir() {
            collect_jsonl_files_into(&path, files)?;
            continue;
        }
        if file_type.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
            files.push(DiscoveredSessionFile {
                fingerprint: file_fingerprint(&path)?,
                path,
            });
        }
    }

    Ok(())
}

fn file_fingerprint(path: &Path) -> Result<SessionFileFingerprint, SessionStoreError> {
    let metadata = fs::metadata(path).map_err(io_error)?;
    Ok(SessionFileFingerprint {
        file_size: metadata.len(),
        modified_at_ms: modified_time_ms(&metadata)?,
    })
}

fn modified_time_ms(metadata: &fs::Metadata) -> Result<i64, SessionStoreError> {
    let duration = metadata
        .modified()
        .map_err(io_error)?
        .duration_since(UNIX_EPOCH)
        .map_err(|error| SessionStoreError::CorruptIndex {
            message: format!("file modified time is before unix epoch: {error}"),
        })?;
    i64::try_from(duration.as_millis()).map_err(|_| SessionStoreError::CorruptIndex {
        message: "file modified time exceeds i64 range".to_string(),
    })
}
