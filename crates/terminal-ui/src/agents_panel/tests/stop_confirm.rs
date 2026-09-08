use crossterm::event::KeyCode;
use runtime_domain::agent::{AgentId, AgentProjectionStatus, AgentRuntimeGeneration};

use crate::AppEffect;

use super::common::{
    apply_overview_delta, apply_overview_snapshot, overview_row, press_key, ready_panel_model,
    ready_panel_model_with_rows, sample_rows,
};

#[test]
fn first_x_arms_confirmation_second_x_dispatches_stop() {
    let mut model = ready_panel_model();

    let first = press_key(&mut model, KeyCode::Char('x'));
    assert_eq!(first, None, "first x only arms the confirmation");
    assert_eq!(
        model.agents_panel.as_ref().unwrap().stop_confirmation,
        Some(AgentId::new(2))
    );

    let second = press_key(&mut model, KeyCode::Char('x'));
    assert_eq!(
        second,
        Some(AppEffect::StopAgent {
            agent_id: AgentId::new(2),
            generation: AgentRuntimeGeneration::new(1),
        })
    );
    assert_eq!(model.agents_panel.as_ref().unwrap().stop_confirmation, None);
}

#[test]
fn x_on_terminal_agent_never_arms() {
    let mut model = ready_panel_model_with_rows(vec![super::common::overview_row(
        3,
        "write docs",
        AgentProjectionStatus::Completed,
    )]);

    assert_eq!(press_key(&mut model, KeyCode::Char('x')), None);
    assert_eq!(model.agents_panel.as_ref().unwrap().stop_confirmation, None);
    assert_eq!(press_key(&mut model, KeyCode::Char('x')), None);
}

#[test]
fn selection_move_cancels_confirmation() {
    // 两行均可 stop，验证取消语义不受终态行干扰。
    let mut model = super::common::ready_panel_model_with_rows(vec![
        super::common::overview_row(2, "research task", AgentProjectionStatus::Working),
        super::common::overview_row(3, "write docs", AgentProjectionStatus::WaitingPermission),
    ]);
    press_key(&mut model, KeyCode::Char('x'));
    assert!(
        model
            .agents_panel
            .as_ref()
            .unwrap()
            .stop_confirmation
            .is_some()
    );

    press_key(&mut model, KeyCode::Char('j'));

    assert_eq!(
        model.agents_panel.as_ref().unwrap().stop_confirmation,
        None,
        "moving selection must cancel the pending stop confirmation"
    );
    // 取消后再次 x 需要重新确认，不跨 selection 生效。
    assert_eq!(press_key(&mut model, KeyCode::Char('x')), None);
    assert_eq!(
        model.agents_panel.as_ref().unwrap().stop_confirmation,
        Some(AgentId::new(3))
    );
}

#[test]
fn other_keys_cancel_confirmation() {
    let mut model = ready_panel_model();
    press_key(&mut model, KeyCode::Char('x'));

    press_key(&mut model, KeyCode::Char('q'));

    assert_eq!(model.agents_panel.as_ref().unwrap().stop_confirmation, None);
}

#[test]
fn entering_surface_cancels_confirmation() {
    let mut model = ready_panel_model();
    press_key(&mut model, KeyCode::Char('x'));

    let effect = press_key(&mut model, KeyCode::Char(' '));

    assert!(effect.is_some(), "Space still opens the transcript surface");
    assert_eq!(model.agents_panel.as_ref().unwrap().stop_confirmation, None);
}

#[test]
fn delta_removing_confirmed_agent_cancels_confirmation() {
    let mut model = ready_panel_model();
    press_key(&mut model, KeyCode::Char('x'));

    apply_overview_delta(
        &mut model,
        runtime_domain::agent::AgentOverviewDeltaKind::Remove {
            agent_id: AgentId::new(2),
        },
    );

    assert_eq!(
        model.agents_panel.as_ref().unwrap().stop_confirmation,
        None,
        "removing the confirmed agent must cancel the confirmation"
    );
}

#[test]
fn delta_making_confirmed_agent_terminal_cancels_confirmation() {
    let mut model = ready_panel_model();
    press_key(&mut model, KeyCode::Char('x'));

    apply_overview_delta(
        &mut model,
        runtime_domain::agent::AgentOverviewDeltaKind::Upsert(overview_row(
            2,
            "research task",
            AgentProjectionStatus::Completed,
        )),
    );

    assert_eq!(
        model.agents_panel.as_ref().unwrap().stop_confirmation,
        None,
        "a terminal delta on the confirmed agent must cancel the confirmation"
    );
}

#[test]
fn x_while_loading_shows_unavailable_notice_and_stays_disarmed() {
    // 复现真机"按 x 无效果"：面板打开后 overview snapshot 未到达（loading 态），
    // stop 没有可寻址目标。此时按键不得被静默吞掉——footer 必须给出可见反馈。
    let mut model = crate::Model::new(crate::StartupBannerOptions::default());
    model.set_window(100, 24);
    model.set_palette(crate::theme::default_palette(), true);
    model.open_agents_panel_loading();

    assert_eq!(press_key(&mut model, KeyCode::Char('x')), None);
    assert_eq!(press_key(&mut model, KeyCode::Char('x')), None);
    assert!(
        model
            .agents_panel
            .as_ref()
            .unwrap()
            .stop_confirmation
            .is_none()
    );

    let rows = crate::test_helpers::rendered_rows(&crate::test_helpers::render_model_buffer(
        &mut model, 100, 24,
    ));
    assert!(
        rows.iter()
            .any(|row| row.contains("Agents state is loading")),
        "x during loading must surface a visible notice: {rows:?}"
    );
}

#[test]
fn loading_notice_clears_once_snapshot_arrives() {
    let mut model = crate::Model::new(crate::StartupBannerOptions::default());
    model.set_window(100, 24);
    model.set_palette(crate::theme::default_palette(), true);
    let request_id = model.open_agents_panel_loading();
    press_key(&mut model, KeyCode::Char('x'));

    apply_overview_snapshot(&mut model, request_id, sample_rows());

    let rows = crate::test_helpers::rendered_rows(&crate::test_helpers::render_model_buffer(
        &mut model, 100, 24,
    ));
    assert!(
        rows.iter()
            .all(|row| !row.contains("Agents state is loading")),
        "the loading notice must clear once the snapshot arrives: {rows:?}"
    );
    // snapshot 就绪后 x 恢复常规二次确认链路。
    assert_eq!(press_key(&mut model, KeyCode::Char('x')), None);
    assert_eq!(
        model.agents_panel.as_ref().unwrap().stop_confirmation,
        Some(AgentId::new(2))
    );
    assert_eq!(
        press_key(&mut model, KeyCode::Char('x')),
        Some(AppEffect::StopAgent {
            agent_id: AgentId::new(2),
            generation: AgentRuntimeGeneration::new(1),
        })
    );
}

#[test]
fn space_and_esc_do_not_implicitly_stop() {
    let mut model = ready_panel_model();
    press_key(&mut model, KeyCode::Char('x'));

    // Esc 直接关闭 panel：不隐式 stop。
    assert_eq!(press_key(&mut model, KeyCode::Esc), None);
    assert!(!model.agents_panel_active());
}
