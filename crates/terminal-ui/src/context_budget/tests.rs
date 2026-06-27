use crossterm::event::KeyCode;
use runtime_domain::session::ContextBudgetDisplayPayload;

use crate::{
    Model, ModelOptions, StartupBannerOptions,
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
    use crate::context_budget::state::header_summary;
    let text = header_summary(
        "local/qwen3",
        ContextBudgetDisplayPayload::Relative { used: 1_200 },
    );
    assert!(text.contains("/ ?"));
}

#[test]
fn context_overlay_renders_page_rule_body_divider_and_empty_capacity_grid() {
    let mut model = ready_model();
    model.set_window(72, 14);
    model.set_palette(default_palette(), true);
    model.open_context_budget_loading();
    model.apply_context_budget_snapshot(context_budget_snapshot());

    let buffer = render_model_buffer(&mut model, 72, 14);
    let rows = rendered_rows(&buffer);
    let empty_cell_color = default_palette()
        .surface
        .expect("default palette should expose a surface color");

    assert!(
        rows.iter().any(|row| row.contains(" Page 1/1 ")),
        "context overlay should render fullscreen page rule chrome: {rows:?}"
    );
    assert!(
        rows.iter().filter(|row| row.contains('╌')).count() >= 2,
        "context overlay should render both header rule and body divider: {rows:?}"
    );
    let empty_capacity = buffer
        .content()
        .iter()
        .filter(|cell| cell.symbol() == "■" && cell.fg == empty_cell_color)
        .count();
    assert!(
        empty_capacity > 0,
        "heatmap should keep visible empty-capacity cells instead of leaving trailing blanks"
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
