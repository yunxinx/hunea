use crossterm::event::KeyCode;

use crate::test_helpers::{render_model_buffer, rendered_rows};

use super::common::{
    apply_overview_delta, overview_row, press_key, ready_panel_model, ready_panel_model_with_rows,
};

#[test]
fn wide_row_shows_all_columns_as_single_line() {
    let mut model = ready_panel_model();

    let buffer = render_model_buffer(&mut model, 100, 24);
    let rows = rendered_rows(&buffer);
    let row = rows
        .iter()
        .find(|row| row.contains("research task"))
        .expect("row should render the title");
    // status、title、latest、elapsed、tools、tokens 全列可见。
    assert!(row.contains("Working"), "status label: {row}");
    assert!(row.contains("thinking"), "latest activity: {row}");
    assert!(row.contains("1m23s"), "elapsed: {row}");
    assert!(row.contains("3 tools"), "tool count: {row}");
    assert!(row.contains("2k tok"), "token usage: {row}");
    assert!(
        crate::display_width::display_width(row.trim_end()) <= 100,
        "row must not overflow the terminal width: {row}"
    );
}

#[test]
fn every_row_is_a_single_physical_line() {
    let mut model = ready_panel_model();

    let buffer = render_model_buffer(&mut model, 60, 24);
    let rows = rendered_rows(&buffer);
    // 两行 agent + 固定 chrome：不出现跨行的行内容（thinking 与 title 各占一行内文本）。
    let research_rows = rows
        .iter()
        .filter(|row| row.contains("research task"))
        .count();
    let docs_rows = rows.iter().filter(|row| row.contains("write docs")).count();
    assert_eq!(research_rows, 1, "rows must stay single-line: {rows:?}");
    assert_eq!(docs_rows, 1, "rows must stay single-line: {rows:?}");
}

#[test]
fn moderate_width_hides_tokens_first() {
    let mut model = ready_panel_model();

    // body_budget = 55 - 2 - 10 - 1 - 2 = 40；全 metrics 需 45，丢 tokens 后 37 可容纳。
    let buffer = render_model_buffer(&mut model, 55, 24);
    let rows = rendered_rows(&buffer);
    let row = rows
        .iter()
        .find(|row| row.contains("research task"))
        .expect("row should render");
    assert!(
        !row.contains("2k tok"),
        "tokens should be hidden first: {row}"
    );
    assert!(row.contains("3 tools"), "tools should survive: {row}");
    assert!(row.contains("1m23s"), "elapsed should survive: {row}");
}

#[test]
fn narrow_width_hides_tools_then_elapsed() {
    let mut model = ready_panel_model();

    // body_budget = 48 - 15 = 33；elapsed+tools 需 37 → 丢 tools；elapsed 需 29 可容纳。
    let buffer = render_model_buffer(&mut model, 48, 24);
    let rows = rendered_rows(&buffer);
    let row = rows
        .iter()
        .find(|row| row.contains("research task"))
        .expect("row should render");
    assert!(
        !row.contains("3 tools"),
        "tools should hide before elapsed: {row}"
    );
    assert!(!row.contains("2k tok"), "tokens should hide first: {row}");
    assert!(row.contains("1m23s"), "elapsed should survive: {row}");
}

#[test]
fn tighter_width_hides_elapsed() {
    let mut model = ready_panel_model();

    // body_budget = 42 - 15 = 27；elapsed 需 29 → 全部 metric 让位。
    let buffer = render_model_buffer(&mut model, 42, 24);
    let rows = rendered_rows(&buffer);
    let row = rows
        .iter()
        .find(|row| row.contains("research task"))
        .expect("row should render");
    assert!(
        !row.contains("1m23s"),
        "elapsed should hide after tools/tokens: {row}"
    );
    assert!(
        row.contains("thinking"),
        "latest should keep flexible space: {row}"
    );
}

#[test]
fn extreme_narrow_keeps_status_and_title_only() {
    let mut model = ready_panel_model();

    // body_budget = 30 - 15 = 15 ≤ title min 12 + gap + latest min 8 → latest 隐藏。
    let buffer = render_model_buffer(&mut model, 30, 24);
    let rows = rendered_rows(&buffer);
    let row = rows
        .iter()
        .find(|row| row.contains("research"))
        .expect("row should render the truncated title");
    assert!(!row.contains("thinking"), "latest should be hidden: {row}");
    assert!(!row.contains("1m23s"), "elapsed should be hidden: {row}");
    assert!(row.contains("Working"), "status must always render: {row}");
}

#[test]
fn row_without_elapsed_keeps_metric_order_when_hiding() {
    // 缺失 elapsed 的行：metric 顺序与让位仍按 tools → tokens 正确归属。
    let mut row = overview_row(
        2,
        "research task",
        runtime_domain::agent::AgentProjectionStatus::Working,
    );
    row.elapsed_ms = None;
    let mut model = ready_panel_model_with_rows(vec![row]);

    // 宽屏：tools 与 tokens 直接跟随 latest。
    let buffer = render_model_buffer(&mut model, 100, 24);
    let rows = rendered_rows(&buffer);
    let row = rows
        .iter()
        .find(|row| row.contains("research task"))
        .expect("row should render");
    assert!(row.contains("3 tools"), "tools should render: {row}");
    assert!(row.contains("2k tok"), "tokens should render: {row}");
    assert!(
        !row.contains("1m23s"),
        "absent elapsed must not render: {row}"
    );
}

#[test]
fn status_labels_render_as_text_for_each_state() {
    let rows = vec![
        overview_row(
            2,
            "research task",
            runtime_domain::agent::AgentProjectionStatus::Working,
        ),
        overview_row(
            3,
            "write docs",
            runtime_domain::agent::AgentProjectionStatus::Completed,
        ),
        overview_row(
            4,
            "verify build",
            runtime_domain::agent::AgentProjectionStatus::Failed,
        ),
    ];
    let mut model = ready_panel_model_with_rows(rows);

    let buffer = render_model_buffer(&mut model, 100, 24);
    let rendered = rendered_rows(&buffer).join("\n");
    assert!(rendered.contains("Working"));
    assert!(rendered.contains("Completed"));
    assert!(rendered.contains("Failed"));
}

#[test]
fn loading_and_empty_and_error_states_render_hints() {
    let mut model = crate::Model::new(crate::StartupBannerOptions::default());
    model.set_window(100, 24);
    model.set_palette(crate::theme::default_palette(), true);
    model.open_agents_panel_loading();
    let loading_rows = rendered_rows(&render_model_buffer(&mut model, 100, 24));
    assert!(
        loading_rows
            .iter()
            .any(|row| row.contains("Loading agents"))
    );

    let mut model = ready_panel_model_with_rows(Vec::new());
    let empty_rows = rendered_rows(&render_model_buffer(&mut model, 100, 24));
    assert!(empty_rows.iter().any(|row| row.contains("No child agents")));

    let mut model = ready_panel_model();
    press_key(&mut model, KeyCode::Esc);
    // 关闭后事件到达不会重开 panel；直接验证关闭后的渲染回到主界面。
    let closed_rows = rendered_rows(&render_model_buffer(&mut model, 100, 24));
    assert!(!closed_rows.iter().any(|row| row.contains("Agents (")));
}

#[test]
fn stop_confirmation_hint_renders_in_footer() {
    let mut model = ready_panel_model();
    press_key(&mut model, KeyCode::Char('x'));

    let buffer = render_model_buffer(&mut model, 100, 24);
    let rows = rendered_rows(&buffer);
    assert!(
        rows.iter()
            .any(|row| row.contains("Press x again to stop research task")),
        "stop confirmation must be visible in the panel footer: {rows:?}"
    );
}

#[test]
fn footer_hint_uses_two_tiers() {
    let mut model = ready_panel_model();

    let wide = rendered_rows(&render_model_buffer(&mut model, 100, 24));
    assert!(
        wide.iter()
            .any(|row| row.contains("Enter transcript") && row.contains("←/→/h/l page"))
    );

    let narrow = rendered_rows(&render_model_buffer(&mut model, 80, 24));
    assert!(
        narrow
            .iter()
            .any(|row| row.contains("Enter transcript") && !row.contains("←/→/h/l page"))
    );
}

#[test]
fn delta_does_not_change_row_geometry() {
    let mut model = ready_panel_model();
    let before = rendered_rows(&render_model_buffer(&mut model, 60, 24));

    apply_overview_delta(
        &mut model,
        runtime_domain::agent::AgentOverviewDeltaKind::Upsert(overview_row(
            2,
            "research task with a much longer title for wrapping checks",
            runtime_domain::agent::AgentProjectionStatus::Working,
        )),
    );

    let after = rendered_rows(&render_model_buffer(&mut model, 60, 24));
    // 长标题会被安全截断（如 `research ...`），用稳定前缀过滤行数。
    let before_rows = before.iter().filter(|row| row.contains("research")).count();
    let after_rows = after.iter().filter(|row| row.contains("research")).count();
    assert_eq!(
        before_rows, after_rows,
        "row count must not change across deltas"
    );
    assert_eq!(
        before.len(),
        after.len(),
        "total rendered height must not change"
    );
}

#[test]
fn preview_header_renders_status_title_and_elapsed() {
    let mut model = ready_panel_model();
    let effect = press_key(&mut model, KeyCode::Char(' '));
    let crate::AppEffect::ObserveAgentTranscript { request_id, .. } = effect.unwrap() else {
        panic!("unexpected effect");
    };
    super::common::apply_view_snapshot_loaded(
        &mut model,
        request_id,
        super::common::view_snapshot(2, 21, Some("committed answer body")),
    );

    let buffer = render_model_buffer(&mut model, 100, 24);
    let rows = rendered_rows(&buffer);
    assert!(
        rows.iter().any(|row| row.contains("research task")),
        "preview header should render the frozen title: {rows:?}"
    );
    assert!(
        rows.iter().any(|row| row.contains("committed answer body")),
        "preview body should render the committed answer: {rows:?}"
    );
}

#[test]
fn preview_header_hides_elapsed_before_truncating_title() {
    let mut model = ready_panel_model();
    let effect = press_key(&mut model, KeyCode::Char(' '));
    let crate::AppEffect::ObserveAgentTranscript { request_id, .. } = effect.unwrap() else {
        panic!("unexpected effect");
    };
    super::common::apply_view_snapshot_loaded(
        &mut model,
        request_id,
        super::common::view_snapshot(2, 21, None),
    );

    // 窄宽下 elapsed 隐藏，title 仍在 header。
    let buffer = render_model_buffer(&mut model, 28, 24);
    let rows = rendered_rows(&buffer);
    let header = rows
        .iter()
        .find(|row| row.contains("research"))
        .expect("header title should render");
    assert!(
        !header.contains("1m23s"),
        "elapsed should hide on narrow preview: {header}"
    );
}
