mod editor;
mod git;
mod workdir;

pub use editor::{
    ExternalEditor, ExternalEditorError, resolve_external_editor,
    validate_configured_external_editor,
};
pub use git::{git_branch, git_head};
pub use workdir::{short_work_dir, shorten_home_prefix};
