#[cfg(not(feature = "bench-support"))]
#[doc(hidden)]
// 默认构建仍编译 benchmark helper 以校验性能路径；其公开入口只在 bench-support 下使用。
#[allow(dead_code)]
mod benchmark;
#[cfg(feature = "bench-support")]
#[doc(hidden)]
pub mod benchmark;

mod command_panel;
mod composer;
mod debug;
mod display_width;
mod document;
mod exit_confirmation;
mod external_editor;
mod file_picker;
mod file_search;
mod floating;
mod history_scroll_indicator;
mod inline_panel;
mod message;
mod message_revisit;
mod model;
mod model_panel;
mod path_resolve;
mod render_frame;
mod runner;
mod runtime;
mod selection;
mod sender;
mod shimmer;
mod startup_banner;
mod status_line;
mod status_phrases;
mod stream_activity;
mod style_mode;
mod styled_text;
mod terminal_grid;
mod terminal_text;
mod tool_approval_panel;
mod tool_result;
mod transcript;
mod transcript_overlay;
mod update;
mod view;

pub mod theme;

pub use external_editor::ExternalEditorLaunch;
pub use model::{Model, ModelOptions, RequestMetrics};
pub use runner::{
    NoopRuntimeCoordinator, RuntimeCoordinator, run, run_with_options,
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
pub use transcript::ReasoningDisplayMode;
pub use update::{AppEffect, AppEvent, STARTUP_PROBE_TIMEOUT};

pub use runtime_domain::request_policy::RuntimeRequestPolicy;
