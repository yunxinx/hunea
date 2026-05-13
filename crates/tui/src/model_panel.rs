use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    style::Modifier,
    text::{Line, Span},
};

use ::mo_native_agent::models::ProviderSyncRequest;
use mo_core::model_catalog::{ModelEntry, ModelProvider, ModelSelection, ModelSource};

use super::{
    AppEffect, Model,
    inline_panel::{
        InlinePanelRenderResult, append_wrapped_inline_value, inline_panel_render_result,
        inline_panel_rule_line, inline_panel_visible_rows, wrap_inline_text,
    },
    model::acp_model_provider_id,
    theme::{
        command_accent_text_style, primary_text_style, secondary_text_style, surface_text_style,
        tertiary_text_style,
    },
};

const MODEL_LIST_MAX_VISIBLE_ROWS: usize = 7;
const MODEL_PANEL_VISIBLE_ROWS: usize = 19;

/// `ModelPanelState` 保存沉浸式模型面板的导航状态。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct ModelPanelState {
    pub(super) is_open: bool,
    pub(super) provider_index: usize,
    pub(super) model_index: usize,
    pub(super) scroll: usize,
}

pub(crate) type ModelPanelRenderResult = InlinePanelRenderResult;

impl Model {
    pub(crate) fn model_panel_active(&self) -> bool {
        self.model_panel.is_open
    }

    pub(crate) fn open_model_panel(&mut self) {
        let old_value = self.composer_text().to_string();
        let old_line = self.composer.line();
        let old_column = self.composer.column();

        self.composer.replace_text_and_move_to_end(String::new());
        self.acp_panel.is_open = false;
        self.close_tool_approval_panel();
        self.model_panel.is_open = true;
        self.sync_model_panel_to_selection();
        self.sync_command_panel_navigation();
        self.sync_file_picker_state();
        self.sync_external_editor_helper_after_draft_change(&old_value);
        self.sync_composer_height();
        self.sync_document_viewport_after_composer_interaction(&old_value, old_line, old_column);
    }

    pub(crate) fn close_model_panel(&mut self) {
        if !self.model_panel.is_open {
            return;
        }

        self.model_panel.is_open = false;
        self.sync_composer_height();
        self.sync_document_viewport_for_composer_cursor();
    }

    pub(crate) fn handle_model_panel_key(
        &mut self,
        key: KeyEvent,
    ) -> Option<Option<super::AppEffect>> {
        if !self.model_panel_active() {
            return None;
        }

        match key.code {
            KeyCode::Esc => {
                self.close_model_panel();
                Some(None)
            }
            KeyCode::Tab if key.modifiers.is_empty() => {
                self.move_model_panel_provider(1);
                Some(None)
            }
            KeyCode::BackTab => {
                self.move_model_panel_provider(-1);
                Some(None)
            }
            KeyCode::Up if key.modifiers.is_empty() => {
                self.move_model_panel_model(-1);
                Some(None)
            }
            KeyCode::Down if key.modifiers.is_empty() => {
                self.move_model_panel_model(1);
                Some(None)
            }
            KeyCode::Char('u' | 'U') if is_model_refresh_key(key) => {
                Some(self.refresh_current_model_panel_provider())
            }
            KeyCode::Enter if key.modifiers.is_empty() => {
                Some(self.select_current_model_panel_model())
            }
            _ => Some(None),
        }
    }

    pub fn selected_model(&self) -> Option<ModelSelection> {
        self.selected_model.clone()
    }

    pub(crate) fn current_inline_model_panel_render_result(&self) -> ModelPanelRenderResult {
        if !self.model_panel_active() {
            return ModelPanelRenderResult::default();
        }

        let visible_rows = self.model_panel_visible_rows();
        let width = usize::from(self.width.max(1));
        let mut lines = build_panel_lines(self, width, visible_rows);
        if lines.len() > visible_rows {
            lines.truncate(visible_rows);
        }
        inline_panel_render_result(lines)
    }

    pub(crate) fn model_panel_visible_rows(&self) -> usize {
        inline_panel_visible_rows(self, MODEL_PANEL_VISIBLE_ROWS)
    }

    pub(crate) fn sync_model_panel_to_selection(&mut self) {
        let provider_count = self.model_catalog.enabled_provider_count();
        if provider_count == 0 {
            self.model_panel.provider_index = 0;
            self.model_panel.model_index = 0;
            self.model_panel.scroll = 0;
            return;
        }

        if let Some(selection) = &self.selected_model
            && let Some(provider_index) = self
                .model_catalog
                .enabled_provider_index_for(&selection.provider_id)
        {
            self.model_panel.provider_index = provider_index;
            self.model_panel.model_index = self
                .active_model_panel_provider()
                .and_then(|provider| {
                    provider
                        .models
                        .iter()
                        .position(|model| model.id == selection.model_id)
                })
                .unwrap_or(0);
            self.sync_model_panel_scroll();
            return;
        }

        self.model_panel.provider_index = self.model_panel.provider_index.min(provider_count - 1);
        let model_count = self
            .active_model_panel_provider()
            .map(|provider| provider.models.len())
            .unwrap_or_default();
        self.model_panel.model_index = self
            .model_panel
            .model_index
            .min(model_count.saturating_sub(1));
        self.sync_model_panel_scroll();
    }

    fn active_model_panel_provider(&self) -> Option<&ModelProvider> {
        self.model_catalog
            .enabled_provider_at(self.model_panel.provider_index)
    }

    fn move_model_panel_provider(&mut self, delta: isize) {
        let provider_count = self.model_catalog.enabled_provider_count();
        if provider_count == 0 {
            self.model_panel.provider_index = 0;
            self.model_panel.model_index = 0;
            self.model_panel.scroll = 0;
            return;
        }

        let current = self.model_panel.provider_index.min(provider_count - 1);
        self.model_panel.provider_index = wrapping_index(current, provider_count, delta);
        self.model_panel.model_index = 0;
        self.model_panel.scroll = 0;
    }

    fn move_model_panel_model(&mut self, delta: isize) {
        let Some(provider) = self.active_model_panel_provider() else {
            return;
        };
        if provider.models.is_empty() {
            self.model_panel.model_index = 0;
            self.model_panel.scroll = 0;
            return;
        }

        let last_index = provider.models.len() - 1;
        self.model_panel.model_index = if delta.is_negative() {
            self.model_panel
                .model_index
                .saturating_sub(delta.unsigned_abs())
        } else {
            self.model_panel
                .model_index
                .saturating_add(delta as usize)
                .min(last_index)
        };
        self.sync_model_panel_scroll();
    }

    fn sync_model_panel_scroll(&mut self) {
        let model_count = self
            .active_model_panel_provider()
            .map(|provider| provider.models.len())
            .unwrap_or_default();
        if model_count == 0 {
            self.model_panel.scroll = 0;
            return;
        }

        let selected = self.model_panel.model_index.min(model_count - 1);
        let visible_model_rows = self.model_panel_visible_model_rows();
        let max_scroll = model_count.saturating_sub(visible_model_rows);
        let mut scroll = self.model_panel.scroll.min(max_scroll);
        if selected < scroll {
            scroll = selected;
        }
        if selected >= scroll + visible_model_rows {
            scroll = selected + 1 - visible_model_rows;
        }
        self.model_panel.model_index = selected;
        self.model_panel.scroll = scroll;
    }

    fn model_panel_visible_model_rows(&self) -> usize {
        let visible_rows = self.model_panel_visible_rows();
        let width = usize::from(self.width.max(1));
        let reserved_rows =
            build_panel_header_lines(self, width).len() + model_panel_footer_lines(self).len();
        visible_rows
            .saturating_sub(reserved_rows)
            .clamp(1, MODEL_LIST_MAX_VISIBLE_ROWS)
    }

    fn select_current_model_panel_model(&mut self) -> Option<AppEffect> {
        let (provider_id, model_id, model_label) = {
            let provider = self.active_model_panel_provider()?;
            let model = provider.models.get(self.model_panel.model_index)?;
            let provider_id = provider.id.clone();
            let model_id = model.id.clone();
            let model_label = model
                .description
                .clone()
                .unwrap_or_else(|| model_id.clone());
            (provider_id, model_id, model_label)
        };
        let acp_agent_id = self
            .selected_acp_agent
            .as_ref()
            .filter(|agent_id| provider_id == acp_model_provider_id(agent_id))
            .cloned();
        if let Some(agent_id) = acp_agent_id.as_deref() {
            self.begin_pending_acp_model_change(agent_id);
        }

        let selection = ModelSelection::new(provider_id.clone(), model_id.clone());
        self.selected_model = Some(selection.clone());
        self.bump_status_line_revision();
        if acp_agent_id.is_some() {
            self.set_acp_current_model(Some(model_label.clone()));
            self.show_transient_status_notice(&format!("ACP model selected: {model_label}"));
            self.close_model_panel();
            return Some(AppEffect::SetAcpModel {
                config_id: self.acp_model_config_id.clone(),
                value: model_id,
            });
        }
        self.show_transient_status_notice(&format!(
            "Model selected: {}",
            self.model_selection_display_name(
                selection.provider_id.as_str(),
                selection.model_id.as_str()
            )
        ));
        self.close_model_panel();
        Some(AppEffect::PersistSelectedModel { selection })
    }

    fn refresh_current_model_panel_provider(&mut self) -> Option<AppEffect> {
        let (request, display_name) = {
            let provider = self.active_model_panel_provider()?;
            let Some(native_runtime) = provider.native_runtime() else {
                self.show_transient_status_notice("ACP models are managed by the agent");
                return None;
            };
            (
                ProviderSyncRequest {
                    provider_id: provider.id.clone(),
                    kind: native_runtime.kind,
                    display_name: provider.display_name.clone(),
                    base_url: native_runtime.base_url.clone(),
                    api_key: native_runtime.api_key.clone(),
                    api_key_env: native_runtime.api_key_env.clone(),
                },
                provider.display_name.clone(),
            )
        };
        self.show_transient_status_notice(&format!("Refreshing models: {display_name}"));
        Some(AppEffect::RefreshModelProvider { request })
    }

    pub(crate) fn apply_model_provider_refresh_success(
        &mut self,
        provider_id: &str,
        model_ids: Vec<String>,
    ) {
        let Some(provider) = self.model_catalog.provider_by_id_mut(provider_id) else {
            self.show_transient_status_notice("Refreshed provider is no longer available");
            return;
        };
        let display_name = provider.display_name.clone();
        provider.models = model_ids
            .into_iter()
            .map(|model_id| ModelEntry::new(model_id, None, ModelSource::Synced))
            .collect();
        provider.source = ModelSource::Synced;
        provider.sync_error = None;

        if self
            .selected_model
            .as_ref()
            .is_some_and(|selection| !self.model_catalog.contains_selection(selection))
        {
            self.selected_model = None;
            self.bump_status_line_revision();
        }
        self.sync_model_panel_to_selection();
        self.show_transient_status_notice(&format!("Models refreshed: {display_name}"));
    }

    pub(crate) fn apply_model_provider_refresh_failure(
        &mut self,
        provider_id: &str,
        message: impl Into<String>,
    ) {
        let message = message.into();
        let display_name = self
            .model_catalog
            .provider_by_id_mut(provider_id)
            .map(|provider| {
                provider.sync_error = Some(message.clone());
                provider.display_name.clone()
            });

        match display_name {
            Some(display_name) => self.show_transient_status_notice(&format!(
                "Failed to refresh models for {display_name}: {message}"
            )),
            None => {
                self.show_transient_status_notice(&format!("Failed to refresh models: {message}"))
            }
        }
    }
}

fn is_model_refresh_key(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('u' | 'U'))
        && (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT)
}

fn build_panel_lines(model: &Model, width: usize, visible_rows: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut lines = build_panel_header_lines(model, width);
    let footer_lines = model_panel_footer_lines(model);
    let visible_model_rows = visible_rows
        .saturating_sub(lines.len() + footer_lines.len())
        .clamp(1, MODEL_LIST_MAX_VISIBLE_ROWS);
    append_model_lines(model, width, visible_model_rows, &mut lines);
    lines.extend(footer_lines);

    lines
}

fn build_panel_header_lines(model: &Model, width: usize) -> Vec<Line<'static>> {
    let mut lines = vec![
        inline_panel_rule_line(width, model.palette),
        provider_tabs_line(model),
        Line::raw(""),
        current_model_line(model),
        Line::raw(""),
        Line::styled("  Provider Details:", secondary_text_style(model.palette)),
    ];
    append_provider_details_lines(model, width, &mut lines);
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "  Available Models:",
        secondary_text_style(model.palette).bold(),
    ));

    lines
}

fn model_panel_footer_lines(model: &Model) -> [Line<'static>; 2] {
    [
        Line::raw(""),
        Line::styled(
            "  Enter select · U refresh · Esc exit · Tab providers · ↑↓ navigate",
            tertiary_text_style(model.palette).add_modifier(Modifier::ITALIC),
        ),
    ]
}

fn provider_tabs_line(model: &Model) -> Line<'static> {
    let mut spans = vec![
        Span::raw("  "),
        Span::styled("Providers: ", primary_text_style(model.palette)),
    ];
    let providers = model.model_catalog.enabled_providers().collect::<Vec<_>>();
    if providers.is_empty() {
        spans.push(Span::styled(
            "[No Providers]",
            tertiary_text_style(model.palette),
        ));
        return Line::from(spans);
    }

    for (index, provider) in providers.iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw("  "));
        }
        let is_active = index == model.model_panel.provider_index;
        let label = if is_active {
            format!("[{}]", provider.display_name)
        } else {
            provider.display_name.clone()
        };
        let style = if is_active {
            surface_text_style(model.palette).bold()
        } else {
            tertiary_text_style(model.palette)
        };
        spans.push(Span::styled(label, style));
    }

    Line::from(spans)
}

fn current_model_line(model: &Model) -> Line<'static> {
    let mut spans = vec![
        Span::raw("  "),
        Span::styled("Current Model: ", primary_text_style(model.palette)),
    ];

    let Some(selection) = &model.selected_model else {
        spans.push(Span::styled("none", tertiary_text_style(model.palette)));
        return Line::from(spans);
    };

    let provider_name = model
        .model_catalog
        .enabled_providers()
        .find(|provider| provider.id == selection.provider_id)
        .map(|provider| provider.display_name.as_str())
        .unwrap_or(selection.provider_id.as_str());
    spans.push(Span::styled(
        format!("[{provider_name}]"),
        secondary_text_style(model.palette).bold(),
    ));
    spans.push(Span::raw(" "));
    spans.push(Span::styled(
        selection.model_id.clone(),
        command_accent_text_style(model.palette),
    ));

    Line::from(spans)
}

fn append_provider_details_lines(model: &Model, width: usize, lines: &mut Vec<Line<'static>>) {
    let Some(provider) = model.active_model_panel_provider() else {
        append_wrapped_inline_value(
            lines,
            width,
            "• Model Source      : ",
            "No enabled models",
            tertiary_text_style(model.palette),
            secondary_text_style(model.palette),
        );
        return;
    };

    append_wrapped_inline_value(
        lines,
        width,
        "• Model Source      : ",
        provider.source.label(),
        tertiary_text_style(model.palette),
        secondary_text_style(model.palette),
    );
    append_wrapped_inline_value(
        lines,
        width,
        "• Endpoint          : ",
        provider
            .native_runtime()
            .and_then(|runtime| runtime.base_url.as_deref())
            .unwrap_or("not configured"),
        tertiary_text_style(model.palette),
        secondary_text_style(model.palette),
    );
}

fn append_model_lines(
    model: &Model,
    width: usize,
    visible_rows: usize,
    lines: &mut Vec<Line<'static>>,
) {
    let Some(provider) = model.active_model_panel_provider() else {
        lines.push(Line::styled(
            "  No enabled models",
            tertiary_text_style(model.palette),
        ));
        return;
    };

    if provider.models.is_empty() {
        if let Some(error) = &provider.sync_error {
            append_wrapped_inline_value(
                lines,
                width,
                "  ",
                &format!("Sync failed: {error}"),
                tertiary_text_style(model.palette),
                secondary_text_style(model.palette),
            );
            return;
        }
        lines.push(Line::styled(
            "  No models available for this provider",
            tertiary_text_style(model.palette),
        ));
        return;
    }

    let start = model.model_panel.scroll;
    let end = (start + visible_rows).min(provider.models.len());
    for (offset, entry) in provider.models[start..end].iter().enumerate() {
        let index = start + offset;
        append_model_entry_lines(
            model,
            provider,
            entry,
            index == model.model_panel.model_index,
            width,
            lines,
        );
    }
}

fn append_model_entry_lines(
    model: &Model,
    provider: &ModelProvider,
    entry: &ModelEntry,
    selected: bool,
    width: usize,
    lines: &mut Vec<Line<'static>>,
) {
    let marker = if selected { "➜ " } else { "  " };
    let active = model.selected_model.as_ref().is_some_and(|selection| {
        selection.provider_id == provider.id && selection.model_id == entry.id
    });
    let style = if active && selected {
        command_accent_text_style(model.palette).bold()
    } else if active {
        command_accent_text_style(model.palette)
    } else if selected {
        primary_text_style(model.palette).bold()
    } else {
        secondary_text_style(model.palette)
    };

    if provider.source == ModelSource::Acp {
        let label = entry
            .description
            .as_deref()
            .map(str::trim)
            .filter(|description| !description.is_empty())
            .unwrap_or(entry.id.as_str());
        append_wrapped_inline_value(
            lines,
            width,
            marker,
            label,
            style,
            secondary_text_style(model.palette),
        );
        return;
    }

    append_wrapped_inline_value(
        lines,
        width,
        marker,
        &entry.id,
        style,
        secondary_text_style(model.palette),
    );

    if let Some(description) = &entry.description {
        for line in wrap_inline_text(description, width.saturating_sub(4).max(1)) {
            lines.push(Line::styled(
                format!("    {line}"),
                tertiary_text_style(model.palette),
            ));
        }
    }
}

#[cfg(test)]
fn model_list_plain_lines(plain_lines: &[String]) -> Vec<&str> {
    let start = plain_lines
        .iter()
        .position(|line| line.contains("Available Models:"))
        .map(|index| index + 1)
        .unwrap_or(plain_lines.len());

    plain_lines[start..]
        .iter()
        .take_while(|line| !line.trim_start().starts_with("Enter select"))
        .map(String::as_str)
        .filter(|line| !line.trim().is_empty())
        .collect()
}

#[cfg(test)]
fn model_row_label(line: &str) -> &str {
    let line = line.trim_start();
    line.strip_prefix("➜ ").unwrap_or(line).trim()
}

fn wrapping_index(current: usize, len: usize, delta: isize) -> usize {
    if len == 0 {
        return 0;
    }
    if delta.is_negative() {
        (current + len - (delta.unsigned_abs() % len)) % len
    } else {
        (current + delta as usize) % len
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppEvent, HeroOptions, ModelOptions};
    use ::mo_acp::{AcpModelConfig, AcpModelOption};
    use ::mo_native_agent::ProviderKind;
    use mo_core::model_catalog::{ModelCatalog, ModelProvider, ModelSource};

    #[test]
    fn provider_refresh_success_replaces_models_and_drops_stale_selection() {
        let mut model = model_with_single_provider();
        model.open_model_panel();

        model.apply_model_provider_refresh_success(
            "local",
            vec!["fresh-a".to_string(), "fresh-b".to_string()],
        );

        let provider = model
            .model_catalog
            .enabled_provider_by_id("local")
            .expect("refreshed provider should remain enabled");
        assert_eq!(provider.source, ModelSource::Synced);
        assert_eq!(provider.sync_error, None);
        assert_eq!(
            provider
                .models
                .iter()
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>(),
            vec!["fresh-a", "fresh-b"]
        );
        assert_eq!(model.selected_model, None);
        assert_eq!(model.model_panel.model_index, 0);
        assert_eq!(model.model_panel.scroll, 0);
    }

    #[test]
    fn open_model_panel_closes_tool_approval_panel_and_resumes_stream_activity() {
        let mut model = model_with_single_provider();
        model.show_stream_activity_with_header("Working");
        model.update(AppEvent::AcpPermissionRequested {
            request_id: "permission-1".to_string(),
            title: Some("Write file".to_string()),
            allow_option_id: Some("allow-once".to_string()),
            allow_always_option_id: None,
            reject_option_id: Some("reject-once".to_string()),
            reject_always_option_id: None,
        });
        assert!(!model.current_stream_activity_render_result().has_content);

        model.open_model_panel();

        assert!(model.current_stream_activity_render_result().has_content);
    }

    #[test]
    fn acp_model_panel_selects_agent_model_without_persisting_native_default() {
        let mut model = Model::new_with_options(
            HeroOptions::default(),
            ModelOptions {
                model_catalog: ModelCatalog::new(vec![ModelProvider::native(
                    "local",
                    ProviderKind::OpenAiCompatible,
                    "Local",
                    Some("http://127.0.0.1:1234/v1".to_string()),
                    ModelSource::Configured,
                    vec![ModelEntry::new("qwen3", None, ModelSource::Configured)],
                )]),
                selected_model: Some(ModelSelection::new("local", "qwen3")),
                ..ModelOptions::default()
            },
        );
        model.selected_acp_agent = Some("Kimi Code CLI".to_string());
        model.apply_acp_model_config(
            "Kimi Code CLI",
            AcpModelConfig {
                config_id: Some("model".to_string()),
                current_value: "kimi-k2".to_string(),
                current_name: "Kimi K2".to_string(),
                options: vec![
                    AcpModelOption {
                        value: "kimi-k2".to_string(),
                        name: "Kimi K2".to_string(),
                    },
                    AcpModelOption {
                        value: "kimi-k1.5".to_string(),
                        name: "Kimi K1.5".to_string(),
                    },
                ],
            },
        );
        assert!(
            model
                .model_catalog
                .enabled_provider_by_id("acp:Kimi Code CLI")
                .expect("ACP model provider should be visible")
                .native_runtime()
                .is_none()
        );
        model.open_model_panel();
        model.move_model_panel_model(1);

        let effect = model.select_current_model_panel_model();

        assert_eq!(
            effect,
            Some(AppEffect::SetAcpModel {
                config_id: Some("model".to_string()),
                value: "kimi-k1.5".to_string(),
            })
        );
        assert_eq!(
            model.selected_model,
            Some(ModelSelection::new("acp:Kimi Code CLI", "kimi-k1.5"))
        );
        assert_eq!(model.acp_current_model.as_deref(), Some("Kimi K1.5"));
    }

    #[test]
    fn acp_model_panel_selects_legacy_agent_model_without_config_id() {
        let mut model = Model::new_with_options(
            HeroOptions::default(),
            ModelOptions {
                model_catalog: ModelCatalog::new(vec![ModelProvider::native(
                    "local",
                    ProviderKind::OpenAiCompatible,
                    "Local",
                    Some("http://127.0.0.1:1234/v1".to_string()),
                    ModelSource::Configured,
                    vec![ModelEntry::new("qwen3", None, ModelSource::Configured)],
                )]),
                selected_model: Some(ModelSelection::new("local", "qwen3")),
                ..ModelOptions::default()
            },
        );
        model.selected_acp_agent = Some("Kimi Code CLI".to_string());
        model.apply_acp_model_config(
            "Kimi Code CLI",
            AcpModelConfig {
                config_id: None,
                current_value: "kimi-for-coding".to_string(),
                current_name: "Kimi for Coding".to_string(),
                options: vec![
                    AcpModelOption {
                        value: "kimi-for-coding".to_string(),
                        name: "Kimi for Coding".to_string(),
                    },
                    AcpModelOption {
                        value: "kimi-for-coding(thinking)".to_string(),
                        name: "Kimi for Coding (thinking)".to_string(),
                    },
                ],
            },
        );
        model.open_model_panel();
        model.move_model_panel_model(1);

        let effect = model.select_current_model_panel_model();

        assert_eq!(
            effect,
            Some(AppEffect::SetAcpModel {
                config_id: None,
                value: "kimi-for-coding(thinking)".to_string(),
            })
        );
        assert_eq!(
            model.selected_model,
            Some(ModelSelection::new(
                "acp:Kimi Code CLI",
                "kimi-for-coding(thinking)"
            ))
        );
        assert_eq!(
            model.acp_current_model.as_deref(),
            Some("Kimi for Coding (thinking)")
        );
    }

    #[test]
    fn acp_model_panel_prefers_model_name_and_falls_back_to_id() {
        let mut model = Model::new_with_options(
            HeroOptions::default(),
            ModelOptions {
                model_catalog: ModelCatalog::new(vec![ModelProvider::native(
                    "local",
                    ProviderKind::OpenAiCompatible,
                    "Local",
                    Some("http://127.0.0.1:1234/v1".to_string()),
                    ModelSource::Configured,
                    vec![ModelEntry::new("qwen3", None, ModelSource::Configured)],
                )]),
                selected_model: Some(ModelSelection::new("local", "qwen3")),
                ..ModelOptions::default()
            },
        );
        model.selected_acp_agent = Some("Kimi Code CLI".to_string());
        model.apply_acp_model_config(
            "Kimi Code CLI",
            AcpModelConfig {
                config_id: Some("models".to_string()),
                current_value: "kimi-code/kimi-for-coding".to_string(),
                current_name: "kimi-for-coding".to_string(),
                options: vec![
                    AcpModelOption {
                        value: "kimi-code/kimi-for-coding".to_string(),
                        name: "Kimi for Coding".to_string(),
                    },
                    AcpModelOption {
                        value: "kimi-code/kimi-for-coding,thinking".to_string(),
                        name: String::new(),
                    },
                ],
            },
        );
        model.set_window(80, 24);
        model.open_model_panel();

        let render = model.current_inline_model_panel_render_result();
        let plain_lines = render.plain_lines;
        let model_lines = model_list_plain_lines(&plain_lines);

        assert_eq!(
            model_lines
                .iter()
                .filter(|line| line.contains("Kimi for Coding"))
                .count(),
            1
        );
        assert!(
            model_lines
                .iter()
                .any(|line| model_row_label(line) == "kimi-code/kimi-for-coding,thinking")
        );
        assert!(
            !model_lines
                .iter()
                .any(|line| model_row_label(line) == "kimi-code/kimi-for-coding")
        );

        model.move_model_panel_model(1);
        let effect = model.select_current_model_panel_model();

        assert_eq!(
            effect,
            Some(AppEffect::SetAcpModel {
                config_id: Some("models".to_string()),
                value: "kimi-code/kimi-for-coding,thinking".to_string(),
            })
        );
        assert_eq!(
            model.acp_current_model.as_deref(),
            Some("kimi-code/kimi-for-coding,thinking")
        );
    }

    #[test]
    fn provider_refresh_failure_keeps_existing_models_and_records_error() {
        let mut model = model_with_single_provider();
        model.open_model_panel();

        model.apply_model_provider_refresh_failure("local", "connection refused");

        let provider = model
            .model_catalog
            .enabled_provider_by_id("local")
            .expect("provider should remain visible after failed refresh");
        assert_eq!(provider.source, ModelSource::Configured);
        assert_eq!(provider.sync_error.as_deref(), Some("connection refused"));
        assert_eq!(
            provider
                .models
                .iter()
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>(),
            vec!["qwen3"]
        );
        assert_eq!(
            model.selected_model,
            Some(ModelSelection::new("local", "qwen3"))
        );
    }

    fn model_with_single_provider() -> Model {
        Model::new_with_options(
            HeroOptions::default(),
            ModelOptions {
                model_catalog: ModelCatalog::new(vec![ModelProvider::native(
                    "local",
                    ProviderKind::OpenAiCompatible,
                    "Local",
                    Some("http://127.0.0.1:1234/v1".to_string()),
                    ModelSource::Configured,
                    vec![ModelEntry::new("qwen3", None, ModelSource::Configured)],
                )]),
                selected_model: Some(ModelSelection::new("local", "qwen3")),
                ..ModelOptions::default()
            },
        )
    }
}
