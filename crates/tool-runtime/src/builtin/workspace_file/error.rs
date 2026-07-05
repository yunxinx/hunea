use std::{io, path::PathBuf};

#[derive(Debug, thiserror::Error)]
pub(crate) enum WorkspaceFileError {
    #[error("{tool} arguments are invalid: {source}")]
    InvalidArguments {
        tool: &'static str,
        source: serde_json::Error,
    },
    #[error("'path' is required")]
    MissingPath,
    #[error("path not found: {requested}: {source}")]
    PathNotFound {
        requested: String,
        source: io::Error,
    },
    #[error("path is outside workspace: {requested}")]
    PathOutsideWorkspace { requested: String },
    #[error("stat failed for '{path}': {source}")]
    Metadata { path: String, source: io::Error },
    #[error("'{path}' is a directory, use {replacement} instead")]
    Directory {
        path: String,
        replacement: &'static str,
    },
    #[error("'{path}' is not a regular file")]
    NotRegularFile { path: String },
    #[error("'{path}' is not a directory, use {replacement} instead")]
    NotDirectory {
        path: String,
        replacement: &'static str,
    },
    #[error("image path '{path}' is not a file")]
    ImageNotFile { path: String },
    #[error("read failed for '{path}': {detail}")]
    ReadRejected { path: PathBuf, detail: String },
    #[error("read failed for '{path}': {source}")]
    Read { path: PathBuf, source: io::Error },
    #[error("read directory failed for '{path}': {source}")]
    ReadDirectory { path: PathBuf, source: io::Error },
    #[error("workspace root '{path}' is unavailable: {source}")]
    WorkspaceRoot { path: PathBuf, source: io::Error },
    #[error("invalid .gitignore '{path}': {source}")]
    Gitignore {
        path: PathBuf,
        source: ignore::Error,
    },
    #[error("invalid gitignore matcher for '{root}': {source}")]
    GitignoreMatcher {
        root: PathBuf,
        source: ignore::Error,
    },
    #[error(
        "read failed for '{path}': offset {start_line} is beyond end of file ({total_lines} lines total)"
    )]
    OffsetBeyondEnd {
        path: PathBuf,
        start_line: usize,
        total_lines: usize,
    },
    #[error("Tool call interrupted")]
    Interrupted,
    #[error("image file '{path}' is too large for view_image ({bytes} bytes, limit {limit} bytes)")]
    ImageTooLarge {
        path: String,
        bytes: u64,
        limit: u64,
    },
    #[error("unsupported image type for '{path}'")]
    UnsupportedImageType { path: String },
    #[error("read image failed for '{path}': {source}")]
    ReadImage { path: PathBuf, source: io::Error },
    #[error("'{path}' does not look like {mime_type}")]
    ImageSignatureMismatch {
        path: String,
        mime_type: &'static str,
    },
}
