use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

/// `WorkspaceMetadata` 是 builtin 工具层使用的精简文件元信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspaceMetadata {
    pub(crate) is_dir: bool,
    pub(crate) is_file: bool,
    pub(crate) len: u64,
    pub(crate) modified_at: Option<SystemTime>,
}

/// `WorkspaceDirectoryEntry` 是可跨后端传递的目录项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspaceDirectoryEntry {
    pub(crate) path: PathBuf,
    pub(crate) name: String,
    pub(crate) is_dir: bool,
}

/// `WorkspaceAccess` 抽象 builtin 工具访问 workspace 的最小能力。
pub(crate) trait WorkspaceAccess: Send + Sync {
    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf>;
    fn metadata(&self, path: &Path) -> io::Result<WorkspaceMetadata>;
    fn open_reader(&self, path: &Path) -> io::Result<Box<dyn Read + Send>>;
    fn read_dir(&self, path: &Path) -> io::Result<Vec<WorkspaceDirectoryEntry>>;

    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        let _ = path;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "workspace backend does not support directory creation",
        ))
    }

    fn write_text_file(&self, path: &Path, content: &str) -> io::Result<()> {
        let _ = (path, content);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "workspace backend does not support file writing",
        ))
    }
}

pub(crate) type SharedWorkspaceAccess = Arc<dyn WorkspaceAccess>;

/// `local_workspace_access` 返回默认的本地 filesystem backend。
pub(crate) fn local_workspace_access() -> SharedWorkspaceAccess {
    Arc::new(LocalWorkspaceAccess)
}

#[derive(Debug, Default)]
struct LocalWorkspaceAccess;

impl WorkspaceAccess for LocalWorkspaceAccess {
    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        fs::canonicalize(path)
    }

    fn metadata(&self, path: &Path) -> io::Result<WorkspaceMetadata> {
        let metadata = fs::metadata(path)?;
        Ok(WorkspaceMetadata {
            is_dir: metadata.is_dir(),
            is_file: metadata.is_file(),
            len: metadata.len(),
            modified_at: metadata.modified().ok(),
        })
    }

    fn open_reader(&self, path: &Path) -> io::Result<Box<dyn Read + Send>> {
        Ok(Box::new(fs::File::open(path)?))
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<WorkspaceDirectoryEntry>> {
        fs::read_dir(path)?
            .map(|entry| {
                let entry = entry?;
                let file_type = entry.file_type()?;
                Ok(WorkspaceDirectoryEntry {
                    path: entry.path(),
                    name: entry.file_name().to_string_lossy().to_string(),
                    is_dir: file_type.is_dir(),
                })
            })
            .collect()
    }

    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        fs::create_dir_all(path)
    }

    fn write_text_file(&self, path: &Path, content: &str) -> io::Result<()> {
        fs::write(path, content)
    }
}
