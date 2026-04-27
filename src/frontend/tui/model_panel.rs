use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    style::Modifier,
    text::{Line, Span},
};

use crate::runtime::models::{ModelEntry, ModelProvider, ModelSelection};

use super::{
    AppEffect, Model,
    inline_panel::{
        InlinePanelRenderResult, append_wrapped_inline_value, inline_panel_render_result,
        inline_panel_rule_line, inline_panel_visible_rows, wrap_inline_text,
    },
    theme::{
        command_accent_text_style, primary_text_style, secondary_text_style, surface_text_style,
        tertiary_text_style,
    },
};

const MODEL_LIST_VISIBLE_ROWS: usize = 8;
const MODEL_PANEL_VISIBLE_ROWS: usize = 14;

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
        if self.tool_approval_panel.is_open {
            self.tool_approval_panel = Default::default();
            self.pending_acp_permission = None;
            self.tool_approval_panel_revision = self.tool_approval_panel_revision.saturating_add(1);
        }
        self.model_panel.is_open = true;
        self.sync_model_panel_to_selection();
        self.sync_command_panel_navigation();
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
        let mut lines = build_panel_lines(self, width);
        if lines.len() > visible_rows {
            lines.truncate(visible_rows);
        }
        inline_panel_render_result(lines)
    }

    pub(crate) fn model_panel_visible_rows(&self) -> usize {
        inline_panel_visible_rows(self, MODEL_PANEL_VISIBLE_ROWS)
    }

    fn sync_model_panel_to_selection(&mut self) {
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
        let max_scroll = model_count.saturating_sub(MODEL_LIST_VISIBLE_ROWS);
        let mut scroll = self.model_panel.scroll.min(max_scroll);
        if selected < scroll {
            scroll = selected;
        }
        if selected >= scroll + MODEL_LIST_VISIBLE_ROWS {
            scroll = selected + 1 - MODEL_LIST_VISIBLE_ROWS;
        }
        self.model_panel.model_index = selected;
        self.model_panel.scroll = scroll;
    }

    fn select_current_model_panel_model(&mut self) -> Option<AppEffect> {
        let provider = self.active_model_panel_provider()?;
        let model = provider.models.get(self.model_panel.model_index)?;

        let selection = ModelSelection::new(provider.id.clone(), model.id.clone());
        self.selected_model = Some(selection.clone());
        self.show_transient_status_notice(&format!("Model selected: {}", selection.display_name()));
        self.close_model_panel();
        Some(AppEffect::PersistSelectedModel { selection })
    }
}

fn build_panel_lines(model: &Model, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
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
    append_model_lines(model, width, &mut lines);
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "  Press Enter to select · Esc to exit · Tab to cycle providers · ↑↓ to navigate",
        tertiary_text_style(model.palette).add_modifier(Modifier::ITALIC),
    ));

    lines
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
        provider.base_url.as_deref().unwrap_or("not configured"),
        tertiary_text_style(model.palette),
        secondary_text_style(model.palette),
    );
}

fn append_model_lines(model: &Model, width: usize, lines: &mut Vec<Line<'static>>) {
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
    let end = (start + MODEL_LIST_VISIBLE_ROWS).min(provider.models.len());
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
