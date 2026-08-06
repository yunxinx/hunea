pub(super) mod error;
pub(super) mod find;
pub(super) mod grep;
pub(super) mod ripgrep;
pub(super) mod search_fallback;

pub use find::find_tool;
pub use grep::grep_tool;
pub use ripgrep::{
    MANAGED_RIPGREP_NAME, MANAGED_RIPGREP_VERSION, ManagedRipgrepConfig,
    ManagedRipgrepInstallError, ManagedRipgrepProgress, ManagedRipgrepStatus,
    detect_managed_ripgrep_status, install_managed_ripgrep_with_progress,
};
