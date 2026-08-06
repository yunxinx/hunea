use std::{io, path::PathBuf};

use super::search_fallback::TOOL_CALL_INTERRUPTED;
use crate::builtin::workspace_file::error::WorkspaceFileError;

#[derive(Debug, thiserror::Error)]
pub(crate) enum SearchToolError {
    #[error("{tool} arguments are invalid: {source}")]
    InvalidArguments {
        tool: &'static str,
        source: serde_json::Error,
    },
    #[error("'pattern' is required")]
    MissingPattern,
    #[error("workspace root '{path}' is unavailable: {source}")]
    WorkspaceRoot { path: PathBuf, source: io::Error },
    #[error("{source}")]
    WorkspacePath {
        #[source]
        source: WorkspaceFileError,
    },
    #[error("spawn ripgrep failed: {source}")]
    RipgrepSpawn { source: io::Error },
    #[error("ripgrep stdout is unavailable")]
    RipgrepStdoutUnavailable,
    #[error("read ripgrep output failed: {source}")]
    RipgrepOutputRead { source: io::Error },
    #[error("wait for ripgrep failed: {source}")]
    RipgrepWait { source: io::Error },
    #[error("ripgrep failed: {stderr}")]
    RipgrepFailed { stderr: String },
    #[error("invalid glob pattern {pattern:?}: {source}")]
    InvalidGlob {
        pattern: String,
        source: globset::Error,
    },
    #[error("invalid grep pattern: {source}")]
    InvalidRegex { source: grep_regex::Error },
    #[error("walk search path failed: {source}")]
    WalkSearchPath { source: ignore::Error },
    #[error("read file type failed for '{path}'")]
    FileTypeUnavailable { path: PathBuf },
    #[error("find search path '{path}' is not a directory")]
    SearchPathNotDirectory { path: PathBuf },
    #[error("{TOOL_CALL_INTERRUPTED}")]
    Interrupted,
    #[error("no managed ripgrep asset for this platform")]
    NoManagedRipgrepAsset,
    #[error("{operation} task failed: {source}")]
    JoinTask {
        operation: &'static str,
        source: tokio::task::JoinError,
    },
    #[error("invalid managed ripgrep URL: {source}")]
    InvalidManagedRipgrepUrl { source: url::ParseError },
    #[error("managed ripgrep URL is not an official GitHub URL: {url}")]
    UnofficialManagedRipgrepUrl { url: String },
    #[error("download failed: {source}")]
    Download { source: reqwest::Error },
    #[error("read download body failed: {source}")]
    ReadDownloadBody { source: reqwest::Error },
    #[error("{operation} failed: {source}")]
    Io {
        operation: &'static str,
        source: io::Error,
    },
    #[error("{operation} failed for '{path}': {source}")]
    PathIo {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    #[error("checksum mismatch for managed ripgrep archive: expected {expected}, got {actual}")]
    ChecksumMismatch { expected: String, actual: String },
    #[error("tar archive contains a path outside the extraction directory")]
    TarPathOutsideExtraction,
    #[error("read zip failed: {source}")]
    ReadZip { source: zip::result::ZipError },
    #[error("read zip entry failed: {source}")]
    ReadZipEntry { source: zip::result::ZipError },
    #[error("zip archive contains a path outside the extraction directory")]
    ZipPathOutsideExtraction,
    #[error("archive did not contain {file_name}")]
    MissingExtractedBinary { file_name: String },
}
