#[cfg(not(feature = "bench-support"))]
#[doc(hidden)]
// 默认构建仍编译 benchmark helper 以校验性能路径；其公开入口只在 bench-support 下使用。
#[allow(dead_code)]
mod benchmark;
#[cfg(feature = "bench-support")]
#[doc(hidden)]
pub mod benchmark;

mod acp;
mod command_panel;
mod composer;
mod composer_mouse;
mod debug;
mod document;
mod exit_confirmation;
mod external_editor;
mod hero;
mod hero_item;
mod history_scroll_indicator;
mod inline_panel;
mod message;
mod model;
mod model_panel;
mod reasoning_message;
mod runner;
mod selection;
mod sender;
mod shimmer;
mod status_line;
mod status_phrases;
mod style_mode;
mod styled_text;
mod system_message;
mod tool_approval_panel;
mod tool_result;
mod transcript;
mod update;
mod view;

pub mod theme;

pub use external_editor::ExternalEditorLaunch;
pub use hero::{
    HeroOptions, print_hero, render_hero, render_hero_buffer_with_palette,
    render_hero_lines_with_palette, render_hero_plain_lines_with_palette, render_hero_with_palette,
    write_hero_to,
};
pub use model::{Model, ModelOptions};
pub use reasoning_message::ReasoningDisplayMode;
pub use runner::{
    RuntimeOptions, RuntimeRequestPolicy, run, run_with_options, run_with_runtime_options,
    run_with_style_mode,
};
pub use sender::Sender;
pub use status_line::StatusLineItem;
pub use style_mode::StyleMode;
pub use update::{AppEffect, AppEvent, STARTUP_PROBE_TIMEOUT};
