use super::*;

pub(super) fn prompt_overlay_tool_sel_label(row: &PromptOverlayInactiveRow) -> String {
    match row {
        PromptOverlayInactiveRow::ToolCandidate { tool } => {
            if !tool.selection.can_select() {
                "-".to_string()
            } else if tool.selection.is_selected() {
                "●".to_string()
            } else {
                "○".to_string()
            }
        }
        _ => "-".to_string(),
    }
}

pub(super) fn prompt_overlay_tool_name_cell(
    row: &PromptOverlayInactiveRow,
    width: usize,
) -> String {
    match row {
        PromptOverlayInactiveRow::ToolCandidate { tool } => {
            let label = tool.label.as_deref().unwrap_or(&tool.name);
            prompt_overlay_cell_with_trailing_marker(label, None, width)
        }
        _ => prompt_overlay_fill_cell("", width),
    }
}

pub(super) fn prompt_overlay_tool_order_label(row: &PromptOverlayInactiveRow) -> String {
    match row {
        PromptOverlayInactiveRow::ToolCandidate { tool } => tool
            .selection
            .selected_order()
            .map(|order| order.to_string())
            .unwrap_or_else(|| "-".to_string()),
        _ => "-".to_string(),
    }
}

pub(super) fn prompt_overlay_tool_origin(row: &PromptOverlayInactiveRow) -> PromptSourceOrigin {
    match row {
        PromptOverlayInactiveRow::ToolCandidate { tool } => tool.origin,
        _ => PromptSourceOrigin::Project,
    }
}

#[derive(Clone, Copy)]
pub(super) struct PromptOverlayDynamicSelection<'a> {
    pub(super) row_id: Option<&'a str>,
    pub(super) snapshot_kind: DynamicEnvironmentSnapshotKind,
}

pub(super) fn prompt_overlay_dynamic_lines(
    rows: &[PromptOverlayInactiveRow],
    selection: PromptOverlayDynamicSelection<'_>,
    scroll: usize,
    focused: bool,
    width: usize,
    body_height: usize,
    palette: crate::theme::TerminalPalette,
) -> Vec<Line<'static>> {
    if body_height == 0 {
        return Vec::new();
    }
    if rows.is_empty() {
        return vec![prompt_overlay_empty_inactive_line(
            "No dynamic sources",
            width,
            palette,
        )];
    }

    rows.iter()
        .map(|row| {
            let selected = selection.row_id == Some(prompt_overlay_inactive_row_id(row).as_str());
            prompt_overlay_dynamic_row_line(
                row,
                selected,
                focused,
                selection.snapshot_kind,
                width,
                palette,
            )
        })
        .skip(scroll)
        .take(body_height)
        .collect()
}

pub(super) fn prompt_overlay_dynamic_row_line(
    row: &PromptOverlayInactiveRow,
    selected: bool,
    focused: bool,
    selected_snapshot_kind: DynamicEnvironmentSnapshotKind,
    width: usize,
    palette: crate::theme::TerminalPalette,
) -> Line<'static> {
    let content_width = width.saturating_sub(PROMPT_OVERLAY_ROW_PREFIX_WIDTH).max(1);
    let (item_style, marker_style, marker) =
        prompt_overlay_selection_styles(selected, focused, palette);
    let highlighted_cell_style = if selected && focused {
        command_accent_text_style(palette)
            .bold()
            .add_modifier(Modifier::UNDERLINED)
    } else {
        item_style
    };
    let name_width = prompt_overlay_dynamic_name_width(content_width);
    let left_pad = " ".repeat(PROMPT_OVERLAY_OUTER_PADDING);
    let gap = " ".repeat(PROMPT_OVERLAY_COLUMN_GAP);
    let scope = format!(
        "{:<width$}",
        prompt_overlay_origin_label(prompt_overlay_dynamic_origin(row)),
        width = PROMPT_OVERLAY_RIGHT_SCOPE_WIDTH
    );
    let trailing = " ".repeat(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING);
    let baseline_style = if selected_snapshot_kind == DynamicEnvironmentSnapshotKind::Baseline {
        highlighted_cell_style
    } else {
        item_style
    };
    let changes_style = if selected_snapshot_kind == DynamicEnvironmentSnapshotKind::Changes {
        highlighted_cell_style
    } else {
        item_style
    };
    let baseline = prompt_overlay_dynamic_checkbox_spans(
        row,
        DynamicEnvironmentSnapshotKind::Baseline,
        baseline_style,
        item_style,
    );
    let changes = prompt_overlay_dynamic_checkbox_spans(
        row,
        DynamicEnvironmentSnapshotKind::Changes,
        changes_style,
        item_style,
    );
    let name = prompt_overlay_dynamic_name_cell(row, name_width);

    Line::from(vec![
        Span::styled(marker.to_string(), marker_style),
        Span::styled(left_pad, item_style),
        baseline.0,
        baseline.1,
        baseline.2,
        Span::styled(gap.clone(), item_style),
        changes.0,
        changes.1,
        changes.2,
        Span::styled(gap.clone(), item_style),
        Span::styled(name, item_style),
        Span::styled(gap, item_style),
        Span::styled(scope, item_style),
        Span::styled(trailing, item_style),
    ])
}

pub(super) fn prompt_overlay_dynamic_plain_row_text(
    row: &PromptOverlayInactiveRow,
    width: usize,
) -> String {
    let name_width = prompt_overlay_dynamic_name_width(width);
    let left_pad = " ".repeat(PROMPT_OVERLAY_OUTER_PADDING);
    let baseline = prompt_overlay_center_text(
        &prompt_overlay_dynamic_checkbox_label(row, DynamicEnvironmentSnapshotKind::Baseline),
        PROMPT_OVERLAY_DYNAMIC_CHECKBOX_WIDTH,
    );
    let changes = prompt_overlay_center_text(
        &prompt_overlay_dynamic_checkbox_label(row, DynamicEnvironmentSnapshotKind::Changes),
        PROMPT_OVERLAY_DYNAMIC_CHECKBOX_WIDTH,
    );
    let name = prompt_overlay_dynamic_name_cell(row, name_width);
    let scope = format!(
        "{:<width$}",
        prompt_overlay_origin_label(prompt_overlay_dynamic_origin(row)),
        width = PROMPT_OVERLAY_RIGHT_SCOPE_WIDTH
    );
    let gap = " ".repeat(PROMPT_OVERLAY_COLUMN_GAP);
    let trailing = " ".repeat(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING);
    format!("{left_pad}{baseline}{gap}{changes}{gap}{name}{gap}{scope}{trailing}")
}

pub(super) fn prompt_overlay_dynamic_checkbox_label(
    row: &PromptOverlayInactiveRow,
    snapshot_kind: DynamicEnvironmentSnapshotKind,
) -> String {
    match row {
        PromptOverlayInactiveRow::DynamicEnvironmentCandidate { source } => {
            if prompt_overlay_dynamic_source_selected(source, snapshot_kind) {
                "[x]".to_string()
            } else {
                "[ ]".to_string()
            }
        }
        _ => "-".to_string(),
    }
}

pub(super) fn prompt_overlay_dynamic_checkbox_spans(
    row: &PromptOverlayInactiveRow,
    snapshot_kind: DynamicEnvironmentSnapshotKind,
    checkbox_style: Style,
    padding_style: Style,
) -> (Span<'static>, Span<'static>, Span<'static>) {
    let label = prompt_overlay_dynamic_checkbox_label(row, snapshot_kind);
    let label_width = display_width(&label).min(PROMPT_OVERLAY_DYNAMIC_CHECKBOX_WIDTH);
    let left_padding = PROMPT_OVERLAY_DYNAMIC_CHECKBOX_WIDTH.saturating_sub(label_width) / 2;
    let right_padding = PROMPT_OVERLAY_DYNAMIC_CHECKBOX_WIDTH
        .saturating_sub(label_width)
        .saturating_sub(left_padding);

    (
        Span::styled(" ".repeat(left_padding), padding_style),
        Span::styled(label, checkbox_style),
        Span::styled(" ".repeat(right_padding), padding_style),
    )
}

pub(super) fn prompt_overlay_dynamic_checkbox_hit_test(
    column: u16,
    body_area: Rect,
) -> Option<DynamicEnvironmentSnapshotKind> {
    if body_area.width == 0 || column < body_area.x {
        return None;
    }

    let relative_column = usize::from(column.saturating_sub(body_area.x));
    let baseline_start = PROMPT_OVERLAY_ROW_PREFIX_WIDTH + PROMPT_OVERLAY_OUTER_PADDING;
    let baseline_end = baseline_start.saturating_add(PROMPT_OVERLAY_DYNAMIC_CHECKBOX_WIDTH);
    if relative_column >= baseline_start && relative_column < baseline_end {
        return Some(DynamicEnvironmentSnapshotKind::Baseline);
    }

    let changes_start = baseline_end.saturating_add(PROMPT_OVERLAY_COLUMN_GAP);
    let changes_end = changes_start.saturating_add(PROMPT_OVERLAY_DYNAMIC_CHECKBOX_WIDTH);
    if relative_column >= changes_start && relative_column < changes_end {
        return Some(DynamicEnvironmentSnapshotKind::Changes);
    }

    None
}

pub(super) fn prompt_overlay_dynamic_source_selected(
    source: &PromptAssemblyDynamicEnvironmentCandidate,
    snapshot_kind: DynamicEnvironmentSnapshotKind,
) -> bool {
    match snapshot_kind {
        DynamicEnvironmentSnapshotKind::Baseline => source.baseline_selected,
        DynamicEnvironmentSnapshotKind::Changes => source.changes_selected,
    }
}

pub(super) fn prompt_overlay_dynamic_name_cell(
    row: &PromptOverlayInactiveRow,
    width: usize,
) -> String {
    match row {
        PromptOverlayInactiveRow::DynamicEnvironmentCandidate { source } => {
            prompt_overlay_cell_with_trailing_marker(&source.label, None, width)
        }
        _ => prompt_overlay_fill_cell("", width),
    }
}

pub(super) fn prompt_overlay_dynamic_origin(row: &PromptOverlayInactiveRow) -> PromptSourceOrigin {
    match row {
        PromptOverlayInactiveRow::DynamicEnvironmentCandidate { source } => source.origin,
        _ => PromptSourceOrigin::Project,
    }
}

pub(super) fn prompt_overlay_dynamic_header_text(width: usize) -> String {
    let name_width = prompt_overlay_dynamic_name_width(width);
    let left_pad = " ".repeat(PROMPT_OVERLAY_OUTER_PADDING);
    let baseline = prompt_overlay_center_text("Base", PROMPT_OVERLAY_DYNAMIC_CHECKBOX_WIDTH);
    let changes = prompt_overlay_center_text("Change", PROMPT_OVERLAY_DYNAMIC_CHECKBOX_WIDTH);
    let name = format!(
        "{:<width$}",
        truncate_display_width_with_ellipsis("Source", name_width),
        width = name_width
    );
    let scope = format!(
        "{:<width$}",
        "Scope",
        width = PROMPT_OVERLAY_RIGHT_SCOPE_WIDTH
    );
    let gap = " ".repeat(PROMPT_OVERLAY_COLUMN_GAP);
    let trailing = " ".repeat(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING);
    format!("{left_pad}{baseline}{gap}{changes}{gap}{name}{gap}{scope}{trailing}")
}

pub(super) fn prompt_overlay_tool_header_text(width: usize) -> String {
    let name_width = prompt_overlay_right_inactive_name_width(width);
    let left_pad = " ".repeat(PROMPT_OVERLAY_OUTER_PADDING);
    let sel = prompt_overlay_center_text("Sel", PROMPT_OVERLAY_LEFT_SEL_WIDTH);
    let ord = left_pad_display_width("Ord", PROMPT_OVERLAY_RIGHT_ORD_WIDTH);
    let name = format!(
        "{:<width$}",
        truncate_display_width_with_ellipsis("Name", name_width),
        width = name_width
    );
    let scope = format!(
        "{:<width$}",
        "Scope",
        width = PROMPT_OVERLAY_RIGHT_SCOPE_WIDTH
    );
    let gap = " ".repeat(PROMPT_OVERLAY_COLUMN_GAP);
    let trailing = " ".repeat(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING);
    format!("{left_pad}{sel}{gap}{ord}{gap}{name}{gap}{scope}{trailing}")
}

pub(super) fn prompt_overlay_shadowed_count_marker(shadowed_count: usize) -> Option<String> {
    (shadowed_count > 0).then(|| format!("+{shadowed_count} shadowed"))
}

pub(super) fn prompt_overlay_managed_status_marker(
    status: PromptOverlayManagedStatus,
    shadowed_count: usize,
) -> Option<String> {
    match status {
        PromptOverlayManagedStatus::Active => prompt_overlay_shadowed_count_marker(shadowed_count),
        PromptOverlayManagedStatus::Missing => Some("missing".to_string()),
        PromptOverlayManagedStatus::Shadowed => Some("shadowed".to_string()),
        PromptOverlayManagedStatus::Disabled => None,
    }
}

pub(super) fn prompt_overlay_cell_with_trailing_marker(
    text: &str,
    trailing_marker: Option<&str>,
    width: usize,
) -> String {
    let width = width.max(1);
    let Some(trailing_marker) = trailing_marker.filter(|marker| !marker.is_empty()) else {
        return prompt_overlay_fill_cell(text, width);
    };

    let trailing_width = display_width(trailing_marker);
    let reserved_width = trailing_width.saturating_add(1);
    if width <= reserved_width {
        return truncate_display_width_with_ellipsis(trailing_marker, width);
    }

    let text_width = width.saturating_sub(reserved_width);
    let visible_text = truncate_display_width_with_ellipsis(text, text_width);
    let padding = text_width.saturating_sub(display_width(&visible_text));
    format!("{visible_text}{} {trailing_marker}", " ".repeat(padding))
}

pub(super) fn prompt_overlay_fill_cell(text: &str, width: usize) -> String {
    let visible_text = truncate_display_width_with_ellipsis(text, width);
    let padding = width.saturating_sub(display_width(&visible_text));
    format!("{visible_text}{}", " ".repeat(padding))
}

pub(super) fn prompt_overlay_left_source_width(width: usize) -> usize {
    width
        .saturating_sub(PROMPT_OVERLAY_OUTER_PADDING)
        .saturating_sub(PROMPT_OVERLAY_LEFT_SEL_WIDTH)
        .saturating_sub(PROMPT_OVERLAY_LEFT_ORD_WIDTH)
        .saturating_sub(PROMPT_OVERLAY_LEFT_KIND_WIDTH)
        .saturating_sub(PROMPT_OVERLAY_LEFT_SCOPE_WIDTH)
        .saturating_sub(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING)
        .saturating_sub(PROMPT_OVERLAY_COLUMN_GAP * 4)
        .max(12)
}

pub(super) fn prompt_overlay_right_extra_name_width(width: usize) -> usize {
    width
        .saturating_sub(PROMPT_OVERLAY_OUTER_PADDING)
        .saturating_sub(PROMPT_OVERLAY_LEFT_SEL_WIDTH)
        .saturating_sub(PROMPT_OVERLAY_RIGHT_SCOPE_WIDTH)
        .saturating_sub(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING)
        .saturating_sub(PROMPT_OVERLAY_COLUMN_GAP * 2)
        .max(12)
}

pub(super) fn prompt_overlay_right_inactive_name_width(width: usize) -> usize {
    width
        .saturating_sub(PROMPT_OVERLAY_OUTER_PADDING)
        .saturating_sub(PROMPT_OVERLAY_LEFT_SEL_WIDTH)
        .saturating_sub(PROMPT_OVERLAY_RIGHT_ORD_WIDTH)
        .saturating_sub(PROMPT_OVERLAY_RIGHT_SCOPE_WIDTH)
        .saturating_sub(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING)
        .saturating_sub(PROMPT_OVERLAY_COLUMN_GAP * 3)
        .max(12)
}

pub(super) fn prompt_overlay_dynamic_name_width(width: usize) -> usize {
    width
        .saturating_sub(PROMPT_OVERLAY_OUTER_PADDING)
        .saturating_sub(PROMPT_OVERLAY_DYNAMIC_CHECKBOX_WIDTH * 2)
        .saturating_sub(PROMPT_OVERLAY_RIGHT_SCOPE_WIDTH)
        .saturating_sub(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING)
        .saturating_sub(PROMPT_OVERLAY_COLUMN_GAP * 3)
        .max(12)
}

pub(super) fn prompt_overlay_list_line(
    marker: &str,
    marker_style: Style,
    content: String,
    content_style: Style,
) -> Line<'static> {
    Line::from(vec![
        Span::styled(marker.to_string(), marker_style),
        Span::styled(content, content_style),
    ])
}

pub(super) fn prompt_overlay_empty_inactive_line(
    message: &str,
    width: usize,
    palette: crate::theme::TerminalPalette,
) -> Line<'static> {
    let content_width = width.saturating_sub(PROMPT_OVERLAY_ROW_PREFIX_WIDTH).max(1);
    let right_prefix = " ".repeat(PROMPT_OVERLAY_OUTER_PADDING);
    let content = format!(
        "{}{}",
        right_prefix,
        truncate_display_width_with_ellipsis(
            message,
            content_width
                .saturating_sub(PROMPT_OVERLAY_OUTER_PADDING)
                .max(1)
        ),
    );

    prompt_overlay_list_line(
        " ",
        tertiary_text_style(palette),
        truncate_display_width_with_ellipsis(&content, content_width),
        tertiary_text_style(palette),
    )
}

pub(super) fn prompt_overlay_center_text(value: &str, width: usize) -> String {
    let value_width = display_width(value).min(width);
    let left = width.saturating_sub(value_width) / 2;
    let right = width.saturating_sub(value_width).saturating_sub(left);
    format!("{}{}{}", " ".repeat(left), value, " ".repeat(right))
}

pub(super) fn selection_label(label: Option<&str>, selected: usize, total: usize) -> String {
    let position = if total == 0 {
        0
    } else {
        selected.min(total.saturating_sub(1)) + 1
    };

    match label {
        Some(label) => format!(" {label} {position}/{total} "),
        None => format!(" {position}/{total} "),
    }
}

pub(super) fn clamp_scroll(
    current_scroll: usize,
    selected: usize,
    total: usize,
    visible_rows: usize,
) -> usize {
    VisibleWindowSelection::new(selected, total)
        .scroll_start_for_selection(current_scroll, visible_rows)
}

pub(super) fn prompt_overlay_active_visible_rows(height: u16) -> usize {
    let chrome = fullscreen_list_chrome_rects(Rect::new(0, 0, 1, height));
    let body_height = chrome.map(|rects| rects.body.height).unwrap_or_default();
    usize::from(body_height.saturating_sub(1)).max(1)
}

pub(super) fn prompt_overlay_inactive_visible_rows(height: u16) -> usize {
    let chrome = fullscreen_list_chrome_rects(Rect::new(0, 0, 1, height));
    let body_height = chrome.map(|rects| rects.body.height).unwrap_or_default();
    usize::from(body_height.saturating_sub(1)).max(1)
}

pub(super) fn prompt_scope_from_origin(origin: PromptSourceOrigin) -> Option<PromptAssemblyScope> {
    match origin {
        PromptSourceOrigin::Builtin => None,
        PromptSourceOrigin::Global => Some(PromptAssemblyScope::Global),
        PromptSourceOrigin::Project => Some(PromptAssemblyScope::Project),
    }
}

pub(super) fn allows_shift_only_modifier(modifiers: KeyModifiers) -> bool {
    modifiers.is_empty() || modifiers == KeyModifiers::SHIFT
}
