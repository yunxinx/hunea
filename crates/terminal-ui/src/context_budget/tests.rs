use crossterm::event::KeyCode;
use runtime_domain::context_budget::{
    ContextBudgetSnapshot, ContextSegment, ContextTokenLimit, ContextWindowUsage, SegmentKind,
};

use crate::{
    Model, ModelOptions, StartupBannerOptions, runtime::RuntimeEventApply, theme::default_palette,
    update::AppEffect, update::AppEvent,
};

fn ready_model() -> Model {
    Model::new_with_options(StartupBannerOptions::default(), ModelOptions::default())
}

fn limit(value: usize) -> ContextTokenLimit {
    ContextTokenLimit::try_from(value).expect("fixture limit should be valid")
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
    use crate::context_budget::summary::context_usage_summary;
    let text = context_usage_summary(
        "local/qwen3",
        ContextWindowUsage {
            limit: limit(256_000),
            used: 1_200,
        },
    );
    assert_eq!(text, "local/qwen3 · ~1.2k/256k tokens (0.5%)");
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
            payload: ContextBudgetSnapshot {
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
            payload: ContextBudgetSnapshot {
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

fn context_budget_snapshot() -> ContextBudgetSnapshot {
    ContextBudgetSnapshot {
        model_id: "local/qwen3".to_string(),
        segments: vec![
            segment(SegmentKind::System, 140),
            segment(SegmentKind::UserMessage, 96),
            segment(SegmentKind::AssistantMessage, 220),
            segment(SegmentKind::Reasoning, 44),
            segment(SegmentKind::ToolResult, 28),
            segment(SegmentKind::ToolDefinitions, 12),
        ],
        total_estimated_tokens: 540,
        usage: ContextWindowUsage {
            limit: limit(1_280),
            used: 540,
        },
    }
}

fn segment(kind: SegmentKind, estimated_tokens: usize) -> ContextSegment {
    ContextSegment {
        kind,
        estimated_tokens,
    }
}
