use super::*;

pub(super) struct PromptOverlayLineListWidget<'a> {
    pub(super) lines: &'a [Line<'static>],
}

impl Widget for PromptOverlayLineListWidget<'_> {
    fn render(self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        for (row, line) in self.lines.iter().take(usize::from(area.height)).enumerate() {
            let y = area.y + u16::try_from(row).unwrap_or(u16::MAX);
            // WHY: `Paragraph` 不会把每个 `Line` 的背景延展到未占用单元格；
            // prompt overlay 的选中行需要整行背景铺满 pane 宽度。
            render_line_with_full_width_background(line, Rect::new(area.x, y, area.width, 1), buf);
        }
    }
}

pub(super) struct PromptOverlayVerticalRule {
    pub(super) palette: crate::theme::TerminalPalette,
}

impl Widget for PromptOverlayVerticalRule {
    fn render(self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        let style = tertiary_text_style(self.palette);
        for y in area.top()..area.bottom() {
            buf[(area.x, y)].set_symbol("│").set_style(style);
        }
    }
}

pub(super) fn prompt_overlay_active_lines(
    sources: &[PromptOverlayLeftRow],
    selected: usize,
    scroll: usize,
    focused: bool,
    width: usize,
    body_height: usize,
    palette: crate::theme::TerminalPalette,
) -> Vec<Line<'static>> {
    if body_height == 0 {
        return Vec::new();
    }
    if sources.is_empty() {
        return vec![Line::styled(
            truncate_display_width_with_ellipsis("  No active sources", width.max(1)),
            tertiary_text_style(palette),
        )];
    }

    let mut lines = Vec::new();
    for (index, source) in sources.iter().enumerate().skip(scroll).take(body_height) {
        lines.push(prompt_overlay_left_row_line(
            source,
            index == selected,
            focused,
            width,
            palette,
        ));
    }
    lines
}

pub(super) fn prompt_overlay_inactive_lines(
    rows: &[PromptOverlayInactiveRow],
    selected_row_id: Option<&str>,
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
            "No candidates",
            width,
            palette,
        )];
    }

    let mut lines = Vec::new();
    for row in rows.iter().skip(scroll).take(body_height) {
        lines.push(prompt_overlay_inactive_row_line(
            row,
            selected_row_id == Some(prompt_overlay_inactive_row_id(row).as_str()),
            focused,
            width,
            palette,
        ));
    }
    lines
}

pub(super) fn prompt_overlay_left_row_line(
    row: &PromptOverlayLeftRow,
    selected: bool,
    focused: bool,
    width: usize,
    palette: crate::theme::TerminalPalette,
) -> Line<'static> {
    let content_width = width.saturating_sub(PROMPT_OVERLAY_ROW_PREFIX_WIDTH).max(1);
    let label = match row {
        PromptOverlayLeftRow::ManagedSource {
            source,
            status,
            shadowed_count,
        } => prompt_overlay_active_row_text(source, *status, *shadowed_count, content_width),
        PromptOverlayLeftRow::ShadowedDetail { source } => {
            prompt_overlay_shadowed_detail_row_text(source, content_width)
        }
    };
    let (item_style, marker_style, marker) =
        prompt_overlay_selection_styles(selected, focused, palette);
    prompt_overlay_list_line(
        marker,
        marker_style,
        truncate_display_width_with_ellipsis(&label, content_width),
        item_style,
    )
}

pub(super) fn prompt_overlay_discovered_skill_lines(
    skills: &[PromptOverlayInactiveRow],
    selected_row_id: Option<&str>,
    scroll: usize,
    focused: bool,
    width: usize,
    body_height: usize,
    palette: crate::theme::TerminalPalette,
) -> Vec<Line<'static>> {
    if body_height == 0 {
        return Vec::new();
    }
    if skills.is_empty() {
        return vec![prompt_overlay_empty_inactive_line(
            "No discovered skills",
            width,
            palette,
        )];
    }

    skills
        .iter()
        .map(|row| {
            let selected = selected_row_id == Some(prompt_overlay_inactive_row_id(row).as_str());
            let content_width = width.saturating_sub(PROMPT_OVERLAY_ROW_PREFIX_WIDTH).max(1);
            let (item_style, marker_style, marker) =
                prompt_overlay_selection_styles(selected, focused, palette);
            let label = prompt_overlay_skill_row_text(row, content_width);
            prompt_overlay_list_line(
                marker,
                marker_style,
                truncate_display_width_with_ellipsis(&label, content_width),
                item_style,
            )
        })
        .skip(scroll)
        .take(body_height)
        .collect()
}

pub(super) fn prompt_overlay_inactive_row_line(
    row: &PromptOverlayInactiveRow,
    selected: bool,
    focused: bool,
    width: usize,
    palette: crate::theme::TerminalPalette,
) -> Line<'static> {
    let content_width = width.saturating_sub(PROMPT_OVERLAY_ROW_PREFIX_WIDTH).max(1);
    let (item_style, marker_style, marker) =
        prompt_overlay_selection_styles(selected, focused, palette);
    let label = match row {
        PromptOverlayInactiveRow::ExtraPromptCandidate {
            source,
            shadowed_count,
        } => prompt_overlay_extra_row_text(source, *shadowed_count, content_width),
        PromptOverlayInactiveRow::ExtraPromptShadowedDetail { source } => {
            prompt_overlay_extra_shadowed_detail_row_text(source, content_width)
        }
        PromptOverlayInactiveRow::DiscoveredSkill {
            shadowed_count: _, ..
        } => prompt_overlay_skill_row_text(row, content_width),
        PromptOverlayInactiveRow::DiscoveredSkillShadowedDetail { .. } => {
            prompt_overlay_skill_row_text(row, content_width)
        }
        PromptOverlayInactiveRow::ToolCandidate { .. } => {
            prompt_overlay_tool_row_text(row, content_width)
        }
        PromptOverlayInactiveRow::DynamicEnvironmentCandidate { .. } => {
            prompt_overlay_dynamic_plain_row_text(row, content_width)
        }
    };
    prompt_overlay_list_line(
        marker,
        marker_style,
        truncate_display_width_with_ellipsis(&label, content_width),
        item_style,
    )
}

pub(super) fn prompt_overlay_origin_label(origin: PromptSourceOrigin) -> &'static str {
    match origin {
        PromptSourceOrigin::Builtin => "builtin",
        PromptSourceOrigin::Global => "global",
        PromptSourceOrigin::Project => "project",
    }
}

pub(super) fn prompt_overlay_kind_label(kind: PromptSourceKind) -> &'static str {
    match kind {
        PromptSourceKind::CoreSystemPrompt => "system",
        PromptSourceKind::InstructionsFile => "instructions",
        PromptSourceKind::ExtraPrompt => "custom",
        PromptSourceKind::SkillDiscovery => "discovery",
        PromptSourceKind::LongLivedSkill => "skill",
        PromptSourceKind::ToolGuidelines => "tools",
        PromptSourceKind::DynamicEnvironmentBaseline => "dynamic",
        PromptSourceKind::DynamicEnvironmentChanges => "dynamic",
    }
}

pub(super) fn prompt_overlay_active_header_text(width: usize) -> String {
    let source_width = prompt_overlay_left_source_width(width);
    let left_pad = " ".repeat(PROMPT_OVERLAY_OUTER_PADDING);
    let sel = prompt_overlay_center_text("Sel", PROMPT_OVERLAY_LEFT_SEL_WIDTH);
    let ord = left_pad_display_width("Ord", PROMPT_OVERLAY_LEFT_ORD_WIDTH);
    let source = format!(
        "{:<width$}",
        truncate_display_width_with_ellipsis("Source", source_width),
        width = source_width
    );
    let kind = format!("{:<width$}", "Type", width = PROMPT_OVERLAY_LEFT_KIND_WIDTH);
    let scope = format!(
        "{:<width$}",
        "Scope",
        width = PROMPT_OVERLAY_LEFT_SCOPE_WIDTH
    );
    let gap = " ".repeat(PROMPT_OVERLAY_COLUMN_GAP);
    let trailing = " ".repeat(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING);
    format!("{left_pad}{sel}{gap}{ord}{gap}{source}{gap}{kind}{gap}{scope}{trailing}")
}

pub(super) fn prompt_overlay_extra_header_text(width: usize) -> String {
    let name_width = prompt_overlay_right_extra_name_width(width);
    let left_pad = " ".repeat(PROMPT_OVERLAY_OUTER_PADDING);
    let sel = prompt_overlay_center_text("Sel", PROMPT_OVERLAY_LEFT_SEL_WIDTH);
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
    format!("{left_pad}{sel}{gap}{name}{gap}{scope}{trailing}")
}

pub(super) fn prompt_overlay_skill_header_text(width: usize) -> String {
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

pub(super) fn prompt_overlay_active_row_text(
    source: &PromptAssemblyManagedSource,
    status: PromptOverlayManagedStatus,
    shadowed_count: usize,
    width: usize,
) -> String {
    let source_width = prompt_overlay_left_source_width(width);
    let left_pad = " ".repeat(PROMPT_OVERLAY_OUTER_PADDING);
    let sel = prompt_overlay_center_text(
        if source.enabled { "●" } else { "○" },
        PROMPT_OVERLAY_LEFT_SEL_WIDTH,
    );
    let ord = left_pad_display_width(&source.order.to_string(), PROMPT_OVERLAY_LEFT_ORD_WIDTH);
    let status_marker = prompt_overlay_managed_status_marker(status, shadowed_count);
    let source_name = prompt_overlay_cell_with_trailing_marker(
        &source.title,
        status_marker.as_deref(),
        source_width,
    );
    let kind = format!(
        "{:<width$}",
        prompt_overlay_kind_label(source.kind),
        width = PROMPT_OVERLAY_LEFT_KIND_WIDTH
    );
    let scope = format!(
        "{:<width$}",
        source
            .origin
            .map(prompt_overlay_origin_label)
            .unwrap_or("-"),
        width = PROMPT_OVERLAY_LEFT_SCOPE_WIDTH
    );
    let gap = " ".repeat(PROMPT_OVERLAY_COLUMN_GAP);
    let trailing = " ".repeat(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING);
    format!("{left_pad}{sel}{gap}{ord}{gap}{source_name}{gap}{kind}{gap}{scope}{trailing}")
}

pub(super) fn prompt_overlay_shadowed_detail_row_text(
    source: &ResolvedPromptSource,
    width: usize,
) -> String {
    let source_width = prompt_overlay_left_source_width(width);
    let left_pad = " ".repeat(PROMPT_OVERLAY_OUTER_PADDING);
    let sel = prompt_overlay_center_text("↳", PROMPT_OVERLAY_LEFT_SEL_WIDTH);
    let ord = left_pad_display_width("", PROMPT_OVERLAY_LEFT_ORD_WIDTH);
    let source_name = prompt_overlay_cell_with_trailing_marker(
        &format!(
            "shadowed {}",
            source
                .origin
                .map(prompt_overlay_origin_label)
                .unwrap_or("-")
        ),
        None,
        source_width,
    );
    let kind = format!(
        "{:<width$}",
        prompt_overlay_kind_label(source.kind),
        width = PROMPT_OVERLAY_LEFT_KIND_WIDTH
    );
    let scope = format!(
        "{:<width$}",
        source
            .origin
            .map(prompt_overlay_origin_label)
            .unwrap_or("-"),
        width = PROMPT_OVERLAY_LEFT_SCOPE_WIDTH
    );
    let gap = " ".repeat(PROMPT_OVERLAY_COLUMN_GAP);
    let trailing = " ".repeat(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING);
    format!("{left_pad}{sel}{gap}{ord}{gap}{source_name}{gap}{kind}{gap}{scope}{trailing}")
}

pub(super) fn prompt_overlay_extra_row_text(
    source: &PromptAssemblyExtraPromptCandidate,
    shadowed_count: usize,
    width: usize,
) -> String {
    let name_width = prompt_overlay_right_extra_name_width(width);
    let left_pad = " ".repeat(PROMPT_OVERLAY_OUTER_PADDING);
    let sel = prompt_overlay_center_text(
        if source.selected { "●" } else { "○" },
        PROMPT_OVERLAY_LEFT_SEL_WIDTH,
    );
    let name = prompt_overlay_cell_with_trailing_marker(
        &source.title,
        prompt_overlay_shadowed_count_marker(shadowed_count).as_deref(),
        name_width,
    );
    let scope = format!(
        "{:<width$}",
        prompt_overlay_origin_label(source.origin),
        width = PROMPT_OVERLAY_RIGHT_SCOPE_WIDTH
    );
    let gap = " ".repeat(PROMPT_OVERLAY_COLUMN_GAP);
    let trailing = " ".repeat(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING);
    format!("{left_pad}{sel}{gap}{name}{gap}{scope}{trailing}")
}

pub(super) fn prompt_overlay_skill_row_text(
    row: &PromptOverlayInactiveRow,
    width: usize,
) -> String {
    let name_width = prompt_overlay_right_inactive_name_width(width);
    let left_pad = " ".repeat(PROMPT_OVERLAY_OUTER_PADDING);
    let sel = prompt_overlay_center_text(
        &prompt_overlay_skill_sel_label(row),
        PROMPT_OVERLAY_LEFT_SEL_WIDTH,
    );
    let ord = left_pad_display_width(
        &prompt_overlay_skill_order_label(row),
        PROMPT_OVERLAY_RIGHT_ORD_WIDTH,
    );
    let name = prompt_overlay_skill_name_cell(row, name_width);
    let scope = format!(
        "{:<width$}",
        prompt_overlay_origin_label(prompt_overlay_inactive_skill_origin(row)),
        width = PROMPT_OVERLAY_RIGHT_SCOPE_WIDTH
    );
    let gap = " ".repeat(PROMPT_OVERLAY_COLUMN_GAP);
    let trailing = " ".repeat(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING);
    format!("{left_pad}{sel}{gap}{ord}{gap}{name}{gap}{scope}{trailing}")
}

pub(super) fn prompt_overlay_extra_shadowed_detail_row_text(
    source: &PromptAssemblyExtraPromptCandidate,
    width: usize,
) -> String {
    let name_width = prompt_overlay_right_extra_name_width(width);
    let left_pad = " ".repeat(PROMPT_OVERLAY_OUTER_PADDING);
    let sel = prompt_overlay_center_text(
        if source.selected { "●" } else { "○" },
        PROMPT_OVERLAY_LEFT_SEL_WIDTH,
    );
    let name = prompt_overlay_cell_with_trailing_marker(
        &format!("shadowed {}", prompt_overlay_origin_label(source.origin)),
        None,
        name_width,
    );
    let scope = format!(
        "{:<width$}",
        prompt_overlay_origin_label(source.origin),
        width = PROMPT_OVERLAY_RIGHT_SCOPE_WIDTH
    );
    let gap = " ".repeat(PROMPT_OVERLAY_COLUMN_GAP);
    let trailing = " ".repeat(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING);
    format!("{left_pad}{sel}{gap}{name}{gap}{scope}{trailing}")
}

pub(super) fn prompt_overlay_skill_sel_label(row: &PromptOverlayInactiveRow) -> String {
    match row {
        PromptOverlayInactiveRow::DiscoveredSkill { skill, .. }
        | PromptOverlayInactiveRow::DiscoveredSkillShadowedDetail { skill } => {
            if !skill.selection.can_select() {
                "-".to_string()
            } else if skill.selection.is_selected() {
                "●".to_string()
            } else {
                "○".to_string()
            }
        }
        _ => "-".to_string(),
    }
}

pub(super) fn prompt_overlay_skill_name_cell(
    row: &PromptOverlayInactiveRow,
    width: usize,
) -> String {
    match row {
        PromptOverlayInactiveRow::DiscoveredSkill {
            skill,
            shadowed_count,
        } => {
            let trailing = if *shadowed_count > 0 {
                prompt_overlay_shadowed_count_marker(*shadowed_count)
            } else if !skill.selection.can_select() {
                Some("(manual)".to_string())
            } else {
                None
            };
            prompt_overlay_cell_with_trailing_marker(&skill.title, trailing.as_deref(), width)
        }
        PromptOverlayInactiveRow::DiscoveredSkillShadowedDetail { skill } => {
            prompt_overlay_cell_with_trailing_marker(
                &format!("shadowed {}", prompt_overlay_origin_label(skill.origin)),
                None,
                width,
            )
        }
        _ => prompt_overlay_fill_cell("", width),
    }
}

pub(super) fn prompt_overlay_inactive_skill_origin(
    row: &PromptOverlayInactiveRow,
) -> PromptSourceOrigin {
    match row {
        PromptOverlayInactiveRow::DiscoveredSkill { skill, .. }
        | PromptOverlayInactiveRow::DiscoveredSkillShadowedDetail { skill } => skill.origin,
        _ => PromptSourceOrigin::Project,
    }
}

pub(super) fn prompt_overlay_skill_order_label(row: &PromptOverlayInactiveRow) -> String {
    match row {
        PromptOverlayInactiveRow::DiscoveredSkill { skill, .. } => skill
            .selection
            .selected_order()
            .map(|order| order.to_string())
            .unwrap_or_else(|| "-".to_string()),
        PromptOverlayInactiveRow::DiscoveredSkillShadowedDetail { .. } => String::new(),
        _ => "-".to_string(),
    }
}

pub(super) fn prompt_overlay_tool_lines(
    rows: &[PromptOverlayInactiveRow],
    selected_row_id: Option<&str>,
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
            "No tools available",
            width,
            palette,
        )];
    }

    rows.iter()
        .map(|row| {
            let selected = selected_row_id == Some(prompt_overlay_inactive_row_id(row).as_str());
            let content_width = width.saturating_sub(PROMPT_OVERLAY_ROW_PREFIX_WIDTH).max(1);
            let (item_style, marker_style, marker) =
                prompt_overlay_selection_styles(selected, focused, palette);
            let label = prompt_overlay_tool_row_text(row, content_width);
            prompt_overlay_list_line(
                marker,
                marker_style,
                truncate_display_width_with_ellipsis(&label, content_width),
                item_style,
            )
        })
        .skip(scroll)
        .take(body_height)
        .collect()
}

pub(super) fn prompt_overlay_tool_row_text(row: &PromptOverlayInactiveRow, width: usize) -> String {
    let name_width = prompt_overlay_right_inactive_name_width(width);
    let left_pad = " ".repeat(PROMPT_OVERLAY_OUTER_PADDING);
    let sel = prompt_overlay_center_text(
        &prompt_overlay_tool_sel_label(row),
        PROMPT_OVERLAY_LEFT_SEL_WIDTH,
    );
    let ord = left_pad_display_width(
        &prompt_overlay_tool_order_label(row),
        PROMPT_OVERLAY_RIGHT_ORD_WIDTH,
    );
    let name = prompt_overlay_tool_name_cell(row, name_width);
    let scope = format!(
        "{:<width$}",
        prompt_overlay_origin_label(prompt_overlay_tool_origin(row)),
        width = PROMPT_OVERLAY_RIGHT_SCOPE_WIDTH
    );
    let gap = " ".repeat(PROMPT_OVERLAY_COLUMN_GAP);
    let trailing = " ".repeat(PROMPT_OVERLAY_SCOPE_TRAILING_PADDING);
    format!("{left_pad}{sel}{gap}{ord}{gap}{name}{gap}{scope}{trailing}")
}
