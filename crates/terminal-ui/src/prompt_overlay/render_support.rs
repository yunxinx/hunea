use super::*;

pub(super) fn prompt_overlay_matches_resolved_source(
    resolved: &ResolvedPromptSource,
    kind: PromptSourceKind,
    reference_id: &str,
    origin: Option<PromptSourceOrigin>,
) -> bool {
    resolved.kind == kind && resolved.reference_id == reference_id && resolved.origin == origin
}

pub(super) fn prompt_overlay_scope_picker_line(
    scope: PromptAssemblyScope,
    palette: crate::theme::TerminalPalette,
) -> Line<'static> {
    let (project_style, global_style) = match scope {
        PromptAssemblyScope::Project => (
            surface_text_style(palette).bold(),
            secondary_text_style(palette),
        ),
        PromptAssemblyScope::Global => (
            secondary_text_style(palette),
            surface_text_style(palette).bold(),
        ),
    };

    match scope {
        PromptAssemblyScope::Project => Line::from(vec![
            Span::styled("[Project]", project_style),
            Span::raw(" "),
            Span::styled("Global", global_style),
        ]),
        PromptAssemblyScope::Global => Line::from(vec![
            Span::styled("Project", project_style),
            Span::raw(" "),
            Span::styled("[Global]", global_style),
        ]),
    }
}

pub(super) fn prompt_overlay_inactive_row_id(row: &PromptOverlayInactiveRow) -> String {
    match row {
        PromptOverlayInactiveRow::ExtraPromptCandidate { source, .. }
        | PromptOverlayInactiveRow::ExtraPromptShadowedDetail { source } => {
            format!(
                "extra:{}:{}",
                source.reference_id,
                prompt_overlay_origin_label(source.origin)
            )
        }
        PromptOverlayInactiveRow::DiscoveredSkill { skill, .. }
        | PromptOverlayInactiveRow::DiscoveredSkillShadowedDetail { skill } => {
            format!(
                "skill:{}:{}",
                skill.skill_name,
                prompt_overlay_origin_label(skill.origin)
            )
        }
        PromptOverlayInactiveRow::ToolCandidate { tool } => {
            format!("tool:{}", tool.name)
        }
        PromptOverlayInactiveRow::DynamicEnvironmentCandidate { source } => {
            format!("dynamic:{:?}", source.source_kind)
        }
    }
}

pub(super) fn prompt_overlay_left_row_id(row: &PromptOverlayLeftRow) -> String {
    match row {
        PromptOverlayLeftRow::ManagedSource { source, .. } => format!(
            "managed:{}:{}:{}",
            prompt_overlay_kind_label(source.kind),
            source.reference_id,
            source.origin.map_or("none", prompt_overlay_origin_label),
        ),
        PromptOverlayLeftRow::ShadowedDetail { source } => format!(
            "shadowed:{}:{}:{}",
            prompt_overlay_kind_label(source.kind),
            source.reference_id,
            source.origin.map_or("none", prompt_overlay_origin_label),
        ),
    }
}

pub(super) fn prompt_overlay_partition_extra_candidates(
    mut candidates: Vec<PromptAssemblyExtraPromptCandidate>,
) -> Option<(
    PromptAssemblyExtraPromptCandidate,
    Vec<PromptAssemblyExtraPromptCandidate>,
)> {
    if candidates.is_empty() {
        return None;
    }
    candidates.sort_by_key(|candidate| prompt_overlay_origin_sort_key(candidate.origin));
    let mut candidates = candidates.into_iter();
    let winner = candidates.next()?;
    Some((winner, candidates.collect()))
}

pub(super) fn prompt_overlay_extra_candidate_winner(
    candidates: &[PromptAssemblyExtraPromptCandidate],
) -> Option<&PromptAssemblyExtraPromptCandidate> {
    candidates
        .iter()
        .min_by_key(|candidate| prompt_overlay_origin_sort_key(candidate.origin))
}

pub(super) fn prompt_overlay_partition_discovered_skills(
    mut skills: Vec<PromptAssemblyDiscoveredSkill>,
) -> Option<(
    PromptAssemblyDiscoveredSkill,
    Vec<PromptAssemblyDiscoveredSkill>,
)> {
    if skills.is_empty() {
        return None;
    }
    skills.sort_by_key(|skill| prompt_overlay_origin_sort_key(skill.origin));
    let mut skills = skills.into_iter();
    let winner = skills.next()?;
    Some((winner, skills.collect()))
}

pub(super) fn prompt_overlay_discovered_skill_winner(
    skills: &[PromptAssemblyDiscoveredSkill],
) -> Option<&PromptAssemblyDiscoveredSkill> {
    skills
        .iter()
        .min_by_key(|skill| prompt_overlay_origin_sort_key(skill.origin))
}

pub(super) fn prompt_overlay_origin_sort_key(origin: PromptSourceOrigin) -> u8 {
    match origin {
        PromptSourceOrigin::Project => 0,
        PromptSourceOrigin::Global => 1,
        PromptSourceOrigin::Builtin => 2,
    }
}

pub(super) fn prompt_overlay_layout_rects(area: Rect) -> Option<PromptOverlayLayoutRects> {
    let chrome = fullscreen_list_chrome_rects(area)?;
    let [left_pane, _gutter, right_pane] = Layout::horizontal([
        Constraint::Ratio(
            PROMPT_OVERLAY_LEFT_PANE_RATIO_NUMERATOR,
            PROMPT_OVERLAY_PANE_RATIO_DENOMINATOR,
        ),
        Constraint::Length(1),
        Constraint::Ratio(
            PROMPT_OVERLAY_RIGHT_PANE_RATIO_NUMERATOR,
            PROMPT_OVERLAY_PANE_RATIO_DENOMINATOR,
        ),
    ])
    .areas(chrome.body);
    let [_left_header, left_body] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(left_pane);
    let [_right_header, right_body] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(right_pane);
    Some(PromptOverlayLayoutRects {
        chrome,
        left_pane,
        left_body,
        right_pane,
        right_body,
    })
}

pub(super) fn prompt_overlay_dialog_area(anchor_area: Rect) -> Rect {
    let dialog_width = anchor_area.width.min(52);
    let dialog_height = 7u16.min(anchor_area.height);
    let dialog_x = anchor_area
        .x
        .saturating_add(anchor_area.width.saturating_sub(dialog_width) / 2);
    let dialog_y = anchor_area
        .y
        .saturating_add(anchor_area.height.saturating_sub(dialog_height) / 2);
    Rect::new(dialog_x, dialog_y, dialog_width, dialog_height)
}

pub(super) fn prompt_overlay_rect_contains(area: Rect, column: u16, row: u16) -> bool {
    column >= area.x
        && column < area.x.saturating_add(area.width)
        && row >= area.y
        && row < area.y.saturating_add(area.height)
}

pub(super) fn prompt_overlay_visible_offset_for_row(body_area: Rect, row: u16) -> Option<usize> {
    (row >= body_area.y && row < body_area.y.saturating_add(body_area.height))
        .then(|| usize::from(row.saturating_sub(body_area.y)))
}

pub(super) fn prompt_overlay_selection_styles(
    selected: bool,
    focused: bool,
    palette: crate::theme::TerminalPalette,
) -> (Style, Style, &'static str) {
    let visually_selected = selected && focused;
    let item_style = if visually_selected {
        primary_text_style(palette).bold()
    } else {
        secondary_text_style(palette)
    };
    let marker_style = if visually_selected {
        command_accent_text_style(palette)
    } else {
        tertiary_text_style(palette)
    };
    let marker = if visually_selected { "█" } else { " " };
    (item_style, marker_style, marker)
}
