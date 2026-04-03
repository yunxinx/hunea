use std::io::{self, IsTerminal, Write};

use color_eyre::eyre::{Result, WrapErr};

use crate::{
    appconfig::{self, TuiConfig, UserInputStyle},
    envinfo,
    frontend::tui::{self, HeroOptions, Model, ModelOptions, StatusLineItem, StyleMode},
};

/// `run` 负责组装并启动交互式 TUI 应用。
pub fn run() -> Result<()> {
    let stdout = io::stdout();
    let preserve_ansi = stdout.is_terminal();
    let mut handle = stdout.lock();
    let config = appconfig::load().wrap_err("failed to load app config")?;
    run_with_writer(&mut handle, preserve_ansi, &config.tui)
}

/// `run_with_writer` 允许调用方注入退出 AltScreen 后的 terminal replay 输出目标。
pub fn run_with_writer<W: Write>(
    writer: &mut W,
    preserve_ansi: bool,
    tui_config: &TuiConfig,
) -> Result<()> {
    let model = tui::run_with_options(
        HeroOptions::default(),
        model_options_from_config(tui_config),
    )
    .wrap_err("failed to run tui application")?;
    write_terminal_replay_with_context(writer, &model, preserve_ansi)
}

/// `write_terminal_replay` 将 terminal replay 内容输出到目标 writer。
pub fn write_terminal_replay<W: Write>(writer: &mut W, model: &Model) -> io::Result<()> {
    write_terminal_replay_with_mode(writer, model, false)
}

/// `write_terminal_replay_preserving_ansi` 在目标仍支持终端样式时保留 ANSI。
pub fn write_terminal_replay_preserving_ansi<W: Write>(
    writer: &mut W,
    model: &Model,
) -> io::Result<()> {
    write_terminal_replay_with_mode(writer, model, true)
}

fn write_terminal_replay_with_mode<W: Write>(
    writer: &mut W,
    model: &Model,
    preserve_ansi: bool,
) -> io::Result<()> {
    let items = model.terminal_replay_items(preserve_ansi);

    for (index, item) in items.iter().enumerate() {
        writeln!(writer, "{item}")?;
        if index + 1 < items.len() {
            writeln!(writer)?;
        }
    }

    Ok(())
}

/// `write_terminal_replay_with_context` 为 terminal replay 输出补充入口层错误上下文。
pub fn write_terminal_replay_with_context<W: Write>(
    writer: &mut W,
    model: &Model,
    preserve_ansi: bool,
) -> Result<()> {
    write_terminal_replay_with_mode(writer, model, preserve_ansi)
        .wrap_err("failed to write terminal replay")
}

fn style_mode_from_config(style: UserInputStyle) -> StyleMode {
    match style {
        UserInputStyle::Cx => StyleMode::Cx,
        UserInputStyle::Cc => StyleMode::Cc,
        UserInputStyle::Ms => StyleMode::Ms,
    }
}

fn model_options_from_config(tui_config: &TuiConfig) -> ModelOptions {
    ModelOptions {
        style_mode: style_mode_from_config(tui_config.user_input_style),
        status_line_items: status_line_items_from_config(&tui_config.status_line),
        external_editor: tui_config.external_editor.clone(),
        external_editor_hint: external_editor_hint_from_config(&tui_config.external_editor),
        show_external_editor_helper: tui_config.show_external_editor_helper,
        copy_on_mouse_selection_release: tui_config.copy_on_mouse_selection_release,
        swap_enter_and_send: tui_config.swap_enter_and_send,
    }
}

fn status_line_items_from_config(items: &[String]) -> Vec<StatusLineItem> {
    items
        .iter()
        .filter_map(|item| StatusLineItem::from_config_value(item))
        .collect()
}

fn external_editor_hint_from_config(configured: &[String]) -> String {
    envinfo::resolve_external_editor(configured)
        .map(|editor| editor.display_name)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_options_from_config_carries_mouse_selection_copy_flag() {
        let options = model_options_from_config(&TuiConfig {
            user_input_style: UserInputStyle::Cx,
            status_line: Vec::new(),
            external_editor: Vec::new(),
            show_external_editor_helper: true,
            copy_on_mouse_selection_release: true,
            swap_enter_and_send: false,
        });

        assert!(options.copy_on_mouse_selection_release);
    }

    #[test]
    fn model_options_from_config_carries_swap_enter_and_send_flag() {
        let options = model_options_from_config(&TuiConfig {
            user_input_style: UserInputStyle::Cx,
            status_line: Vec::new(),
            external_editor: Vec::new(),
            show_external_editor_helper: true,
            copy_on_mouse_selection_release: false,
            swap_enter_and_send: true,
        });

        assert!(options.swap_enter_and_send);
    }
}
