use crossterm::event::{KeyCode, MouseButton};
use runtime_domain::agent::{AgentId, AgentOverviewRow, AgentProjectionStatus};

use crate::agents_panel::groups::{
    AgentsRowGroupKind, agents_panel_row_groups, agents_row_group_kind,
};
use crate::agents_panel::{AgentsPanelPageBodyLine, agents_panel_list_page_size};
use crate::test_helpers::{render_model_buffer, rendered_rows};

use super::common::{overview_row, press_key, ready_panel_model_with_rows};

/// 分组判定测试的固定墙钟。
const NOW_MS: i64 = 1_000_000_000_000;
/// 渲染/输入链路走真实时钟：远未来 settled 确定性地落在 Just finished。
const FAR_FUTURE_SETTLED_MS: i64 = 9_000_000_000_000_000_000;

fn settled_row(
    agent_id: u64,
    title: &str,
    status: AgentProjectionStatus,
    settled_at_ms: Option<i64>,
) -> AgentOverviewRow {
    let mut row = overview_row(agent_id, title, status);
    row.settled_at_ms = settled_at_ms;
    row
}

fn group_kinds(groups: &[crate::agents_panel::groups::AgentsRowGroup]) -> Vec<AgentsRowGroupKind> {
    groups.iter().map(|group| group.kind).collect()
}

fn group_agent_ids(
    groups: &[crate::agents_panel::groups::AgentsRowGroup],
    rows: &[AgentOverviewRow],
) -> Vec<Vec<u64>> {
    groups
        .iter()
        .map(|group| {
            group
                .row_indices
                .iter()
                .map(|&index| rows[index].agent_id.get())
                .collect()
        })
        .collect()
}

fn row_refs(rows: &[AgentOverviewRow]) -> Vec<&AgentOverviewRow> {
    rows.iter().collect()
}

fn header_line(kind: AgentsRowGroupKind, row_count: usize) -> AgentsPanelPageBodyLine {
    AgentsPanelPageBodyLine::GroupHeader { kind, row_count }
}

fn row_line(position: usize) -> AgentsPanelPageBodyLine {
    AgentsPanelPageBodyLine::Row { position }
}

// ---- 分组判定（纯函数） ----

#[test]
fn group_kind_partitions_every_projection_status() {
    let running = [
        AgentProjectionStatus::Pending,
        AgentProjectionStatus::Working,
        AgentProjectionStatus::WaitingPermission,
        AgentProjectionStatus::Stopping,
        // CleanupBlocked 归 Running 组尾（清理未收敛，仍占用 runtime authority）。
        AgentProjectionStatus::CleanupBlocked,
    ];
    for status in running {
        let row = overview_row(2, "task", status);
        assert_eq!(
            agents_row_group_kind(&row, NOW_MS),
            AgentsRowGroupKind::Running,
            "status {status:?} must stay in the Running group"
        );
    }

    for status in [
        AgentProjectionStatus::Completed,
        AgentProjectionStatus::Failed,
        AgentProjectionStatus::Cancelled,
    ] {
        let recent = settled_row(2, "task", status, Some(NOW_MS - 1_000));
        assert_eq!(
            agents_row_group_kind(&recent, NOW_MS),
            AgentsRowGroupKind::JustFinished,
            "recently settled {status:?} must be Just finished"
        );
        let old = settled_row(2, "task", status, Some(NOW_MS - 60_000));
        assert_eq!(
            agents_row_group_kind(&old, NOW_MS),
            AgentsRowGroupKind::Completed,
            "long-settled {status:?} must be Completed"
        );
        // 无计时起点（resume 恢复的投影）没有过渡组可判，直接归 Completed。
        let resumed = settled_row(2, "task", status, None);
        assert_eq!(
            agents_row_group_kind(&resumed, NOW_MS),
            AgentsRowGroupKind::Completed,
            "settled without a timestamp must be Completed"
        );
    }
}

#[test]
fn just_finished_window_boundary_is_exactly_ten_seconds() {
    let status = AgentProjectionStatus::Completed;
    let inside = settled_row(2, "task", status, Some(NOW_MS - 9_999));
    assert_eq!(
        agents_row_group_kind(&inside, NOW_MS),
        AgentsRowGroupKind::JustFinished
    );
    let outside = settled_row(2, "task", status, Some(NOW_MS - 10_000));
    assert_eq!(
        agents_row_group_kind(&outside, NOW_MS),
        AgentsRowGroupKind::Completed,
        "the boundary itself belongs to Completed"
    );
}

// ---- 组序与组内排序（纯函数） ----

#[test]
fn row_groups_order_kinds_and_sort_within_each_group() {
    let rows = vec![
        // Running：agent_id 升序，CleanupBlocked 殿后。
        overview_row(5, "working", AgentProjectionStatus::Working),
        overview_row(2, "cleanup blocked", AgentProjectionStatus::CleanupBlocked),
        overview_row(7, "pending", AgentProjectionStatus::Pending),
        // Just finished：settled 降序（新在前）。
        settled_row(
            3,
            "newer",
            AgentProjectionStatus::Failed,
            Some(NOW_MS - 1_000),
        ),
        settled_row(
            4,
            "older",
            AgentProjectionStatus::Completed,
            Some(NOW_MS - 5_000),
        ),
        // Completed：settled 升序（旧在前），无时刻的恢复行视为最旧。
        settled_row(
            9,
            "less old",
            AgentProjectionStatus::Cancelled,
            Some(NOW_MS - 20_000),
        ),
        settled_row(8, "resumed", AgentProjectionStatus::Completed, None),
        settled_row(
            6,
            "oldest",
            AgentProjectionStatus::Completed,
            Some(NOW_MS - 30_000),
        ),
    ];

    let groups = agents_panel_row_groups(&row_refs(&rows), NOW_MS);

    assert_eq!(
        group_kinds(&groups),
        vec![
            AgentsRowGroupKind::Running,
            AgentsRowGroupKind::JustFinished,
            AgentsRowGroupKind::Completed,
        ]
    );
    assert_eq!(
        group_agent_ids(&groups, &rows),
        vec![vec![5, 7, 2], vec![3, 4], vec![8, 6, 9],]
    );
}

#[test]
fn empty_groups_are_omitted() {
    let rows = vec![
        overview_row(2, "working", AgentProjectionStatus::Working),
        overview_row(3, "pending", AgentProjectionStatus::Pending),
    ];

    let groups = agents_panel_row_groups(&row_refs(&rows), NOW_MS);

    assert_eq!(group_kinds(&groups), vec![AgentsRowGroupKind::Running]);
    assert_eq!(
        group_agent_ids(&groups, &rows),
        vec![vec![2, 3]],
        "empty groups must not produce headers"
    );

    let groups = agents_panel_row_groups(&[], NOW_MS);
    assert!(groups.is_empty());
}

// ---- 面板层：页行计划、时间迁移 ----

#[test]
fn page_body_line_plan_interleaves_headers_and_rows() {
    let rows = vec![
        settled_row(
            9,
            "less old",
            AgentProjectionStatus::Cancelled,
            Some(NOW_MS - 20_000),
        ),
        settled_row(
            4,
            "older",
            AgentProjectionStatus::Completed,
            Some(NOW_MS - 5_000),
        ),
        overview_row(7, "pending", AgentProjectionStatus::Pending),
        settled_row(8, "resumed", AgentProjectionStatus::Completed, None),
        overview_row(5, "working", AgentProjectionStatus::Working),
        settled_row(
            3,
            "newer",
            AgentProjectionStatus::Failed,
            Some(NOW_MS - 1_000),
        ),
    ];
    let mut model = ready_panel_model_with_rows(rows);
    let page_size = agents_panel_list_page_size(24);

    let panel = model.agents_panel.as_mut().expect("panel should be ready");
    panel.refresh_display_order(NOW_MS);
    let plan = model
        .agents_panel
        .as_ref()
        .expect("panel should be ready")
        .page_body_line_plan(page_size, NOW_MS);

    assert_eq!(
        plan,
        vec![
            header_line(AgentsRowGroupKind::Running, 2),
            row_line(0),
            row_line(1),
            header_line(AgentsRowGroupKind::JustFinished, 2),
            row_line(2),
            row_line(3),
            header_line(AgentsRowGroupKind::Completed, 2),
            row_line(4),
            row_line(5),
        ],
        "display order: Running [5, 7] -> Just finished [3, 4] (newest first) \
         -> Completed [8, 9] (no-timestamp first)"
    );
}

#[test]
fn display_order_migrates_with_now_across_the_window_boundary() {
    let settled_at = NOW_MS;
    let rows = vec![
        settled_row(
            6,
            "just done",
            AgentProjectionStatus::Completed,
            Some(settled_at),
        ),
        settled_row(
            9,
            "old one",
            AgentProjectionStatus::Completed,
            Some(settled_at - 20_000),
        ),
    ];
    let mut model = ready_panel_model_with_rows(rows);
    let page_size = agents_panel_list_page_size(24);

    let panel = model.agents_panel.as_mut().expect("panel should be ready");
    panel.refresh_display_order(settled_at + 5_000);
    let fresh = model
        .agents_panel
        .as_ref()
        .expect("panel should be ready")
        .page_body_line_plan(page_size, settled_at + 5_000);
    assert_eq!(
        fresh,
        vec![
            header_line(AgentsRowGroupKind::JustFinished, 1),
            row_line(0),
            header_line(AgentsRowGroupKind::Completed, 1),
            row_line(1),
        ],
        "within the window: Just finished leads, the older row is Completed"
    );

    let panel = model.agents_panel.as_mut().expect("panel should be ready");
    panel.refresh_display_order(settled_at + 15_000);
    let panel = model.agents_panel.as_ref().expect("panel should be ready");
    // 迁移后两行同归 Completed：旧 settled（agent 9）重排到最前，
    // 显示位置随行序归一重新编号。
    assert_eq!(
        panel.filtered_row_at(0).map(|row| row.agent_id.get()),
        Some(9)
    );
    assert_eq!(
        panel.filtered_row_at(1).map(|row| row.agent_id.get()),
        Some(6)
    );
    assert_eq!(
        panel.page_body_line_plan(page_size, settled_at + 15_000),
        vec![
            header_line(AgentsRowGroupKind::Completed, 2),
            row_line(0),
            row_line(1),
        ],
        "after migration both rows are Completed, oldest first"
    );
}

#[test]
fn continuation_page_does_not_repeat_the_group_header() {
    let rows: Vec<AgentOverviewRow> = (0..14)
        .map(|index| {
            overview_row(
                100 + index,
                &format!("task {index}"),
                AgentProjectionStatus::Working,
            )
        })
        .collect();
    let mut model = ready_panel_model_with_rows(rows);
    let page_size = agents_panel_list_page_size(24);
    // 14 行 / 每页 12 行：`l` 进入第二页（选中 position 12）。
    press_key(&mut model, KeyCode::Char('l'));

    let panel = model.agents_panel.as_ref().expect("panel should be ready");
    assert_eq!(panel.page_start(page_size), 12);
    assert_eq!(
        panel.page_body_line_plan(page_size, NOW_MS),
        vec![row_line(12), row_line(13)],
        "a group continuing from the previous page must not repeat its header"
    );

    // 回到第一页：组头随组首行出现，组头计数是全组行数。
    press_key(&mut model, KeyCode::Char('h'));
    let panel = model.agents_panel.as_ref().expect("panel should be ready");
    let mut expected = vec![header_line(AgentsRowGroupKind::Running, 14)];
    expected.extend((0..12).map(row_line));
    assert_eq!(panel.page_body_line_plan(page_size, NOW_MS), expected);
}

// ---- 渲染 ----

/// 三组齐全、组内顺序敏感的渲染 fixture（真实时钟下确定性分组：
/// 远未来 settled → Just finished；Some(1) / None → Completed）。
/// 首行是 running 行：初始 selection 落在显示顺序的首行。
fn grouped_render_rows() -> Vec<AgentOverviewRow> {
    vec![
        overview_row(2, "working two", AgentProjectionStatus::Working),
        overview_row(5, "pending five", AgentProjectionStatus::Pending),
        settled_row(
            6,
            "just done",
            AgentProjectionStatus::Completed,
            Some(FAR_FUTURE_SETTLED_MS),
        ),
        settled_row(8, "resumed none", AgentProjectionStatus::Cancelled, None),
        settled_row(9, "old one", AgentProjectionStatus::Completed, Some(1)),
        settled_row(7, "old two", AgentProjectionStatus::Failed, Some(2)),
    ]
}

#[test]
fn group_headers_render_between_the_column_header_and_rows() {
    let mut model = ready_panel_model_with_rows(grouped_render_rows());

    let rows = rendered_rows(&render_model_buffer(&mut model, 100, 24));
    let position = |needle: &str| {
        rows.iter()
            .position(|row| row.contains(needle))
            .unwrap_or_else(|| panic!("row {needle} should render: {rows:?}"))
    };

    // 列头 → 组头 → 组内行 → 下一组；组内顺序与分组排序一致。
    let ordered = [
        "Tokens",
        "Running (2)",
        "working two",
        "pending five",
        "Just finished (1)",
        "just done",
        "Completed (3)",
        "resumed none",
        "old one",
        "old two",
    ];
    let positions: Vec<usize> = ordered.iter().map(|needle| position(needle)).collect();
    assert!(
        positions.windows(2).all(|pair| pair[0] < pair[1]),
        "grouped render order: {rows:?}"
    );

    // 计数语义：`N of M` 只数可选数据行（列头与组头不计入）。
    assert!(
        rows.iter().any(|row| row.contains("Agents (1 of 6)")),
        "position label must count selectable rows only: {rows:?}"
    );
}

#[test]
fn group_header_uses_the_table_header_palette_slot() {
    let mut model = ready_panel_model_with_rows(grouped_render_rows());

    let buffer = render_model_buffer(&mut model, 100, 24);
    let palette = crate::theme::default_palette();
    let header_y = (0..buffer.area.height)
        .find(|&y| buffer_row_text(&buffer, y).contains("Running (2)"))
        .expect("the group header should render");
    // 组头缩进对齐列头（status 列起点 2），文字走 table_header 槽位。
    assert_eq!(buffer[(2, header_y)].fg, palette.table_header);
}

fn buffer_row_text(buffer: &ratatui::buffer::Buffer, y: u16) -> String {
    (0..buffer.area.width)
        .map(|x| buffer[(x, y)].symbol().to_string())
        .collect()
}

#[test]
fn search_filter_regroups_and_hides_empty_groups() {
    let mut model = ready_panel_model_with_rows(vec![
        overview_row(2, "research task", AgentProjectionStatus::Working),
        settled_row(
            6,
            "write docs",
            AgentProjectionStatus::Completed,
            Some(FAR_FUTURE_SETTLED_MS),
        ),
    ]);

    press_key(&mut model, KeyCode::Char('/'));
    for character in ['d', 'o', 'c'] {
        press_key(&mut model, KeyCode::Char(character));
    }

    let rows = rendered_rows(&render_model_buffer(&mut model, 100, 24));
    let rendered = rows.join("\n");
    assert!(
        rendered.contains("Just finished (1)"),
        "the filtered row's group must re-group with its own count: {rendered}"
    );
    assert!(
        !rendered.contains("Running ("),
        "empty groups must hide after filtering: {rendered}"
    );
    // 过滤后仍只有一条可选数据行。
    assert!(rendered.contains("Agents (1 of 1)"));
}

// ---- 交互：组头不可选、跨组导航 ----

#[test]
fn followup_upsert_regroups_the_selected_row_and_keeps_selection_anchored() {
    // 选中行落在 Completed 组；followup turn 让该行回 Active（settled_at 随新
    // terminal 周期清除）：upsert 原位替换后，顺序归一把行移回 Running 组，
    // selection 以 stable id 重锚在原行上。
    let rows = vec![
        overview_row(5, "working sibling", AgentProjectionStatus::Working),
        settled_row(
            6,
            "selected done",
            AgentProjectionStatus::Completed,
            Some(1),
        ),
    ];
    let mut model = ready_panel_model_with_rows(rows);
    // 初始 selection 在显示顺序首行（agent 5）；j 选中 Completed 组的 agent 6。
    press_key(&mut model, KeyCode::Char('j'));
    assert_eq!(
        model
            .agents_panel
            .as_ref()
            .unwrap()
            .selected_row()
            .map(|row| row.agent_id.get()),
        Some(6)
    );

    // followup turn：同 id 行回到 Working（fixture 的 settled_at 为 None）。
    super::common::apply_overview_delta(
        &mut model,
        runtime_domain::agent::AgentOverviewDeltaKind::Upsert(overview_row(
            6,
            "selected done",
            AgentProjectionStatus::Working,
        )),
    );

    let page_size = agents_panel_list_page_size(24);
    let now_ms = crate::agents_panel::groups::agents_panel_now_unix_ms();
    let panel = model.agents_panel.as_mut().expect("panel should be ready");
    panel.refresh_display_order(now_ms);
    let panel = model.agents_panel.as_ref().expect("panel should be ready");
    assert_eq!(
        panel.selected_row().map(|row| row.agent_id.get()),
        Some(6),
        "selection must stay anchored on the followed-up row"
    );
    assert_eq!(
        panel.page_body_line_plan(page_size, now_ms),
        vec![
            header_line(AgentsRowGroupKind::Running, 2),
            row_line(0),
            row_line(1),
        ],
        "the followed-up row must regroup into Running (agent_id ascending)"
    );
}

#[test]
fn mouse_click_on_a_group_header_selects_nothing() {
    let mut model = ready_panel_model_with_rows(grouped_render_rows());

    // body 自终端第 2 行起：列头（第 2 行）→ "Running (2)"（第 3 行）→ 选中行
    // （第 4 行）→ "Just finished (1)"（第 6 行）→ "Completed (3)"（第 8 行）。
    // 点击三个组头行都不得改变 selection。
    for header_row in [3_u16, 6, 8] {
        let _ = model.handle_agents_panel_mouse_down(MouseButton::Left, 0, header_row);
        assert_eq!(
            model
                .agents_panel
                .as_ref()
                .unwrap()
                .selected_row()
                .map(|row| row.agent_id),
            Some(AgentId::new(2)),
            "clicking a group header must not select: row {header_row}"
        );
    }

    // 组头之后的首个数据行照常可选（换算计入了组头占用的物理行）。
    let _ = model.handle_agents_panel_mouse_down(MouseButton::Left, 0, 7);
    assert_eq!(
        model
            .agents_panel
            .as_ref()
            .unwrap()
            .selected_row()
            .map(|row| row.agent_id),
        Some(AgentId::new(6)),
        "the row right below a group header must stay clickable"
    );
}

#[test]
fn selection_moves_across_groups_without_selecting_headers() {
    let mut model = ready_panel_model_with_rows(grouped_render_rows());

    // 显示顺序：working two(2) → pending five(5) → just done(6) → resumed(8) →
    // old one(9) → old two(7)；组头行不是可选目标，j/k 直接跨组移动。
    for expected in [2_u64, 5, 6, 8, 9, 7] {
        assert_eq!(
            model
                .agents_panel
                .as_ref()
                .unwrap()
                .selected_row()
                .map(|row| row.agent_id.get()),
            Some(expected)
        );
        press_key(&mut model, KeyCode::Char('j'));
    }

    // 越过最后一行后 saturate，不得滚入任何组头或列头。
    press_key(&mut model, KeyCode::Char('j'));
    assert_eq!(
        model
            .agents_panel
            .as_ref()
            .unwrap()
            .selected_row()
            .map(|row| row.agent_id.get()),
        Some(7)
    );
}
