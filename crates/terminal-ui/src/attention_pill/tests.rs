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
