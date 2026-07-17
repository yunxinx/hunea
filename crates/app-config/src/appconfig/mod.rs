mod defaults;
mod error;
mod file_config;
mod load;
mod managed_search_authorization;
mod merge;
mod types;
mod validate;

pub use error::AppConfigError;
pub use load::{load_from_paths, load_with_resolution};
pub use managed_search_authorization::{
    ManagedSearchAuthorization, persist_managed_search_tool_authorization_to_path,
    persist_managed_search_tool_rejection_to_path, read_managed_search_authorization,
};
pub use types::{
    BRANCH_PICKER_LIST_ROWS_DEFAULT, BRANCH_PICKER_LIST_ROWS_MAX, BRANCH_PICKER_LIST_ROWS_MIN,
    COMPOSER_UNDO_DEFAULT_LIMIT, COMPOSER_UNDO_MAX_LIMIT, COMPOSER_UNDO_MIN_LIMIT, Config,
    DebugConfig, EscRewindMode, FILE_PICKER_POPUP_MAX_HEIGHT, FILE_PICKER_POPUP_MIN_HEIGHT,
    KeyboardEnhancementMode, MESSAGE_HISTORY_LIMIT_DEFAULT, MESSAGE_HISTORY_LIMIT_MAX,
    MESSAGE_HISTORY_LIMIT_MIN, MotionMode, ReasoningContentDisplay, RuntimeConfig,
    ScrollAnimationMode, TuiConfig, UserInputStyle,
};
