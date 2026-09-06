use crossterm::event::KeyCode;
use runtime_domain::agent::{
    AgentId, AgentPermissionRequest, AgentPermissionState, AgentPermissionTarget, AgentTurnId,
};
use runtime_domain::session::{
    RuntimePermissionOption, RuntimePermissionOptionKind, RuntimePermissionRequest, RuntimeTarget,
};

use crate::{
    AppEffect, Model,
    agents_panel::AgentsPanelPreviewPermissionChoice,
    test_helpers::{render_model_buffer, rendered_rows},
};

use super::common::{
    apply_view_snapshot_loaded, apply_view_updated, permission_request, press_key,
    ready_panel_model, view_snapshot, view_snapshot_with_permission,
};

/// 打开 preview 并载入带 permission head 的 snapshot。
fn preview_model_with_permission(permission: AgentPermissionRequest) -> Model {
    let mut model = ready_panel_model();
    let effect = press_key(&mut model, KeyCode::Char(' '));
    let AppEffect::ObserveAgentTranscript { request_id, .. } =
        effect.expect("space dispatches observe")
    else {
        panic!("unexpected effect");
    };
    apply_view_snapshot_loaded(
        &mut model,
        request_id,
        view_snapshot_with_permission(2, 21, Some("committed answer"), Some(permission)),
    );
    model
}

fn pending_permission() -> AgentPermissionRequest {
    permission_request(2, "req-perm", AgentPermissionState::Pending, 100)
}

fn surface_permission_choice(model: &Model) -> Option<AgentsPanelPreviewPermissionChoice> {
    model
        .agents_panel
        .as_ref()?
        .surface
        .as_ref()
        .map(|surface| match surface {
            crate::agents_panel::AgentsPanelSurface::Preview {
                permission_choice, ..
            } => permission_choice.clone(),
            _ => AgentsPanelPreviewPermissionChoice::None,
        })
}

// ---- 渲染形态 ----

#[test]
fn pending_permission_renders_request_and_full_option_set_horizontally() {
    let mut model = preview_model_with_permission(pending_permission());

    let buffer = render_model_buffer(&mut model, 100, 24);
    let rows = rendered_rows(&buffer);

    let request_row = rows
        .iter()
        .find(|row| row.contains("Run database query"))
        .expect("delivery-safe request line should render");
    assert!(
        !request_row.contains("Permission:"),
        "permission block must not add a redundant heading: {request_row}"
    );

    // 宽度允许时 options 紧凑横排：全部 option 同一物理行，marker 在首个 option。
    let options_row = rows
        .iter()
        .find(|row| row.contains("1. Allow"))
        .expect("options should render");
    assert!(
        options_row.contains("2. Deny"),
        "wide layout must join all options on one line: {options_row}"
    );
    assert!(
        options_row.contains("➜ 1. Allow"),
        "selected option must carry the marker: {options_row}"
    );
}

#[test]
fn narrow_budget_degrades_options_to_stable_vertical_lines() {
    // 长标签使横排预算必然不足：稳定降级为每 option 一行。
    let permission = long_label_permission();
    let mut model = preview_model_with_permission(permission);

    let buffer = render_model_buffer(&mut model, 100, 24);
    let rows = rendered_rows(&buffer);
    let allow_row = rows
        .iter()
        .find(|row| row.contains("1. "))
        .expect("first option row should render");
    let deny_row = rows
        .iter()
        .find(|row| row.contains("2. "))
        .expect("second option row should render");
    assert_ne!(
        allow_row, deny_row,
        "options must degrade to separate lines"
    );
    assert!(
        allow_row.contains("➜"),
        "selected marker survives: {allow_row}"
    );
    assert!(
        !deny_row.contains("➜"),
        "unselected has no marker: {deny_row}"
    );
}

#[test]
fn submitted_head_locks_marker_on_submitted_option() {
    let mut model = preview_model_with_permission(permission_request(
        2,
        "req-perm",
        AgentPermissionState::Submitted,
        100,
    ));

    let buffer = render_model_buffer(&mut model, 100, 24);
    let rows = rendered_rows(&buffer);
    // runtime 投影 Submitted 且本地不知道具体 option：无 marker、无可移动 selection。
    let options_row = rows
        .iter()
        .find(|row| row.contains("1. Allow"))
        .expect("options should render");
    assert!(
        !options_row.contains("➜"),
        "projected Submitted without a local choice must not mark any option: {options_row}"
    );

    // 本地锁定（Slice 3 Enter 之后）：marker 固定在已提交 option，不随按键移动。
    let choice = surface_permission_choice(&model).unwrap();
    let AgentsPanelPreviewPermissionChoice::Submitted { .. } = choice else {
        panic!("expected submitted choice, got {choice:?}");
    };
}

#[test]
fn no_pending_renders_no_permission_block() {
    let mut model = ready_panel_model();
    let effect = press_key(&mut model, KeyCode::Char(' '));
    let AppEffect::ObserveAgentTranscript { request_id, .. } = effect.unwrap() else {
        panic!("unexpected effect");
    };
    apply_view_snapshot_loaded(&mut model, request_id, view_snapshot(2, 21, Some("answer")));

    let buffer = render_model_buffer(&mut model, 100, 24);
    let rows = rendered_rows(&buffer);
    assert!(
        rows.iter()
            .all(|row| !row.contains("1. Allow") && !row.contains("waiting")),
        "no pending head must render no permission block: {rows:?}"
    );
}

// ---- selection 移动 ----

#[test]
fn up_down_move_selection_cyclically_while_pending() {
    let mut model = preview_model_with_permission(pending_permission());

    // Down：0 → 1。
    assert_eq!(press_key(&mut model, KeyCode::Down), None);
    assert_eq!(
        surface_permission_choice(&model),
        Some(AgentsPanelPreviewPermissionChoice::Selecting {
            request_id: "req-perm".to_string(),
            selected: 1,
        })
    );

    // j 键等价 Down：循环回 0。
    assert_eq!(press_key(&mut model, KeyCode::Char('j')), None);
    assert_eq!(
        surface_permission_choice(&model),
        Some(AgentsPanelPreviewPermissionChoice::Selecting {
            request_id: "req-perm".to_string(),
            selected: 0,
        })
    );

    // k 键等价 Up：循环到末尾。
    assert_eq!(press_key(&mut model, KeyCode::Char('k')), None);
    assert_eq!(
        surface_permission_choice(&model),
        Some(AgentsPanelPreviewPermissionChoice::Selecting {
            request_id: "req-perm".to_string(),
            selected: 1,
        })
    );
}

#[test]
fn pending_left_right_keep_page_navigation() {
    let mut model = preview_model_with_permission(pending_permission());

    // Left/Right 仍翻页（不动 selection）：长正文制造可滚动区。
    // 每行约 80 列：宽 100（wrap 96）下逐词换行后仍各占一个显示行，
    // 40 行正文必然超过 body 高度（21）。
    let long_answer = (1..=40)
        .map(|index| format!("answer line {index} {}", "x".repeat(64)))
        .collect::<Vec<_>>()
        .join(" ");
    let snapshot = view_snapshot_with_permission(
        2,
        21,
        Some(long_answer.as_str()),
        Some(pending_permission()),
    );
    apply_view_updated(&mut model, snapshot);

    assert_eq!(press_key(&mut model, KeyCode::Right), None);
    let scroll_after_right = current_scroll_offset(&model);
    assert!(
        scroll_after_right > 0,
        "Right must keep paging the body while pending"
    );
    assert_eq!(
        surface_permission_choice(&model),
        Some(AgentsPanelPreviewPermissionChoice::Selecting {
            request_id: "req-perm".to_string(),
            selected: 0,
        }),
        "Left/Right must not move the option selection"
    );

    assert_eq!(press_key(&mut model, KeyCode::Left), None);
    assert_eq!(current_scroll_offset(&model), 0);
}

#[test]
fn no_pending_keeps_paging_keys_unchanged() {
    let mut model = ready_panel_model();
    let effect = press_key(&mut model, KeyCode::Char(' '));
    let AppEffect::ObserveAgentTranscript { request_id, .. } = effect.unwrap() else {
        panic!("unexpected effect");
    };
    // 每行约 80 列：宽 100（wrap 96）下逐词换行后仍各占一个显示行，
    // 40 行正文必然超过 body 高度（21）。
    let long_answer = (1..=40)
        .map(|index| format!("answer line {index} {}", "x".repeat(64)))
        .collect::<Vec<_>>()
        .join(" ");
    apply_view_snapshot_loaded(
        &mut model,
        request_id,
        view_snapshot(2, 21, Some(long_answer.as_str())),
    );

    // 无 pending：Up/Down 回落到翻页（R15 语义零变化）。
    assert_eq!(press_key(&mut model, KeyCode::Down), None);
    assert!(current_scroll_offset(&model) > 0);
    assert_eq!(press_key(&mut model, KeyCode::Up), None);
    assert_eq!(current_scroll_offset(&model), 0);
}

// ---- head 变化重置 ----

#[test]
fn same_request_view_update_keeps_selection() {
    let mut model = preview_model_with_permission(pending_permission());
    press_key(&mut model, KeyCode::Down);

    // 无关快照更新（answer 变化、permission head 同 request）：selection 保持。
    apply_view_updated(
        &mut model,
        view_snapshot_with_permission(2, 21, Some("refreshed answer"), Some(pending_permission())),
    );

    assert_eq!(
        surface_permission_choice(&model),
        Some(AgentsPanelPreviewPermissionChoice::Selecting {
            request_id: "req-perm".to_string(),
            selected: 1,
        })
    );
}

#[test]
fn new_request_head_resets_selection() {
    let mut model = preview_model_with_permission(pending_permission());
    press_key(&mut model, KeyCode::Down);

    // 收敛后推进到新 request：旧 choice 立即失效，selection 重置。
    apply_view_updated(
        &mut model,
        view_snapshot_with_permission(
            2,
            21,
            Some("answer"),
            Some(permission_request(
                2,
                "req-next",
                AgentPermissionState::Pending,
                200,
            )),
        ),
    );

    assert_eq!(
        surface_permission_choice(&model),
        Some(AgentsPanelPreviewPermissionChoice::Selecting {
            request_id: "req-next".to_string(),
            selected: 0,
        })
    );
}

#[test]
fn head_convergence_removes_permission_block() {
    let mut model = preview_model_with_permission(pending_permission());

    apply_view_updated(
        &mut model,
        view_snapshot_with_permission(2, 21, Some("answer"), None),
    );

    assert_eq!(
        surface_permission_choice(&model),
        Some(AgentsPanelPreviewPermissionChoice::None)
    );
    let buffer = render_model_buffer(&mut model, 100, 24);
    assert!(
        rendered_rows(&buffer)
            .iter()
            .all(|row| !row.contains("1. Allow"))
    );
}

#[test]
fn reopened_preview_initializes_choice_from_snapshot_head() {
    let mut model = preview_model_with_permission(pending_permission());
    press_key(&mut model, KeyCode::Down);
    // 返回 list 后重新进入：record snapshot 复用，choice 按 head 重新初始化。
    press_key(&mut model, KeyCode::Char(' '));
    let effect = press_key(&mut model, KeyCode::Char(' '));
    assert_eq!(effect, None, "cached snapshot must not redispatch observe");

    assert_eq!(
        surface_permission_choice(&model),
        Some(AgentsPanelPreviewPermissionChoice::Selecting {
            request_id: "req-perm".to_string(),
            selected: 0,
        })
    );
}

// ---- footer 三档 ----

#[test]
fn footer_hint_has_minimal_back_without_overflow_or_pending() {
    let mut model = ready_panel_model();
    let effect = press_key(&mut model, KeyCode::Char(' '));
    let AppEffect::ObserveAgentTranscript { request_id, .. } = effect.unwrap() else {
        panic!("unexpected effect");
    };
    apply_view_snapshot_loaded(&mut model, request_id, view_snapshot(2, 21, Some("short")));

    let rows = rendered_rows(&render_model_buffer(&mut model, 100, 24));
    let footer = rows
        .iter()
        .find(|row| row.contains("Esc back"))
        .expect("footer should render");
    assert!(footer.contains("Space back"), "back hint: {footer}");
    assert!(
        !footer.contains("page"),
        "no overflow → no scroll hint: {footer}"
    );
    assert!(
        !footer.contains("choose"),
        "no pending → no choose/confirm hint: {footer}"
    );
}

#[test]
fn footer_hint_adds_scroll_tier_when_overflow() {
    let mut model = ready_panel_model();
    let effect = press_key(&mut model, KeyCode::Char(' '));
    let AppEffect::ObserveAgentTranscript { request_id, .. } = effect.unwrap() else {
        panic!("unexpected effect");
    };
    // 每行约 80 列：宽 100（wrap 96）下逐词换行后仍各占一个显示行，
    // 40 行正文必然超过 body 高度（21）。
    let long_answer = (1..=40)
        .map(|index| format!("answer line {index} {}", "x".repeat(64)))
        .collect::<Vec<_>>()
        .join(" ");
    apply_view_snapshot_loaded(
        &mut model,
        request_id,
        view_snapshot(2, 21, Some(long_answer.as_str())),
    );

    let rows = rendered_rows(&render_model_buffer(&mut model, 100, 24));
    let footer = rows
        .iter()
        .find(|row| row.contains("Esc back"))
        .expect("footer should render");
    assert!(footer.contains("←/→/h/l page"), "overflow hint: {footer}");
    assert!(!footer.contains("choose"), "no pending: {footer}");
}

#[test]
fn footer_hint_adds_choose_confirm_tier_when_pending() {
    let mut model = preview_model_with_permission(pending_permission());

    let rows = rendered_rows(&render_model_buffer(&mut model, 100, 24));
    let footer = rows
        .iter()
        .find(|row| row.contains("Esc back"))
        .expect("footer should render");
    assert!(
        footer.contains("↑/↓ choose · Enter confirm"),
        "pending hint: {footer}"
    );
}

#[test]
fn footer_hint_shows_submitted_tier_when_locked() {
    let mut model = preview_model_with_permission(permission_request(
        2,
        "req-perm",
        AgentPermissionState::Submitted,
        100,
    ));

    let rows = rendered_rows(&render_model_buffer(&mut model, 100, 24));
    let footer = rows
        .iter()
        .find(|row| row.contains("Esc back"))
        .expect("footer should render");
    assert!(footer.contains("submitted"), "submitted hint: {footer}");
    assert!(!footer.contains("choose"), "no actionable choose: {footer}");
}

// ---- helpers ----

fn current_scroll_offset(model: &Model) -> usize {
    model
        .agents_panel
        .as_ref()
        .and_then(|panel| panel.surface.as_ref())
        .map(|surface| match surface {
            crate::agents_panel::AgentsPanelSurface::Preview { scroll_offset, .. } => {
                *scroll_offset
            }
            _ => 0,
        })
        .unwrap_or(0)
}

/// 长标签 fixture：保证宽屏下横排预算也不足，触发稳定降级。
fn long_label_permission() -> AgentPermissionRequest {
    AgentPermissionRequest {
        target: AgentPermissionTarget {
            agent_id: AgentId::new(2),
            turn_id: AgentTurnId::new(7),
            generation: runtime_domain::agent::AgentRuntimeGeneration::new(1),
            runtime_target: RuntimeTarget::provider("local", "qwen3"),
            request_id: "req-long".to_string(),
        },
        request: RuntimePermissionRequest::new(
            "req-long",
            Some("Run database query".to_string()),
            vec![
                RuntimePermissionOption::new(
                    "opt-long-allow",
                    "Allow this very verbose permission request label",
                    RuntimePermissionOptionKind::AllowOnce,
                ),
                RuntimePermissionOption::new(
                    "opt-long-deny",
                    "Deny this very verbose permission request label",
                    RuntimePermissionOptionKind::RejectOnce,
                ),
            ],
        ),
        state: AgentPermissionState::Pending,
        occurred_at_ms: 100,
    }
}

// ---- 提交链路（Slice 3） ----

#[test]
fn enter_submits_selected_option_with_typed_target_identity() {
    let permission = pending_permission();
    let expected_target = permission.target.clone();
    let mut model = preview_model_with_permission(permission);

    // 选中第二个 option 后 Enter。
    press_key(&mut model, KeyCode::Down);
    assert_eq!(
        press_key(&mut model, KeyCode::Enter),
        Some(crate::AppEffect::RespondAgentPermission {
            target: expected_target,
            option_id: "req-perm-deny".to_string(),
        }),
        "response must carry the FIFO head's AgentPermissionTarget verbatim"
    );

    // preview 保持打开，choice 立即进入不可重复提交的 submitted state。
    assert!(model.agents_panel_preview_active());
    assert_eq!(
        surface_permission_choice(&model),
        Some(AgentsPanelPreviewPermissionChoice::Submitted {
            request_id: "req-perm".to_string(),
            option_id: Some("req-perm-deny".to_string()),
        })
    );
}

#[test]
fn repeated_enter_dispatches_nothing() {
    let mut model = preview_model_with_permission(pending_permission());

    assert!(press_key(&mut model, KeyCode::Enter).is_some());
    assert_eq!(
        press_key(&mut model, KeyCode::Enter),
        None,
        "local Submitted must swallow repeated Enter without a second dispatch"
    );
}

#[test]
fn submitted_head_enter_dispatches_nothing() {
    let mut model = preview_model_with_permission(permission_request(
        2,
        "req-perm",
        AgentPermissionState::Submitted,
        100,
    ));

    assert_eq!(
        press_key(&mut model, KeyCode::Enter),
        None,
        "runtime-projected Submitted head must not be submittable"
    );
}

#[test]
fn no_pending_enter_dispatches_nothing() {
    let mut model = ready_panel_model();
    let effect = press_key(&mut model, KeyCode::Char(' '));
    let crate::AppEffect::ObserveAgentTranscript { request_id, .. } = effect.unwrap() else {
        panic!("unexpected effect");
    };
    apply_view_snapshot_loaded(&mut model, request_id, view_snapshot(2, 21, Some("answer")));

    assert_eq!(press_key(&mut model, KeyCode::Enter), None);
}

#[test]
fn space_and_esc_return_without_dispatching_any_response() {
    let mut model = preview_model_with_permission(pending_permission());
    // 全局 pending 投影同步置位：Space/Esc 只返回，pending 事实必须保留。
    super::common::apply_permission_update(
        &mut model,
        2,
        super::common::FIXTURE_GENERATION,
        Some(pending_permission()),
    );

    // Space 只返回 list，不派发任何 response（与 main 流 Esc=cancel 的关键差异）。
    assert_eq!(press_key(&mut model, KeyCode::Char(' ')), None);
    assert!(!model.agents_panel_preview_active());
    assert!(
        model
            .agent_pending_permission_head_for_test(AgentId::new(2))
            .is_some(),
        "pending request must be preserved after leaving the preview"
    );

    // 再次进入后 Esc 同样只返回。
    press_key(&mut model, KeyCode::Char(' '));
    assert!(model.agents_panel_preview_active());
    assert_eq!(press_key(&mut model, KeyCode::Esc), None);
    assert!(!model.agents_panel_preview_active());
}

#[test]
fn runner_dispatches_respond_agent_permission_command() {
    let mut model = preview_model_with_permission(pending_permission());
    let effect = press_key(&mut model, KeyCode::Enter).expect("enter dispatches respond effect");
    let crate::AppEffect::RespondAgentPermission { target, option_id } = effect else {
        panic!("unexpected effect");
    };

    let mut port = super::common::RecordingRuntimePort::default();
    crate::runner::run_respond_agent_permission_effect(&mut model, &mut port, target, option_id);

    assert!(
        port.commands.contains(
            &runtime_domain::session::RuntimeCommand::RespondAgentPermission {
                target: pending_permission().target,
                option_id: Some("req-perm-allow".to_string()),
            }
        ),
        "runner must dispatch the existing RespondAgentPermission command: {:?}",
        port.commands
    );
}

#[test]
fn runtime_rejection_unlocks_local_submission_for_retry() {
    let mut model = preview_model_with_permission(pending_permission());
    let effect = press_key(&mut model, KeyCode::Enter).expect("enter dispatches");
    let crate::AppEffect::RespondAgentPermission { target, .. } = effect else {
        panic!("unexpected effect");
    };
    assert_eq!(
        surface_permission_choice(&model),
        Some(AgentsPanelPreviewPermissionChoice::Submitted {
            request_id: "req-perm".to_string(),
            option_id: Some("req-perm-allow".to_string()),
        })
    );

    // runtime 拒绝（Noop port）：toast 报错，snapshot 仍 Pending → 解除本地锁定。
    let mut port = crate::runner::NoopUiRuntimePort;
    crate::runner::run_respond_agent_permission_effect(
        &mut model,
        &mut port,
        target,
        "req-perm-allow".to_string(),
    );

    assert_eq!(
        model.active_toast_text_for_test(),
        Some("Runtime is not available"),
        "late submit / runtime failure must surface as an error toast"
    );
    assert_eq!(
        surface_permission_choice(&model),
        Some(AgentsPanelPreviewPermissionChoice::Selecting {
            request_id: "req-perm".to_string(),
            selected: 0,
        }),
        "snapshot still Pending must unlock the local Submitted state for retry"
    );
}

#[test]
fn submitted_projection_confirms_lock_and_keeps_submitted_marker() {
    let mut model = preview_model_with_permission(pending_permission());
    press_key(&mut model, KeyCode::Down);
    assert!(press_key(&mut model, KeyCode::Enter).is_some());

    // runtime 投影 Submitted（同一 request）：本地锁定保持，marker 固定在已提交 option。
    apply_view_updated(
        &mut model,
        view_snapshot_with_permission(
            2,
            21,
            Some("answer"),
            Some(permission_request(
                2,
                "req-perm",
                AgentPermissionState::Submitted,
                100,
            )),
        ),
    );

    assert_eq!(
        surface_permission_choice(&model),
        Some(AgentsPanelPreviewPermissionChoice::Submitted {
            request_id: "req-perm".to_string(),
            option_id: Some("req-perm-deny".to_string()),
        })
    );
    let rows = rendered_rows(&render_model_buffer(&mut model, 100, 24));
    let options_row = rows
        .iter()
        .find(|row| row.contains("1. Allow"))
        .expect("options should render");
    assert!(
        options_row.contains("➜ 2. Deny"),
        "submitted option keeps the marker without movement: {options_row}"
    );

    // 后续 Enter 仍无派发（runtime 已 Submitted）。
    assert_eq!(press_key(&mut model, KeyCode::Enter), None);
}
