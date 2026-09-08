use crossterm::event::{KeyCode, MouseButton};
use runtime_domain::agent::{AgentId, AgentProjectionStatus, AgentTranscriptItem};

use crate::agents_panel::{agents_activity_fold_entries, agents_panel_list_page_size};
use crate::test_helpers::{render_model_buffer, rendered_rows};

use super::common::{
    apply_overview_delta, apply_view_snapshot_loaded, apply_view_updated, overview_row, press_key,
    ready_panel_model, view_snapshot, view_snapshot_with_items,
};

/// 六条 tool 活动条目 fixture：折叠区只展示尾部 3 条，其余计为 more。
fn six_tool_items() -> Vec<AgentTranscriptItem> {
    (0..6)
        .map(|index| AgentTranscriptItem::Tool {
            title: format!("step {index}"),
            content: String::new(),
        })
        .collect()
}

/// 选中 agent 建立带 6 条活动的 view snapshot，回到 overview list 并按 Tab 展开折叠区。
fn ready_model_with_selected_fold() -> crate::Model {
    let mut model = ready_panel_model();
    let crate::AppEffect::ObserveAgentTranscript { request_id, .. } =
        press_key(&mut model, KeyCode::Enter).expect("Enter should request the view observation")
    else {
        panic!("unexpected effect");
    };
    apply_view_snapshot_loaded(
        &mut model,
        request_id,
        view_snapshot_with_items(2, 21, None, six_tool_items()),
    );
    // Esc 从 transcript surface 回到 list；record 保留供折叠区复用。
    press_key(&mut model, KeyCode::Esc);
    // 折叠区仅由"选中行上按 Tab"展开。
    press_key(&mut model, KeyCode::Tab);
    model
}

#[test]
fn fold_entries_take_latest_tool_and_assistant_items() {
    let (entries, hidden) = agents_activity_fold_entries(&six_tool_items());
    assert_eq!(entries, vec!["step 3", "step 4", "step 5"]);
    assert_eq!(hidden, 3, "earlier entries fold into the more counter");

    // User 条目是发起指令不算活动；多行内容取首个非空行。
    let items = vec![
        AgentTranscriptItem::User {
            content: "run it".to_string(),
        },
        AgentTranscriptItem::Assistant {
            content: "\n  first answer line\nsecond line".to_string(),
        },
    ];
    let (entries, hidden) = agents_activity_fold_entries(&items);
    assert_eq!(entries, vec!["first answer line"]);
    assert_eq!(hidden, 0);

    // 空内容条目不产生折叠行。
    let items = vec![AgentTranscriptItem::Assistant {
        content: "  \n ".to_string(),
    }];
    let (entries, hidden) = agents_activity_fold_entries(&items);
    assert!(entries.is_empty());
    assert_eq!(hidden, 0);
}

#[test]
fn idle_activity_text_is_filtered_from_fold_and_latest_column() {
    // 摘要恰为 Idle 文案的条目不进入折叠区（无价值信息）。
    let items = vec![
        AgentTranscriptItem::Assistant {
            content: "idle".to_string(),
        },
        AgentTranscriptItem::Tool {
            title: "step 1".to_string(),
            content: String::new(),
        },
    ];
    let (entries, hidden) = agents_activity_fold_entries(&items);
    assert_eq!(entries, vec!["step 1"]);
    assert_eq!(hidden, 0);

    // latest 列的 Idle 文本同样不渲染。
    let mut row = overview_row(2, "research task", AgentProjectionStatus::Working);
    row.latest_activity = runtime_domain::agent::AgentActivitySummary::Idle;
    let layout = crate::agents_panel::list_render::agents_panel_row_layout(&row, 100, false);
    assert!(
        layout.latest.is_none(),
        "Idle latest activity must hide the latest column"
    );
}

#[test]
fn fold_cache_follows_selection_and_view_snapshots() {
    let mut model = ready_panel_model();
    assert!(
        model
            .agents_panel
            .as_ref()
            .unwrap()
            .selected_activity_fold()
            .entries
            .is_empty(),
        "no view snapshot yet: fold stays empty"
    );

    let crate::AppEffect::ObserveAgentTranscript { request_id, .. } =
        press_key(&mut model, KeyCode::Enter).expect("Enter should dispatch the observation")
    else {
        panic!("unexpected effect");
    };
    apply_view_snapshot_loaded(&mut model, request_id, view_snapshot(2, 21, None));
    press_key(&mut model, KeyCode::Esc);

    let fold = model
        .agents_panel
        .as_ref()
        .unwrap()
        .selected_activity_fold();
    assert_eq!(fold.agent_id, Some(AgentId::new(2)));
    // 默认 fixture：User 被跳过，Tool 与 Assistant 各一条。
    assert_eq!(fold.entries, vec!["QueryDatabase: users", "partial draft"]);
    assert_eq!(fold.more_count, 0);

    // 选中移动到无 snapshot 的 agent：折叠区跟随并清空。
    press_key(&mut model, KeyCode::Char('j'));
    let fold = model
        .agents_panel
        .as_ref()
        .unwrap()
        .selected_activity_fold();
    assert_eq!(fold.agent_id, None);
    assert!(fold.entries.is_empty());

    // 移回：缓存从保留的 record snapshot 重建。
    press_key(&mut model, KeyCode::Char('k'));
    let fold = model
        .agents_panel
        .as_ref()
        .unwrap()
        .selected_activity_fold();
    assert_eq!(fold.agent_id, Some(AgentId::new(2)));
    assert_eq!(fold.entries.len(), 2);
}

#[test]
fn view_updates_refresh_the_fold_cache() {
    let mut model = ready_model_with_selected_fold();

    // 更长 transcript 到达：取尾部 3 条，其余折叠为 more。
    apply_view_updated(
        &mut model,
        view_snapshot_with_items(2, 21, None, six_tool_items()),
    );
    let fold = model
        .agents_panel
        .as_ref()
        .unwrap()
        .selected_activity_fold();
    assert_eq!(fold.entries, vec!["step 3", "step 4", "step 5"]);
    assert_eq!(fold.more_count, 3);
}

#[test]
fn removing_the_selected_agent_clears_the_stale_fold() {
    let mut model = ready_model_with_selected_fold();
    assert_eq!(
        model
            .agents_panel
            .as_ref()
            .unwrap()
            .selected_activity_fold()
            .agent_id,
        Some(AgentId::new(2))
    );

    apply_overview_delta(
        &mut model,
        runtime_domain::agent::AgentOverviewDeltaKind::Remove {
            agent_id: AgentId::new(2),
        },
    );

    // selection clamp 到剩余行后，旧缓存归属随之失效清空。
    let panel = model.agents_panel.as_ref().unwrap();
    assert_eq!(
        panel.selected_row().map(|row| row.agent_id),
        Some(AgentId::new(3))
    );
    assert_eq!(panel.selected_activity_fold().agent_id, None);
    assert!(panel.selected_activity_fold().entries.is_empty());
}

// ---- Tab 折叠区状态机 ----

#[test]
fn fold_is_collapsed_until_tab_and_tab_toggles() {
    let mut model = ready_panel_model();
    let crate::AppEffect::ObserveAgentTranscript { request_id, .. } =
        press_key(&mut model, KeyCode::Enter).expect("Enter should dispatch the observation")
    else {
        panic!("unexpected effect");
    };
    apply_view_snapshot_loaded(
        &mut model,
        request_id,
        view_snapshot_with_items(2, 21, None, six_tool_items()),
    );
    press_key(&mut model, KeyCode::Esc);

    // 默认未展开：选中行下方没有折叠区行。
    let rows = rendered_rows(&render_model_buffer(&mut model, 100, 24));
    assert!(
        !rows.iter().any(|row| row.contains("step 5")),
        "fold must stay collapsed without Tab: {rows:?}"
    );

    // Tab 展开，再 Tab 关闭。
    press_key(&mut model, KeyCode::Tab);
    let rows = rendered_rows(&render_model_buffer(&mut model, 100, 24));
    assert!(rows.iter().any(|row| row.contains("step 5")));
    press_key(&mut model, KeyCode::Tab);
    let rows = rendered_rows(&render_model_buffer(&mut model, 100, 24));
    assert!(!rows.iter().any(|row| row.contains("step 5")));
}

#[test]
fn selection_change_resets_the_expanded_state() {
    let mut model = ready_model_with_selected_fold();
    assert!(model.agents_panel.as_ref().unwrap().activity_fold_expanded);

    // 离开选中行即重置；回到原行也保持未展开（须重新按 Tab）。
    press_key(&mut model, KeyCode::Char('j'));
    assert!(!model.agents_panel.as_ref().unwrap().activity_fold_expanded);
    press_key(&mut model, KeyCode::Char('k'));
    assert!(
        !model.agents_panel.as_ref().unwrap().activity_fold_expanded,
        "returning to the row must not restore the previous expansion"
    );
    let rows = rendered_rows(&render_model_buffer(&mut model, 100, 24));
    assert!(!rows.iter().any(|row| row.contains("step 5")));

    // 快照更新不改变展开态。
    press_key(&mut model, KeyCode::Tab);
    apply_view_updated(
        &mut model,
        view_snapshot_with_items(2, 21, None, six_tool_items()),
    );
    assert!(model.agents_panel.as_ref().unwrap().activity_fold_expanded);
}

#[test]
fn search_filter_row_switch_resets_the_expanded_state() {
    let mut model = ready_model_with_selected_fold();

    // 搜索过滤导致选中行迁移：折叠态重置。
    press_key(&mut model, KeyCode::Char('/'));
    press_key(&mut model, KeyCode::Char('d'));
    press_key(&mut model, KeyCode::Char('o'));
    press_key(&mut model, KeyCode::Char('c'));
    assert_eq!(
        model.agents_panel.as_ref().unwrap().filtered_count(),
        1,
        "only 'write docs' matches 'doc'"
    );
    assert!(!model.agents_panel.as_ref().unwrap().activity_fold_expanded);
}

#[test]
fn selected_row_renders_activity_fold_lines() {
    let mut model = ready_model_with_selected_fold();

    let rows = rendered_rows(&render_model_buffer(&mut model, 100, 24));
    let step_rows: Vec<&String> = rows.iter().filter(|row| row.contains("step")).collect();
    assert_eq!(step_rows.len(), 3, "3 activity lines: {rows:?}");
    assert!(rows.iter().any(|row| row.contains("+3 more")));
    // 连续展开视觉：非最后行竖线 `│`，仅最后一行（more 行）`↳`。
    let continuation_rows: Vec<&String> = rows.iter().filter(|row| row.contains("│")).collect();
    assert_eq!(
        continuation_rows.len(),
        3,
        "non-last fold rows use │: {rows:?}"
    );
    let arrow_rows: Vec<&String> = rows.iter().filter(|row| row.contains("↳")).collect();
    assert_eq!(
        arrow_rows.len(),
        1,
        "only the last fold row uses ↳: {rows:?}"
    );
    assert!(arrow_rows[0].contains("+3 more"));
}

#[test]
fn fold_without_more_line_terminates_with_arrow_on_last_entry() {
    // 只有一条活动：该行自身收尾用 `↳`。
    let items = vec![AgentTranscriptItem::Tool {
        title: "only step".to_string(),
        content: String::new(),
    }];
    let mut model = ready_panel_model();
    let crate::AppEffect::ObserveAgentTranscript { request_id, .. } =
        press_key(&mut model, KeyCode::Enter).expect("Enter should dispatch the observation")
    else {
        panic!("unexpected effect");
    };
    apply_view_snapshot_loaded(
        &mut model,
        request_id,
        view_snapshot_with_items(2, 21, None, items),
    );
    press_key(&mut model, KeyCode::Esc);
    press_key(&mut model, KeyCode::Tab);

    let rows = rendered_rows(&render_model_buffer(&mut model, 100, 24));
    let arrow_rows: Vec<&String> = rows.iter().filter(|row| row.contains("↳")).collect();
    assert_eq!(arrow_rows.len(), 1, "single entry: {rows:?}");
    assert!(arrow_rows[0].contains("only step"));
    assert!(!rows.iter().any(|row| row.contains("│")));
}

#[test]
fn narrow_width_hides_the_whole_fold_region() {
    let mut model = ready_model_with_selected_fold();

    let rows = rendered_rows(&render_model_buffer(&mut model, 59, 24));
    assert!(
        !rows
            .iter()
            .any(|row| row.contains("↳") || row.contains("│")),
        "fold region must hide below the width floor: {rows:?}"
    );
    // 主行不受折叠区隐藏影响，标题仍在。
    assert!(rows.iter().any(|row| row.contains("research task")));
}

#[test]
fn page_budget_reserves_lines_for_the_activity_fold() {
    // 高度 24 - chrome 4 = 20 行 body；列头 1 行 + 折叠区恒定预留 4 行 → 每页 15 行。
    assert_eq!(agents_panel_list_page_size(24), 15);
    assert_eq!(agents_panel_list_page_size(12), 3);
    // body 行数不足时保底 1 行。
    assert_eq!(agents_panel_list_page_size(9), 1);
}

#[test]
fn fold_lines_count_into_the_page_row_budget() {
    // 40 行 agents、高度 24：page size 15。选中行 + 4 折叠行必须完整渲染在
    // body 内（page 预算已为其预留），不得被 body 截尾。
    let rows = (0..40)
        .map(|index| {
            overview_row(
                100 + index,
                &format!("task {index}"),
                AgentProjectionStatus::Working,
            )
        })
        .collect();
    let mut model = super::common::ready_panel_model_with_rows(rows);
    let crate::AppEffect::ObserveAgentTranscript { request_id, .. } =
        press_key(&mut model, KeyCode::Enter).expect("Enter should dispatch the observation")
    else {
        panic!("unexpected effect");
    };
    apply_view_snapshot_loaded(
        &mut model,
        request_id,
        view_snapshot_with_items(100, 21, None, six_tool_items()),
    );
    press_key(&mut model, KeyCode::Esc);
    press_key(&mut model, KeyCode::Tab);

    let rendered = rendered_rows(&render_model_buffer(&mut model, 100, 24));
    let fold_rows: Vec<&String> = rendered
        .iter()
        .filter(|row| row.contains("step") || row.contains("+3 more"))
        .collect();
    assert_eq!(
        fold_rows.len(),
        4,
        "the full fold must fit inside the page body: {rendered:?}"
    );
    // page rule 仍在（page 预算没有把 chrome 挤掉）。
    assert!(rendered.iter().any(|row| row.contains("1/3")));
}

#[test]
fn mouse_click_maps_physical_lines_with_fold_rows() {
    let mut model = ready_model_with_selected_fold();

    // body 自终端第 2 行起，首行是列头；选中行占 5 个物理行（1 主行 + 4 折叠行）。
    // 点击折叠行本身：归属选中行，selection 不变。
    let _ = model.handle_agents_panel_mouse_down(MouseButton::Left, 0, 2 + 3);
    assert_eq!(
        model
            .agents_panel
            .as_ref()
            .unwrap()
            .selected_row()
            .map(|row| row.agent_id),
        Some(AgentId::new(2)),
        "fold lines belong to the selected row itself"
    );

    // 点击物理偏移 6（折叠区之后的下一行）应选中第二行 agent，而非跳过折叠行数。
    let _ = model.handle_agents_panel_mouse_down(MouseButton::Left, 0, 2 + 6);
    assert_eq!(
        model
            .agents_panel
            .as_ref()
            .unwrap()
            .selected_row()
            .map(|row| row.agent_id),
        Some(AgentId::new(3)),
        "clicks below the fold must select the next row, not skip it"
    );
}

#[test]
fn collapsed_fold_keeps_rows_single_line_for_mouse_mapping() {
    // 未展开时折叠区不占物理行：点击紧跟选中行的物理行直接命中下一数据行。
    let mut model = ready_panel_model();
    let crate::AppEffect::ObserveAgentTranscript { request_id, .. } =
        press_key(&mut model, KeyCode::Enter).expect("Enter should dispatch the observation")
    else {
        panic!("unexpected effect");
    };
    apply_view_snapshot_loaded(
        &mut model,
        request_id,
        view_snapshot_with_items(2, 21, None, six_tool_items()),
    );
    press_key(&mut model, KeyCode::Esc);

    // body 首行是列头（不可选）：物理偏移 0 是选中行自身，偏移 1 是下一数据行。
    let _ = model.handle_agents_panel_mouse_down(MouseButton::Left, 0, 2 + 2);
    assert_eq!(
        model
            .agents_panel
            .as_ref()
            .unwrap()
            .selected_row()
            .map(|row| row.agent_id),
        Some(AgentId::new(3)),
        "collapsed fold must not consume physical rows"
    );
}

#[test]
fn mouse_click_on_the_column_header_line_selects_nothing() {
    let mut model = ready_model_with_selected_fold();

    // body 首行（终端第 2 行）是列头行：点击不得改变 selection。
    let _ = model.handle_agents_panel_mouse_down(MouseButton::Left, 0, 2);
    assert_eq!(
        model
            .agents_panel
            .as_ref()
            .unwrap()
            .selected_row()
            .map(|row| row.agent_id),
        Some(AgentId::new(2)),
        "column header clicks must not select a row"
    );
}
