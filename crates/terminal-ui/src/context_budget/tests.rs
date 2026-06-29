use crossterm::event::KeyCode;
use ratatui::style::Color;
use runtime_domain::session::ContextWindowUsagePayload;

use crate::{
    Model, ModelOptions, StartupBannerOptions,
    context_budget::heatmap::is_context_budget_heatmap_cell,
    runtime::RuntimeEventApply,
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
fn context_overlay_header_shows_documented_absolute_limit() {
    use crate::context_budget::state::context_usage_summary;
    let text = context_usage_summary(
        "local/qwen3",
        ContextWindowUsagePayload {
            limit: 256_000,
            used: 1_200,
            percent: 0.5,
        },
    );
    assert_eq!(text, "local/qwen3 · 1.2k/256k tokens (0.5%)");
}

#[test]
fn context_panel_renders_as_inline_two_column_panel_with_empty_capacity_grid() {
    let mut model = ready_model();
    model.transcript_mut().clear();
    model.set_window(72, 20);
    model.set_palette(default_palette(), true);
    let request_id = model.open_context_budget_loading();
    model.apply_context_budget_snapshot(request_id, context_budget_snapshot());

    let buffer = render_model_buffer(&mut model, 72, 20);
    let rows = rendered_rows(&buffer);
    let empty_cell_color = default_palette().tertiary;

    assert!(
        rows.iter().any(|row| row.trim() == "Context Usage"),
        "context panel title should keep only the stable Context Usage header: {rows:?}"
    );
    assert!(
        rows.iter().any(|row| row.contains("local/qwen3 · 540")),
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
fn context_panel_uses_available_terminal_width_for_body_content() {
    let mut model = ready_model();
    model.transcript_mut().clear();
    model.set_window(120, 20);
    model.set_palette(default_palette(), true);
    let request_id = model.open_context_budget_loading();
    model.apply_context_budget_snapshot(request_id, context_budget_snapshot());

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
        top_rule_width == 120,
        "context panel rule should span the full terminal width: {top_rule_width}"
    );
    assert!(
        header_gap_width == 0,
        "context panel should keep one blank row between the header and the body"
    );
    assert!(
        body_width > 80,
        "context panel body should use the available terminal width instead of staying capped on the left: {body_width}"
    );
}

#[test]
fn context_panel_summary_row_keeps_full_model_usage_text_when_width_allows() {
    let mut model = ready_model();
    model.transcript_mut().clear();
    model.set_window(160, 20);
    model.set_palette(default_palette(), true);
    let request_id = model.open_context_budget_loading();
    model.apply_context_budget_snapshot(
        request_id,
        ContextBudgetSnapshotPayload {
            model_id: "deepseek-v4-flash".to_string(),
            segments: context_budget_snapshot().segments,
            total_estimated_tokens: 1_200,
            usage: ContextWindowUsagePayload {
                limit: 256_000,
                used: 1_200,
                percent: 0.5,
            },
        },
    );

    let rows = rendered_rows(&render_model_buffer(&mut model, 160, 20));

    assert!(
        rows.iter()
            .any(|row| row.contains("deepseek-v4-flash · 1.2k/256k tokens (0.5%)")),
        "summary row should use the available right-side width before truncating: {rows:?}"
    );
}

#[test]
fn context_panel_keeps_blank_row_above_footer_hint() {
    let mut model = ready_model();
    model.transcript_mut().clear();
    model.set_window(72, 20);
    model.set_palette(default_palette(), true);
    let request_id = model.open_context_budget_loading();
    model.apply_context_budget_snapshot(request_id, context_budget_snapshot());

    let render = model.current_inline_context_budget_render_result();
    let footer_index = render
        .plain_lines
        .iter()
        .position(|line| line.contains("Esc close"))
        .expect("footer hint should render");

    assert!(
        footer_index > 0,
        "footer hint should not be the first row: {:?}",
        render.plain_lines
    );
    assert!(
        render.plain_lines[footer_index - 1].trim().is_empty(),
        "context panel should keep a blank spacer row above the footer hint: {:?}",
        render.plain_lines
    );
}

#[test]
fn stale_context_budget_snapshot_is_ignored_after_panel_reopens_loading() {
    let mut model = ready_model();

    let stale_request_id = model.open_context_budget_loading();
    model.update(AppEvent::Key(KeyCode::Esc.into()));
    let current_request_id = model.open_context_budget_loading();

    model.apply_runtime_event(
        runtime_domain::session::RuntimeEvent::ContextBudgetSnapshotLoaded {
            request_id: stale_request_id,
            payload: ContextBudgetSnapshotPayload {
                model_id: "stale-model".to_string(),
                ..context_budget_snapshot()
            },
        },
    );

    let state = model
        .context_budget
        .as_ref()
        .expect("context budget should stay open");
    assert!(state.loading);
    assert_eq!(
        model.context_budget_pending_request_id_for_test(),
        Some(current_request_id)
    );

    model.apply_runtime_event(
        runtime_domain::session::RuntimeEvent::ContextBudgetSnapshotLoaded {
            request_id: current_request_id,
            payload: ContextBudgetSnapshotPayload {
                model_id: "current-model".to_string(),
                ..context_budget_snapshot()
            },
        },
    );

    let state = model
        .context_budget
        .as_ref()
        .expect("context budget should be loaded");
    assert!(!state.loading);
    assert_eq!(
        state
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.model_id.as_str()),
        Some("current-model")
    );
}

#[test]
fn late_context_budget_snapshot_after_close_does_not_reopen_panel() {
    let mut model = ready_model();

    let request_id = model.open_context_budget_loading();
    model.update(AppEvent::Key(KeyCode::Esc.into()));

    model.apply_runtime_event(
        runtime_domain::session::RuntimeEvent::ContextBudgetSnapshotLoaded {
            request_id,
            payload: context_budget_snapshot(),
        },
    );

    assert!(!model.context_budget_active());
}

#[test]
fn late_context_budget_error_after_close_does_not_reopen_panel() {
    let mut model = ready_model();

    let request_id = model.open_context_budget_loading();
    model.update(AppEvent::Key(KeyCode::Esc.into()));

    model.apply_runtime_event(
        runtime_domain::session::RuntimeEvent::ContextBudgetSnapshotLoadFailed {
            request_id,
            error: runtime_domain::session::ContextBudgetLoadErrorPayload::RuntimeInternal {
                detail: Some("stale failure".to_string()),
            },
        },
    );

    assert!(!model.context_budget_active());
}

#[test]
fn context_budget_unsupported_provider_error_uses_actionable_copy() {
    let mut model = ready_model();
    model.transcript_mut().clear();
    model.set_window(72, 20);
    model.set_palette(default_palette(), true);

    let request_id = model.open_context_budget_loading();
    model.apply_runtime_event(
        runtime_domain::session::RuntimeEvent::ContextBudgetSnapshotLoadFailed {
            request_id,
            error: runtime_domain::session::ContextBudgetLoadErrorPayload::UnsupportedProvider {
                provider_kind: runtime_domain::provider::ProviderKind::Anthropic,
            },
        },
    );

    let rows = rendered_rows(&render_model_buffer(&mut model, 72, 20));

    assert!(
        rows.iter()
            .any(|row| row.contains("anthropic cannot show context budget")),
        "unsupported provider error should render actionable guidance instead of a raw enum dump: {rows:?}"
    );
    assert!(
        rows.iter()
            .any(|row| row.contains("OpenAI/OpenAI-compatible")),
        "unsupported provider error should explain which provider family is supported: {rows:?}"
    );
}

fn context_budget_snapshot() -> ContextBudgetSnapshotPayload {
    ContextBudgetSnapshotPayload {
        model_id: "local/qwen3".to_string(),
        segments: vec![
            segment(SegmentKind::System, 0, 140),
            segment(SegmentKind::UserMessage, 1, 96),
            segment(SegmentKind::AssistantMessage, 2, 220),
            segment(SegmentKind::Reasoning, 3, 44),
            segment(SegmentKind::ToolResult, 4, 28),
            segment(SegmentKind::ToolDefinitions, 5, 12),
        ],
        total_estimated_tokens: 540,
        usage: ContextWindowUsagePayload {
            limit: 1_280,
            used: 540,
            percent: 42.2,
        },
    }
}

fn segment(
    kind: SegmentKind,
    stack_order: usize,
    estimated_tokens: usize,
) -> ContextBudgetSegmentPayload {
    ContextBudgetSegmentPayload {
        kind,
        stack_order,
        estimated_tokens,
    }
}
