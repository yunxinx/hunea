use crossterm::event::KeyCode;
use ratatui::style::Color;
use runtime_domain::session::ContextBudgetDisplayPayload;

use crate::{
    Model, ModelOptions, StartupBannerOptions,
    context_budget::heatmap::is_context_budget_heatmap_cell,
    test_helpers::{render_model_buffer, rendered_rows},
    theme::default_palette,
    update::AppEffect,
    update::AppEvent,
};
use runtime_domain::{
    context_budget::SegmentKind,
    session::{ContextBudgetSegmentPayload, ContextBudgetSnapshotPayload},
};

fn ready_model() -> Model {
    Model::new_with_options(StartupBannerOptions::default(), ModelOptions::default())
}

#[test]
fn context_command_emits_open_effect() {
    let mut model = ready_model();
    model.update(AppEvent::Key(KeyCode::Char('/').into()));
    for ch in "context".chars() {
        model.update(AppEvent::Key(KeyCode::Char(ch).into()));
    }
    let effect = model.update(AppEvent::Key(KeyCode::Enter.into()));
    assert_eq!(effect, Some(AppEffect::OpenContextBudget));
}

#[test]
fn context_overlay_header_relative_question_mark() {
    use crate::context_budget::state::context_usage_summary;
    let text = context_usage_summary(
        "local/qwen3",
        ContextBudgetDisplayPayload::Relative { used: 1_200 },
    );
    assert_eq!(text, "local/qwen3 · 1.2k tokens");
}

#[test]
fn context_panel_renders_as_inline_two_column_panel_with_empty_capacity_grid() {
    let mut model = ready_model();
    model.transcript_mut().clear();
    model.set_window(72, 20);
    model.set_palette(default_palette(), true);
    model.open_context_budget_loading();
    model.apply_context_budget_snapshot(context_budget_snapshot());

    let buffer = render_model_buffer(&mut model, 72, 20);
    let rows = rendered_rows(&buffer);
    let empty_cell_color = default_palette().tertiary;

    assert!(
        rows.iter().any(|row| row.trim() == "Context Usage"),
        "context panel title should keep only the stable Context Usage header: {rows:?}"
    );
    assert!(
        rows.iter()
            .any(|row| row.contains("local/qwen3 · 540")),
        "context panel should keep the model name and render legend-style usage summary on the first right-side row: {rows:?}"
    );
    assert_eq!(
        rows.iter().filter(|row| row.contains('━')).count(),
        1,
        "context panel should only render the models-style top rule: {rows:?}"
    );
    let first_legend_row = rows
        .iter()
        .position(|row| row.contains("Messages"))
        .expect("legend should render the leading segment row");
    let messages_row_index = rows
        .iter()
        .position(|row| row.contains("Messages:"))
        .expect("legend should render the messages row");
    let messages_row = &rows[messages_row_index];
    let legend_x = messages_row
        .find("Messages:")
        .expect("messages legend text should exist in the rendered row");
    let has_heatmap_before_legend = (0..legend_x).any(|x| {
        is_context_budget_heatmap_cell(
            &buffer[(
                u16::try_from(x).unwrap_or(0),
                u16::try_from(messages_row_index).unwrap_or(0),
            )],
            default_palette(),
        )
    });
    assert!(
        has_heatmap_before_legend,
        "context panel should render heatmap on the left and legend on the right in the same row: {rows:?}"
    );
    assert!(
        rows.iter().all(|row| !row.contains("Page 1/1")),
        "inline context panel must not render fullscreen page chrome: {rows:?}"
    );
    assert!(
        first_legend_row < 16,
        "legend should stay inside the inline panel body instead of falling below the heatmap: {rows:?}"
    );
    let empty_capacity = buffer
        .content()
        .iter()
        .filter(|cell| {
            is_context_budget_heatmap_cell(cell, default_palette()) && cell.fg == empty_cell_color
        })
        .count();
    assert!(
        empty_capacity > 0,
        "heatmap should keep visible empty-capacity cells instead of leaving trailing blanks"
    );

    let colored_heatmap_cells = buffer
        .content()
        .iter()
        .filter(|cell| {
            is_context_budget_heatmap_cell(cell, default_palette())
                && cell.fg != empty_cell_color
                && cell.fg != Color::Reset
        })
        .count();
    assert!(
        colored_heatmap_cells <= 70,
        "heatmap should stay closer to the reduced-density grid instead of expanding back out: {colored_heatmap_cells}"
    );
    assert!(
        rows.iter().any(|row| row.contains("Free space")),
        "legend should include the free-space row for the full source-based breakdown: {rows:?}"
    );
    assert!(
        rows.iter().any(|row| row.contains("Messages:")),
        "legend rows should use natural language labels instead of the old percent-only format: {rows:?}"
    );
}

#[test]
fn context_panel_only_uses_left_side_of_the_terminal_width() {
    let mut model = ready_model();
    model.transcript_mut().clear();
    model.set_window(72, 20);
    model.set_palette(default_palette(), true);
    model.open_context_budget_loading();
    model.apply_context_budget_snapshot(context_budget_snapshot());

    let render = model.current_inline_context_budget_render_result();
    let top_rule_width = render
        .lines
        .first()
        .map(ratatui::text::Line::width)
        .unwrap_or_default();
    let header_gap_width = render
        .lines
        .get(2)
        .map(ratatui::text::Line::width)
        .unwrap_or_default();
    let body_width = render
        .lines
        .iter()
        .skip(3)
        .take(11)
        .map(ratatui::text::Line::width)
        .max()
        .unwrap_or_default();

    assert!(
        top_rule_width == 72,
        "context panel rule should remain full width even when the content area is narrower: {top_rule_width}"
    );
    assert!(
        header_gap_width == 0,
        "context panel should keep one blank row between the header and the body"
    );
    assert!(
        body_width <= 45,
        "context panel body should stay around the left 60% of the terminal width: {body_width}"
    );
}

fn context_budget_snapshot() -> ContextBudgetSnapshotPayload {
    ContextBudgetSnapshotPayload {
        model_id: "local/qwen3".to_string(),
        segments: vec![
            segment(SegmentKind::System, 0, 140, "system prompt"),
            segment(SegmentKind::UserMessage, 1, 96, "user history"),
            segment(SegmentKind::AssistantMessage, 2, 220, "assistant history"),
            segment(SegmentKind::Reasoning, 3, 44, "reasoning"),
            segment(SegmentKind::ToolResult, 4, 28, "tool results"),
            segment(SegmentKind::ToolDefinitions, 5, 12, "tool schemas"),
        ],
        total_estimated_tokens: 540,
        context_limit: Some(1_280),
        display: ContextBudgetDisplayPayload::Absolute {
            limit: 1_280,
            used: 540,
            percent: 42.2,
        },
    }
}

fn segment(
    kind: SegmentKind,
    stack_order: u16,
    estimated_tokens: usize,
    label: &str,
) -> ContextBudgetSegmentPayload {
    ContextBudgetSegmentPayload {
        kind_tag: kind.default_label().to_string(),
        stack_order,
        estimated_tokens,
        label: label.to_string(),
    }
}
