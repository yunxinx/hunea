use ratatui::text::{Line, Span};

use super::{
    display_width::display_width,
    search_highlight::{highlighted_substring_spans, search_match_style},
    selection::SelectableLineRange,
    status_line::truncate_display_width_with_ellipsis,
    theme::{
        TerminalPalette, command_accent_text_style, primary_text_style, secondary_text_style,
        tertiary_text_style,
    },
};

pub(super) const ATTACHED_PROMPT_PICKER_INSET_WIDTH: usize = 2;

const ATTACHED_PROMPT_PICKER_NAME_COLUMN_MAX_WIDTH: usize = 28;
const ATTACHED_PROMPT_PICKER_COLUMN_GAP: usize = 2;
const ATTACHED_PROMPT_PICKER_DESCRIPTION_MIN_WIDTH: usize = 12;

pub(super) struct AttachedPromptPickerRowContent<'a> {
    pub(super) display_name: &'a str,
    pub(super) description: &'a str,
    pub(super) trailing_suffix: Option<&'a str>,
}

pub(super) fn attached_prompt_picker_name_column_width<'a>(
    names: impl Iterator<Item = &'a str>,
    content_width: usize,
) -> usize {
    let max_name_width = names
        .map(display_width)
        .max()
        .unwrap_or(0)
        .min(ATTACHED_PROMPT_PICKER_NAME_COLUMN_MAX_WIDTH);

    if content_width <= ATTACHED_PROMPT_PICKER_DESCRIPTION_MIN_WIDTH {
        return content_width;
    }

    max_name_width
        .min(content_width.saturating_sub(
            ATTACHED_PROMPT_PICKER_COLUMN_GAP + ATTACHED_PROMPT_PICKER_DESCRIPTION_MIN_WIDTH,
        ))
        .max(1)
}

pub(super) fn attached_prompt_picker_selectable_range(
    plain_line: &str,
    width: usize,
) -> SelectableLineRange {
    let end_column = display_width(plain_line.trim_end());
    if end_column <= ATTACHED_PROMPT_PICKER_INSET_WIDTH {
        return SelectableLineRange::blank_hit_range(0, width);
    }

    SelectableLineRange::new(ATTACHED_PROMPT_PICKER_INSET_WIDTH, end_column)
}

pub(super) fn render_attached_prompt_picker_row(
    row: AttachedPromptPickerRowContent<'_>,
    query: &str,
    selected: bool,
    width: usize,
    name_column_width: usize,
    palette: TerminalPalette,
) -> (Line<'static>, String) {
    let inset = ATTACHED_PROMPT_PICKER_INSET_WIDTH.min(width);
    let content_width = width.saturating_sub(inset);
    let name_style = if selected {
        command_accent_text_style(palette).bold()
    } else {
        secondary_text_style(palette)
    };
    let description_style = if selected {
        primary_text_style(palette)
    } else {
        tertiary_text_style(palette)
    };
    let highlighted_name_style = search_match_style(name_style, palette.surface);
    let highlighted_description_style = search_match_style(description_style, palette.surface);
    let description = row.description.trim();
    let trailing_suffix = row.trailing_suffix.and_then(|suffix| {
        let trimmed = suffix.trim();
        (!trimmed.is_empty()).then_some(trimmed)
    });
    let name_width = if description.is_empty() && trailing_suffix.is_none() {
        content_width
    } else {
        name_column_width.min(content_width).max(1)
    };
    let name = truncate_display_width_with_ellipsis(row.display_name, name_width);

    let mut spans = vec![Span::raw(" ".repeat(inset))];
    spans.extend(highlighted_substring_spans(
        &name,
        query,
        name_style,
        highlighted_name_style,
    ));
    let mut plain_line = format!("{}{}", " ".repeat(inset), name);

    if !description.is_empty() || trailing_suffix.is_some() {
        let reserved_name_width = name_width;
        let rendered_name_width = display_width(&name);
        let gap_width = reserved_name_width
            .saturating_sub(rendered_name_width)
            .saturating_add(ATTACHED_PROMPT_PICKER_COLUMN_GAP);
        spans.push(Span::raw(" ".repeat(gap_width)));
        plain_line.push_str(&" ".repeat(gap_width));

        let trailing_suffix = trailing_suffix
            .map(|suffix| truncate_display_width_with_ellipsis(suffix, content_width.max(1)));
        let trailing_suffix_width = trailing_suffix.as_deref().map(display_width).unwrap_or(0);
        let suffix_reserved_width = trailing_suffix
            .as_ref()
            .map(|_| trailing_suffix_width.saturating_add(1))
            .unwrap_or(0);
        let remaining_width = content_width
            .saturating_sub(rendered_name_width + gap_width)
            .saturating_sub(suffix_reserved_width);
        if !description.is_empty() && remaining_width > 0 {
            let description = truncate_display_width_with_ellipsis(description, remaining_width);
            spans.extend(highlighted_substring_spans(
                &description,
                query,
                description_style,
                highlighted_description_style,
            ));
            plain_line.push_str(&description);
        }

        if let Some(trailing_suffix) = trailing_suffix.as_deref() {
            let suffix_start = width.saturating_sub(trailing_suffix_width);
            let spaces_before_suffix = suffix_start.saturating_sub(display_width(&plain_line));
            spans.push(Span::raw(" ".repeat(spaces_before_suffix)));
            spans.extend(highlighted_substring_spans(
                trailing_suffix,
                query,
                description_style,
                highlighted_description_style,
            ));
            plain_line.push_str(&" ".repeat(spaces_before_suffix));
            plain_line.push_str(trailing_suffix);
        }
    }

    plain_line.push_str(&" ".repeat(width.saturating_sub(display_width(&plain_line))));
    spans.push(Span::raw(" ".repeat(
        width.saturating_sub(display_width(plain_line.trim_end())),
    )));
    (Line::from(spans), plain_line)
}
