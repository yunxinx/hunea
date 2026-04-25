use std::io::{self, IsTerminal, Write};

use color_eyre::eyre::{Result, WrapErr};

use crate::{
    appconfig::{self, Config, RuntimeConfig, TuiConfig, UserInputStyle},
    envinfo,
    frontend::tui::{self, HeroOptions, Model, ModelOptions, StatusLineItem, StyleMode},
};

/// `run` 负责组装并启动交互式 TUI 应用。
pub fn run() -> Result<()> {
    let stdout = io::stdout();
    let preserve_ansi = stdout.is_terminal();
    let mut handle = stdout.lock();
    let config = appconfig::load().wrap_err("failed to load app config")?;
    run_with_config_writer(&mut handle, preserve_ansi, &config)
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
    write_terminal_replay_on_exit(writer, &model, preserve_ansi, tui_config)
}

/// `run_with_config_writer` 使用完整配置启动 TUI，包含 ACP 命令入口。
pub fn run_with_config_writer<W: Write>(
    writer: &mut W,
    preserve_ansi: bool,
    config: &Config,
) -> Result<()> {
    let model = tui::run_with_options(
        HeroOptions::default(),
        model_options_from_app_config(config),
    )
    .wrap_err("failed to run tui application")?;
    write_terminal_replay_on_exit(writer, &model, preserve_ansi, &config.tui)
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

fn write_terminal_replay_on_exit<W: Write>(
    writer: &mut W,
    model: &Model,
    preserve_ansi: bool,
    tui_config: &TuiConfig,
) -> Result<()> {
    if !tui_config.print_transcript_on_exit {
        return Ok(());
    }

    write_terminal_replay_with_context(writer, model, preserve_ansi)
}

fn style_mode_from_config(style: UserInputStyle) -> StyleMode {
    match style {
        UserInputStyle::Cx => StyleMode::Cx,
        UserInputStyle::Cc => StyleMode::Cc,
        UserInputStyle::Ms => StyleMode::Ms,
    }
}

fn model_options_from_config(tui_config: &TuiConfig) -> ModelOptions {
    model_options_from_configs(tui_config, None)
}

fn model_options_from_app_config(config: &Config) -> ModelOptions {
    model_options_from_configs(&config.tui, Some(&config.runtime))
}

fn model_options_from_configs(
    tui_config: &TuiConfig,
    runtime_config: Option<&RuntimeConfig>,
) -> ModelOptions {
    ModelOptions {
        style_mode: style_mode_from_config(tui_config.user_input_style),
        status_line_items: status_line_items_from_config(&tui_config.status_line),
        external_editor: tui_config.external_editor.clone(),
        external_editor_hint: external_editor_hint_from_config(&tui_config.external_editor),
        show_external_editor_helper: tui_config.show_external_editor_helper,
        copy_on_mouse_selection_release: tui_config.copy_on_mouse_selection_release,
        swap_enter_and_send: tui_config.swap_enter_and_send,
        ctrl_c_clears_input: tui_config.ctrl_c_clears_input,
        acp_agent_servers: acp_agent_servers_from_config(runtime_config),
    }
}

fn acp_agent_servers_from_config(runtime_config: Option<&RuntimeConfig>) -> Vec<String> {
    let Some(runtime_config) = runtime_config else {
        return Vec::new();
    };
    if !runtime_config.enabled {
        return Vec::new();
    }

    runtime_config.agent_servers.keys().cloned().collect()
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
    use crate::appconfig::{
        AgentServerConfig, AgentServerType, RuntimeDistribution, RuntimeInstallRoot,
    };

    #[test]
    fn model_options_from_config_carries_mouse_selection_copy_flag() {
        let options = model_options_from_config(&TuiConfig {
            user_input_style: UserInputStyle::Cx,
            status_line: Vec::new(),
            external_editor: Vec::new(),
            show_external_editor_helper: true,
            copy_on_mouse_selection_release: true,
            swap_enter_and_send: false,
            ctrl_c_clears_input: true,
            print_transcript_on_exit: false,
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
            ctrl_c_clears_input: true,
            print_transcript_on_exit: false,
        });

        assert!(options.swap_enter_and_send);
    }

    #[test]
    fn model_options_from_config_carries_ctrl_c_clears_input_flag() {
        let options = model_options_from_config(&TuiConfig {
            user_input_style: UserInputStyle::Cx,
            status_line: Vec::new(),
            external_editor: Vec::new(),
            show_external_editor_helper: true,
            copy_on_mouse_selection_release: false,
            swap_enter_and_send: false,
            ctrl_c_clears_input: false,
            print_transcript_on_exit: false,
        });

        assert!(!options.ctrl_c_clears_input);
    }

    #[test]
    fn model_options_from_app_config_exposes_enabled_acp_servers() {
        let mut agent_servers = std::collections::BTreeMap::new();
        agent_servers.insert(
            "kimi".to_string(),
            AgentServerConfig {
                server_type: AgentServerType::Registry,
                agent: "kimi".to_string(),
                command: String::new(),
                args: Vec::new(),
                env: std::collections::BTreeMap::new(),
                default_model: None,
                default_mode: None,
            },
        );
        let config = Config {
            tui: default_tui_config(),
            runtime: RuntimeConfig {
                enabled: true,
                registry_url: "https://example.test/registry.json".to_string(),
                install_root: RuntimeInstallRoot::Config,
                custom_install_dir: std::path::PathBuf::new(),
                distribution_preference: vec![RuntimeDistribution::Binary],
                auto_update_check: true,
                agent_servers,
            },
        };

        let options = model_options_from_app_config(&config);

        assert_eq!(options.acp_agent_servers, vec!["kimi"]);
    }

    #[test]
    fn exit_replay_skips_writer_when_config_disables_transcript_printing() {
        let model = Model::new(HeroOptions::default());
        let config = TuiConfig {
            user_input_style: UserInputStyle::Cx,
            status_line: Vec::new(),
            external_editor: Vec::new(),
            show_external_editor_helper: true,
            copy_on_mouse_selection_release: false,
            swap_enter_and_send: false,
            ctrl_c_clears_input: true,
            print_transcript_on_exit: false,
        };

        write_terminal_replay_on_exit(&mut FailingWriter, &model, false, &config)
            .expect("disabled terminal replay should not touch the writer");
    }

    #[test]
    fn exit_replay_writes_when_config_enables_transcript_printing() {
        let model = Model::new(HeroOptions::default());
        let config = TuiConfig {
            user_input_style: UserInputStyle::Cx,
            status_line: Vec::new(),
            external_editor: Vec::new(),
            show_external_editor_helper: true,
            copy_on_mouse_selection_release: false,
            swap_enter_and_send: false,
            ctrl_c_clears_input: true,
            print_transcript_on_exit: true,
        };
        let mut output = Vec::new();

        write_terminal_replay_on_exit(&mut output, &model, false, &config)
            .expect("enabled terminal replay should write transcript output");

        assert!(!output.is_empty());
    }

    fn default_tui_config() -> TuiConfig {
        TuiConfig {
            user_input_style: UserInputStyle::Cx,
            status_line: Vec::new(),
            external_editor: Vec::new(),
            show_external_editor_helper: true,
            copy_on_mouse_selection_release: false,
            swap_enter_and_send: false,
            ctrl_c_clears_input: true,
            print_transcript_on_exit: false,
        }
    }

    struct FailingWriter;

    impl Write for FailingWriter {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("writer should not be called"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
}
