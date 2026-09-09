use crossterm::event::KeyCode;
use runtime_domain::agent::{AgentId, AgentProjectionStatus, AgentRuntimeGeneration};

use crate::AppEffect;
use crate::agents_panel::AgentsPanelStopConfirmation;

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
        Some(AgentsPanelStopConfirmation::Stop(AgentId::new(2)))
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
fn first_x_arms_delete_confirmation_on_settled_row_second_x_dispatches_stop() {
    // settled 投影行的 x 是 delete 语义：同一 StopAgent 命令，runtime 侧行删除清理。
    let mut model = ready_panel_model_with_rows(vec![overview_row(
        3,
        "write docs",
        AgentProjectionStatus::Completed,
    )]);

    let first = press_key(&mut model, KeyCode::Char('x'));
    assert_eq!(first, None, "first x on a settled row arms the delete");
    assert_eq!(
        model.agents_panel.as_ref().unwrap().stop_confirmation,
        Some(AgentsPanelStopConfirmation::Delete(AgentId::new(3)))
    );

    let second = press_key(&mut model, KeyCode::Char('x'));
    assert_eq!(
        second,
        Some(AppEffect::StopAgent {
            agent_id: AgentId::new(3),
            generation: AgentRuntimeGeneration::new(1),
        })
    );
    assert_eq!(model.agents_panel.as_ref().unwrap().stop_confirmation, None);
}

#[test]
fn delete_confirmation_removes_the_row_projection() {
    // 派发 delete 后 runtime 侧发布 Remove delta：投影行从面板移除。
    let mut model = ready_panel_model_with_rows(vec![
        overview_row(2, "research task", AgentProjectionStatus::Working),
        overview_row(3, "write docs", AgentProjectionStatus::Completed),
    ]);
    // 选中 settled 行。
    press_key(&mut model, KeyCode::Char('j'));
    assert_eq!(
        model.agents_panel_selected_agent_id_for_test(),
        Some(AgentId::new(3))
    );

    press_key(&mut model, KeyCode::Char('x'));
    press_key(&mut model, KeyCode::Char('x'));

    apply_overview_delta(
        &mut model,
        runtime_domain::agent::AgentOverviewDeltaKind::Remove {
            agent_id: AgentId::new(3),
        },
    );
    let panel = model.agents_panel.as_ref().unwrap();
    assert!(
        !panel
            .list
            .rows()
            .iter()
            .any(|row| row.agent_id == AgentId::new(3)),
        "the deleted agent's projection row must be removed"
    );
    assert_eq!(panel.filtered_count(), 1);
}

#[test]
fn x_on_cleanup_blocked_row_is_inert() {
    let mut model = ready_panel_model_with_rows(vec![overview_row(
        3,
        "blocked docs",
        AgentProjectionStatus::CleanupBlocked,
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
        Some(AgentsPanelStopConfirmation::Stop(AgentId::new(3)))
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
        "a terminal delta on the confirmed agent must cancel the stop confirmation"
    );
}

#[test]
fn delta_making_confirmed_agent_running_cancels_delete_confirmation() {
    // armed 的是 delete：行回到 running 也不得静默转为 stop 确认。
    let mut model = ready_panel_model_with_rows(vec![overview_row(
        3,
        "write docs",
        AgentProjectionStatus::Completed,
    )]);
    press_key(&mut model, KeyCode::Char('x'));
    assert_eq!(
        model.agents_panel.as_ref().unwrap().stop_confirmation,
        Some(AgentsPanelStopConfirmation::Delete(AgentId::new(3)))
    );

    apply_overview_delta(
        &mut model,
        runtime_domain::agent::AgentOverviewDeltaKind::Upsert(overview_row(
            3,
            "write docs",
            AgentProjectionStatus::Working,
        )),
    );

    assert_eq!(
        model.agents_panel.as_ref().unwrap().stop_confirmation,
        None,
        "a running delta on the delete-confirmed agent must cancel the confirmation"
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
        Some(AgentsPanelStopConfirmation::Stop(AgentId::new(2)))
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
fn narrow_width_falls_back_to_footer_for_the_confirm_hint() {
    // 80 列下 latest 列只剩 20 宽，放不下 23 宽的完整内联提示：footer 回退显示
    // 完整文案，行内 latest 恢复活动文本——截断成 "…" 的提示让二次确认要求
    // 不可读，第二次 x 会在用户无感知的情况下直接触发 stop。
    let mut model = ready_panel_model();
    model.set_window(80, 24);
    press_key(&mut model, KeyCode::Char('x'));

    let rows = crate::test_helpers::rendered_rows(&crate::test_helpers::render_model_buffer(
        &mut model, 80, 24,
    ));
    // footer 是全屏 chrome 的最后一行。
    let footer = rows.last().expect("list footer should render");
    assert!(
        footer.contains("Press x again to stop"),
        "narrow width must surface the confirm hint in the footer: {footer}"
    );
    assert!(
        rows[..rows.len() - 1]
            .iter()
            .all(|row| !row.contains("Press x again")),
        "the truncated inline hint must not render in the row: {rows:?}"
    );
}

#[test]
fn wide_width_keeps_the_inline_confirm_hint_out_of_the_footer() {
    // 100 列下 latest 列（40 宽）放得下完整提示：维持内联现状，footer 不重复。
    let mut model = ready_panel_model();
    press_key(&mut model, KeyCode::Char('x'));

    let rows = crate::test_helpers::rendered_rows(&crate::test_helpers::render_model_buffer(
        &mut model, 100, 24,
    ));
    let cursor_row = rows
        .iter()
        .find(|row| row.contains("research task"))
        .expect("the selected row should render");
    assert!(
        cursor_row.contains("Press x again to stop"),
        "wide width keeps the inline hint on the selected row: {cursor_row}"
    );
    let footer = rows.last().expect("list footer should render");
    assert!(
        !footer.contains("Press x again"),
        "the footer must not duplicate the inline hint: {footer}"
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
