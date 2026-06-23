use crate::{list_selection::PagedSelection, theme::TerminalPalette};

use super::*;

#[derive(Clone, Copy)]
pub(in crate::entry_tree::render) struct EntryTreeRowsRenderState<'a> {
    rows: &'a [SessionTreeRow],
    selected: usize,
    is_loading: bool,
    error: Option<&'a str>,
    loading_text: &'static str,
    empty_text: &'static str,
}

impl<'a> EntryTreeRowsRenderState<'a> {
    pub(in crate::entry_tree::render) fn from_tree(state: &'a EntryTreeState) -> Self {
        Self {
            rows: &state.rows,
            selected: state.selected,
            is_loading: state.is_loading,
            error: state.error.as_deref(),
            loading_text: "  Loading session tree...",
            empty_text: "  No messages yet",
        }
    }

    pub(in crate::entry_tree::render) fn from_branch_preview(
        preview: &'a EntryTreeBranchPreviewState,
    ) -> Self {
        Self {
            rows: &preview.rows,
            selected: preview.selected,
            is_loading: preview.is_loading,
            error: preview.error.as_deref(),
            loading_text: "  Loading session tree...",
            empty_text: "  No messages yet",
        }
    }

    pub(in crate::entry_tree::render) fn page_number(&self, page_size: usize) -> usize {
        self.selection().page_number(page_size)
    }

    pub(in crate::entry_tree::render) fn page_count(&self, page_size: usize) -> usize {
        self.selection().page_count(page_size)
    }

    fn selection(&self) -> PagedSelection {
        PagedSelection::new(self.selected, self.rows.len())
    }

    fn page_start(&self, page_size: usize) -> usize {
        self.selection().page_start(page_size)
    }

    fn page_indices(&self, page_size: usize) -> impl Iterator<Item = usize> {
        self.selection().page_indices(page_size)
    }
}

impl Model {
    pub(in crate::entry_tree::render) fn entry_tree_body_lines(
        &self,
        state: EntryTreeRowsRenderState<'_>,
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
                state.loading_text,
                tertiary_text_style(self.palette),
            ));
        } else if let Some(error) = state.error {
            lines.push(Line::styled(
                truncate_display_width_with_ellipsis(&format!("  {error}"), width),
                tertiary_text_style(self.palette),
            ));
        } else if state.rows.is_empty() {
            lines.push(Line::styled(
                state.empty_text,
                tertiary_text_style(self.palette),
            ));
        } else {
            let graph_lines = entry_tree_graph_lines(state.rows, state.selected, width);
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
        let kind_prefix =
            session_tree_row_kind_prefix(row.kind, TreeRowKindPrefixAlignment::CenterTool);
        let left_padding = " ".repeat(ENTRY_TREE_BODY_HORIZONTAL_PADDING);
        let prefix_width =
            display_width(&left_padding) + graph_line.display_width() + display_width(kind_prefix);
        let text_width = width
            .saturating_sub(prefix_width)
            .saturating_sub(ENTRY_TREE_BODY_HORIZONTAL_PADDING);
        let text_style = entry_tree_content_style(row, self.palette, is_selected);
        let selected_text_style = if is_selected {
            text_style.add_modifier(Modifier::REVERSED)
        } else {
            text_style
        };
        let kind_style = entry_tree_kind_style(row, self.palette, is_selected);

        let mut spans = Vec::new();
        spans.push(Span::raw(left_padding));
        spans.extend(graph_line.spans.into_iter().map(|span| {
            let style = entry_tree_graph_span_style(&span.text, self.palette);
            Span::styled(span.text, style)
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

    pub(in crate::entry_tree::render) fn entry_tree_selected_row_style(&self) -> Style {
        primary_text_style(self.palette)
            .bold()
            .add_modifier(Modifier::REVERSED)
    }
}

pub(in crate::entry_tree::render) struct EntryTreeWidget<'a> {
    pub(in crate::entry_tree::render) lines: &'a [Line<'static>],
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

fn entry_tree_content_style(
    row: &SessionTreeRow,
    palette: TerminalPalette,
    is_selected: bool,
) -> Style {
    match row.kind {
        SessionTreeRowKind::Assistant if is_selected => primary_text_style(palette).bold(),
        SessionTreeRowKind::Assistant if row.is_active_path => primary_text_style(palette),
        SessionTreeRowKind::Assistant => secondary_text_style(palette),
        kind => session_tree_row_kind_label_style(kind, palette),
    }
}

fn entry_tree_kind_style(
    row: &SessionTreeRow,
    palette: TerminalPalette,
    is_selected: bool,
) -> Style {
    let content_style = entry_tree_content_style(row, palette, is_selected);
    match content_style.fg {
        Some(color) => Style::new().fg(color),
        None => Style::new(),
    }
}

pub(in crate::entry_tree::render) fn entry_tree_preview_footer_hint(width: u16) -> &'static str {
    if width < 76 {
        "  Esc/Space back · ↑/←/h prev · ↓/→/l next"
    } else {
        "  Esc/Space back · ↑/←/h previous page · ↓/→/l next page"
    }
}

pub(in crate::entry_tree::render) fn branch_message_count_label(message_count: usize) -> String {
    if message_count > 999 {
        "999+".to_string()
    } else {
        message_count.to_string()
    }
}

/// Branch preview 标题等行内文案（无表格列宽 padding）。
pub(in crate::entry_tree) fn branch_picker_relative_age_label(
    now_ms: i64,
    timestamp_ms: i64,
) -> String {
    crate::relative_age::relative_age_label(now_ms, timestamp_ms)
}
