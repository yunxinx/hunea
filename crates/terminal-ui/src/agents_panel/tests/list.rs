use crossterm::event::{KeyCode, MouseButton};
use runtime_domain::agent::{AgentId, AgentProjectionStatus};

use crate::agents_panel::AgentsPanelSurface;

use super::common::{apply_overview_delta, overview_row, press_key, ready_panel_model};

#[test]
fn selection_survives_upsert_of_other_rows() {
    let mut model = ready_panel_model();
    // 选中第二行（agent 3）。
    press_key(&mut model, KeyCode::Char('j'));
    assert_eq!(
        model
            .agents_panel
            .as_ref()
            .unwrap()
            .selected_row()
            .map(|r| r.agent_id),
        Some(AgentId::new(3))
    );

    apply_overview_delta(
        &mut model,
        runtime_domain::agent::AgentOverviewDeltaKind::Upsert(overview_row(
            2,
            "research task (updated)",
            AgentProjectionStatus::Working,
        )),
    );

    assert_eq!(
        model
            .agents_panel
            .as_ref()
            .unwrap()
            .selected_row()
            .map(|r| r.agent_id),
        Some(AgentId::new(3)),
        "upsert of another row must not change selection identity"
    );
}

#[test]
fn selection_clamps_deterministically_when_selected_agent_removed() {
    let mut model = ready_panel_model();
    press_key(&mut model, KeyCode::Char('j'));
    assert_eq!(
        model
            .agents_panel
            .as_ref()
            .unwrap()
            .selected_row()
            .map(|r| r.agent_id),
        Some(AgentId::new(3))
    );

    apply_overview_delta(
        &mut model,
        runtime_domain::agent::AgentOverviewDeltaKind::Remove {
            agent_id: AgentId::new(3),
        },
    );

    let panel = model.agents_panel.as_ref().unwrap();
    assert_eq!(panel.filtered_count(), 1);
    // 确定规则：selected position clamp 到剩余列表（此处回落到唯一行）。
    assert_eq!(
        panel.selected_row().map(|r| r.agent_id),
        Some(AgentId::new(2))
    );
}

#[test]
fn selection_moves_to_newly_upserted_agent_keeps_row_count() {
    let mut model = ready_panel_model();

    apply_overview_delta(
        &mut model,
        runtime_domain::agent::AgentOverviewDeltaKind::Upsert(overview_row(
            4,
            "fresh task",
            AgentProjectionStatus::Pending,
        )),
    );

    let panel = model.agents_panel.as_ref().unwrap();
    assert_eq!(panel.filtered_count(), 3);
    assert_eq!(
        panel.selected_row().map(|r| r.agent_id),
        Some(AgentId::new(2)),
        "selection must stay bound to the original AgentId"
    );
}

#[test]
fn search_filters_by_title_and_exit_restores() {
    let mut model = ready_panel_model();

    press_key(&mut model, KeyCode::Char('/'));
    press_key(&mut model, KeyCode::Char('d'));
    press_key(&mut model, KeyCode::Char('o'));
    press_key(&mut model, KeyCode::Char('c'));

    let panel = model.agents_panel.as_ref().unwrap();
    assert_eq!(panel.filtered_count(), 1, "only 'write docs' matches 'doc'");

    // Esc 先退搜索，再 Esc 关闭 panel。
    press_key(&mut model, KeyCode::Esc);
    assert_eq!(model.agents_panel.as_ref().unwrap().filtered_count(), 2);
    press_key(&mut model, KeyCode::Esc);
    assert!(!model.agents_panel_active());
}

#[test]
fn wheel_moves_selection_by_delta() {
    let mut model = ready_panel_model();

    model.move_agents_panel_selection_by_delta(1);

    assert_eq!(
        model
            .agents_panel
            .as_ref()
            .unwrap()
            .selected_row()
            .map(|r| r.agent_id),
        Some(AgentId::new(3))
    );
}

#[test]
fn mouse_click_selects_visible_row_and_cancels_confirmation() {
    let mut model = ready_panel_model();
    press_key(&mut model, KeyCode::Char('x'));
    assert!(
        model
            .agents_panel
            .as_ref()
            .unwrap()
            .stop_confirmation
            .is_some()
    );

    // 点击 body 第二行（chrome 头部 2 行 + 偏移 1）。
    let _ = model.handle_agents_panel_mouse_down(MouseButton::Left, 0, 3);

    let panel = model.agents_panel.as_ref().unwrap();
    assert_eq!(
        panel.selected_row().map(|r| r.agent_id),
        Some(AgentId::new(3))
    );
    assert!(
        panel.stop_confirmation.is_none(),
        "click changing selection must cancel the stop confirmation"
    );
}

#[test]
fn mouse_click_consumed_in_surface_mode_without_selection_change() {
    let mut model = ready_panel_model();
    let effect = press_key(&mut model, KeyCode::Char(' '));
    assert!(matches!(
        model.agents_panel.as_ref().unwrap().surface,
        Some(AgentsPanelSurface::Preview { .. })
    ));
    assert!(effect.is_some());

    let result = model.handle_agents_panel_mouse_down(MouseButton::Left, 0, 3);

    assert!(!result.is_ignored());
    assert_eq!(
        model
            .agents_panel
            .as_ref()
            .unwrap()
            .selected_row()
            .map(|r| r.agent_id),
        Some(AgentId::new(2)),
        "surface-mode clicks must not change list selection"
    );
}

#[test]
fn page_navigation_moves_selection_by_page() {
    // 30 行、page size 20（高度 24 - chrome 4）：`l` 跳到第二页首行（offset 20）。
    let rows = (0..30)
        .map(|index| {
            overview_row(
                100 + index,
                &format!("task {index}"),
                AgentProjectionStatus::Working,
            )
        })
        .collect();
    let mut model = super::common::ready_panel_model_with_rows(rows);

    press_key(&mut model, KeyCode::Char('l'));

    assert_eq!(
        model
            .agents_panel
            .as_ref()
            .unwrap()
            .selected_row()
            .map(|r| r.agent_id),
        Some(AgentId::new(120)),
        "page next should jump to the first row of the next page"
    );
    press_key(&mut model, KeyCode::Char('h'));
    assert_eq!(
        model
            .agents_panel
            .as_ref()
            .unwrap()
            .selected_row()
            .map(|r| r.agent_id),
        Some(AgentId::new(100)),
        "page previous should jump back to the first page"
    );
}

#[test]
fn empty_snapshot_keeps_panel_with_empty_state() {
    let model = super::common::ready_panel_model_with_rows(vec![]);
    assert!(model.agents_panel_active());
    let panel = model.agents_panel.as_ref().unwrap();
    assert!(!panel.has_rows());
    assert!(panel.selected_row().is_none());
}

#[test]
fn key_down_moves_selection_and_up_returns() {
    let mut model = ready_panel_model();
    press_key(&mut model, KeyCode::Down);
    assert_eq!(
        model
            .agents_panel
            .as_ref()
            .unwrap()
            .selected_row()
            .map(|r| r.agent_id),
        Some(AgentId::new(3))
    );
    press_key(&mut model, KeyCode::Up);
    assert_eq!(
        model
            .agents_panel
            .as_ref()
            .unwrap()
            .selected_row()
            .map(|r| r.agent_id),
        Some(AgentId::new(2))
    );
}
