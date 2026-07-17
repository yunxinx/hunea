#[cfg(not(feature = "bench-support"))]
#[doc(hidden)]
// benchmark 同时是内部性能路径的编译期消费者；移出普通构建前必须先把这些
// benchmark-only 查询从生产 API 逐一 feature-gate，避免掩盖真实 dead-code 变化。
#[allow(dead_code)]
mod benchmark;
#[cfg(feature = "bench-support")]
#[doc(hidden)]
pub mod benchmark;

mod attached_prompt_picker_row;
mod attention_pill;
mod bounded_lru_cache;
mod command_panel;
mod composer;
mod composer_inline_picker;
mod context_budget;
mod copy_picker;
mod custom_prompt_picker;
mod debug;
mod display_width;
mod document;
mod entry_tree;
mod exit_confirmation;
mod external_editor;
mod file_picker;
mod file_search;
mod floating;
mod frame_time;
mod fullscreen_list_chrome;
mod fullscreen_search_list;
mod history_scroll_indicator;
mod image_attachment;
mod inline_panel;
mod keyboard_enhancement;
mod list_selection;
mod markdown_display;
mod markdown_source;
mod message;
mod message_history_picker;
mod message_history_recall;
mod message_revisit;
mod modal_layer;
mod model;
mod model_panel;
mod motion;
mod overlay_input_result;
mod path_resolve;
mod picker_scrollbar;
mod plain_text_preview;
mod prompt_overlay;
mod relative_age;
mod render_frame;
mod runner;
mod runtime;
mod search_highlight;
mod selection;
mod sender;
mod session_picker;
mod session_preview;
mod session_tree_preview_replay;
mod session_tree_row_kind_view;
mod shimmer;
mod shortcut_help_popover;
mod skill_picker;
mod startup_banner;
mod status_line;
mod status_phrases;
mod stream_activity;
mod style_mode;
mod styled_text;
mod terminal_grid;
mod terminal_lifecycle;
mod terminal_panic;
pub mod terminal_session;
mod terminal_text;
#[cfg(test)]
mod test_helpers;
mod text_search;
mod time;
mod toast;
mod tool_approval_panel;
mod tool_result;
mod transcript;
mod transcript_overlay;
mod transcript_preview;
mod update;
mod view;

pub mod theme;

pub use external_editor::ExternalEditorLaunch;
pub use model::{
    EscRewindMode, KeyboardEnhancementPreference, Model, ModelOptions, RequestMetrics,
};
pub use motion::MotionMode;
pub use runner::{
    LoopEventWaker, NoopRuntimeCoordinator, RuntimeCoordinator, run, run_with_options,
    run_with_runtime_coordinator, run_with_style_mode,
};
pub use sender::Sender;
pub use startup_banner::{
    StartupBannerOptions, print_startup_banner, render_startup_banner,
    render_startup_banner_buffer_with_palette, render_startup_banner_lines_with_palette,
    render_startup_banner_plain_lines_with_palette, render_startup_banner_with_palette,
    write_startup_banner_to,
};
pub use status_line::StatusLineItem;
pub use style_mode::StyleMode;
pub use terminal_panic::install_terminal_panic_hook;
pub use terminal_session::MinimalTerminalSession;
pub use transcript::ReasoningDisplayMode;
pub use update::{AppEffect, AppEvent, STARTUP_PROBE_TIMEOUT};

pub use runtime_domain::request_policy::RuntimeRequestPolicy;
