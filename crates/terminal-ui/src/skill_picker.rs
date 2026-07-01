use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::text::{Line, Span};
use runtime_domain::prompt_assembly::PromptAssemblyDiscoveredSkill;

use super::{
    Model,
    display_width::display_width,
    inline_panel::InlinePanelRenderResult,
    overlay_input_result::OverlayInputResult,
    selection::{SelectableLineRange, selectable_range_for_plain_line},
    status_line::truncate_display_width_with_ellipsis,
    theme::{
        command_accent_text_style, primary_text_style, secondary_text_style, tertiary_text_style,
    },
};

const SKILL_PICKER_INSET_WIDTH: usize = 2;
const SKILL_PICKER_NAME_COLUMN_MAX_WIDTH: usize = 28;
const SKILL_PICKER_COLUMN_GAP: usize = 2;
const SKILL_PICKER_DESCRIPTION_MIN_WIDTH: usize = 12;

/// `SkillPickerState` 保存 `$skill` 选择器的当前查询、结果和导航位置。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct SkillPickerState {
    pub(crate) query: String,
    pub(crate) items: Vec<PromptAssemblyDiscoveredSkill>,
    pub(crate) selected: usize,
    pub(crate) scroll: usize,
}

impl Model {
    pub(crate) fn skill_picker_active(&self) -> bool {
        self.skill_picker.is_some()
    }

    pub(crate) fn sync_composer_attached_picker_state(&mut self) {
        self.sync_file_picker_state();
        self.sync_skill_picker_state();
    }

    pub(crate) fn sync_skill_picker_state(&mut self) {
        if self.blocks_composer_input() || self.command_panel_active() {
            self.close_skill_picker();
            return;
        }

        let Some(query) = self.composer.current_skill_token() else {
            self.close_skill_picker();
            self.dismissed_skill_picker_token = None;
            return;
        };

        if self.dismissed_skill_picker_token.as_ref() == Some(&query) {
            self.close_skill_picker();
            return;
        }

        let items = filter_manual_skill_items(&self.prompt_assembly.manual_skills, &query);
        let visible_rows = self.file_picker_list_visible_rows();
        let previous = self.skill_picker.as_ref();
        let query_changed = previous.is_none_or(|state| state.query != query);
        let bound_skill_name = self
            .composer
            .current_skill_binding()
            .map(|binding| binding.skill_name);
        let mut selected = if query_changed {
            bound_skill_name
                .as_deref()
                .and_then(|skill_name| {
                    items
                        .iter()
                        .position(|item| item.skill_name.as_str() == skill_name)
                })
                .unwrap_or(0)
        } else {
            previous.map(|state| state.selected).unwrap_or(0)
        };
        let mut scroll = if query_changed {
            0
        } else {
            previous.map(|state| state.scroll).unwrap_or(0)
        };

        if items.is_empty() {
            selected = 0;
            scroll = 0;
        } else {
            selected = selected.min(items.len() - 1);
            scroll = clamp_skill_picker_scroll(scroll, selected, items.len(), visible_rows);
        }

        self.skill_picker = Some(SkillPickerState {
            query,
            items,
            selected,
            scroll,
        });
    }

    pub(crate) fn handle_skill_picker_key(&mut self, key: KeyEvent) -> OverlayInputResult {
        if !self.skill_picker_active() {
            return OverlayInputResult::Ignored;
        }

        match key.code {
            KeyCode::Up if key.modifiers.is_empty() => {
                self.move_skill_picker_selection(-1);
                OverlayInputResult::Handled
            }
            KeyCode::Down if key.modifiers.is_empty() => {
                self.move_skill_picker_selection(1);
                OverlayInputResult::Handled
            }
            KeyCode::Char('p') if key.modifiers == KeyModifiers::CONTROL => {
                self.move_skill_picker_selection(-1);
                OverlayInputResult::Handled
            }
            KeyCode::Char('n') if key.modifiers == KeyModifiers::CONTROL => {
                self.move_skill_picker_selection(1);
                OverlayInputResult::Handled
            }
            KeyCode::Esc if key.modifiers.is_empty() => {
                self.dismiss_current_skill_picker_token();
                self.close_skill_picker();
                OverlayInputResult::Handled
            }
            KeyCode::Tab if key.modifiers.is_empty() => {
                self.complete_skill_picker_common_prefix();
                OverlayInputResult::Handled
            }
            KeyCode::Enter if key.modifiers.is_empty() => {
                let _ = self.insert_selected_skill_picker_skill();
                OverlayInputResult::Handled
            }
            _ => OverlayInputResult::Ignored,
        }
    }

    pub(crate) fn current_skill_picker_render_result(&self) -> InlinePanelRenderResult {
        let Some(state) = self.skill_picker.as_ref() else {
            return InlinePanelRenderResult::default();
        };

        let visible_rows = self.file_picker_list_visible_rows();
        let width = usize::from(self.width.max(1));
        let has_scrollbar = state.items.len() > visible_rows;
        let content_width = width.saturating_sub(usize::from(has_scrollbar && width > 1));
        let (lines, plain_lines, selectable) =
            self.render_skill_picker_lines(state, content_width, visible_rows);

        InlinePanelRenderResult {
            lines,
            plain_lines,
            selectable,
            has_content: true,
        }
    }

    fn render_skill_picker_lines(
        &self,
        state: &SkillPickerState,
        width: usize,
        visible_rows: usize,
    ) -> (Vec<Line<'static>>, Vec<String>, Vec<SelectableLineRange>) {
        let width = width.max(1);
        let visible_rows = visible_rows.max(1);
        let mut lines = Vec::with_capacity(visible_rows);
        let mut plain_lines = Vec::with_capacity(visible_rows);
        let mut selectable = Vec::with_capacity(visible_rows);
        let name_column_width = skill_picker_name_column_width(
            &state.items,
            width.saturating_sub(SKILL_PICKER_INSET_WIDTH),
        );

        if state.items.is_empty() {
            let plain_line = pad_display_width_right("  No skills", width);
            lines.push(Line::styled(
                plain_line.clone(),
                tertiary_text_style(self.palette),
            ));
            plain_lines.push(plain_line.clone());
            selectable.push(selectable_range_for_plain_line(&plain_line));
            return (lines, plain_lines, selectable);
        }

        for row in 0..visible_rows {
            let index = state.scroll + row;
            let Some(item) = state.items.get(index) else {
                lines.push(Line::raw(""));
                plain_lines.push(String::new());
                selectable.push(SelectableLineRange::default());
                continue;
            };

            let selected = index == state.selected;
            let (line, plain_line) = self.render_skill_picker_line(
                item,
                &state.query,
                selected,
                width,
                name_column_width,
            );
            selectable.push(skill_picker_selectable_range(&plain_line, width));
            lines.push(line);
            plain_lines.push(plain_line);
        }

        (lines, plain_lines, selectable)
    }

    fn render_skill_picker_line(
        &self,
        item: &PromptAssemblyDiscoveredSkill,
        query: &str,
        selected: bool,
        width: usize,
        name_column_width: usize,
    ) -> (Line<'static>, String) {
        let inset = SKILL_PICKER_INSET_WIDTH.min(width);
        let content_width = width.saturating_sub(inset);
        let name_style = if selected {
            command_accent_text_style(self.palette).bold()
        } else {
            secondary_text_style(self.palette)
        };
        let description_style = if selected {
            primary_text_style(self.palette)
        } else {
            tertiary_text_style(self.palette)
        };
        let highlighted_name_style = skill_picker_match_style(name_style, self.palette.surface);
        let highlighted_description_style =
            skill_picker_match_style(description_style, self.palette.surface);

        let display_name = skill_picker_display_name(item);
        let name_width = if item.description.trim().is_empty() {
            content_width
        } else {
            name_column_width.min(content_width).max(1)
        };
        let name = truncate_display_width_with_ellipsis(display_name, name_width);

        let mut spans = vec![Span::raw(" ".repeat(inset))];
        spans.extend(highlighted_query_spans(
            &name,
            query,
            name_style,
            highlighted_name_style,
        ));
        let mut plain_line = format!("{}{}", " ".repeat(inset), name);

        if !item.description.trim().is_empty() {
            let reserved_name_width = name_width;
            let rendered_name_width = display_width(&name);
            let gap_width = reserved_name_width
                .saturating_sub(rendered_name_width)
                .saturating_add(SKILL_PICKER_COLUMN_GAP);
            let remaining_width = content_width.saturating_sub(rendered_name_width + gap_width);
            if remaining_width > 0 {
                let description =
                    truncate_display_width_with_ellipsis(item.description.trim(), remaining_width);
                spans.push(Span::raw(" ".repeat(gap_width)));
                spans.extend(highlighted_query_spans(
                    &description,
                    query,
                    description_style,
                    highlighted_description_style,
                ));
                plain_line.push_str(&" ".repeat(gap_width));
                plain_line.push_str(&description);
            }
        }

        plain_line.push_str(&" ".repeat(width.saturating_sub(display_width(&plain_line))));

        spans.push(Span::raw(" ".repeat(
            width.saturating_sub(display_width(plain_line.trim_end())),
        )));
        (Line::from(spans), plain_line)
    }

    fn move_skill_picker_selection(&mut self, delta: isize) {
        let visible_rows = self.file_picker_list_visible_rows();
        let Some(state) = self.skill_picker.as_mut() else {
            return;
        };
        if state.items.is_empty() {
            return;
        }

        let last = state.items.len() - 1;
        if delta.is_negative() {
            state.selected = state.selected.saturating_sub(delta.unsigned_abs());
        } else {
            state.selected = state.selected.saturating_add(delta as usize).min(last);
        }
        state.scroll = clamp_skill_picker_scroll(
            state.scroll,
            state.selected,
            state.items.len(),
            visible_rows,
        );
    }

    fn complete_skill_picker_common_prefix(&mut self) {
        let Some(state) = self.skill_picker.as_ref() else {
            return;
        };
        let prefix = common_skill_completion_prefix(&state.items, &state.query);
        if prefix.is_empty() || state.query == prefix {
            return;
        }

        self.replace_skill_picker_token(format!("${prefix}"));
    }

    fn insert_selected_skill_picker_skill(&mut self) -> bool {
        let Some(skill) = self
            .skill_picker
            .as_ref()
            .and_then(|state| state.items.get(state.selected))
            .cloned()
        else {
            return false;
        };

        let old_value = self.composer_text().to_string();
        let old_line = self.composer.line();
        let old_column = self.composer.column();
        if !self.composer.replace_current_skill_token(
            &skill.skill_name,
            &skill.skill_path,
            skill.origin,
        ) {
            return false;
        }
        self.dismissed_skill_picker_token = None;
        self.sync_command_panel_navigation();
        self.sync_composer_attached_picker_state();
        self.sync_external_editor_helper_after_draft_change(&old_value);
        self.sync_composer_height();
        self.sync_document_viewport_after_composer_interaction(&old_value, old_line, old_column);
        true
    }

    fn replace_skill_picker_token(&mut self, replacement: String) {
        let old_value = self.composer_text().to_string();
        let old_line = self.composer.line();
        let old_column = self.composer.column();
        if self
            .composer
            .replace_current_prefixed_token('$', &replacement)
        {
            self.dismissed_skill_picker_token = None;
            self.sync_command_panel_navigation();
            self.sync_composer_attached_picker_state();
            self.sync_external_editor_helper_after_draft_change(&old_value);
            self.sync_composer_height();
            self.sync_document_viewport_after_composer_interaction(
                &old_value, old_line, old_column,
            );
        }
    }

    pub(crate) fn close_skill_picker(&mut self) {
        self.skill_picker = None;
    }

    fn dismiss_current_skill_picker_token(&mut self) {
        self.dismissed_skill_picker_token = self.composer.current_skill_token();
    }
}

fn clamp_skill_picker_scroll(
    scroll: usize,
    selected: usize,
    item_count: usize,
    visible_rows: usize,
) -> usize {
    if item_count == 0 {
        return 0;
    }
    let visible_rows = visible_rows.max(1);
    let max_scroll = item_count.saturating_sub(visible_rows);
    let mut scroll = scroll.min(max_scroll);
    if selected < scroll {
        scroll = selected;
    }
    if selected >= scroll + visible_rows {
        scroll = selected + 1 - visible_rows;
    }
    scroll.min(max_scroll)
}

fn filter_manual_skill_items(
    skills: &[PromptAssemblyDiscoveredSkill],
    query: &str,
) -> Vec<PromptAssemblyDiscoveredSkill> {
    let trimmed_query = query.trim().to_ascii_lowercase();
    if trimmed_query.is_empty() {
        return skills.to_vec();
    }

    let mut prefix_matches = Vec::new();
    let mut fuzzy_matches = Vec::new();
    for skill in skills {
        let skill_name = skill.skill_name.to_ascii_lowercase();
        let title = skill_picker_display_name(skill).to_ascii_lowercase();
        let description = skill.description.to_ascii_lowercase();
        if skill_name.starts_with(&trimmed_query) || title.starts_with(&trimmed_query) {
            prefix_matches.push(skill.clone());
        } else if skill_name.contains(&trimmed_query)
            || title.contains(&trimmed_query)
            || description.contains(&trimmed_query)
        {
            fuzzy_matches.push(skill.clone());
        }
    }
    prefix_matches.extend(fuzzy_matches);
    prefix_matches
}

fn common_skill_completion_prefix(skills: &[PromptAssemblyDiscoveredSkill], query: &str) -> String {
    let mut iter = skills.iter().map(|skill| skill.skill_name.as_str());
    let Some(first) = iter.next() else {
        return String::new();
    };
    let mut prefix = first.to_string();
    for name in iter {
        let next_len = prefix
            .chars()
            .zip(name.chars())
            .take_while(|(left, right)| left == right)
            .count();
        prefix = prefix.chars().take(next_len).collect();
        if prefix.is_empty() {
            break;
        }
    }

    if prefix.len() <= query.len() {
        String::new()
    } else {
        prefix
    }
}

fn skill_picker_selectable_range(plain_line: &str, width: usize) -> SelectableLineRange {
    let end_column = display_width(plain_line.trim_end());
    if end_column <= SKILL_PICKER_INSET_WIDTH {
        return SelectableLineRange::blank_hit_range(0, width);
    }

    SelectableLineRange::new(SKILL_PICKER_INSET_WIDTH, end_column)
}

fn highlighted_query_spans(
    text: &str,
    query: &str,
    base_style: ratatui::style::Style,
    highlighted_style: ratatui::style::Style,
) -> Vec<Span<'static>> {
    let trimmed_query = query.trim();
    if trimmed_query.is_empty() {
        return vec![Span::styled(text.to_string(), base_style)];
    }

    let Some((match_start, match_end)) = find_case_insensitive_substring(text, trimmed_query)
    else {
        return vec![Span::styled(text.to_string(), base_style)];
    };

    let mut spans = Vec::new();
    if match_start > 0 {
        spans.push(Span::styled(
            slice_char_range(text, 0, match_start),
            base_style,
        ));
    }
    spans.push(Span::styled(
        slice_char_range(text, match_start, match_end),
        highlighted_style,
    ));
    let text_char_len = text.chars().count();
    if match_end < text_char_len {
        spans.push(Span::styled(
            slice_char_range(text, match_end, text_char_len),
            base_style,
        ));
    }
    spans
}

fn skill_picker_match_style(
    base_style: ratatui::style::Style,
    surface: Option<ratatui::style::Color>,
) -> ratatui::style::Style {
    match surface {
        Some(surface) => base_style.bg(surface),
        None => base_style.reversed(),
    }
}

fn find_case_insensitive_substring(text: &str, query: &str) -> Option<(usize, usize)> {
    let text_chars = text.chars().collect::<Vec<_>>();
    let query_chars = query.chars().collect::<Vec<_>>();
    if query_chars.is_empty() || query_chars.len() > text_chars.len() {
        return None;
    }

    let lowered_query = query_chars
        .iter()
        .flat_map(|ch| ch.to_lowercase())
        .collect::<String>();
    for start in 0..=text_chars.len() - query_chars.len() {
        let candidate = text_chars[start..start + query_chars.len()]
            .iter()
            .flat_map(|ch| ch.to_lowercase())
            .collect::<String>();
        if candidate == lowered_query {
            return Some((start, start + query_chars.len()));
        }
    }
    None
}

fn slice_char_range(text: &str, start: usize, end: usize) -> String {
    text.chars()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect()
}

fn skill_picker_display_name(item: &PromptAssemblyDiscoveredSkill) -> &str {
    let trimmed_title = item.title.trim();
    if trimmed_title.is_empty() {
        item.skill_name.as_str()
    } else {
        trimmed_title
    }
}

fn skill_picker_name_column_width(
    items: &[PromptAssemblyDiscoveredSkill],
    content_width: usize,
) -> usize {
    if items.is_empty() || content_width == 0 {
        return 0;
    }

    let max_name_width = items
        .iter()
        .map(skill_picker_display_name)
        .map(display_width)
        .max()
        .unwrap_or(0)
        .min(SKILL_PICKER_NAME_COLUMN_MAX_WIDTH);

    if content_width <= SKILL_PICKER_DESCRIPTION_MIN_WIDTH {
        return content_width;
    }

    max_name_width
        .min(
            content_width
                .saturating_sub(SKILL_PICKER_COLUMN_GAP + SKILL_PICKER_DESCRIPTION_MIN_WIDTH),
        )
        .max(1)
}

fn pad_display_width_right(text: &str, width: usize) -> String {
    let mut padded = text.to_string();
    let current_width = display_width(text);
    if current_width < width {
        padded.push_str(&" ".repeat(width - current_width));
    }
    padded
}
