use crossterm::event::{KeyCode, KeyEvent, MouseButton};
use runtime_domain::session::{ConversationResponse, RuntimeEvent, RuntimeTarget};

use crate::{
    AppEffect, AppEvent, Model, StartupBannerOptions, modal_layer::ModalLayer,
    runtime::RuntimeEventApply, runtime::tool_activity_preview::ToolApprovalPreview,
    theme::default_palette, tool_approval_panel::ToolApprovalSource,
};

fn scrollable_model() -> Model {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(40, 6);
    model.set_palette(default_palette(), true);
    for index in 0..20 {
        model.append_assistant_message_from_runtime(format!("history message {index}"));
    }
    model
}

fn message_finished_event() -> RuntimeEvent {
    RuntimeEvent::MessageFinished {
        target: Some(RuntimeTarget::provider("local", "qwen3")),
        response: ConversationResponse::assistant_text("final answer"),
        finish_reason: None,
        metrics: None,
        context_usage: None,
    }
}

fn runtime_permission_source() -> ToolApprovalSource {
    ToolApprovalSource::RuntimePermission {
        target: RuntimeTarget::provider("local", "qwen3"),
        request_id: "permission-write".to_string(),
        allow_option_id: Some("allow-once".to_string()),
        allow_always_option_id: None,
        reject_option_id: Some("reject-once".to_string()),
        reject_always_option_id: None,
    }
}

fn open_inline_tool_approval(model: &mut Model) {
    model.open_tool_approval_panel_with_preview(
        runtime_permission_source(),
        "WriteFile: temp.md".to_string(),
        Vec::new(),
        None,
    );
}

fn open_fullscreen_capable_tool_approval(model: &mut Model) {
    let content = (1..=30)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    model.open_tool_approval_panel_with_preview(
        runtime_permission_source(),
        "WriteFile: temp.md".to_string(),
        Vec::new(),
        Some(ToolApprovalPreview::create_file(
            "temp.md".to_string(),
            content,
        )),
    );
}

fn press_key(model: &mut Model, code: KeyCode) -> Option<AppEffect> {
    model.update(AppEvent::Key(KeyEvent::from(code)))
}

fn click(model: &mut Model, column: u16, row: u16) -> Option<AppEffect> {
    model.update(AppEvent::MouseDown {
        button: MouseButton::Left,
        column,
        row,
    })
}

#[test]
fn scrolled_up_final_messages_accumulate_and_clear_when_repinned() {
    let mut model = scrollable_model();
    model.scroll_document_by(-4);
    assert!(!model.document_pinned_to_bottom());

    for _ in 0..3 {
        model.apply_runtime_event(message_finished_event());
    }
    assert_eq!(model.attention_pill_new_message_count_for_test(), Some(3));
    assert!(!model.document_pinned_to_bottom());

    // 滚回底部恢复贴底后 pill 清除并清零。
    model.scroll_document_by(100);
    assert!(model.document_pinned_to_bottom());
    assert_eq!(model.attention_pill_new_message_count_for_test(), None);
}

#[test]
fn pinned_final_message_without_modal_layer_shows_no_pill() {
    let mut model = scrollable_model();
    assert!(model.document_pinned_to_bottom());

    model.apply_runtime_event(message_finished_event());

    assert_eq!(model.attention_pill_new_message_count_for_test(), None);
    assert_eq!(model.active_toast_text_for_test(), None);
}

#[test]
fn final_message_behind_fullscreen_modal_shows_pill_and_keeps_layer() {
    let mut model = scrollable_model();
    model.open_session_picker_loading();

    model.apply_runtime_event(message_finished_event());

    assert_eq!(model.attention_pill_new_message_count_for_test(), Some(1));
    assert_eq!(model.top_modal_layer(), Some(ModalLayer::SessionPicker));

    // Esc 关层且用户贴底：消息已可见，pill 在汇聚点被清除。
    assert!(model.document_pinned_to_bottom());
    assert_eq!(press_key(&mut model, KeyCode::Esc), None);
    assert_eq!(model.top_modal_layer(), None);
    assert_eq!(model.attention_pill_new_message_count_for_test(), None);
}

#[test]
fn obscured_approval_sets_pill_and_clears_when_layer_closes() {
    let mut model = scrollable_model();
    model.open_session_picker_loading();

    open_inline_tool_approval(&mut model);
    assert!(model.attention_pill_approval_pending_for_test());
    assert_eq!(model.active_toast_text_for_test(), None);

    // Esc 关层后面板可见，审批 pill 消失。
    assert_eq!(press_key(&mut model, KeyCode::Esc), None);
    assert_eq!(model.top_modal_layer(), None);
    assert!(model.tool_approval_panel_active());
    assert!(!model.attention_pill_approval_pending_for_test());
}

#[test]
fn obscured_approval_pill_clears_when_panel_is_resolved_in_background() {
    let mut model = scrollable_model();
    model.open_session_picker_loading();
    open_inline_tool_approval(&mut model);
    assert!(model.attention_pill_approval_pending_for_test());

    // 审批在后台被处理 / 取消（面板关闭）即清除，无需等待层关闭。
    model.close_runtime_permission_approval_panel();
    assert!(!model.attention_pill_approval_pending_for_test());
}

#[test]
fn approval_on_pinned_main_screen_shows_no_pill() {
    let mut model = scrollable_model();
    assert!(model.document_pinned_to_bottom());

    open_inline_tool_approval(&mut model);

    assert!(model.tool_approval_panel_active());
    assert!(model.tool_approval_panel_visible());
    assert!(!model.attention_pill_approval_pending_for_test());
}

#[test]
fn clicking_new_message_pill_closes_layers_and_repins() {
    let mut model = scrollable_model();
    model.scroll_document_by(-4);
    model.open_session_picker_loading();
    model.apply_runtime_event(message_finished_event());
    assert_eq!(model.attention_pill_new_message_count_for_test(), Some(1));

    // 只有新消息 pill 时它位于第一行。
    assert_eq!(click(&mut model, 1, 0), None);

    assert_eq!(model.top_modal_layer(), None);
    assert!(model.document_pinned_to_bottom());
    assert_eq!(model.attention_pill_new_message_count_for_test(), None);
}

#[test]
fn clicking_approval_pill_closes_layers_and_triggers_deferred_upgrade() {
    let mut model = scrollable_model();
    model.open_session_picker_loading();
    open_fullscreen_capable_tool_approval(&mut model);
    assert!(model.attention_pill_approval_pending_for_test());
    assert!(!model.tool_approval_fullscreen_preview_active());

    assert_eq!(click(&mut model, 1, 0), None);

    // 全屏层关闭、延迟升级生效，Enter 由审批面板消费。
    assert!(model.tool_approval_fullscreen_preview_active());
    assert!(!model.attention_pill_approval_pending_for_test());
    assert_eq!(
        press_key(&mut model, KeyCode::Enter),
        Some(AppEffect::RespondRuntimePermission {
            target: RuntimeTarget::provider("local", "qwen3"),
            request_id: "permission-write".to_string(),
            option_id: Some("allow-once".to_string()),
        })
    );
}

#[test]
fn both_pills_stack_and_dismiss_independently() {
    let mut model = scrollable_model();
    model.scroll_document_by(-4);
    model.open_session_picker_loading();
    // 先到达最终消息，再到达新的审批请求（MessageFinished 会关闭已存在的审批面板）。
    model.apply_runtime_event(message_finished_event());
    open_inline_tool_approval(&mut model);

    let area = ratatui::layout::Rect::new(0, 0, 40, 6);
    let targets = model.attention_pill_hit_targets(area);
    assert_eq!(targets.len(), 2);
    // 审批在上（优先级高），新消息在下。
    assert_eq!(targets[0].1.y, 0);
    assert_eq!(targets[1].1.y, 1);
    assert!(targets[1].2.contains("1 new message ↓"));

    // 点击第二行的新消息 pill：回主界面并贴底；审批面板变为可见，审批 pill 一并收敛。
    assert_eq!(click(&mut model, 1, 1), None);
    assert_eq!(model.attention_pill_new_message_count_for_test(), None);
    assert!(model.document_pinned_to_bottom());
    assert!(model.tool_approval_panel_active());
}

#[test]
fn reset_paths_clear_pill_state() {
    let mut model = scrollable_model();
    model.scroll_document_by(-4);
    model.open_session_picker_loading();
    // 先到达最终消息，再到达新的审批请求（MessageFinished 会关闭已存在的审批面板）。
    model.apply_runtime_event(message_finished_event());
    open_inline_tool_approval(&mut model);
    assert!(model.attention_pill_approval_pending_for_test());
    assert_eq!(model.attention_pill_new_message_count_for_test(), Some(1));

    model.reset_to_initial_tui_state();

    assert!(!model.attention_pill_approval_pending_for_test());
    assert_eq!(model.attention_pill_new_message_count_for_test(), None);
}

#[test]
fn pill_click_requires_hit_and_left_button() {
    let mut model = scrollable_model();
    model.scroll_document_by(-4);
    model.apply_runtime_event(message_finished_event());
    let viewport_y = model.document_runtime.viewport_y;

    // 未命中 pill 的点击不改变待办状态与视口。
    let _ = click(&mut model, 39, 5);
    assert_eq!(model.attention_pill_new_message_count_for_test(), Some(1));
    assert_eq!(model.document_runtime.viewport_y, viewport_y);
}

// ---- v3（R5 / D6）：审批面板开/关不拉底 + 屏外面板按键防护 ----

fn respond_permission_effect() -> AppEffect {
    AppEffect::RespondRuntimePermission {
        target: RuntimeTarget::provider("local", "qwen3"),
        request_id: "permission-write".to_string(),
        option_id: Some("allow-once".to_string()),
    }
}

/// D6-1：主界面非贴底时打开审批面板保持视口位置，并置审批 pill。
#[test]
fn approval_open_while_scrolled_up_keeps_viewport_and_sets_pill() {
    let mut model = scrollable_model();
    model.scroll_document_by(-4);
    assert!(!model.document_pinned_to_bottom());
    let viewport_y = model.document_runtime.viewport_y;

    open_inline_tool_approval(&mut model);

    assert!(model.tool_approval_panel_active());
    assert!(
        !model.tool_approval_panel_visible(),
        "非贴底时内联面板在屏外，不可见"
    );
    assert_eq!(
        model.document_runtime.viewport_y, viewport_y,
        "打开审批面板不得把非贴底视口拉回底部"
    );
    assert!(!model.document_pinned_to_bottom());
    assert!(model.attention_pill_approval_pending_for_test());
}

/// D6-2：屏外面板吞掉审批动作按键——零审批响应、零选择变更，
/// 也不落入 composer 或触发退出确认；面板不因 Esc 关闭。
#[test]
fn offscreen_approval_panel_swallows_action_keys_without_effects() {
    let mut model = scrollable_model();
    model.scroll_document_by(-4);
    open_inline_tool_approval(&mut model);
    let viewport_y = model.document_runtime.viewport_y;

    assert_eq!(
        press_key(&mut model, KeyCode::Enter),
        None,
        "屏外面板 Enter 不得盲批"
    );
    assert!(model.tool_approval_panel_active());

    assert_eq!(press_key(&mut model, KeyCode::Char('y')), None);
    assert!(model.tool_approval_panel_active(), "y 不得在屏外批准");
    assert!(
        model.composer_text().is_empty(),
        "被吞掉的字符不得落入 composer"
    );

    assert_eq!(press_key(&mut model, KeyCode::Down), None);
    assert_eq!(
        model.tool_approval_panel.selected, 0,
        "屏外面板不响应选择移动"
    );

    assert_eq!(press_key(&mut model, KeyCode::Esc), None);
    assert!(
        model.tool_approval_panel_active(),
        "屏外面板 Esc 不得取消审批"
    );
    assert!(
        model.current_status_notice_text().is_empty(),
        "Esc 不得触发退出确认或中断提示"
    );
    assert_eq!(model.document_runtime.viewport_y, viewport_y);
}

/// D6-2 补充：屏外面板不拦截滚动类输入，滚轮照常滚动文档。
#[test]
fn offscreen_approval_panel_keeps_document_wheel_scrolling() {
    let mut model = scrollable_model();
    model.scroll_document_by(-4);
    open_inline_tool_approval(&mut model);
    let viewport_y = model.document_runtime.viewport_y;

    model.update(AppEvent::MouseWheel { delta_lines: -2 });
    // 平滑滚动默认开启：位移发生在渲染帧 drain，此处直接收敛后断言。
    model.settle_smooth_scroll_for_test();

    assert!(
        model.document_runtime.viewport_y < viewport_y,
        "滚轮应照常滚动文档而不被屏外面板吞掉"
    );
    assert!(model.tool_approval_panel_active());
}

/// D6-3：滚回底部后面板可见——pill 消失、按键恢复完整交互。
#[test]
fn scrolling_back_to_bottom_restores_approval_pill_and_keys() {
    let mut model = scrollable_model();
    model.scroll_document_by(-4);
    open_inline_tool_approval(&mut model);
    assert!(model.attention_pill_approval_pending_for_test());
    assert_eq!(press_key(&mut model, KeyCode::Enter), None);

    model.scroll_document_by(100);

    assert!(model.document_pinned_to_bottom());
    assert!(model.tool_approval_panel_visible());
    assert!(
        !model.attention_pill_approval_pending_for_test(),
        "贴底恢复汇聚点应收敛审批 pill"
    );
    assert_eq!(
        press_key(&mut model, KeyCode::Enter),
        Some(respond_permission_effect())
    );
}

/// D6-4：非贴底主界面点击审批 pill——贴底恢复、pill 消失、面板可交互。
#[test]
fn clicking_approval_pill_on_scrolled_main_screen_repins_and_focuses_panel() {
    let mut model = scrollable_model();
    model.scroll_document_by(-4);
    open_inline_tool_approval(&mut model);
    assert!(model.attention_pill_approval_pending_for_test());

    // 审批 pill 是唯一 pill，位于第一行。
    assert_eq!(click(&mut model, 1, 0), None);

    assert!(
        model.document_pinned_to_bottom(),
        "点击审批 pill 应恢复贴底"
    );
    assert!(model.tool_approval_panel_visible());
    assert!(!model.attention_pill_approval_pending_for_test());
    assert_eq!(
        press_key(&mut model, KeyCode::Enter),
        Some(respond_permission_effect())
    );
}

/// D6-5：非贴底打开大 preview 审批不升级 fullscreen；贴底恢复后延迟升级生效。
#[test]
fn offscreen_large_preview_upgrade_defers_until_repinned() {
    let mut model = scrollable_model();
    model.scroll_document_by(-4);
    let viewport_y = model.document_runtime.viewport_y;

    open_fullscreen_capable_tool_approval(&mut model);

    assert!(model.tool_approval_panel_active());
    assert!(
        !model.tool_approval_fullscreen_preview_active(),
        "非贴底时 fullscreen 升级同样是抢屏，必须抑制"
    );
    assert_eq!(model.document_runtime.viewport_y, viewport_y);
    assert!(model.attention_pill_approval_pending_for_test());

    model.scroll_document_by(100);

    assert!(
        model.tool_approval_fullscreen_preview_active(),
        "贴底恢复汇聚点应触发被抑制的延迟升级"
    );
    assert!(!model.attention_pill_approval_pending_for_test());
    assert_eq!(
        press_key(&mut model, KeyCode::Enter),
        Some(respond_permission_effect())
    );
}

/// D6-6：非贴底时面板被 runtime 关闭（PermissionCancelled）保持视口位置。
#[test]
fn offscreen_approval_close_keeps_viewport() {
    let mut model = scrollable_model();
    model.scroll_document_by(-4);
    open_inline_tool_approval(&mut model);
    let viewport_y = model.document_runtime.viewport_y;

    model.apply_runtime_event(RuntimeEvent::PermissionCancelled {
        target: RuntimeTarget::provider("local", "qwen3"),
        request_id: Some("permission-write".to_string()),
    });

    assert!(!model.tool_approval_panel_active());
    assert!(!model.attention_pill_approval_pending_for_test());
    assert!(!model.document_pinned_to_bottom());
    assert_eq!(
        model.document_runtime.viewport_y, viewport_y,
        "runtime 关闭审批面板不得改变非贴底视口"
    );
}

/// D6-6 补充：MessageFinished 在非贴底时关闭面板同样不拉底，
/// 审批 pill 随面板关闭清除，新消息 pill 照常累计。
#[test]
fn offscreen_approval_close_via_message_finished_keeps_viewport() {
    let mut model = scrollable_model();
    model.scroll_document_by(-4);
    open_inline_tool_approval(&mut model);
    let viewport_y = model.document_runtime.viewport_y;

    model.apply_runtime_event(message_finished_event());

    assert!(!model.tool_approval_panel_active());
    assert!(!model.attention_pill_approval_pending_for_test());
    assert_eq!(model.attention_pill_new_message_count_for_test(), Some(1));
    assert!(!model.document_pinned_to_bottom());
    assert_eq!(model.document_runtime.viewport_y, viewport_y);
}

/// D6-7：贴底场景回归——面板开/关保持贴底跟随，可见面板完整交互。
#[test]
fn pinned_approval_open_and_close_keep_bottom_follow() {
    let mut model = scrollable_model();
    assert!(model.document_pinned_to_bottom());

    open_inline_tool_approval(&mut model);
    assert!(
        model.document_pinned_to_bottom(),
        "贴底时打开面板保持贴底跟随"
    );
    assert!(model.tool_approval_panel_visible());
    assert!(!model.attention_pill_approval_pending_for_test());

    assert_eq!(
        press_key(&mut model, KeyCode::Enter),
        Some(respond_permission_effect())
    );
    assert!(!model.tool_approval_panel_active());
    assert!(
        model.document_pinned_to_bottom(),
        "贴底时关闭面板保持贴底跟随"
    );
}

/// R5 范围限定回归：用户主动触发的 Preview 面板非贴底打开必须恢复贴底——
/// Preview 来源不置审批 pill，若面板留在屏外，按键门控会吞掉 Esc，
/// 用户将失去键盘关闭面板的途径。
#[test]
fn user_preview_panel_open_while_scrolled_up_repins_and_stays_interactive() {
    let mut model = scrollable_model();
    model.scroll_document_by(-4);
    assert!(!model.document_pinned_to_bottom());

    model.open_tool_approval_panel_with_preview(
        ToolApprovalSource::Preview,
        "sed -n '1,80p' src/main.rs".to_string(),
        Vec::new(),
        None,
    );

    assert!(
        model.document_pinned_to_bottom(),
        "用户主动预览应恢复贴底使面板可见"
    );
    assert!(model.tool_approval_panel_visible());
    assert!(!model.attention_pill_approval_pending_for_test());

    // 面板可见即保持完整键盘交互；Preview 来源 Esc 关闭面板且不产生审批响应。
    assert_eq!(press_key(&mut model, KeyCode::Esc), None);
    assert!(!model.tool_approval_panel_active());
}

// ---- Agent approval pill（R14/R17）----

use runtime_domain::agent::{
    AgentActivitySummary, AgentId, AgentObjective, AgentObservationId, AgentOverviewRow,
    AgentPermissionRequest, AgentPermissionState, AgentPermissionTarget, AgentPermissionUpdate,
    AgentProjectionEvent, AgentProjectionRevision, AgentProjectionStatus, AgentRuntimeGeneration,
    AgentTitle, AgentTurnId,
};

use crate::attention_pill::AttentionPillKind;

/// 记录派发命令的 test port（与 agents_panel tests 的 RecordingRuntimePort 同构）。
#[derive(Default)]
struct RecordingRuntimePort {
    commands: Vec<runtime_domain::session::RuntimeCommand>,
}

impl crate::runner::runtime_port::RuntimeCommandPort for RecordingRuntimePort {
    fn dispatch_runtime_command(
        &mut self,
        command: runtime_domain::session::RuntimeCommand,
    ) -> Result<runtime_domain::session::RuntimeCommandReceipt, String> {
        self.commands.push(command);
        Ok(runtime_domain::session::RuntimeCommandReceipt::Accepted)
    }
}

impl crate::runner::runtime_port::ModelRuntimePort for RecordingRuntimePort {
    fn drain_model_provider_refresh_events(
        &mut self,
    ) -> Vec<runtime_domain::model_catalog::ModelProviderRefreshEvent> {
        Vec::new()
    }

    fn persist_selected_model(
        &mut self,
        _selection: &runtime_domain::model_catalog::ModelSelection,
    ) -> Result<(), String> {
        Ok(())
    }

    fn refresh_model_provider(
        &mut self,
        _request: runtime_domain::model_catalog::ProviderSyncRequest,
    ) -> Result<(), String> {
        Ok(())
    }
}

impl crate::runner::runtime_port::PromptRuntimePort for RecordingRuntimePort {
    fn begin_prompt_assembly_edit(
        &mut self,
    ) -> Result<runtime_domain::prompt_assembly::PromptAssemblyManagerSnapshot, String> {
        Err("Prompt assembly editing is not available".to_string())
    }

    fn apply_prompt_assembly_edit_mutation(
        &mut self,
        _mutation: runtime_domain::prompt_assembly::PromptAssemblyMutation,
    ) -> Result<runtime_domain::prompt_assembly::PromptAssemblyManagerSnapshot, String> {
        Err("Prompt assembly editing is not available".to_string())
    }

    fn commit_prompt_assembly_edit(&mut self) -> Result<(), String> {
        Ok(())
    }
}

fn agent_permission_request(
    agent_id: u64,
    request_id: &str,
    state: AgentPermissionState,
    occurred_at_ms: i64,
) -> AgentPermissionRequest {
    AgentPermissionRequest {
        target: AgentPermissionTarget {
            agent_id: AgentId::new(agent_id),
            turn_id: AgentTurnId::new(7),
            generation: AgentRuntimeGeneration::new(1),
            runtime_target: runtime_domain::session::RuntimeTarget::provider("local", "qwen3"),
            request_id: request_id.to_string(),
        },
        request: runtime_domain::session::RuntimePermissionRequest::new(
            request_id,
            Some("Run database query".to_string()),
            vec![
                runtime_domain::session::RuntimePermissionOption::new(
                    format!("{request_id}-allow"),
                    "Allow",
                    runtime_domain::session::RuntimePermissionOptionKind::AllowOnce,
                ),
                runtime_domain::session::RuntimePermissionOption::new(
                    format!("{request_id}-deny"),
                    "Deny",
                    runtime_domain::session::RuntimePermissionOptionKind::RejectOnce,
                ),
            ],
        ),
        state,
        occurred_at_ms,
    }
}

fn apply_agent_permission_update(
    model: &mut Model,
    agent_id: u64,
    request: Option<AgentPermissionRequest>,
) {
    model.apply_runtime_event(RuntimeEvent::AgentProjection(Box::new(
        AgentProjectionEvent::AgentPermissionUpdated {
            update: AgentPermissionUpdate {
                agent_id: AgentId::new(agent_id),
                generation: AgentRuntimeGeneration::new(1),
                request,
            },
        },
    )));
}

fn agent_pill_target(model: &Model) -> Option<(u16, u16)> {
    let area = ratatui::layout::Rect::new(0, 0, model.width, model.height);
    model
        .attention_pill_hit_targets(area)
        .into_iter()
        .find(|(kind, _, _)| matches!(kind, AttentionPillKind::AgentApproval))
        .map(|(_, rect, _)| (rect.x + 1, rect.y))
}

fn click_agent_pill(model: &mut Model) -> Option<AppEffect> {
    let (column, row) = agent_pill_target(model).expect("agent approval pill should be visible");
    click(model, column, row)
}

fn waiting_permission_row(agent_id: u64, title: &str) -> AgentOverviewRow {
    AgentOverviewRow {
        agent_id: AgentId::new(agent_id),
        title: AgentTitle::resolve(
            &AgentObjective::new("fallback").expect("objective should be valid"),
            Some(title),
        )
        .expect("title should resolve"),
        status: AgentProjectionStatus::WaitingPermission,
        latest_activity: AgentActivitySummary::Idle,
        elapsed_ms: Some(1000),
        tool_uses: None,
        token_usage: None,
    }
}

/// 打开 panel 并完成 snapshot 投影（复刻 runner open effect + 回包链路）。
fn open_panel_with_rows(
    model: &mut Model,
    port: &mut RecordingRuntimePort,
    rows: Vec<AgentOverviewRow>,
) {
    crate::runner::run_open_agents_panel_effect(model, port);
    let request_id = model
        .agents_panel_pending_overview_request_id_for_test()
        .expect("panel should be loading");
    model.apply_runtime_event(RuntimeEvent::AgentProjection(Box::new(
        AgentProjectionEvent::AgentsOverviewSnapshotLoaded {
            request_id,
            snapshot: runtime_domain::agent::AgentOverviewSnapshot {
                observation_id: AgentObservationId::new(11),
                generation: AgentRuntimeGeneration::new(1),
                revision: AgentProjectionRevision::new(1),
                rows,
            },
        },
    )));
}

#[test]
fn agent_approval_pill_visibility_follows_pending_projection() {
    let mut model = scrollable_model();

    // Pending head 到达即置位（无需打开任何 surface）。
    apply_agent_permission_update(
        &mut model,
        2,
        Some(agent_permission_request(
            2,
            "req-1",
            AgentPermissionState::Pending,
            100,
        )),
    );
    assert!(agent_pill_target(&model).is_some());

    // 收敛（None）后消失。
    apply_agent_permission_update(&mut model, 2, None);
    assert!(agent_pill_target(&model).is_none());
}

#[test]
fn agent_approval_pill_ignores_submitted_heads_and_uses_count_text() {
    let mut model = scrollable_model();
    apply_agent_permission_update(
        &mut model,
        2,
        Some(agent_permission_request(
            2,
            "req-1",
            AgentPermissionState::Submitted,
            100,
        )),
    );
    assert!(
        agent_pill_target(&model).is_none(),
        "Submitted head must not drive the pill"
    );

    // 单 pending：单数文案。
    apply_agent_permission_update(
        &mut model,
        3,
        Some(agent_permission_request(
            3,
            "req-2",
            AgentPermissionState::Pending,
            100,
        )),
    );
    let area = ratatui::layout::Rect::new(0, 0, model.width, model.height);
    let texts: Vec<String> = model
        .attention_pill_hit_targets(area)
        .into_iter()
        .map(|(_, _, text)| text)
        .collect();
    assert!(
        texts
            .iter()
            .any(|text| text.contains("Agent waiting for approval")),
        "single pending text: {texts:?}"
    );

    // 多 pending：复数文案。
    apply_agent_permission_update(
        &mut model,
        4,
        Some(agent_permission_request(
            4,
            "req-3",
            AgentPermissionState::Pending,
            100,
        )),
    );
    let texts: Vec<String> = model
        .attention_pill_hit_targets(area)
        .into_iter()
        .map(|(_, _, text)| text)
        .collect();
    assert!(
        texts
            .iter()
            .any(|text| text.contains("2 agents waiting for approval")),
        "plural pending text: {texts:?}"
    );
}

#[test]
fn agent_pill_stacks_between_tool_approval_and_new_messages() {
    let mut model = scrollable_model();
    model.scroll_document_by(-4);
    model.open_session_picker_loading();
    model.apply_runtime_event(message_finished_event());
    open_inline_tool_approval(&mut model);
    apply_agent_permission_update(
        &mut model,
        2,
        Some(agent_permission_request(
            2,
            "req-1",
            AgentPermissionState::Pending,
            100,
        )),
    );

    let area = ratatui::layout::Rect::new(0, 0, 40, 6);
    let kinds: Vec<AttentionPillKind> = model
        .attention_pill_hit_targets(area)
        .into_iter()
        .map(|(kind, _, _)| kind)
        .collect();
    assert_eq!(kinds.len(), 3);
    assert!(matches!(kinds[0], AttentionPillKind::ToolApproval));
    assert!(matches!(kinds[1], AttentionPillKind::AgentApproval));
    assert!(matches!(kinds[2], AttentionPillKind::NewMessages));
}

#[test]
fn single_pending_click_opens_panel_then_navigates_to_preview() {
    let mut model = scrollable_model();
    apply_agent_permission_update(
        &mut model,
        2,
        Some(agent_permission_request(
            2,
            "req-1",
            AgentPermissionState::Pending,
            100,
        )),
    );

    // 点击：设导航意图并返回 OpenAgentsPanel；pill 保持（pending 事实未消失）。
    assert_eq!(
        click_agent_pill(&mut model),
        Some(AppEffect::OpenAgentsPanel)
    );
    assert_eq!(
        model.agents_panel_pill_navigation,
        Some(
            crate::agents_panel::AgentsPanelPillNavigation::OpenPreview {
                agent_id: AgentId::new(2)
            }
        )
    );
    assert!(
        agent_pill_target(&model).is_some(),
        "pill must stay after click"
    );

    // runner 打开 panel；snapshot 回包后意图消费：直达 preview。
    let mut port = RecordingRuntimePort::default();
    open_panel_with_rows(
        &mut model,
        &mut port,
        vec![waiting_permission_row(2, "research task")],
    );

    assert!(
        model.agents_panel_preview_active(),
        "single pending must open preview"
    );
    assert_eq!(model.agents_panel_pill_navigation, None);
    // ObserveAgentTranscript 经 pending-flag 暂存，runner effect 循环消费派发。
    crate::runner::dispatch_pending_agent_view_observes_if_needed(&mut model, &mut port);
    assert!(
        port.commands.iter().any(|command| matches!(
            command,
            runtime_domain::session::RuntimeCommand::ObserveAgentTranscript { agent_id, .. }
                if *agent_id == AgentId::new(2)
        )),
        "staged observe must be dispatched: {:?}",
        port.commands
    );
}

#[test]
fn multiple_pending_click_preselects_earliest_owner_on_overview() {
    let mut model = scrollable_model();
    apply_agent_permission_update(
        &mut model,
        2,
        Some(agent_permission_request(
            2,
            "req-late",
            AgentPermissionState::Pending,
            300,
        )),
    );
    apply_agent_permission_update(
        &mut model,
        3,
        Some(agent_permission_request(
            3,
            "req-early",
            AgentPermissionState::Pending,
            100,
        )),
    );

    assert_eq!(
        click_agent_pill(&mut model),
        Some(AppEffect::OpenAgentsPanel)
    );
    assert_eq!(
        model.agents_panel_pill_navigation,
        Some(crate::agents_panel::AgentsPanelPillNavigation::Preselect {
            agent_id: AgentId::new(3)
        })
    );

    // 打开 + snapshot：预选最早 owner，停在 overview list（不开 preview）。
    let mut port = RecordingRuntimePort::default();
    open_panel_with_rows(
        &mut model,
        &mut port,
        vec![
            waiting_permission_row(2, "late task"),
            waiting_permission_row(3, "early task"),
        ],
    );

    assert!(
        !model.agents_panel_preview_active(),
        "multi pending stays on list"
    );
    assert_eq!(
        model.agents_panel_selected_agent_id_for_test(),
        Some(AgentId::new(3)),
        "earliest occurred_at owner must be preselected"
    );
    assert_eq!(model.agents_panel_pill_navigation, None);
}

#[test]
fn open_panel_click_navigates_in_place_without_rebuild() {
    // AgentsOverview 已开：不关不重开（observer 不注销），直接导航。
    let mut model = scrollable_model();
    let mut port = RecordingRuntimePort::default();
    open_panel_with_rows(
        &mut model,
        &mut port,
        vec![
            waiting_permission_row(2, "research task"),
            waiting_permission_row(3, "other task"),
        ],
    );
    let observation_before = model.agents_panel_observation_id_for_test();
    apply_agent_permission_update(
        &mut model,
        2,
        Some(agent_permission_request(
            2,
            "req-1",
            AgentPermissionState::Pending,
            100,
        )),
    );

    // 单 pending + panel 已开：直接打开 preview surface，返回 observe effect。
    let effect = click_agent_pill(&mut model);
    assert!(
        matches!(effect, Some(AppEffect::ObserveAgentTranscript { agent_id, .. }) if agent_id == AgentId::new(2)),
        "already-open panel must navigate directly: {effect:?}"
    );
    assert!(model.agents_panel_preview_active());
    assert_eq!(
        model.agents_panel_observation_id_for_test(),
        observation_before,
        "navigation must not close/reopen the panel (observer stays bound)"
    );
}

#[test]
fn navigation_intent_fails_closed_when_target_missing_from_snapshot() {
    let mut model = scrollable_model();
    apply_agent_permission_update(
        &mut model,
        9,
        Some(agent_permission_request(
            9,
            "req-1",
            AgentPermissionState::Pending,
            100,
        )),
    );
    assert_eq!(
        click_agent_pill(&mut model),
        Some(AppEffect::OpenAgentsPanel)
    );

    // snapshot 里没有目标 agent：意图失效停在 list（fail closed）。
    let mut port = RecordingRuntimePort::default();
    open_panel_with_rows(
        &mut model,
        &mut port,
        vec![waiting_permission_row(2, "other task")],
    );

    assert!(!model.agents_panel_preview_active());
    assert_eq!(
        model.agents_panel_selected_agent_id_for_test(),
        Some(AgentId::new(2)),
        "failed navigation must leave the default selection"
    );
    assert_eq!(model.agents_panel_pill_navigation, None);
}

#[test]
fn navigation_intent_is_cancelled_when_panel_closes_while_loading() {
    let mut model = scrollable_model();
    apply_agent_permission_update(
        &mut model,
        2,
        Some(agent_permission_request(
            2,
            "req-1",
            AgentPermissionState::Pending,
            100,
        )),
    );
    assert_eq!(
        click_agent_pill(&mut model),
        Some(AppEffect::OpenAgentsPanel)
    );

    // panel 打开后（loading 中）用户 Esc 关闭：意图作废，后续 snapshot 不得触发导航。
    let mut port = RecordingRuntimePort::default();
    crate::runner::run_open_agents_panel_effect(&mut model, &mut port);
    assert_eq!(press_key(&mut model, KeyCode::Esc), None);
    assert!(!model.agents_panel_active());
    assert_eq!(model.agents_panel_pill_navigation, None);
}
