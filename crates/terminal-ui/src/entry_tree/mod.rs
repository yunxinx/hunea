use std::collections::{HashMap, HashSet};

use crossterm::event::{KeyCode, KeyEvent, MouseButton};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph, Widget},
};
use runtime_domain::session::{
    SessionTreePayload, SessionTreeRow, SessionTreeRowKind, TranscriptReplayItem,
    TranscriptReplayRole,
};

use crate::{
    AppEffect, Model,
    display_width::display_width,
    render_frame::RenderFrame,
    status_line::truncate_display_width_with_ellipsis,
    theme::{
        accent_text_style, command_accent_text_style, muted_text_style, primary_text_style,
        secondary_text_style, subtle_rule_line, tertiary_text_style,
    },
    transcript::Transcript,
    transcript_overlay::{
        TranscriptOverlayProgressStyle, TranscriptOverlayRenderOptions, TranscriptOverlayState,
        render_transcript_overlay_view,
    },
};

#[cfg(test)]
mod tests;

const ENTRY_TREE_HEADER_HEIGHT: u16 = 1;
const ENTRY_TREE_HEADER_RULE_HEIGHT: u16 = 1;
const ENTRY_TREE_PAGE_RULE_HEIGHT: u16 = 1;
const ENTRY_TREE_FOOTER_HEIGHT: u16 = 1;
const ENTRY_TREE_CHROME_HEIGHT: u16 = ENTRY_TREE_HEADER_HEIGHT
    + ENTRY_TREE_HEADER_RULE_HEIGHT
    + ENTRY_TREE_PAGE_RULE_HEIGHT
    + ENTRY_TREE_FOOTER_HEIGHT;
const ENTRY_TREE_KIND_WIDTH: usize = 9;
const ENTRY_TREE_SELECTION_MARKER_WIDTH: usize = 2;
const ENTRY_TREE_KIND_PREFIX_WIDTH: usize = ENTRY_TREE_KIND_WIDTH + 1;
const ENTRY_TREE_GRAPH_MAX_WIDTH: usize = 12;
const ENTRY_TREE_GRAPH_MIN_WIDTH: usize = 2;
const ENTRY_TREE_MIN_SUMMARY_WIDTH: usize = 22;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct EntryTreeState {
    rows: Vec<SessionTreeRow>,
    selected: usize,
    is_loading: bool,
    error: Option<String>,
    preview: Option<EntryTreePreviewState>,
}

#[derive(Debug, Clone)]
struct EntryTreePreviewState {
    transcript: Transcript,
    overlay: TranscriptOverlayState,
    is_following_bottom: bool,
}

impl PartialEq for EntryTreePreviewState {
    fn eq(&self, other: &Self) -> bool {
        self.transcript == other.transcript && self.overlay == other.overlay
    }
}

impl Eq for EntryTreePreviewState {}

impl Model {
    pub(crate) fn entry_tree_active(&self) -> bool {
        self.entry_tree.is_some()
    }

    pub(crate) fn open_entry_tree_loading(&mut self) {
        self.entry_tree = Some(EntryTreeState {
            is_loading: true,
            ..EntryTreeState::default()
        });
    }

    pub(crate) fn apply_entry_tree_payload(&mut self, payload: SessionTreePayload) {
        let mut state = self.entry_tree.take().unwrap_or_default();
        state.rows = payload.rows;
        state.is_loading = false;
        state.error = None;
        state.preview = None;
        state.select_latest_row();
        self.entry_tree = Some(state);
    }

    pub(crate) fn move_entry_tree_selection(&mut self, direction: isize) {
        if let Some(state) = self.entry_tree.as_mut() {
            state.move_selection(direction);
        }
    }

    pub(crate) fn move_entry_tree_preview_page(&mut self, direction: isize) {
        let content_height = self.transcript_overlay_content_height();
        let Some(preview) = self
            .entry_tree
            .as_mut()
            .and_then(|state| state.preview.as_mut())
        else {
            return;
        };
        preview.overlay.scroll_offset = entry_tree_preview_page_offset(
            &mut preview.transcript,
            content_height,
            preview.overlay.scroll_offset,
            direction,
        );
        preview.is_following_bottom = false;
    }

    pub(crate) fn handle_entry_tree_key(&mut self, key: KeyEvent) -> Option<Option<AppEffect>> {
        if !self.entry_tree_active() {
            return None;
        }

        if self.entry_tree_preview_active() {
            return Some(self.handle_entry_tree_preview_key(key));
        }

        match key.code {
            KeyCode::Esc if key.modifiers.is_empty() => {
                self.entry_tree = None;
                Some(None)
            }
            KeyCode::Up | KeyCode::Char('k') if key.modifiers.is_empty() => {
                self.move_entry_tree_selection(-1);
                Some(None)
            }
            KeyCode::Down | KeyCode::Char('j') if key.modifiers.is_empty() => {
                self.move_entry_tree_selection(1);
                Some(None)
            }
            KeyCode::Left | KeyCode::Char('h') if key.modifiers.is_empty() => {
                let page_size = self.entry_tree_page_size();
                if let Some(state) = self.entry_tree.as_mut() {
                    state.move_page(-1, page_size);
                }
                Some(None)
            }
            KeyCode::Right | KeyCode::Char('l') if key.modifiers.is_empty() => {
                let page_size = self.entry_tree_page_size();
                if let Some(state) = self.entry_tree.as_mut() {
                    state.move_page(1, page_size);
                }
                Some(None)
            }
            KeyCode::Char(' ') if key.modifiers.is_empty() => {
                self.open_entry_tree_preview();
                Some(None)
            }
            KeyCode::Enter if key.modifiers.is_empty() => {
                let selected = self
                    .entry_tree
                    .as_ref()
                    .and_then(EntryTreeState::selected_row);
                if let Some(row) = selected
                    && row.rewind_target_id.is_some()
                {
                    let entry_id = row.row_id.clone();
                    let prefill = row.rewind_prefill.clone();
                    self.entry_tree = None;
                    return Some(Some(AppEffect::SelectEntryRewind { entry_id, prefill }));
                }
                Some(None)
            }
            _ => Some(None),
        }
    }

    pub(crate) fn render_entry_tree(&mut self, frame: &mut RenderFrame<'_>, area: Rect) {
        if self.entry_tree_preview_active() {
            self.render_entry_tree_preview(frame, area);
            return;
        }

        let Some(state) = self.entry_tree.as_ref() else {
            return;
        };
        frame.render_widget(Clear, area);
        if area.is_empty() || area.height < ENTRY_TREE_CHROME_HEIGHT {
            return;
        }

        let body_height = area.height.saturating_sub(ENTRY_TREE_CHROME_HEIGHT);
        let page_size = entry_tree_page_size_for_height(area.height);
        let header_area = Rect::new(area.x, area.y, area.width, ENTRY_TREE_HEADER_HEIGHT);
        let header_rule_area = Rect::new(
            area.x,
            area.y + ENTRY_TREE_HEADER_HEIGHT,
            area.width,
            ENTRY_TREE_HEADER_RULE_HEIGHT,
        );
        let body_area = Rect::new(
            area.x,
            area.y + ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT,
            area.width,
            body_height,
        );
        let page_rule_area = Rect::new(
            area.x,
            area.y
                + area
                    .height
                    .saturating_sub(ENTRY_TREE_PAGE_RULE_HEIGHT + ENTRY_TREE_FOOTER_HEIGHT),
            area.width,
            ENTRY_TREE_PAGE_RULE_HEIGHT,
        );
        let footer_area = Rect::new(
            area.x,
            area.y + area.height.saturating_sub(ENTRY_TREE_FOOTER_HEIGHT),
            area.width,
            ENTRY_TREE_FOOTER_HEIGHT,
        );

        frame.render_widget(
            Paragraph::new(self.entry_tree_header_line(state, usize::from(area.width))),
            header_area,
        );
        frame.render_widget(
            Paragraph::new(subtle_rule_line(usize::from(area.width), self.palette)),
            header_rule_area,
        );

        let lines =
            self.entry_tree_body_lines(state, usize::from(area.width), usize::from(body_height));
        frame.render_widget(EntryTreeWidget { lines: &lines }, body_area);

        frame.render_widget(
            Paragraph::new(build_entry_tree_page_rule(
                area.width,
                state.page_number(page_size),
                state.page_count(page_size),
                self.palette,
            )),
            page_rule_area,
        );
        frame.render_widget(
            Paragraph::new(Line::styled(
                entry_tree_footer_hint(
                    area.width,
                    state
                        .selected_row()
                        .is_some_and(|row| row.rewind_target_id.is_some()),
                ),
                tertiary_text_style(self.palette).add_modifier(Modifier::ITALIC),
            )),
            footer_area,
        );
    }

    fn entry_tree_page_size(&self) -> usize {
        entry_tree_page_size_for_height(self.height)
    }

    fn entry_tree_header_line(&self, state: &EntryTreeState, width: usize) -> Line<'static> {
        let title = format!(
            "Session Tree ({} of {})",
            state.selected_position_label(),
            state.rows.len()
        );
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                truncate_display_width_with_ellipsis(&title, width.saturating_sub(2).max(1)),
                primary_text_style(self.palette).bold(),
            ),
        ])
    }

    fn entry_tree_body_lines(
        &self,
        state: &EntryTreeState,
        width: usize,
        body_height: usize,
    ) -> Vec<Line<'static>> {
        let width = width.max(1);
        let page_size = entry_tree_page_size_for_height(
            u16::try_from(body_height)
                .unwrap_or(u16::MAX)
                .saturating_add(ENTRY_TREE_CHROME_HEIGHT),
        );
        let mut lines = Vec::new();

        if state.is_loading {
            lines.push(Line::styled(
                "  Loading session tree...",
                tertiary_text_style(self.palette),
            ));
        } else if let Some(error) = state.error.as_deref() {
            lines.push(Line::styled(
                truncate_display_width_with_ellipsis(&format!("  {error}"), width),
                tertiary_text_style(self.palette),
            ));
        } else if state.rows.is_empty() {
            lines.push(Line::styled(
                "  No messages",
                tertiary_text_style(self.palette),
            ));
        } else {
            let graph_lines = entry_tree_graph_lines(&state.rows, state.selected, width);
            let page_start = state.page_start(page_size);
            for (visible_position, row_index) in state.page_indices(page_size).enumerate() {
                let row = &state.rows[row_index];
                let absolute_position = page_start + visible_position;
                let graph_line = graph_lines.get(row_index).cloned().unwrap_or_default();
                lines.push(self.entry_tree_row_line(
                    row,
                    width,
                    graph_line,
                    absolute_position == state.selected,
                ));
            }
        }

        lines.truncate(body_height);
        lines
    }

    fn entry_tree_row_line(
        &self,
        row: &SessionTreeRow,
        width: usize,
        graph_line: EntryTreeGraphLine,
        is_selected: bool,
    ) -> Line<'static> {
        let marker = if is_selected { "❯ " } else { "  " };
        let kind = entry_tree_kind_label(row.kind);
        let kind_prefix = format!("{kind:<ENTRY_TREE_KIND_WIDTH$} ");
        let prefix_width =
            display_width(marker) + graph_line.display_width() + display_width(&kind_prefix);
        let text_width = width.saturating_sub(prefix_width);
        let text_style = entry_tree_content_style(row, self.palette, is_selected);
        let selected_text_style = if is_selected {
            text_style.add_modifier(Modifier::REVERSED)
        } else {
            text_style
        };
        let kind_style = entry_tree_kind_style(row.kind, self.palette);
        let marker_style = if is_selected {
            command_accent_text_style(self.palette).bold()
        } else {
            muted_text_style(self.palette)
        };

        let mut spans = vec![Span::styled(marker.to_string(), marker_style)];
        spans.extend(graph_line.spans.into_iter().map(|span| {
            Span::styled(
                span.text,
                entry_tree_graph_span_style(span.is_selected_branch, self.palette),
            )
        }));
        spans.extend([
            Span::styled(kind_prefix, kind_style),
            Span::styled(
                truncate_display_width_with_ellipsis(&row.summary, text_width),
                selected_text_style,
            ),
        ]);

        Line::from(spans)
    }

    pub(crate) fn handle_entry_tree_mouse_down(
        &mut self,
        button: MouseButton,
        _column: u16,
        row: u16,
    ) -> Option<AppEffect> {
        if button != MouseButton::Left
            || !self.entry_tree_active()
            || self.entry_tree_preview_active()
        {
            return None;
        }
        if self.height < ENTRY_TREE_CHROME_HEIGHT {
            return None;
        }

        let body_top = ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT;
        let body_height = self.height.saturating_sub(ENTRY_TREE_CHROME_HEIGHT);
        if row < body_top || row >= body_top.saturating_add(body_height) {
            return None;
        }

        let page_size = self.entry_tree_page_size();
        let visible_offset = usize::from(row.saturating_sub(body_top));
        if let Some(state) = self.entry_tree.as_mut() {
            state.select_visible_row(page_size, visible_offset);
        }
        None
    }

    pub(crate) fn entry_tree_preview_active(&self) -> bool {
        self.entry_tree
            .as_ref()
            .is_some_and(|state| state.preview.is_some())
    }

    fn open_entry_tree_preview(&mut self) {
        let selected_row = self
            .entry_tree
            .as_ref()
            .and_then(EntryTreeState::selected_row)
            .cloned();
        let Some(row) = selected_row else {
            return;
        };

        let transcript =
            self.transcript_from_replay_items(vec![entry_tree_preview_replay_item(&row)]);
        let content_height = self.transcript_overlay_content_height();
        let mut preview = EntryTreePreviewState {
            transcript,
            overlay: TranscriptOverlayState::new(),
            is_following_bottom: true,
        };
        preview.overlay.scroll_offset =
            latest_entry_tree_preview_offset(&mut preview.transcript, content_height);

        if let Some(state) = self.entry_tree.as_mut() {
            state.preview = Some(preview);
        }
    }

    fn close_entry_tree_preview(&mut self) {
        if let Some(state) = self.entry_tree.as_mut() {
            state.preview = None;
        }
    }

    fn handle_entry_tree_preview_key(&mut self, key: KeyEvent) -> Option<AppEffect> {
        match key.code {
            KeyCode::Esc | KeyCode::Char(' ') if key.modifiers.is_empty() => {
                self.close_entry_tree_preview();
                None
            }
            KeyCode::Left | KeyCode::Up | KeyCode::Char('h') if key.modifiers.is_empty() => {
                self.move_entry_tree_preview_page(-1);
                None
            }
            KeyCode::Right | KeyCode::Down | KeyCode::Char('l') if key.modifiers.is_empty() => {
                self.move_entry_tree_preview_page(1);
                None
            }
            _ => None,
        }
    }

    fn render_entry_tree_preview(&mut self, frame: &mut RenderFrame<'_>, area: Rect) {
        let palette = self.palette;
        let content_height = area.height.saturating_sub(2).max(1) as usize;
        let Some(preview) = self
            .entry_tree
            .as_mut()
            .and_then(|state| state.preview.as_mut())
        else {
            return;
        };
        if preview.is_following_bottom {
            preview.overlay.scroll_offset =
                latest_entry_tree_preview_offset(&mut preview.transcript, content_height);
        }
        render_transcript_overlay_view(
            frame,
            area,
            &mut preview.transcript,
            &mut preview.overlay,
            TranscriptOverlayRenderOptions {
                palette,
                content_height,
                footer_hint: entry_tree_preview_footer_hint(area.width),
                progress_style: TranscriptOverlayProgressStyle::Page,
            },
        );
    }
}

impl EntryTreeState {
    fn select_latest_row(&mut self) {
        self.selected = self.rows.len().saturating_sub(1);
    }

    fn move_selection(&mut self, direction: isize) {
        if self.rows.is_empty() {
            self.selected = 0;
            return;
        }
        let last = self.rows.len().saturating_sub(1);
        self.selected = if direction.is_negative() {
            self.selected.saturating_sub(direction.unsigned_abs())
        } else {
            self.selected.saturating_add(direction as usize).min(last)
        };
    }

    fn move_page(&mut self, direction: isize, page_size: usize) {
        if self.rows.is_empty() {
            self.selected = 0;
            return;
        }
        let page_size = page_size.max(1);
        let current_page = self.selected / page_size;
        let last_page = self.rows.len().saturating_sub(1) / page_size;
        let next_page = if direction.is_negative() {
            current_page.saturating_sub(direction.unsigned_abs())
        } else {
            current_page
                .saturating_add(direction as usize)
                .min(last_page)
        };
        self.selected = (next_page * page_size).min(self.rows.len().saturating_sub(1));
    }

    fn selected_row(&self) -> Option<&SessionTreeRow> {
        self.rows.get(self.selected)
    }

    fn select_visible_row(&mut self, page_size: usize, visible_offset: usize) {
        let row_index = self.page_start(page_size).saturating_add(visible_offset);
        if row_index < self.rows.len() {
            self.selected = row_index;
        }
    }

    fn page_start(&self, page_size: usize) -> usize {
        let page_size = page_size.max(1);
        self.selected / page_size * page_size
    }

    fn page_indices(&self, page_size: usize) -> impl Iterator<Item = usize> + '_ {
        let page_size = page_size.max(1);
        (self.page_start(page_size)..self.rows.len()).take(page_size)
    }

    fn page_number(&self, page_size: usize) -> usize {
        if self.rows.is_empty() {
            return 1;
        }
        self.selected / page_size.max(1) + 1
    }

    fn page_count(&self, page_size: usize) -> usize {
        if self.rows.is_empty() {
            return 1;
        }
        self.rows.len().saturating_sub(1) / page_size.max(1) + 1
    }

    fn selected_position_label(&self) -> usize {
        if self.rows.is_empty() {
            0
        } else {
            self.selected + 1
        }
    }
}

struct EntryTreeWidget<'a> {
    lines: &'a [Line<'static>],
}

impl Widget for EntryTreeWidget<'_> {
    fn render(self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        for (row, line) in self.lines.iter().take(usize::from(area.height)).enumerate() {
            let y = area.y + u16::try_from(row).unwrap_or(u16::MAX);
            let row_area = Rect::new(area.x, y, area.width, 1);
            if line.style != Style::new() {
                buf.set_style(row_area, line.style);
            }
            buf.set_line(area.x, y, line, area.width);
        }
    }
}

fn entry_tree_preview_replay_item(row: &SessionTreeRow) -> TranscriptReplayItem {
    match row.kind {
        SessionTreeRowKind::User => TranscriptReplayItem::Message {
            role: TranscriptReplayRole::User,
            content: row.preview_content.clone(),
        },
        SessionTreeRowKind::Assistant => TranscriptReplayItem::Message {
            role: TranscriptReplayRole::Assistant,
            content: row.preview_content.clone(),
        },
        SessionTreeRowKind::Tool => TranscriptReplayItem::ToolResult {
            content: row.preview_content.clone(),
        },
        SessionTreeRowKind::Reasoning => TranscriptReplayItem::Reasoning {
            content: row.preview_content.clone(),
        },
    }
}

fn latest_entry_tree_preview_offset(transcript: &mut Transcript, content_height: usize) -> usize {
    let content_height = content_height.max(1);
    let mut index = transcript.progressive_item_metrics_index();
    if index.line_count == 0 {
        return 0;
    }

    let mut offset = index.line_count.saturating_sub(content_height);
    let mut remaining_exactization_passes = index.metrics.len().saturating_add(1);
    while remaining_exactization_passes > 0 {
        let effective_total = index.line_count;
        if effective_total == 0 {
            return 0;
        }

        let next_offset = effective_total.saturating_sub(content_height);
        let visible_line_count = content_height.min(effective_total.saturating_sub(next_offset));
        let window = transcript.materialize_line_window(next_offset, visible_line_count);
        let exact_offset = window.index.line_count.saturating_sub(content_height);
        if exact_offset == offset {
            return exact_offset;
        }

        offset = exact_offset;
        index = window.index;
        remaining_exactization_passes -= 1;
    }

    offset
}

fn entry_tree_preview_page_offset(
    transcript: &mut Transcript,
    content_height: usize,
    current_offset: usize,
    direction: isize,
) -> usize {
    let content_height = content_height.max(1);
    let latest_offset = latest_entry_tree_preview_offset(transcript, content_height);
    let index = transcript.progressive_item_metrics_index();
    let total_lines = index.line_count;
    if total_lines == 0 {
        return 0;
    }

    let page_count = total_lines.saturating_sub(1) / content_height + 1;
    let current_page = if current_offset >= latest_offset {
        page_count
    } else {
        current_offset / content_height + 1
    };
    let next_page = if direction.is_negative() {
        current_page.saturating_sub(1).max(1)
    } else {
        current_page.saturating_add(1).min(page_count)
    };

    if next_page >= page_count {
        latest_offset
    } else {
        (next_page - 1) * content_height
    }
}

fn entry_tree_page_size_for_height(height: u16) -> usize {
    usize::from(height.saturating_sub(ENTRY_TREE_CHROME_HEIGHT)).max(1)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EntryTreeGraphParts {
    ancestor_segments: Vec<EntryTreeGraphSpan>,
    own_segment: EntryTreeGraphSpan,
    compact_node_segment: String,
    is_selected_branch: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct EntryTreeGraphLine {
    spans: Vec<EntryTreeGraphSpan>,
}

impl EntryTreeGraphLine {
    fn display_width(&self) -> usize {
        self.spans
            .iter()
            .map(|span| display_width(&span.text))
            .sum()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EntryTreeGraphSpan {
    text: String,
    is_selected_branch: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EntryTreeGraphLayout {
    graph_parent_by_row: Vec<Option<usize>>,
    graph_children_by_parent: HashMap<Option<usize>, Vec<usize>>,
    layout_depth_by_row: Vec<usize>,
    selected_branch_by_row: Vec<bool>,
    selected_lane_depth_by_row: Vec<Option<usize>>,
    selected_branch_start_index: Option<usize>,
    selected_branch_end_index: Option<usize>,
}

fn entry_tree_graph_lines(
    rows: &[SessionTreeRow],
    selected_index: usize,
    width: usize,
) -> Vec<EntryTreeGraphLine> {
    let graph_width_budget = entry_tree_graph_width_budget(width);
    if graph_width_budget == 0 {
        return vec![EntryTreeGraphLine::default(); rows.len()];
    }

    let layout = EntryTreeGraphLayout::new(rows, selected_index);
    rows.iter()
        .enumerate()
        .map(|(row_index, _)| entry_tree_graph_line(rows, &layout, row_index, graph_width_budget))
        .collect()
}

impl EntryTreeGraphLayout {
    fn new(rows: &[SessionTreeRow], selected_index: usize) -> Self {
        let row_index_by_id = entry_tree_row_index_by_id(rows);
        let graph_parent_by_row = entry_tree_graph_parent_by_row(rows, &row_index_by_id);
        let graph_children_by_parent =
            entry_tree_graph_children_by_parent(rows, &graph_parent_by_row);
        let branch_parent_rows = entry_tree_branch_parent_rows(&graph_children_by_parent);
        let layout_depth_by_row =
            entry_tree_graph_layout_depth_by_row(rows, &graph_parent_by_row, &branch_parent_rows);
        let selected_projection_path = entry_tree_selected_projection_path(
            rows,
            selected_index,
            &graph_parent_by_row,
            &graph_children_by_parent,
            &row_index_by_id,
        );
        let selected_projection_nodes = selected_projection_path
            .iter()
            .copied()
            .collect::<HashSet<_>>();
        let selected_branch_by_row = entry_tree_selected_projection_by_row(
            rows,
            &selected_projection_nodes,
            &row_index_by_id,
        );
        let selected_branch_start_index = entry_tree_selected_projection_start(
            &selected_projection_path,
            &graph_children_by_parent,
        );
        let selected_branch_end_index = selected_projection_path.last().copied();
        let selected_lane_depth_by_row = entry_tree_selected_lane_depth_by_row(
            rows,
            &selected_projection_path,
            &graph_children_by_parent,
            &layout_depth_by_row,
            &selected_branch_by_row,
        );

        Self {
            graph_parent_by_row,
            graph_children_by_parent,
            layout_depth_by_row,
            selected_branch_by_row,
            selected_lane_depth_by_row,
            selected_branch_start_index,
            selected_branch_end_index,
        }
    }
}

fn entry_tree_graph_width_budget(width: usize) -> usize {
    let right_side_min_width = ENTRY_TREE_KIND_PREFIX_WIDTH + ENTRY_TREE_MIN_SUMMARY_WIDTH;
    let available = width.saturating_sub(ENTRY_TREE_SELECTION_MARKER_WIDTH + right_side_min_width);
    if available >= ENTRY_TREE_GRAPH_MIN_WIDTH {
        return available.min(ENTRY_TREE_GRAPH_MAX_WIDTH);
    }

    width
        .saturating_sub(ENTRY_TREE_SELECTION_MARKER_WIDTH + ENTRY_TREE_KIND_PREFIX_WIDTH)
        .min(ENTRY_TREE_GRAPH_MIN_WIDTH)
}

fn entry_tree_graph_line(
    rows: &[SessionTreeRow],
    layout: &EntryTreeGraphLayout,
    row_index: usize,
    graph_width_budget: usize,
) -> EntryTreeGraphLine {
    let graph_parts = entry_tree_graph_parts(rows, layout, row_index);
    let mut spans = graph_parts.ancestor_segments.clone();
    spans.push(graph_parts.own_segment.clone());

    let graph_line = EntryTreeGraphLine { spans };
    if graph_line.display_width() <= graph_width_budget {
        return graph_line;
    }

    let has_selected_graph_span = graph_parts.is_selected_branch
        || graph_parts.own_segment.is_selected_branch
        || graph_parts
            .ancestor_segments
            .iter()
            .any(|span| span.is_selected_branch);
    EntryTreeGraphLine {
        spans: vec![EntryTreeGraphSpan {
            text: collapsed_entry_tree_graph_prefix(&graph_parts, graph_width_budget),
            is_selected_branch: has_selected_graph_span,
        }],
    }
}

fn entry_tree_graph_parts(
    rows: &[SessionTreeRow],
    layout: &EntryTreeGraphLayout,
    row_index: usize,
) -> EntryTreeGraphParts {
    let depth = layout.layout_depth_by_row[row_index];
    let own_segment = entry_tree_graph_own_segment(rows, layout, row_index);
    let is_selected_branch = layout.selected_branch_by_row[row_index];
    if depth == 0 {
        return EntryTreeGraphParts {
            ancestor_segments: Vec::new(),
            compact_node_segment: own_segment.text.clone(),
            own_segment,
            is_selected_branch,
        };
    }

    let ancestor_segments = (0..depth.saturating_sub(1))
        .map(|ancestor_depth| {
            let lane_depth = ancestor_depth.saturating_add(1);
            if layout.selected_lane_depth_by_row[row_index] == Some(lane_depth) {
                EntryTreeGraphSpan {
                    text: "│  ".to_string(),
                    is_selected_branch: true,
                }
            } else {
                EntryTreeGraphSpan {
                    text: "   ".to_string(),
                    is_selected_branch: false,
                }
            }
        })
        .collect::<Vec<_>>();

    EntryTreeGraphParts {
        ancestor_segments,
        compact_node_segment: entry_tree_graph_compact_segment(rows, layout, row_index).to_string(),
        own_segment,
        is_selected_branch,
    }
}

fn entry_tree_graph_own_segment(
    rows: &[SessionTreeRow],
    layout: &EntryTreeGraphLayout,
    row_index: usize,
) -> EntryTreeGraphSpan {
    let depth = layout.layout_depth_by_row[row_index];
    let has_selected_own_lane =
        depth > 0 && layout.selected_lane_depth_by_row[row_index] == Some(depth);
    let is_selected_projection = layout.selected_branch_by_row[row_index];
    let is_selected_graph = is_selected_projection || has_selected_own_lane;
    let Some(node) = entry_tree_graph_node(rows, layout, row_index) else {
        let text = if depth == 0 {
            if is_selected_projection {
                "│ ".to_string()
            } else {
                "  ".to_string()
            }
        } else if has_selected_own_lane {
            "│   ".to_string()
        } else {
            "    ".to_string()
        };
        return EntryTreeGraphSpan {
            text,
            is_selected_branch: is_selected_graph,
        };
    };
    if depth == 0 {
        return EntryTreeGraphSpan {
            text: format!("{node} "),
            is_selected_branch: is_selected_graph,
        };
    }

    if !entry_tree_row_is_branch_choice(layout, row_index) {
        let text = if has_selected_own_lane {
            format!("│ {node} ")
        } else {
            format!("  {node} ")
        };
        return EntryTreeGraphSpan {
            text,
            is_selected_branch: is_selected_graph,
        };
    }

    let connector = if entry_tree_row_has_later_graph_sibling(layout, row_index) {
        "├─"
    } else {
        "╰─"
    };
    EntryTreeGraphSpan {
        text: format!("{connector}{node} "),
        is_selected_branch: is_selected_graph,
    }
}

fn entry_tree_graph_compact_segment(
    rows: &[SessionTreeRow],
    layout: &EntryTreeGraphLayout,
    row_index: usize,
) -> &'static str {
    match entry_tree_graph_node(rows, layout, row_index) {
        Some("@") => "@ ",
        Some("●") => "● ",
        Some("·") => "· ",
        _ if layout.selected_lane_depth_by_row[row_index].is_some()
            || layout.selected_branch_by_row[row_index] =>
        {
            "│ "
        }
        _ => "  ",
    }
}

fn collapsed_entry_tree_graph_prefix(
    graph_parts: &EntryTreeGraphParts,
    graph_width_budget: usize,
) -> String {
    let ancestor_count = graph_parts.ancestor_segments.len();

    for kept_ancestor_count in (0..ancestor_count).rev() {
        let hidden_count = ancestor_count.saturating_sub(kept_ancestor_count);
        let mut prefix = format!("…{hidden_count} ");
        for segment in &graph_parts.ancestor_segments[hidden_count..] {
            prefix.push_str(&segment.text);
        }
        prefix.push_str(&graph_parts.own_segment.text);

        if display_width(&prefix) <= graph_width_budget {
            return prefix;
        }
    }

    if display_width(&graph_parts.compact_node_segment) <= graph_width_budget {
        return graph_parts.compact_node_segment.clone();
    }

    truncate_display_width_with_ellipsis(&graph_parts.compact_node_segment, graph_width_budget)
}

fn entry_tree_row_has_later_graph_sibling(layout: &EntryTreeGraphLayout, row_index: usize) -> bool {
    let Some(parent_index) = layout.graph_parent_by_row[row_index] else {
        return false;
    };
    let Some(siblings) = layout.graph_children_by_parent.get(&Some(parent_index)) else {
        return false;
    };
    let Some(position) = siblings
        .iter()
        .position(|sibling_index| *sibling_index == row_index)
    else {
        return false;
    };

    position + 1 < siblings.len()
}

fn entry_tree_row_is_branch_choice(layout: &EntryTreeGraphLayout, row_index: usize) -> bool {
    layout.graph_parent_by_row[row_index].is_some_and(|parent_index| {
        layout
            .graph_children_by_parent
            .get(&Some(parent_index))
            .is_some_and(|children| children.len() > 1)
    })
}

fn entry_tree_graph_node(
    rows: &[SessionTreeRow],
    layout: &EntryTreeGraphLayout,
    row_index: usize,
) -> Option<&'static str> {
    let row = &rows[row_index];
    if !entry_tree_row_has_graph_node(row) || !layout.selected_branch_by_row[row_index] {
        return None;
    }
    if entry_tree_row_is_branch_parent(layout, row_index) {
        Some("@")
    } else if Some(row_index) == layout.selected_branch_start_index
        || Some(row_index) == layout.selected_branch_end_index
    {
        Some("●")
    } else {
        Some("·")
    }
}

fn entry_tree_row_is_branch_parent(layout: &EntryTreeGraphLayout, row_index: usize) -> bool {
    layout
        .graph_children_by_parent
        .get(&Some(row_index))
        .is_some_and(|children| children.len() > 1)
}

fn entry_tree_row_has_graph_node(row: &SessionTreeRow) -> bool {
    matches!(
        row.kind,
        SessionTreeRowKind::User | SessionTreeRowKind::Assistant
    )
}

fn entry_tree_row_index_by_id(rows: &[SessionTreeRow]) -> HashMap<&str, usize> {
    rows.iter()
        .enumerate()
        .map(|(row_index, row)| (row.row_id.as_str(), row_index))
        .collect()
}

fn entry_tree_graph_parent_by_row(
    rows: &[SessionTreeRow],
    row_index_by_id: &HashMap<&str, usize>,
) -> Vec<Option<usize>> {
    rows.iter()
        .enumerate()
        .map(|(row_index, row)| {
            if entry_tree_row_has_graph_node(row) {
                nearest_graph_ancestor_index(rows, row_index, row_index_by_id)
            } else {
                None
            }
        })
        .collect()
}

fn nearest_graph_ancestor_index(
    rows: &[SessionTreeRow],
    row_index: usize,
    row_index_by_id: &HashMap<&str, usize>,
) -> Option<usize> {
    let mut parent_id = rows[row_index].parent_id.as_deref();
    let mut visited = HashSet::new();

    while let Some(current_parent_id) = parent_id {
        let parent_index = *row_index_by_id.get(current_parent_id)?;
        if !visited.insert(parent_index) {
            return None;
        }
        let parent = &rows[parent_index];
        if entry_tree_row_has_graph_node(parent) {
            return Some(parent_index);
        }
        parent_id = parent.parent_id.as_deref();
    }

    None
}

fn entry_tree_graph_children_by_parent(
    rows: &[SessionTreeRow],
    graph_parent_by_row: &[Option<usize>],
) -> HashMap<Option<usize>, Vec<usize>> {
    let mut children_by_parent: HashMap<Option<usize>, Vec<usize>> = HashMap::new();
    for (row_index, row) in rows.iter().enumerate() {
        if entry_tree_row_has_graph_node(row) {
            children_by_parent
                .entry(graph_parent_by_row[row_index])
                .or_default()
                .push(row_index);
        }
    }
    children_by_parent
}

fn entry_tree_branch_parent_rows(
    graph_children_by_parent: &HashMap<Option<usize>, Vec<usize>>,
) -> HashSet<usize> {
    graph_children_by_parent
        .iter()
        .filter_map(|(parent_index, children)| {
            if children.len() > 1 {
                *parent_index
            } else {
                None
            }
        })
        .collect()
}

fn entry_tree_graph_parent_has_multiple_children(
    graph_children_by_parent: &HashMap<Option<usize>, Vec<usize>>,
    parent_index: usize,
) -> bool {
    graph_children_by_parent
        .get(&Some(parent_index))
        .is_some_and(|children| children.len() > 1)
}

fn entry_tree_graph_layout_depth_by_row(
    rows: &[SessionTreeRow],
    graph_parent_by_row: &[Option<usize>],
    branch_parent_rows: &HashSet<usize>,
) -> Vec<usize> {
    let row_index_by_id = entry_tree_row_index_by_id(rows);
    let mut graph_depth_by_row = vec![None; rows.len()];
    for (row_index, row) in rows.iter().enumerate() {
        if entry_tree_row_has_graph_node(row) {
            let mut visiting = HashSet::new();
            let depth = entry_tree_graph_node_layout_depth(
                row_index,
                graph_parent_by_row,
                branch_parent_rows,
                &mut graph_depth_by_row,
                &mut visiting,
            );
            graph_depth_by_row[row_index] = Some(depth);
        }
    }

    rows.iter()
        .enumerate()
        .map(|(row_index, row)| {
            if entry_tree_row_has_graph_node(row) {
                graph_depth_by_row[row_index].unwrap_or_default()
            } else {
                nearest_graph_ancestor_index(rows, row_index, &row_index_by_id)
                    .and_then(|ancestor_index| graph_depth_by_row[ancestor_index])
                    .unwrap_or_default()
            }
        })
        .collect()
}

fn entry_tree_graph_node_layout_depth(
    row_index: usize,
    graph_parent_by_row: &[Option<usize>],
    branch_parent_rows: &HashSet<usize>,
    graph_depth_by_row: &mut [Option<usize>],
    visiting: &mut HashSet<usize>,
) -> usize {
    if let Some(depth) = graph_depth_by_row[row_index] {
        return depth;
    }
    if !visiting.insert(row_index) {
        return 0;
    }

    let depth = match graph_parent_by_row[row_index] {
        Some(parent_index) => {
            let parent_depth = entry_tree_graph_node_layout_depth(
                parent_index,
                graph_parent_by_row,
                branch_parent_rows,
                graph_depth_by_row,
                visiting,
            );
            if branch_parent_rows.contains(&parent_index) {
                parent_depth.saturating_add(1)
            } else {
                parent_depth
            }
        }
        None => 0,
    };

    visiting.remove(&row_index);
    graph_depth_by_row[row_index] = Some(depth);
    depth
}

fn entry_tree_selected_projection_start(
    selected_path: &[usize],
    graph_children_by_parent: &HashMap<Option<usize>, Vec<usize>>,
) -> Option<usize> {
    if selected_path.is_empty() {
        return None;
    }

    selected_path
        .windows(2)
        .find_map(|path_pair| {
            let parent_index = path_pair[0];
            let branch_root_index = path_pair[1];
            entry_tree_graph_parent_has_multiple_children(graph_children_by_parent, parent_index)
                .then_some(branch_root_index)
        })
        .or_else(|| selected_path.first().copied())
}

fn entry_tree_selected_lane_depth_by_row(
    rows: &[SessionTreeRow],
    selected_path: &[usize],
    graph_children_by_parent: &HashMap<Option<usize>, Vec<usize>>,
    layout_depth_by_row: &[usize],
    selected_branch_by_row: &[bool],
) -> Vec<Option<usize>> {
    let mut selected_lane_depth_by_row = vec![None; rows.len()];
    let Some(selected_branch_end) = selected_branch_by_row
        .iter()
        .rposition(|is_selected_branch| *is_selected_branch)
    else {
        return selected_lane_depth_by_row;
    };

    for selected_path_pair in selected_path.windows(2) {
        let parent_index = selected_path_pair[0];
        let branch_root_index = selected_path_pair[1];
        if !entry_tree_graph_parent_has_multiple_children(graph_children_by_parent, parent_index) {
            continue;
        }

        let lane_depth = layout_depth_by_row[branch_root_index];
        if lane_depth == 0 || branch_root_index >= selected_branch_end {
            continue;
        }

        for row_index in branch_root_index.saturating_add(1)..=selected_branch_end {
            if layout_depth_by_row[row_index] >= lane_depth {
                selected_lane_depth_by_row[row_index] = Some(lane_depth);
            }
        }
    }

    selected_lane_depth_by_row
}

fn entry_tree_selected_projection_path(
    rows: &[SessionTreeRow],
    selected_index: usize,
    graph_parent_by_row: &[Option<usize>],
    graph_children_by_parent: &HashMap<Option<usize>, Vec<usize>>,
    row_index_by_id: &HashMap<&str, usize>,
) -> Vec<usize> {
    let Some(selected_owner_index) =
        entry_tree_row_graph_owner_index(rows, selected_index, row_index_by_id)
    else {
        return Vec::new();
    };
    let mut projection_path =
        entry_tree_graph_path_to_root(selected_owner_index, graph_parent_by_row);
    let mut current_index = selected_owner_index;
    let mut visited = projection_path.iter().copied().collect::<HashSet<_>>();

    while let Some(next_index) =
        entry_tree_selected_projection_next_child(rows, graph_children_by_parent, current_index)
    {
        if !visited.insert(next_index) {
            break;
        }
        projection_path.push(next_index);
        current_index = next_index;
    }

    projection_path
}

fn entry_tree_selected_projection_next_child(
    rows: &[SessionTreeRow],
    graph_children_by_parent: &HashMap<Option<usize>, Vec<usize>>,
    parent_index: usize,
) -> Option<usize> {
    let children = graph_children_by_parent.get(&Some(parent_index))?;
    children
        .iter()
        .copied()
        .find(|child_index| {
            rows[*child_index].is_current
                || (rows[*child_index].is_active_path
                    && entry_tree_row_has_graph_node(&rows[*child_index]))
        })
        .or_else(|| (children.len() == 1).then_some(children[0]))
}

fn entry_tree_selected_projection_by_row(
    rows: &[SessionTreeRow],
    selected_projection_nodes: &HashSet<usize>,
    row_index_by_id: &HashMap<&str, usize>,
) -> Vec<bool> {
    rows.iter()
        .enumerate()
        .map(|(row_index, _)| {
            entry_tree_row_graph_owner_index(rows, row_index, row_index_by_id)
                .is_some_and(|owner_index| selected_projection_nodes.contains(&owner_index))
        })
        .collect()
}

fn entry_tree_row_graph_owner_index(
    rows: &[SessionTreeRow],
    row_index: usize,
    row_index_by_id: &HashMap<&str, usize>,
) -> Option<usize> {
    let row = rows.get(row_index)?;
    if entry_tree_row_has_graph_node(row) {
        Some(row_index)
    } else {
        nearest_graph_ancestor_index(rows, row_index, row_index_by_id)
    }
}

fn entry_tree_graph_path_to_root(
    row_index: usize,
    graph_parent_by_row: &[Option<usize>],
) -> Vec<usize> {
    let mut path = Vec::new();
    let mut current_index = Some(row_index);
    let mut visited = HashSet::new();

    while let Some(row_index) = current_index {
        if !visited.insert(row_index) {
            break;
        }
        path.push(row_index);
        current_index = graph_parent_by_row[row_index];
    }

    path.reverse();
    path
}

fn entry_tree_graph_span_style(
    is_selected_branch: bool,
    palette: crate::theme::TerminalPalette,
) -> Style {
    if is_selected_branch {
        accent_text_style(palette)
    } else {
        tertiary_text_style(palette)
    }
}

fn entry_tree_kind_label(kind: SessionTreeRowKind) -> &'static str {
    match kind {
        SessionTreeRowKind::User => "user",
        SessionTreeRowKind::Assistant => "assistant",
        SessionTreeRowKind::Tool => "tool",
        SessionTreeRowKind::Reasoning => "reasoning",
    }
}

fn entry_tree_content_style(
    row: &SessionTreeRow,
    palette: crate::theme::TerminalPalette,
    is_selected: bool,
) -> Style {
    match row.kind {
        SessionTreeRowKind::User => command_accent_text_style(palette),
        SessionTreeRowKind::Reasoning => tertiary_text_style(palette).italic(),
        SessionTreeRowKind::Tool => muted_text_style(palette),
        SessionTreeRowKind::Assistant if is_selected => primary_text_style(palette).bold(),
        SessionTreeRowKind::Assistant if row.is_active_path => primary_text_style(palette),
        SessionTreeRowKind::Assistant => secondary_text_style(palette),
    }
}

fn entry_tree_kind_style(
    kind: SessionTreeRowKind,
    palette: crate::theme::TerminalPalette,
) -> Style {
    match kind {
        SessionTreeRowKind::Tool => primary_text_style(palette).bg(palette.accent),
        SessionTreeRowKind::User
        | SessionTreeRowKind::Assistant
        | SessionTreeRowKind::Reasoning => tertiary_text_style(palette),
    }
}

fn build_entry_tree_page_rule(
    width: u16,
    page_number: usize,
    page_count: usize,
    palette: crate::theme::TerminalPalette,
) -> Line<'static> {
    let width = usize::from(width);
    let compact_label = format!(" {page_number}/{page_count} ");
    let full_label = format!(" Page {page_number}/{page_count} ");
    let label = if width >= 24 {
        full_label
    } else {
        compact_label
    };
    let label_width = display_width(&label);
    let right_pad = 2usize;

    if width <= label_width + right_pad {
        return Line::styled(label, muted_text_style(palette));
    }

    let left_dash_count = width.saturating_sub(label_width + right_pad);
    let mut line = String::with_capacity(width);
    line.push_str(&"─".repeat(left_dash_count));
    line.push_str(&label);
    line.push_str(&"─".repeat(right_pad));

    Line::styled(line, muted_text_style(palette))
}

fn entry_tree_footer_hint(width: u16, can_rewind_selected_row: bool) -> &'static str {
    match (width < 76, can_rewind_selected_row) {
        (true, true) => "  Esc close · Space preview · Enter rewind · j/k · h/l page",
        (true, false) => "  Esc close · Space preview · j/k · h/l page",
        (false, true) => "  Esc close · Space preview · Enter rewind · ↑/↓/j/k move · ←/→/h/l page",
        (false, false) => "  Esc close · Space preview · ↑/↓/j/k move · ←/→/h/l page",
    }
}

fn entry_tree_preview_footer_hint(width: u16) -> &'static str {
    if width < 76 {
        "  Esc/Space back · ↑/←/h prev · ↓/→/l next"
    } else {
        "  Esc/Space back · ↑/←/h previous page · ↓/→/l next page"
    }
}
