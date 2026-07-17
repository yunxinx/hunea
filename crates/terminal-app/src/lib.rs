use std::{
    io::{self, IsTerminal, Write},
    sync::Arc,
};

use app_config::appconfig::{self, Config, TuiConfig};
use color_eyre::eyre::{Result, WrapErr};
use conversation_runtime::models as provider_models;
use runtime_domain::{envinfo, paths::DataDirResolution, phrases};
use session_store::{LocalSessionStore, SessionHeader, SessionId, SessionStore};
use terminal_ui::{self, StartupBannerOptions};

pub use terminal_ui::install_terminal_panic_hook;

mod dynamic_environment;
mod options_mapping;
mod precheck;
mod prompt_assembly;
mod replay;
mod runtime;
mod session_store_bridge;

use options_mapping::{
    model_options_from_app_config_and_models, model_options_from_config_and_models,
    runtime_options_from_app_config_and_models,
};
use prompt_assembly::{PromptAssemblyWorkspace, dynamic_environment_session_config_from_manager};
use replay::write_terminal_replay_on_exit;
pub use replay::{
    write_terminal_replay, write_terminal_replay_preserving_ansi,
    write_terminal_replay_with_context,
};
use runtime::{AppRuntimeCoordinator, AppRuntimeOptions, tool_definitions_for_managed_search};

#[cfg(test)]
use app_config::appconfig::{
    BRANCH_PICKER_LIST_ROWS_DEFAULT, DebugConfig, EscRewindMode, KeyboardEnhancementMode,
    ReasoningContentDisplay, RuntimeConfig, UserInputStyle,
};
#[cfg(test)]
use options_mapping::{
    model_options_from_app_config, model_options_from_config, runtime_options_from_app_config,
};
#[cfg(test)]
use terminal_ui::{KeyboardEnhancementPreference, Model, ReasoningDisplayMode, StatusLineItem};

/// `AppRunError` 区分用户配置错误与运行期错误，便于 CLI 使用不同输出策略。
#[derive(Debug)]
pub enum AppRunError {
    Config(appconfig::AppConfigError),
    Runtime(color_eyre::Report),
}

/// `run` 负责组装并启动交互式 TUI 应用。
///
/// 启动顺序：`precheck`（目录可访问性 / 便携模式）→ `load_with_resolution`
/// （文件级 merge，Read 错误降级为 warning）→ 主 TUI。
/// `working_dir` 来自预检结果，避免启动链上再次 `current_dir()` 产生分叉语义。
pub fn run() -> Result<()> {
    let precheck_result = precheck::run()?;
    if precheck_result.should_exit {
        return Ok(());
    }
    let (mut config, warnings) = appconfig::load_with_resolution(
        precheck_result.working_dir.as_deref(),
        &precheck_result.data_dir_resolution,
    )
    .wrap_err("failed to load app config")?;
    for warning in &warnings {
        eprintln!("warning: {warning}");
    }
    // 磁盘已由 step write-through；此处只同步内存 Config。
    precheck::sync_managed_search_outcomes_to_config(
        &precheck_result.managed_search_outcomes,
        &mut config,
    );
    run_loaded_config(
        &config,
        &precheck_result.data_dir_resolution,
        precheck_result.working_dir.as_deref(),
    )
}

/// `run_for_cli` 为二进制入口保留配置错误的类型信息。
///
/// 预检错误归入 `Runtime`，配置 decode/validation 错误归入 `Config`。
pub fn run_for_cli() -> std::result::Result<(), AppRunError> {
    let precheck_result = precheck::run().map_err(AppRunError::Runtime)?;
    if precheck_result.should_exit {
        return Ok(());
    }
    let (mut config, warnings) = appconfig::load_with_resolution(
        precheck_result.working_dir.as_deref(),
        &precheck_result.data_dir_resolution,
    )
    .map_err(AppRunError::Config)?;
    for warning in &warnings {
        eprintln!("warning: {warning}");
    }
    // 磁盘已由 step write-through；此处只同步内存 Config。
    precheck::sync_managed_search_outcomes_to_config(
        &precheck_result.managed_search_outcomes,
        &mut config,
    );
    run_loaded_config(
        &config,
        &precheck_result.data_dir_resolution,
        precheck_result.working_dir.as_deref(),
    )
    .map_err(AppRunError::Runtime)
}

fn run_loaded_config(
    config: &Config,
    data_dir_resolution: &DataDirResolution,
    working_dir: Option<&std::path::Path>,
) -> Result<()> {
    let stdout = io::stdout();
    let preserve_ansi = stdout.is_terminal();
    let mut handle = stdout.lock();
    run_with_config_writer(
        &mut handle,
        preserve_ansi,
        config,
        data_dir_resolution,
        working_dir,
    )
}

/// 按 resolution 加载 models.toml + phrases.toml，并把可降级错误打到 stderr。
///
/// 与 `appconfig::load_with_resolution` 同一路径决议与错误分层：
/// 文件 Read 失败不 fatal，目录问题已在预检阶段处理。
fn load_models_and_phrases(
    working_dir: Option<&std::path::Path>,
    data_dir_resolution: &DataDirResolution,
) -> Result<(
    provider_models::LoadedModelCatalog,
    phrases::LoadedStatusPhrases,
)> {
    let (loaded_models, model_warnings) =
        provider_models::load_with_resolution(working_dir, data_dir_resolution)
            .wrap_err("failed to load model config")?;
    let (loaded_phrases, phrase_warnings) =
        phrases::load_with_resolution(working_dir, data_dir_resolution)
            .wrap_err("failed to load phrase config")?;
    // 可降级错误暂 stderr；主 TUI toast 接入是后续任务。
    for warning in model_warnings {
        eprintln!("warning: {warning}");
    }
    for warning in phrase_warnings {
        eprintln!("warning: {warning}");
    }
    Ok((loaded_models, loaded_phrases))
}

/// `run_with_writer` 允许调用方注入退出 AltScreen 后的 terminal replay 输出目标。
///
/// 调用方需提供预检阶段决定的 `data_dir_resolution`，session store、models.toml、
/// phrases.toml 均按此解析加载。
pub fn run_with_writer<W: Write>(
    writer: &mut W,
    preserve_ansi: bool,
    tui_config: &TuiConfig,
    data_dir_resolution: &DataDirResolution,
    working_dir: Option<&std::path::Path>,
) -> Result<()> {
    let (loaded_models, loaded_phrases) =
        load_models_and_phrases(working_dir, data_dir_resolution)?;
    let mut model_options =
        model_options_from_config_and_models(tui_config, &loaded_models, &loaded_phrases);
    model_options.working_dir = working_dir.map(std::path::Path::to_path_buf);
    let mut runtime_options = AppRuntimeOptions {
        loaded_models: loaded_models.clone(),
        hunea_config_dir: data_dir_resolution.config_dir().to_path_buf(),
        ..AppRuntimeOptions::default()
    };
    attach_default_session_persistence(
        &mut runtime_options,
        &mut model_options,
        &loaded_models,
        data_dir_resolution,
        working_dir,
    )
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
///
/// `data_dir_resolution` 决定 session store、models.toml、phrases.toml 以及受管搜索
/// 授权写入位置（全局 or 工作区便携）。
pub fn run_with_config_writer<W: Write>(
    writer: &mut W,
    preserve_ansi: bool,
    config: &Config,
    data_dir_resolution: &DataDirResolution,
    working_dir: Option<&std::path::Path>,
) -> Result<()> {
    let (loaded_models, loaded_phrases) =
        load_models_and_phrases(working_dir, data_dir_resolution)?;
    let mut model_options =
        model_options_from_app_config_and_models(config, &loaded_models, &loaded_phrases);
    model_options.working_dir = working_dir.map(std::path::Path::to_path_buf);
    let mut runtime_options =
        runtime_options_from_app_config_and_models(config, &loaded_models, data_dir_resolution);
    attach_default_session_persistence(
        &mut runtime_options,
        &mut model_options,
        &loaded_models,
        data_dir_resolution,
        working_dir,
    )
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
    data_dir_resolution: &DataDirResolution,
    working_dir: Option<&std::path::Path>,
) -> Result<()> {
    options.hunea_config_dir = data_dir_resolution.config_dir().to_path_buf();
    let store = open_local_session_store(data_dir_resolution)?;
    // SessionHeader.work_dir 是必填字段。cwd 不可用时用 "." 占位，
    // 避免为稀有路径把整个启动打成 fatal；真实路径语义在该场景本就不可恢复。
    let work_dir = working_dir
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
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
    let tool_definitions = tool_definitions_for_managed_search(
        &options.managed_search_tools,
        &options.hunea_config_dir,
    );
    // work_dir = 项目目录（找项目 AGENTS.md）；config_dir = 数据目录（找全局 AGENTS.md）。
    // 便携模式下二者都落在工作区 `.hunea/` 一侧，但语义仍要分开传，避免全局模式找错位置。
    let config_dir = data_dir_resolution.config_dir();
    let loaded_prompt_assembly =
        PromptAssemblyWorkspace::new(work_dir.as_path(), config_dir, &tool_definitions)
            .load_manager(store)?;
    model_options.prompt_assembly = Some(loaded_prompt_assembly.clone());
    options.initial_prompt_prelude = Some(loaded_prompt_assembly.resolution.prelude.clone());
    options.initial_dynamic_environment_session_config = Some(
        dynamic_environment_session_config_from_manager(&loaded_prompt_assembly),
    );
    options.prompt_assembly_manager = Some(loaded_prompt_assembly);
    Ok(())
}

fn open_local_session_store(
    data_dir_resolution: &DataDirResolution,
) -> Result<Arc<dyn SessionStore>> {
    let hunea_dir = data_dir_resolution.data_dir().to_path_buf();
    let store = session_store_bridge::run_session_store_future(
        move || LocalSessionStore::open_in(hunea_dir),
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
            motion: app_config::appconfig::MotionMode::Full,
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
            keyboard_enhancement: KeyboardEnhancementMode::Auto,
            scroll_animation: app_config::appconfig::ScrollAnimationMode::Smooth,
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
    fn model_options_from_config_carries_keyboard_enhancement_preference() {
        let options = model_options_from_config(&TuiConfig {
            keyboard_enhancement: KeyboardEnhancementMode::Off,
            ..default_tui_config()
        });

        assert_eq!(
            options.keyboard_enhancement,
            KeyboardEnhancementPreference::Off
        );
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
            motion: app_config::appconfig::MotionMode::Full,
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
            keyboard_enhancement: KeyboardEnhancementMode::Auto,
            scroll_animation: app_config::appconfig::ScrollAnimationMode::Smooth,
        });

        assert!(options.swap_enter_and_send);
    }

    #[test]
    fn model_options_from_config_carries_ctrl_c_clears_input_flag() {
        let options = model_options_from_config(&TuiConfig {
            user_input_style: UserInputStyle::Cx,
            motion: app_config::appconfig::MotionMode::Full,
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
            keyboard_enhancement: KeyboardEnhancementMode::Auto,
            scroll_animation: app_config::appconfig::ScrollAnimationMode::Smooth,
        });

        assert!(!options.ctrl_c_clears_input);
    }

    #[test]
    fn model_options_from_config_carries_esc_interrupt_presses() {
        let options = model_options_from_config(&TuiConfig {
            user_input_style: UserInputStyle::Cx,
            motion: app_config::appconfig::MotionMode::Full,
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
            keyboard_enhancement: KeyboardEnhancementMode::Auto,
            scroll_animation: app_config::appconfig::ScrollAnimationMode::Smooth,
        });

        assert_eq!(options.esc_interrupt_presses, 3);
    }

    #[test]
    fn model_options_from_config_carries_show_esc_interrupt_hint_flag() {
        let options = model_options_from_config(&TuiConfig {
            user_input_style: UserInputStyle::Cx,
            motion: app_config::appconfig::MotionMode::Full,
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
            keyboard_enhancement: KeyboardEnhancementMode::Auto,
            scroll_animation: app_config::appconfig::ScrollAnimationMode::Smooth,
        });

        assert!(!options.show_esc_interrupt_hint);
    }

    #[test]
    fn model_options_from_config_carries_show_reasoning_content_flag() {
        let options = model_options_from_config(&TuiConfig {
            user_input_style: UserInputStyle::Cx,
            motion: app_config::appconfig::MotionMode::Full,
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
            keyboard_enhancement: KeyboardEnhancementMode::Auto,
            scroll_animation: app_config::appconfig::ScrollAnimationMode::Smooth,
        });

        assert!(options.show_reasoning_content);
    }

    #[test]
    fn model_options_from_config_carries_reasoning_display_mode() {
        let options = model_options_from_config(&TuiConfig {
            user_input_style: UserInputStyle::Cx,
            motion: app_config::appconfig::MotionMode::Full,
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
            keyboard_enhancement: KeyboardEnhancementMode::Auto,
            scroll_animation: app_config::appconfig::ScrollAnimationMode::Smooth,
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
            motion: app_config::appconfig::MotionMode::Full,
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
            keyboard_enhancement: KeyboardEnhancementMode::Auto,
            scroll_animation: app_config::appconfig::ScrollAnimationMode::Smooth,
        };

        write_terminal_replay_on_exit(&mut FailingWriter, &model, false, &config)
            .expect("disabled terminal replay should not touch the writer");
    }

    #[test]
    fn exit_replay_writes_when_config_enables_transcript_printing() {
        let model = Model::new(StartupBannerOptions::default());
        let config = TuiConfig {
            user_input_style: UserInputStyle::Cx,
            motion: app_config::appconfig::MotionMode::Full,
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
            keyboard_enhancement: KeyboardEnhancementMode::Auto,
            scroll_animation: app_config::appconfig::ScrollAnimationMode::Smooth,
        };
        let mut output = Vec::new();

        write_terminal_replay_on_exit(&mut output, &model, false, &config)
            .expect("enabled terminal replay should write transcript output");

        assert!(!output.is_empty());
    }

    fn default_tui_config() -> TuiConfig {
        TuiConfig {
            user_input_style: UserInputStyle::Cx,
            motion: app_config::appconfig::MotionMode::Full,
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
            keyboard_enhancement: KeyboardEnhancementMode::Auto,
            scroll_animation: app_config::appconfig::ScrollAnimationMode::Smooth,
        }
    }

    #[test]
    fn model_options_map_reduced_motion_without_stringly_typed_state() {
        let mut config = default_tui_config();
        config.motion = app_config::appconfig::MotionMode::Reduced;

        let options = model_options_from_config(&config);

        assert_eq!(options.motion_mode, terminal_ui::MotionMode::Reduced);
    }

    #[test]
    fn model_options_map_scroll_animation_tiers_without_stringly_typed_state() {
        // 默认档位 Smooth 与逃生档位 Off 分别映射到 terminal-ui 侧同名枚举。
        let default_options = model_options_from_config(&default_tui_config());
        assert_eq!(
            default_options.scroll_animation,
            terminal_ui::ScrollAnimationMode::Smooth
        );

        let off_options = model_options_from_config(&TuiConfig {
            scroll_animation: app_config::appconfig::ScrollAnimationMode::Off,
            ..default_tui_config()
        });
        assert_eq!(
            off_options.scroll_animation,
            terminal_ui::ScrollAnimationMode::Off
        );
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
