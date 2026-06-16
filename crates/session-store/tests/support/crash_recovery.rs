use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use session_store::{ProjectDir, SessionEntry, SessionId, session_filename};

use super::common::TestSessionRoot;

pub fn truncate_last_line(path: &Path, removed_bytes: usize) {
    let original = fs::read(path).expect("jsonl should be readable before truncation");
    let truncated_len = original
        .len()
        .checked_sub(removed_bytes)
        .expect("fixture jsonl should be larger than the truncation amount");
    fs::write(path, &original[..truncated_len]).expect("jsonl tail should be truncated");
}

pub fn remove_index_files(root: &TestSessionRoot) {
    for suffix in ["", "-shm", "-wal"] {
        let path = root.path().join(format!("index.sqlite{suffix}"));
        if path.exists() {
            fs::remove_file(path).expect("sqlite index file should be removable");
        }
    }
}

pub fn read_session_entries(path: &Path) -> Vec<SessionEntry> {
    fs::read_to_string(path)
        .expect("jsonl should be readable")
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).expect("session entry should parse"))
        .collect()
}

pub fn write_session_fixture(
    root: &TestSessionRoot,
    work_dir: &Path,
    session_id: &SessionId,
    entries: &[SessionEntry],
) -> PathBuf {
    let session_path = root
        .path()
        .join("sessions")
        .join(ProjectDir::from_work_dir(work_dir).encoded_session_dir())
        .join(session_filename(session_id));
    let parent_dir = session_path
        .parent()
        .expect("session fixture path should have a parent");
    fs::create_dir_all(parent_dir).expect("fixture parent directory should exist");

    let mut file = fs::File::create(&session_path).expect("fixture jsonl should be creatable");
    for entry in entries {
        let serialized = serde_json::to_string(entry).expect("fixture entry should serialize");
        writeln!(file, "{serialized}").expect("fixture entry should write");
    }

    session_path
}

/// 精确恢复原始权限，避免测试污染文件 mode。
pub struct PermissionGuard {
    path: PathBuf,
    original_permissions: fs::Permissions,
    restored: bool,
}

impl PermissionGuard {
    pub fn make_readonly(path: &Path) -> Self {
        let original_permissions = fs::metadata(path)
            .expect("path metadata should load")
            .permissions();
        let readonly_permissions = readonly_permissions(&original_permissions);
        fs::set_permissions(path, readonly_permissions).expect("permissions should update");
        Self {
            path: path.to_path_buf(),
            original_permissions,
            restored: false,
        }
    }

    pub fn restore(mut self) {
        self.restore_inner();
    }

    fn restore_inner(&mut self) {
        if self.restored {
            return;
        }

        fs::set_permissions(&self.path, self.original_permissions.clone())
            .expect("original permissions should restore");
        self.restored = true;
    }
}

impl Drop for PermissionGuard {
    fn drop(&mut self) {
        if self.restored {
            return;
        }

        let _ = fs::set_permissions(&self.path, self.original_permissions.clone());
    }
}

#[cfg(unix)]
fn readonly_permissions(original_permissions: &fs::Permissions) -> fs::Permissions {
    let mut readonly_permissions = original_permissions.clone();
    readonly_permissions.set_mode(original_permissions.mode() & !0o222);
    readonly_permissions
}

#[cfg(not(unix))]
fn readonly_permissions(original_permissions: &fs::Permissions) -> fs::Permissions {
    let mut readonly_permissions = original_permissions.clone();
    readonly_permissions.set_readonly(true);
    readonly_permissions
}
