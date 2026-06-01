mod defaults;
mod error;
mod file_config;
mod load;
mod managed_search_authorization;
mod merge;
mod paths;
mod types;
mod validate;

pub use error::AppConfigError;
pub use load::{load, load_from_paths};
pub use managed_search_authorization::persist_managed_search_tool_authorization_to_path;
pub use paths::user_config_file_path;
pub use types::{
    COMPOSER_UNDO_DEFAULT_LIMIT, COMPOSER_UNDO_MAX_LIMIT, COMPOSER_UNDO_MIN_LIMIT, Config,
    DebugConfig, FILE_PICKER_POPUP_MAX_HEIGHT, FILE_PICKER_POPUP_MIN_HEIGHT,
    ReasoningContentDisplay, RuntimeConfig, TuiConfig, UserInputStyle,
};
