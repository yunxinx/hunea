//! App config 到 TUI/runtime options 的映射。

use app_config::appconfig::{
    Config, DebugConfig, ReasoningContentDisplay, RuntimeConfig, TuiConfig, UserInputStyle,
};
use conversation_runtime::models::LoadedModelCatalog;
use runtime_domain::{envinfo, paths::DataDirResolution, phrases::LoadedStatusPhrases};
use terminal_ui::{
    EscRewindMode as TuiEscRewindMode, ModelOptions, ReasoningDisplayMode, RuntimeRequestPolicy,
    StatusLineItem, StyleMode,
};
use tool_runtime::builtin::ManagedSearchToolConfig;

use crate::runtime::AppRuntimeOptions;

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
        ReasoningContentDisplay::ExpandedSimplified => ReasoningDisplayMode::ExpandedSimplified,
        ReasoningContentDisplay::Snippet => ReasoningDisplayMode::Snippet,
    }
}

#[cfg(test)]
pub(crate) fn model_options_from_config(tui_config: &TuiConfig) -> ModelOptions {
    model_options_from_config_and_models(
        tui_config,
        &LoadedModelCatalog::default(),
        &LoadedStatusPhrases::default(),
    )
}

#[cfg(test)]
pub(crate) fn model_options_from_app_config(config: &Config) -> ModelOptions {
    model_options_from_app_config_and_models(
        config,
        &LoadedModelCatalog::default(),
        &LoadedStatusPhrases::default(),
    )
}

#[cfg(test)]
pub(crate) fn runtime_options_from_app_config(config: &Config) -> AppRuntimeOptions {
    let resolution = DataDirResolution::Portable(std::path::PathBuf::from(".hunea"));
    runtime_options_from_app_config_and_models(config, &LoadedModelCatalog::default(), &resolution)
}

pub(crate) fn model_options_from_config_and_models(
    tui_config: &TuiConfig,
    loaded_models: &LoadedModelCatalog,
    loaded_phrases: &LoadedStatusPhrases,
) -> ModelOptions {
    model_options_from_configs(tui_config, None, loaded_models, loaded_phrases)
}

pub(crate) fn model_options_from_app_config_and_models(
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

pub(crate) fn runtime_options_from_app_config_and_models(
    config: &Config,
    loaded_models: &LoadedModelCatalog,
    data_dir_resolution: &DataDirResolution,
) -> AppRuntimeOptions {
    AppRuntimeOptions {
        loaded_models: loaded_models.clone(),
        runtime_request_policy: runtime_request_policy_from_config(&config.runtime),
        managed_search_tools: managed_search_tools_from_config(&config.runtime),
        // AGENTS.md 等用户级文件的数据目录由预检注入。
        hunea_config_dir: data_dir_resolution.config_dir().to_path_buf(),
        session_store: None,
        session_header_template: None,
        prompt_assembly_manager: None,
        initial_prompt_prelude: None,
        initial_dynamic_environment_session_config: None,
        dynamic_environment_observer:
            crate::dynamic_environment::default_dynamic_environment_observer(),
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
        working_dir: None,
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
        esc_rewind_mode: esc_rewind_mode_from_config(tui_config.esc_rewind_mode),
        show_esc_interrupt_hint: tui_config.show_esc_interrupt_hint,
        file_picker_popup_height: tui_config.file_picker_popup_height,
        branch_picker_list_rows: tui_config.branch_picker_list_rows,
        composer_undo_limit: tui_config.composer_undo_limit,
        message_history_limit: tui_config.message_history_limit,
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
        prompt_assembly: None,
    }
}

fn esc_rewind_mode_from_config(mode: app_config::appconfig::EscRewindMode) -> TuiEscRewindMode {
    match mode {
        app_config::appconfig::EscRewindMode::Coarse => TuiEscRewindMode::Coarse,
        app_config::appconfig::EscRewindMode::Entry => TuiEscRewindMode::Entry,
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
