use crossterm::event::KeyCode;
use runtime_domain::agent::{AgentId, AgentProjectionStatus, AgentRuntimeGeneration};

use crate::AppEffect;

use super::common::{
    apply_overview_delta, overview_row, press_key, ready_panel_model, ready_panel_model_with_rows,
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

    assert!(effect.is_some(), "Space still opens the preview surface");
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
fn x_while_loading_is_ignored() {
    let mut model = crate::Model::new(crate::StartupBannerOptions::default());
    model.set_window(100, 24);
    model.open_agents_panel_loading();

    assert_eq!(press_key(&mut model, KeyCode::Char('x')), None);
    assert!(
        model
            .agents_panel
            .as_ref()
            .unwrap()
            .stop_confirmation
            .is_none()
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
