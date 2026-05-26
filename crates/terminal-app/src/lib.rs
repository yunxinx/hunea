use std::io::{self, IsTerminal, Write};

use app_config::appconfig::{
    self, Config, DebugConfig, ReasoningContentDisplay, RuntimeConfig, TuiConfig, UserInputStyle,
};
use color_eyre::eyre::{Result, WrapErr};
use conversation_runtime::models::{self as provider_models, LoadedModelCatalog};
use runtime_domain::{
    envinfo,
    phrases::{self, LoadedStatusPhrases},
};
use terminal_ui::{
    self, Model, ModelOptions, ReasoningDisplayMode, RuntimeRequestPolicy, StartupBannerOptions,
    StatusLineItem, StyleMode,
};

mod runtime;

use runtime::{AppRuntimeCoordinator, AppRuntimeOptions};
use tool_runtime::builtin::ManagedSearchToolConfig;

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
    let loaded_models = provider_models::load().wrap_err("failed to load model config")?;
    let loaded_phrases = phrases::load().wrap_err("failed to load phrase config")?;
    let mut runtime_coordinator = AppRuntimeCoordinator::new(AppRuntimeOptions {
        model_config_path: loaded_models.source_path.clone(),
        ..AppRuntimeOptions::default()
    });
    let model = terminal_ui::run_with_runtime_coordinator(
        StartupBannerOptions::default(),
        model_options_from_config_and_models(tui_config, &loaded_models, &loaded_phrases),
        &mut runtime_coordinator,
    )
    .wrap_err("failed to run tui application")?;
    write_terminal_replay_on_exit(writer, &model, preserve_ansi, tui_config)
}

/// `run_with_config_writer` 使用完整配置启动 TUI。
pub fn run_with_config_writer<W: Write>(
    writer: &mut W,
    preserve_ansi: bool,
    config: &Config,
) -> Result<()> {
    let loaded_models = provider_models::load().wrap_err("failed to load model config")?;
    let loaded_phrases = phrases::load().wrap_err("failed to load phrase config")?;
    let mut runtime_coordinator = AppRuntimeCoordinator::new(
        runtime_options_from_app_config_and_models(config, &loaded_models),
    );
    let model = terminal_ui::run_with_runtime_coordinator(
        StartupBannerOptions::default(),
        model_options_from_app_config_and_models(config, &loaded_models, &loaded_phrases),
        &mut runtime_coordinator,
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
fn runtime_options_from_app_config(config: &Config) -> AppRuntimeOptions {
    runtime_options_from_app_config_and_models(config, &LoadedModelCatalog::default())
}

fn model_options_from_config_and_models(
    tui_config: &TuiConfig,
    loaded_models: &LoadedModelCatalog,
    loaded_phrases: &LoadedStatusPhrases,
) -> ModelOptions {
    model_options_from_configs(tui_config, None, loaded_models, loaded_phrases)
}

fn model_options_from_app_config_and_models(
    config: &Config,
    loaded_models: &LoadedModelCatalog,
    loaded_phrases: &LoadedStatusPhrases,
) -> ModelOptions {
    model_options_from_configs(
        &config.tui,
        Some(&config.debug),
        loaded_models,
        loaded_phrases,
    )
}

fn runtime_options_from_app_config_and_models(
    config: &Config,
    loaded_models: &LoadedModelCatalog,
) -> AppRuntimeOptions {
    AppRuntimeOptions {
        model_config_path: loaded_models.source_path.clone(),
        runtime_request_policy: runtime_request_policy_from_config(&config.runtime),
        managed_search_tools: managed_search_tools_from_config(&config.runtime),
        managed_search_authorization_config_path: appconfig::user_config_file_path(),
    }
}

fn runtime_request_policy_from_config(config: &RuntimeConfig) -> RuntimeRequestPolicy {
    RuntimeRequestPolicy::new(
        config.request_retry_attempts,
        config.request_retry_delays.clone(),
        config.request_timeout_seconds,
    )
    .with_tool_max_turns(config.tool_max_turns)
}

fn managed_search_tools_from_config(config: &RuntimeConfig) -> ManagedSearchToolConfig {
    ManagedSearchToolConfig {
        allow_managed_rg: config.allow_managed_rg,
        allow_managed_fd: config.allow_managed_fd,
    }
}

fn model_options_from_configs(
    tui_config: &TuiConfig,
    debug_config: Option<&DebugConfig>,
    loaded_models: &LoadedModelCatalog,
    loaded_phrases: &LoadedStatusPhrases,
) -> ModelOptions {
    ModelOptions {
        style_mode: style_mode_from_config(tui_config.user_input_style),
        status_line_items: status_line_items_from_config(&tui_config.status_line),
        status_line_2_items: status_line_items_from_config(&tui_config.status_line_2),
        external_editor: tui_config.external_editor.clone(),
        external_editor_hint: external_editor_hint_from_config(&tui_config.external_editor),
        show_external_editor_helper: tui_config.show_external_editor_helper,
        copy_on_mouse_selection_release: tui_config.copy_on_mouse_selection_release,
        swap_enter_and_send: tui_config.swap_enter_and_send,
        ctrl_c_clears_input: tui_config.ctrl_c_clears_input,
        esc_interrupt_presses: tui_config.esc_interrupt_presses,
        show_esc_interrupt_hint: tui_config.show_esc_interrupt_hint,
        file_picker_popup_height: tui_config.file_picker_popup_height,
        show_reasoning_content: tui_config.show_reasoning_content,
        reasoning_display_mode: reasoning_display_mode_from_config(
            tui_config.reasoning_content_display,
        ),
        debug_commands_enabled: debug_config.is_some_and(|config| config.enabled),
        model_catalog: loaded_models.catalog.clone(),
        selected_model: loaded_models.selected_model.clone(),
        requires_model_selection: loaded_models.requires_model_selection,
        status_phrases: loaded_phrases.phrases.clone(),
        status_phrase_order: loaded_phrases.order,
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
            status_line_2: Vec::new(),
            external_editor: Vec::new(),
            show_external_editor_helper: true,
            copy_on_mouse_selection_release: true,
            swap_enter_and_send: false,
            ctrl_c_clears_input: true,
            esc_interrupt_presses: 2,
            show_esc_interrupt_hint: true,
            file_picker_popup_height: 7,
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
    fn model_options_from_config_carries_request_metric_status_line_items() {
        let options = model_options_from_config(&TuiConfig {
            status_line: vec!["throughput".to_string(), "latency".to_string()],
            ..default_tui_config()
        });

        assert_eq!(
            options.status_line_items,
            vec![StatusLineItem::Throughput, StatusLineItem::Latency]
        );
    }

    #[test]
    fn model_options_from_config_carries_file_picker_popup_height() {
        let options = model_options_from_config(&TuiConfig {
            file_picker_popup_height: 5,
            ..default_tui_config()
        });

        assert_eq!(options.file_picker_popup_height, 5);
    }

    #[test]
    fn model_options_from_config_carries_second_status_line_items() {
        let options = model_options_from_config(&TuiConfig {
            status_line_2: vec!["current-dir".to_string(), "git-branch".to_string()],
            ..default_tui_config()
        });

        assert_eq!(
            options.status_line_2_items,
            vec![StatusLineItem::CurrentDir, StatusLineItem::GitBranch]
        );
    }

    #[test]
    fn model_options_from_config_carries_swap_enter_and_send_flag() {
        let options = model_options_from_config(&TuiConfig {
            user_input_style: UserInputStyle::Cx,
            status_line: Vec::new(),
            status_line_2: Vec::new(),
            external_editor: Vec::new(),
            show_external_editor_helper: true,
            copy_on_mouse_selection_release: false,
            swap_enter_and_send: true,
            ctrl_c_clears_input: true,
            esc_interrupt_presses: 2,
            show_esc_interrupt_hint: true,
            file_picker_popup_height: 7,
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
            status_line_2: Vec::new(),
            external_editor: Vec::new(),
            show_external_editor_helper: true,
            copy_on_mouse_selection_release: false,
            swap_enter_and_send: false,
            ctrl_c_clears_input: false,
            esc_interrupt_presses: 2,
            show_esc_interrupt_hint: true,
            file_picker_popup_height: 7,
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
            status_line_2: Vec::new(),
            external_editor: Vec::new(),
            show_external_editor_helper: true,
            copy_on_mouse_selection_release: false,
            swap_enter_and_send: false,
            ctrl_c_clears_input: true,
            esc_interrupt_presses: 3,
            show_esc_interrupt_hint: true,
            file_picker_popup_height: 7,
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
            status_line_2: Vec::new(),
            external_editor: Vec::new(),
            show_external_editor_helper: true,
            copy_on_mouse_selection_release: false,
            swap_enter_and_send: false,
            ctrl_c_clears_input: true,
            esc_interrupt_presses: 2,
            show_esc_interrupt_hint: false,
            file_picker_popup_height: 7,
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
            status_line_2: Vec::new(),
            external_editor: Vec::new(),
            show_external_editor_helper: true,
            copy_on_mouse_selection_release: false,
            swap_enter_and_send: false,
            ctrl_c_clears_input: true,
            esc_interrupt_presses: 2,
            show_esc_interrupt_hint: true,
            file_picker_popup_height: 7,
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
            status_line_2: Vec::new(),
            external_editor: Vec::new(),
            show_external_editor_helper: true,
            copy_on_mouse_selection_release: false,
            swap_enter_and_send: false,
            ctrl_c_clears_input: true,
            esc_interrupt_presses: 2,
            show_esc_interrupt_hint: true,
            file_picker_popup_height: 7,
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
        });

        assert!(options.debug_commands_enabled);
    }

    #[test]
    fn runtime_options_from_app_config_carries_runtime_request_policy() {
        let config = Config {
            tui: default_tui_config(),
            runtime: RuntimeConfig {
                request_retry_attempts: 4,
                request_retry_delays: vec![1, 3, 3, 3],
                request_timeout_seconds: 240,
                tool_max_turns: Some(11),
                allow_managed_rg: Some(true),
                allow_managed_fd: Some(false),
            },
            debug: DebugConfig { enabled: false },
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
        assert_eq!(options.runtime_request_policy.tool_max_turns(), Some(11));
        assert_eq!(options.managed_search_tools.allow_managed_rg, Some(true));
        assert_eq!(options.managed_search_tools.allow_managed_fd, Some(false));
    }

    #[test]
    fn exit_replay_skips_writer_when_config_disables_transcript_printing() {
        let model = Model::new(StartupBannerOptions::default());
        let config = TuiConfig {
            user_input_style: UserInputStyle::Cx,
            status_line: Vec::new(),
            status_line_2: Vec::new(),
            external_editor: Vec::new(),
            show_external_editor_helper: true,
            copy_on_mouse_selection_release: false,
            swap_enter_and_send: false,
            ctrl_c_clears_input: true,
            esc_interrupt_presses: 2,
            show_esc_interrupt_hint: true,
            file_picker_popup_height: 7,
            print_transcript_on_exit: false,
            show_reasoning_content: false,
            reasoning_content_display: ReasoningContentDisplay::Collapsed,
        };

        write_terminal_replay_on_exit(&mut FailingWriter, &model, false, &config)
            .expect("disabled terminal replay should not touch the writer");
    }

    #[test]
    fn exit_replay_writes_when_config_enables_transcript_printing() {
        let model = Model::new(StartupBannerOptions::default());
        let config = TuiConfig {
            user_input_style: UserInputStyle::Cx,
            status_line: Vec::new(),
            status_line_2: Vec::new(),
            external_editor: Vec::new(),
            show_external_editor_helper: true,
            copy_on_mouse_selection_release: false,
            swap_enter_and_send: false,
            ctrl_c_clears_input: true,
            esc_interrupt_presses: 2,
            show_esc_interrupt_hint: true,
            file_picker_popup_height: 7,
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
            status_line_2: Vec::new(),
            external_editor: Vec::new(),
            show_external_editor_helper: true,
            copy_on_mouse_selection_release: false,
            swap_enter_and_send: false,
            ctrl_c_clears_input: true,
            esc_interrupt_presses: 2,
            show_esc_interrupt_hint: true,
            file_picker_popup_height: 7,
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
            tool_max_turns: None,
            allow_managed_rg: None,
            allow_managed_fd: None,
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
