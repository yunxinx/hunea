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

/// 选中 agent 建立带 6 条活动的 view snapshot，并回到 overview list。
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

#[test]
fn selected_row_renders_activity_fold_lines() {
    let mut model = ready_model_with_selected_fold();

    let rows = rendered_rows(&render_model_buffer(&mut model, 100, 24));
    let fold_rows: Vec<&String> = rows.iter().filter(|row| row.contains("↳")).collect();
    assert_eq!(
        fold_rows.len(),
        4,
        "3 activity lines + 1 more line: {rows:?}"
    );
    assert!(fold_rows.iter().any(|row| row.contains("step 5")));
    assert!(fold_rows.iter().any(|row| row.contains("+3 more")));
}

#[test]
fn narrow_width_hides_the_whole_fold_region() {
    let mut model = ready_model_with_selected_fold();

    let rows = rendered_rows(&render_model_buffer(&mut model, 59, 24));
    assert!(
        !rows.iter().any(|row| row.contains("↳")),
        "fold region must hide below the width floor: {rows:?}"
    );
    // 主行不受折叠区隐藏影响，标题仍在。
    assert!(rows.iter().any(|row| row.contains("research task")));
}

#[test]
fn page_budget_reserves_lines_for_the_activity_fold() {
    // 高度 24 - chrome 4 = 20 行 body；为折叠区恒定预留 4 行 → 每页 16 行。
    assert_eq!(agents_panel_list_page_size(24), 16);
    assert_eq!(agents_panel_list_page_size(12), 4);
    // body 行数不足时保底 1 行。
    assert_eq!(agents_panel_list_page_size(9), 1);
}

#[test]
fn fold_lines_count_into_the_page_row_budget() {
    // 40 行 agents、高度 24：page size 16。选中行 + 4 折叠行必须完整渲染在
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

    let rendered = rendered_rows(&render_model_buffer(&mut model, 100, 24));
    let fold_rows: Vec<&String> = rendered.iter().filter(|row| row.contains("↳")).collect();
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

    // body 自终端第 2 行起；选中行占 5 个物理行（1 主行 + 4 折叠行）。
    // 点击折叠行本身：归属选中行，selection 不变。
    let _ = model.handle_agents_panel_mouse_down(MouseButton::Left, 0, 2 + 2);
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

    // 点击物理偏移 5（折叠区之后的下一行）应选中第二行 agent，而非跳过折叠行数。
    let _ = model.handle_agents_panel_mouse_down(MouseButton::Left, 0, 2 + 5);
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
