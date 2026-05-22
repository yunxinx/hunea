use ratatui::{
    style::Modifier,
    text::{Line, Span},
};

use crate::{
    Model,
    inline_panel::inline_panel_rule_line,
    theme::{primary_text_style, secondary_text_style, tertiary_text_style},
    tool_result::TOOL_ACTIVITY_LINE_NUMBER_WIDTH,
    transcript::wrap_prompt_visual_lines,
};

use super::{approval_choice_line, tool_approval_choices};

const FILE_PREVIEW_LINE_NUMBER_WIDTH: usize = TOOL_ACTIVITY_LINE_NUMBER_WIDTH;

pub(super) fn build_file_preview_panel_lines(model: &Model, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let preview = model
        .tool_approval_panel
        .preview
        .as_ref()
        .expect("preview panel lines should only be built when preview exists");
    let mut lines = vec![
        inline_panel_rule_line(width, model.palette),
        Line::from(vec![
            Span::raw(" "),
            Span::styled(
                preview.path().to_string(),
                primary_text_style(model.palette).add_modifier(Modifier::BOLD),
            ),
        ]),
        file_preview_separator_line(width, model),
    ];
    append_file_preview_content_lines(model, width, &mut lines, preview.content());
    lines.push(file_preview_separator_line(width, model));
    lines.push(Line::from(vec![
        Span::raw(" "),
        Span::styled(preview.question(), primary_text_style(model.palette)),
    ]));
    append_file_preview_choice_lines(model, &mut lines);
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        " Esc to cancel · Enter to choose",
        tertiary_text_style(model.palette).add_modifier(Modifier::ITALIC),
    ));
    lines
}

fn file_preview_separator_line(width: usize, model: &Model) -> Line<'static> {
    Line::styled("╌".repeat(width.max(1)), tertiary_text_style(model.palette))
}

fn append_file_preview_content_lines(
    model: &Model,
    width: usize,
    lines: &mut Vec<Line<'static>>,
    content: &str,
) {
    let content_width = width
        .saturating_sub(FILE_PREVIEW_LINE_NUMBER_WIDTH + 2)
        .max(1);
    for (index, content_line) in text_lines_for_file_preview(content).iter().enumerate() {
        let line_number = index + 1;
        let wrapped = wrap_prompt_visual_lines(content_line, content_width, 0);
        if wrapped.is_empty() {
            lines.push(file_preview_content_line(
                model,
                Some(line_number),
                String::new(),
            ));
            continue;
        }

        for (wrapped_index, wrapped_line) in wrapped.into_iter().enumerate() {
            lines.push(file_preview_content_line(
                model,
                (wrapped_index == 0).then_some(line_number),
                wrapped_line.text,
            ));
        }
    }
}

fn text_lines_for_file_preview(content: &str) -> Vec<String> {
    let lines = content.lines().map(str::to_string).collect::<Vec<_>>();
    if lines.is_empty() {
        vec![String::new()]
    } else {
        lines
    }
}

fn file_preview_content_line(
    model: &Model,
    line_number: Option<usize>,
    content: String,
) -> Line<'static> {
    let gutter = match line_number {
        Some(line_number) => format!(
            "{line_number:>width$}  ",
            width = FILE_PREVIEW_LINE_NUMBER_WIDTH
        ),
        None => " ".repeat(FILE_PREVIEW_LINE_NUMBER_WIDTH + 2),
    };
    Line::from(vec![
        Span::styled(gutter, tertiary_text_style(model.palette)),
        Span::styled(content, secondary_text_style(model.palette)),
    ])
}

fn append_file_preview_choice_lines(model: &Model, lines: &mut Vec<Line<'static>>) {
    for (index, choice) in tool_approval_choices(&model.tool_approval_panel)
        .into_iter()
        .enumerate()
    {
        let selected = index == model.tool_approval_panel.selected;
        lines.push(approval_choice_line(
            model,
            index,
            selected,
            choice.file_preview_display_label(),
        ));
    }
}
