#[cfg(not(feature = "bench-support"))]
#[doc(hidden)]
#[allow(dead_code)]
mod benchmark;
#[cfg(feature = "bench-support")]
#[doc(hidden)]
pub mod benchmark;

mod acp_activity;
mod acp_panel;
mod acp_permission;
mod command_panel;
mod composer;
mod composer_mouse;
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
mod runner;
mod selection;
mod sender;
mod status_line;
mod style_mode;
mod styled_text;
mod system_message;
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
pub use runner::{
    RuntimeOptions, run, run_with_options, run_with_runtime_options, run_with_style_mode,
};
pub use sender::Sender;
pub use status_line::StatusLineItem;
pub use style_mode::StyleMode;
pub use update::{AppEffect, AppEvent, STARTUP_PROBE_TIMEOUT};
