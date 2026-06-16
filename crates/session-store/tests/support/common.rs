use std::{
    fs,
    path::{Path, PathBuf},
};

use session_store::{LocalSessionStore, SessionHeader, SessionId, SessionStoreError};
use tempfile::{Builder, TempDir};

/// 为 session-store 集成测试提供隔离的临时根目录。
pub struct TestSessionRoot {
    temp_dir: TempDir,
}

impl TestSessionRoot {
    pub fn new(label: &str) -> Self {
        let temp_dir = Builder::new()
            .prefix(&format!("hunea-session-store-{label}-"))
            .tempdir()
            .expect("temporary test root should be creatable");
        Self { temp_dir }
    }

    pub fn path(&self) -> &Path {
        self.temp_dir.path()
    }

    pub fn workspace_path(&self, name: &str) -> PathBuf {
        let work_dir = self.path().join("workspace").join(name);
        fs::create_dir_all(&work_dir).expect("work dir should be creatable");
        work_dir
    }
}

/// 使用测试根目录打开本地 session store。
pub async fn open_store(root: &TestSessionRoot) -> LocalSessionStore {
    LocalSessionStore::open_in(root.path().to_path_buf())
        .await
        .expect("local store should open")
}

/// 构造通用的 session header fixture。
pub fn sample_header(work_dir: &Path, model: &str, session_name: Option<&str>) -> SessionHeader {
    SessionHeader {
        session_id: SessionId::new(),
        work_dir: work_dir.to_path_buf(),
        session_name: session_name.map(str::to_string),
        initial_model: model.to_string(),
        git_head: Some("abc123".to_string()),
        cli_version: Some(env!("CARGO_PKG_VERSION").to_string()),
    }
}

pub fn first_item_entry_id(path: &Path) -> Result<String, SessionStoreError> {
    Ok(item_entry_ids(path)?
        .into_iter()
        .next()
        .expect("session fixture should include first item"))
}

pub fn item_entry_ids(path: &Path) -> Result<Vec<String>, SessionStoreError> {
    let jsonl = fs::read_to_string(path).map_err(|source| SessionStoreError::IoError { source })?;
    jsonl
        .lines()
        .skip(1)
        .map(|line| {
            let value: serde_json::Value =
                serde_json::from_str(line).expect("item line should parse");
            Ok(value["id"]
                .as_str()
                .expect("entry id should exist")
                .to_string())
        })
        .collect()
}
