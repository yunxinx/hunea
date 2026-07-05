use std::{
    io::{self, IsTerminal, Write},
    sync::Arc,
};

use app_config::appconfig::{self, Config, TuiConfig};
use color_eyre::eyre::{Result, WrapErr};
use conversation_runtime::models as provider_models;
use runtime_domain::{envinfo, phrases};
use session_store::{LocalSessionStore, SessionHeader, SessionId, SessionStore};
use terminal_ui::{self, StartupBannerOptions};

mod dynamic_environment;
mod options_mapping;
mod prompt_assembly;
mod replay;
mod runtime;
mod session_store_bridge;

use options_mapping::{
    model_options_from_app_config_and_models, model_options_from_config_and_models,
    runtime_options_from_app_config_and_models,
};
use prompt_assembly::{
    dynamic_environment_session_config_from_manager, load_initial_prompt_assembly,
};
use replay::write_terminal_replay_on_exit;
pub use replay::{
    write_terminal_replay, write_terminal_replay_preserving_ansi,
    write_terminal_replay_with_context,
};
use runtime::{AppRuntimeCoordinator, AppRuntimeOptions, tool_definitions_for_managed_search};

#[cfg(test)]
use app_config::appconfig::{
    BRANCH_PICKER_LIST_ROWS_DEFAULT, DebugConfig, EscRewindMode, ReasoningContentDisplay,
    RuntimeConfig, UserInputStyle,
};
#[cfg(test)]
use options_mapping::{
    model_options_from_app_config, model_options_from_config, runtime_options_from_app_config,
};
#[cfg(test)]
use terminal_ui::{Model, ReasoningDisplayMode, StatusLineItem};

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
    let mut model_options =
        model_options_from_config_and_models(tui_config, &loaded_models, &loaded_phrases);
    let mut runtime_options = AppRuntimeOptions {
        loaded_models: loaded_models.clone(),
        ..AppRuntimeOptions::default()
    };
    attach_default_session_persistence(&mut runtime_options, &mut model_options, &loaded_models)
        .wrap_err("failed to initialize session persistence")?;
    let mut runtime_coordinator = AppRuntimeCoordinator::new(runtime_options)
        .map_err(color_eyre::eyre::Report::msg)
        .wrap_err("failed to initialize app runtime coordinator")?;
    let model = terminal_ui::run_with_runtime_coordinator(
        StartupBannerOptions::default(),
        model_options,
        &mut runtime_coordinator,
    )
    .wrap_err("failed to run tui application")?;
    runtime_coordinator
        .shutdown()
        .map_err(color_eyre::eyre::Report::msg)
        .wrap_err("failed to flush session persistence")?;
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
    let mut model_options =
        model_options_from_app_config_and_models(config, &loaded_models, &loaded_phrases);
    let mut runtime_options = runtime_options_from_app_config_and_models(config, &loaded_models);
    attach_default_session_persistence(&mut runtime_options, &mut model_options, &loaded_models)
        .wrap_err("failed to initialize session persistence")?;
    let mut runtime_coordinator = AppRuntimeCoordinator::new(runtime_options)
        .map_err(color_eyre::eyre::Report::msg)
        .wrap_err("failed to initialize app runtime coordinator")?;
    let model = terminal_ui::run_with_runtime_coordinator(
        StartupBannerOptions::default(),
        model_options,
        &mut runtime_coordinator,
    )
    .wrap_err("failed to run tui application")?;
    runtime_coordinator
        .shutdown()
        .map_err(color_eyre::eyre::Report::msg)
        .wrap_err("failed to flush session persistence")?;
    write_terminal_replay_on_exit(writer, &model, preserve_ansi, &config.tui)
}

fn attach_default_session_persistence(
    options: &mut AppRuntimeOptions,
    model_options: &mut terminal_ui::ModelOptions,
    loaded_models: &provider_models::LoadedModelCatalog,
) -> Result<()> {
    let store = open_local_session_store()?;
    let work_dir = std::env::current_dir().wrap_err("resolve current working directory")?;
    let git_head = envinfo::git_head();
    let initial_model = loaded_models
        .selected_model
        .as_ref()
        .map(|selection| selection.model_id.clone())
        .unwrap_or_default();

    let session_header = SessionHeader {
        session_id: SessionId::new(),
        work_dir: work_dir.clone(),
        session_name: None,
        initial_model,
        git_head,
        cli_version: Some(env!("CARGO_PKG_VERSION").to_string()),
    };
    options.session_store = Some(Arc::clone(&store));
    options.session_header_template = Some(session_header);
    let tool_definitions = tool_definitions_for_managed_search(&options.managed_search_tools);
    let loaded_prompt_assembly =
        load_initial_prompt_assembly(store, work_dir.as_path(), &tool_definitions)?;
    model_options.prompt_assembly = Some(loaded_prompt_assembly.clone());
    options.initial_prompt_prelude = Some(loaded_prompt_assembly.prelude.clone());
    options.initial_dynamic_environment_session_config = Some(
        dynamic_environment_session_config_from_manager(&loaded_prompt_assembly),
    );
    Ok(())
}

fn open_local_session_store() -> Result<Arc<dyn SessionStore>> {
    let store = session_store_bridge::run_session_store_future(
        LocalSessionStore::open,
        "start session store runtime",
    )?
    .wrap_err("open local session store")?;
    Ok(Arc::new(store))
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
            branch_picker_list_rows: BRANCH_PICKER_LIST_ROWS_DEFAULT,
            composer_undo_limit: 50,
            message_history_limit: 100,
            print_transcript_on_exit: false,
            show_reasoning_content: false,
            reasoning_content_display: ReasoningContentDisplay::Collapsed,
            esc_rewind_mode: EscRewindMode::Coarse,
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
    fn model_options_from_config_carries_branch_picker_list_rows() {
        let options = model_options_from_config(&TuiConfig {
            branch_picker_list_rows: 9,
            ..default_tui_config()
        });

        assert_eq!(options.branch_picker_list_rows, 9);
    }

    #[test]
    fn model_options_from_config_carries_composer_undo_limit() {
        let options = model_options_from_config(&TuiConfig {
            composer_undo_limit: 80,
            ..default_tui_config()
        });

        assert_eq!(options.composer_undo_limit, 80);
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
            branch_picker_list_rows: 7,
            composer_undo_limit: 50,
            message_history_limit: 100,
            print_transcript_on_exit: false,
            show_reasoning_content: false,
            reasoning_content_display: ReasoningContentDisplay::Collapsed,
            esc_rewind_mode: EscRewindMode::Coarse,
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
            branch_picker_list_rows: 7,
            composer_undo_limit: 50,
            message_history_limit: 100,
            print_transcript_on_exit: false,
            show_reasoning_content: false,
            reasoning_content_display: ReasoningContentDisplay::Collapsed,
            esc_rewind_mode: EscRewindMode::Coarse,
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
            branch_picker_list_rows: 7,
            composer_undo_limit: 50,
            message_history_limit: 100,
            print_transcript_on_exit: false,
            show_reasoning_content: false,
            reasoning_content_display: ReasoningContentDisplay::Collapsed,
            esc_rewind_mode: EscRewindMode::Coarse,
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
            branch_picker_list_rows: 7,
            composer_undo_limit: 50,
            message_history_limit: 100,
            print_transcript_on_exit: false,
            show_reasoning_content: false,
            reasoning_content_display: ReasoningContentDisplay::Collapsed,
            esc_rewind_mode: EscRewindMode::Coarse,
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
            branch_picker_list_rows: 7,
            composer_undo_limit: 50,
            message_history_limit: 100,
            print_transcript_on_exit: false,
            show_reasoning_content: true,
            reasoning_content_display: ReasoningContentDisplay::Collapsed,
            esc_rewind_mode: EscRewindMode::Coarse,
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
            branch_picker_list_rows: 7,
            composer_undo_limit: 50,
            message_history_limit: 100,
            print_transcript_on_exit: false,
            show_reasoning_content: true,
            reasoning_content_display: ReasoningContentDisplay::Expanded,
            esc_rewind_mode: EscRewindMode::Coarse,
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
    fn model_options_from_config_carries_expanded_simplified_reasoning_display_mode() {
        let options = model_options_from_config(&TuiConfig {
            reasoning_content_display: ReasoningContentDisplay::ExpandedSimplified,
            ..default_tui_config()
        });

        assert_eq!(
            options.reasoning_display_mode,
            ReasoningDisplayMode::ExpandedSimplified
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
            branch_picker_list_rows: 7,
            composer_undo_limit: 50,
            message_history_limit: 100,
            print_transcript_on_exit: false,
            show_reasoning_content: false,
            reasoning_content_display: ReasoningContentDisplay::Collapsed,
            esc_rewind_mode: EscRewindMode::Coarse,
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
            branch_picker_list_rows: 7,
            composer_undo_limit: 50,
            message_history_limit: 100,
            print_transcript_on_exit: true,
            show_reasoning_content: false,
            reasoning_content_display: ReasoningContentDisplay::Collapsed,
            esc_rewind_mode: EscRewindMode::Coarse,
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
            branch_picker_list_rows: 7,
            composer_undo_limit: 50,
            message_history_limit: 100,
            print_transcript_on_exit: false,
            show_reasoning_content: false,
            reasoning_content_display: ReasoningContentDisplay::Collapsed,
            esc_rewind_mode: EscRewindMode::Coarse,
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
