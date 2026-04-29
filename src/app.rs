use std::io::{self, IsTerminal, Write};

use color_eyre::eyre::{Result, WrapErr};

use crate::{
    appconfig::{
        self, AcpConfig, Config, DebugConfig, ReasoningContentDisplay, RuntimeConfig, TuiConfig,
        UserInputStyle,
    },
    envinfo,
    frontend::tui::{
        self, HeroOptions, Model, ModelOptions, ReasoningDisplayMode, RuntimeOptions,
        RuntimeRequestPolicy, StatusLineItem, StyleMode,
    },
    runtime::{
        acp::AcpSessionCatalog,
        models::{self, LoadedModelCatalog},
        phrases::{self, LoadedStatusPhrases},
    },
};

/// `AppRunError` 区分用户配置错误与运行期错误，便于 CLI 使用不同输出策略。
#[derive(Debug)]
pub enum AppRunError {
    Config(appconfig::AppConfigError),
    Runtime(color_eyre::Report),
}

/// `run` 负责组装并启动交互式 TUI 应用。
pub fn run() -> Result<()> {
    let config = appconfig::load().wrap_err("failed to load app config")?;
    run_loaded_config(&config)
}

/// `run_for_cli` 为二进制入口保留配置错误的类型信息。
pub fn run_for_cli() -> std::result::Result<(), AppRunError> {
    let config = appconfig::load().map_err(AppRunError::Config)?;
    run_loaded_config(&config).map_err(AppRunError::Runtime)
}

fn run_loaded_config(config: &Config) -> Result<()> {
    let stdout = io::stdout();
    let preserve_ansi = stdout.is_terminal();
    let mut handle = stdout.lock();
    run_with_config_writer(&mut handle, preserve_ansi, config)
}

/// `run_with_writer` 允许调用方注入退出 AltScreen 后的 terminal replay 输出目标。
pub fn run_with_writer<W: Write>(
    writer: &mut W,
    preserve_ansi: bool,
    tui_config: &TuiConfig,
) -> Result<()> {
    let loaded_models = models::load().wrap_err("failed to load model config")?;
    let loaded_phrases = phrases::load().wrap_err("failed to load phrase config")?;
    let model = tui::run_with_runtime_options(
        HeroOptions::default(),
        model_options_from_config_and_models(tui_config, &loaded_models, &loaded_phrases),
        RuntimeOptions {
            model_config_path: loaded_models.source_path.clone(),
            ..RuntimeOptions::default()
        },
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
    let loaded_models = models::load().wrap_err("failed to load model config")?;
    let loaded_phrases = phrases::load().wrap_err("failed to load phrase config")?;
    let model = tui::run_with_runtime_options(
        HeroOptions::default(),
        model_options_from_app_config_and_models(config, &loaded_models, &loaded_phrases),
        runtime_options_from_app_config_and_models(config, &loaded_models),
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

fn reasoning_display_mode_from_config(display: ReasoningContentDisplay) -> ReasoningDisplayMode {
    match display {
        ReasoningContentDisplay::Collapsed => ReasoningDisplayMode::Collapsed,
        ReasoningContentDisplay::Expanded => ReasoningDisplayMode::Expanded,
        ReasoningContentDisplay::Snippet => ReasoningDisplayMode::Snippet,
    }
}

#[cfg(test)]
fn model_options_from_config(tui_config: &TuiConfig) -> ModelOptions {
    model_options_from_config_and_models(
        tui_config,
        &LoadedModelCatalog::default(),
        &LoadedStatusPhrases::default(),
    )
}

#[cfg(test)]
fn model_options_from_app_config(config: &Config) -> ModelOptions {
    model_options_from_app_config_and_models(
        config,
        &LoadedModelCatalog::default(),
        &LoadedStatusPhrases::default(),
    )
}

#[cfg(test)]
fn runtime_options_from_app_config(config: &Config) -> RuntimeOptions {
    runtime_options_from_app_config_and_models(config, &LoadedModelCatalog::default())
}

fn model_options_from_config_and_models(
    tui_config: &TuiConfig,
    loaded_models: &LoadedModelCatalog,
    loaded_phrases: &LoadedStatusPhrases,
) -> ModelOptions {
    model_options_from_configs(tui_config, None, None, loaded_models, loaded_phrases)
}

fn model_options_from_app_config_and_models(
    config: &Config,
    loaded_models: &LoadedModelCatalog,
    loaded_phrases: &LoadedStatusPhrases,
) -> ModelOptions {
    model_options_from_configs(
        &config.tui,
        Some(&config.debug),
        Some(&config.acp),
        loaded_models,
        loaded_phrases,
    )
}

fn runtime_options_from_app_config_and_models(
    config: &Config,
    loaded_models: &LoadedModelCatalog,
) -> RuntimeOptions {
    RuntimeOptions {
        acp_sessions: AcpSessionCatalog::from_acp_config(&config.acp),
        model_config_path: loaded_models.source_path.clone(),
        runtime_request_policy: runtime_request_policy_from_config(&config.runtime),
    }
}

fn runtime_request_policy_from_config(config: &RuntimeConfig) -> RuntimeRequestPolicy {
    RuntimeRequestPolicy::new(
        config.request_retry_attempts,
        config.request_retry_delays.clone(),
        config.request_timeout_seconds,
    )
}

fn model_options_from_configs(
    tui_config: &TuiConfig,
    debug_config: Option<&DebugConfig>,
    acp_config: Option<&AcpConfig>,
    loaded_models: &LoadedModelCatalog,
    loaded_phrases: &LoadedStatusPhrases,
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
        esc_interrupt_presses: tui_config.esc_interrupt_presses,
        show_esc_interrupt_hint: tui_config.show_esc_interrupt_hint,
        show_reasoning_content: tui_config.show_reasoning_content,
        reasoning_display_mode: reasoning_display_mode_from_config(
            tui_config.reasoning_content_display,
        ),
        debug_commands_enabled: debug_config.is_some_and(|config| config.enabled),
        acp_agent_servers: acp_agent_servers_from_config(acp_config),
        model_catalog: loaded_models.catalog.clone(),
        selected_model: loaded_models.selected_model.clone(),
        requires_model_selection: loaded_models.requires_model_selection,
        status_phrases: loaded_phrases.phrases.clone(),
        status_phrase_order: loaded_phrases.order,
    }
}

fn acp_agent_servers_from_config(acp_config: Option<&AcpConfig>) -> Vec<String> {
    let Some(acp_config) = acp_config else {
        return Vec::new();
    };
    if !acp_config.enabled {
        return Vec::new();
    }

    acp_config.agent_servers.keys().cloned().collect()
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
    use crate::appconfig::{AcpDistribution, AcpInstallRoot, AgentServerConfig, AgentServerType};

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
            esc_interrupt_presses: 2,
            show_esc_interrupt_hint: true,
            print_transcript_on_exit: false,
            show_reasoning_content: false,
            reasoning_content_display: ReasoningContentDisplay::Collapsed,
        });

        assert!(options.copy_on_mouse_selection_release);
    }

    #[test]
    fn model_options_from_config_carries_current_model_status_line() {
        let options = model_options_from_config(&TuiConfig {
            status_line: vec!["current-model".to_string()],
            ..default_tui_config()
        });

        assert_eq!(
            options.status_line_items,
            vec![StatusLineItem::CurrentModel]
        );
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
            esc_interrupt_presses: 2,
            show_esc_interrupt_hint: true,
            print_transcript_on_exit: false,
            show_reasoning_content: false,
            reasoning_content_display: ReasoningContentDisplay::Collapsed,
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
            esc_interrupt_presses: 2,
            show_esc_interrupt_hint: true,
            print_transcript_on_exit: false,
            show_reasoning_content: false,
            reasoning_content_display: ReasoningContentDisplay::Collapsed,
        });

        assert!(!options.ctrl_c_clears_input);
    }

    #[test]
    fn model_options_from_config_carries_esc_interrupt_presses() {
        let options = model_options_from_config(&TuiConfig {
            user_input_style: UserInputStyle::Cx,
            status_line: Vec::new(),
            external_editor: Vec::new(),
            show_external_editor_helper: true,
            copy_on_mouse_selection_release: false,
            swap_enter_and_send: false,
            ctrl_c_clears_input: true,
            esc_interrupt_presses: 3,
            show_esc_interrupt_hint: true,
            print_transcript_on_exit: false,
            show_reasoning_content: false,
            reasoning_content_display: ReasoningContentDisplay::Collapsed,
        });

        assert_eq!(options.esc_interrupt_presses, 3);
    }

    #[test]
    fn model_options_from_config_carries_show_esc_interrupt_hint_flag() {
        let options = model_options_from_config(&TuiConfig {
            user_input_style: UserInputStyle::Cx,
            status_line: Vec::new(),
            external_editor: Vec::new(),
            show_external_editor_helper: true,
            copy_on_mouse_selection_release: false,
            swap_enter_and_send: false,
            ctrl_c_clears_input: true,
            esc_interrupt_presses: 2,
            show_esc_interrupt_hint: false,
            print_transcript_on_exit: false,
            show_reasoning_content: false,
            reasoning_content_display: ReasoningContentDisplay::Collapsed,
        });

        assert!(!options.show_esc_interrupt_hint);
    }

    #[test]
    fn model_options_from_config_carries_show_reasoning_content_flag() {
        let options = model_options_from_config(&TuiConfig {
            user_input_style: UserInputStyle::Cx,
            status_line: Vec::new(),
            external_editor: Vec::new(),
            show_external_editor_helper: true,
            copy_on_mouse_selection_release: false,
            swap_enter_and_send: false,
            ctrl_c_clears_input: true,
            esc_interrupt_presses: 2,
            show_esc_interrupt_hint: true,
            print_transcript_on_exit: false,
            show_reasoning_content: true,
            reasoning_content_display: ReasoningContentDisplay::Collapsed,
        });

        assert!(options.show_reasoning_content);
    }

    #[test]
    fn model_options_from_config_carries_reasoning_display_mode() {
        let options = model_options_from_config(&TuiConfig {
            user_input_style: UserInputStyle::Cx,
            status_line: Vec::new(),
            external_editor: Vec::new(),
            show_external_editor_helper: true,
            copy_on_mouse_selection_release: false,
            swap_enter_and_send: false,
            ctrl_c_clears_input: true,
            esc_interrupt_presses: 2,
            show_esc_interrupt_hint: true,
            print_transcript_on_exit: false,
            show_reasoning_content: true,
            reasoning_content_display: ReasoningContentDisplay::Expanded,
        });

        assert_eq!(
            options.reasoning_display_mode,
            ReasoningDisplayMode::Expanded
        );
    }

    #[test]
    fn model_options_from_config_carries_snippet_reasoning_display_mode() {
        let options = model_options_from_config(&TuiConfig {
            reasoning_content_display: ReasoningContentDisplay::Snippet,
            ..default_tui_config()
        });

        assert_eq!(
            options.reasoning_display_mode,
            ReasoningDisplayMode::Snippet
        );
    }

    #[test]
    fn model_options_from_config_carries_debug_command_flag() {
        let options = model_options_from_app_config(&Config {
            tui: default_tui_config(),
            runtime: default_runtime_config(),
            debug: DebugConfig { enabled: true },
            acp: AcpConfig {
                enabled: false,
                registry_url: String::new(),
                install_root: AcpInstallRoot::Config,
                custom_install_dir: std::path::PathBuf::new(),
                distribution_preference: vec![AcpDistribution::Binary],
                auto_update_check: true,
                agent_servers: std::collections::BTreeMap::new(),
            },
        });

        assert!(options.debug_commands_enabled);
    }

    #[test]
    fn acp_options_from_app_config_exposes_direct_acp_commands() {
        let mut agent_servers = std::collections::BTreeMap::new();
        agent_servers.insert(
            "local-kimi".to_string(),
            AgentServerConfig {
                server_type: AgentServerType::Custom,
                agent: String::new(),
                command: "kimi".to_string(),
                args: vec!["acp".to_string()],
                env: std::collections::BTreeMap::new(),
                default_model: None,
                default_mode: None,
            },
        );
        let config = Config {
            tui: default_tui_config(),
            runtime: default_runtime_config(),
            debug: DebugConfig { enabled: false },
            acp: AcpConfig {
                enabled: true,
                registry_url: "https://example.test/registry.json".to_string(),
                install_root: AcpInstallRoot::Config,
                custom_install_dir: std::path::PathBuf::new(),
                distribution_preference: vec![AcpDistribution::Binary],
                auto_update_check: true,
                agent_servers,
            },
        };

        let options = runtime_options_from_app_config(&config);
        let command = options
            .acp_sessions
            .command("local-kimi")
            .expect("local-kimi should be directly launchable");

        assert_eq!(command.command, "kimi");
        assert_eq!(command.args, vec!["acp"]);
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
            runtime: default_runtime_config(),
            debug: DebugConfig { enabled: false },
            acp: AcpConfig {
                enabled: true,
                registry_url: "https://example.test/registry.json".to_string(),
                install_root: AcpInstallRoot::Config,
                custom_install_dir: std::path::PathBuf::new(),
                distribution_preference: vec![AcpDistribution::Binary],
                auto_update_check: true,
                agent_servers,
            },
        };

        let options = model_options_from_app_config(&config);

        assert_eq!(options.acp_agent_servers, vec!["kimi"]);
    }

    #[test]
    fn runtime_options_from_app_config_carries_runtime_request_policy() {
        let config = Config {
            tui: default_tui_config(),
            runtime: RuntimeConfig {
                request_retry_attempts: 4,
                request_retry_delays: vec![1, 3, 3, 3],
                request_timeout_seconds: 240,
            },
            debug: DebugConfig { enabled: false },
            acp: AcpConfig {
                enabled: false,
                registry_url: String::new(),
                install_root: AcpInstallRoot::Config,
                custom_install_dir: std::path::PathBuf::new(),
                distribution_preference: vec![AcpDistribution::Binary],
                auto_update_check: true,
                agent_servers: std::collections::BTreeMap::new(),
            },
        };

        let options = runtime_options_from_app_config(&config);

        assert_eq!(options.runtime_request_policy.attempts(), 4);
        assert_eq!(
            options.runtime_request_policy.delay_for_retry(2),
            std::time::Duration::from_secs(3)
        );
        assert_eq!(
            options.runtime_request_policy.timeout(),
            std::time::Duration::from_secs(240)
        );
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
            esc_interrupt_presses: 2,
            show_esc_interrupt_hint: true,
            print_transcript_on_exit: false,
            show_reasoning_content: false,
            reasoning_content_display: ReasoningContentDisplay::Collapsed,
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
            esc_interrupt_presses: 2,
            show_esc_interrupt_hint: true,
            print_transcript_on_exit: true,
            show_reasoning_content: false,
            reasoning_content_display: ReasoningContentDisplay::Collapsed,
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
            esc_interrupt_presses: 2,
            show_esc_interrupt_hint: true,
            print_transcript_on_exit: false,
            show_reasoning_content: false,
            reasoning_content_display: ReasoningContentDisplay::Collapsed,
        }
    }

    fn default_runtime_config() -> RuntimeConfig {
        RuntimeConfig {
            request_retry_attempts: 3,
            request_retry_delays: vec![1, 2, 3],
            request_timeout_seconds: 120,
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
