use ratatui::style::Modifier;
use runtime_domain::agent::{AgentActivitySummary, AgentProjectionStatus};

use crate::agents_panel::{
    AGENTS_STATUS_COLUMN_WIDTH, agent_status_dot_style, agent_status_dot_symbol,
    agent_status_label, list_render::AGENTS_CURSOR_PREFIX, list_render::AGENTS_ROW_RIGHT_PADDING,
    list_render::agents_panel_row_layout, list_render::agents_panel_row_line,
};
use crate::display_width::{display_width, line_display_width};
use crate::theme::{default_palette, terminal_default_palette};

use super::common::overview_row;

/// 8 个投影状态全集，供表驱动断言遍历。
const ALL_STATUSES: [AgentProjectionStatus; 8] = [
    AgentProjectionStatus::Pending,
    AgentProjectionStatus::Working,
    AgentProjectionStatus::WaitingPermission,
    AgentProjectionStatus::Completed,
    AgentProjectionStatus::Failed,
    AgentProjectionStatus::Cancelled,
    AgentProjectionStatus::Stopping,
    AgentProjectionStatus::CleanupBlocked,
];

#[test]
fn cursor_row_marks_selection_with_prefix_and_bold_title_only() {
    let row = overview_row(2, "research task", AgentProjectionStatus::Working);
    let palette = default_palette();

    let cursor_line = agents_panel_row_line(&row, 80, true, false, palette);
    let plain_line = agents_panel_row_line(&row, 80, false, false, palette);

    // 选中指示前缀与未选中空白前缀等宽，列几何不随选中变化。
    assert_eq!(display_width(AGENTS_CURSOR_PREFIX), 2);
    assert_eq!(
        cursor_line.spans.first().map(|span| span.content.as_ref()),
        Some(AGENTS_CURSOR_PREFIX)
    );
    assert_eq!(
        plain_line.spans.first().map(|span| span.content.as_ref()),
        Some("  ")
    );
    assert_eq!(
        line_display_width(&cursor_line),
        line_display_width(&plain_line)
    );

    // 选中不再整行反色：行样式与所有 span 都不携带 REVERSED。
    for line in [&cursor_line, &plain_line] {
        assert!(!line.style.add_modifier.contains(Modifier::REVERSED));
        for span in &line.spans {
            assert!(!span.style.add_modifier.contains(Modifier::REVERSED));
        }
    }

    let cursor_title = title_span(&cursor_line);
    assert!(cursor_title.style.add_modifier.contains(Modifier::BOLD));

    let plain_title = title_span(&plain_line);
    assert!(!plain_title.style.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn status_column_renders_dot_symbol_with_label_text() {
    let row = overview_row(2, "research task", AgentProjectionStatus::Failed);
    let line = agents_panel_row_line(&row, 80, false, false, default_palette());

    let row_text: String = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();
    assert!(
        row_text.contains("● Failed"),
        "status column should render dot + label: {row_text}"
    );
}

#[test]
fn agent_status_dot_style_maps_every_status_to_a_semantic_slot() {
    let palette = default_palette();
    for (status, expected) in [
        (AgentProjectionStatus::Pending, palette.tertiary),
        (AgentProjectionStatus::Working, palette.command_accent),
        (AgentProjectionStatus::WaitingPermission, palette.accent),
        (AgentProjectionStatus::Completed, palette.success),
        (AgentProjectionStatus::Failed, palette.system_error),
        (AgentProjectionStatus::Cancelled, palette.approval_rejected),
        (AgentProjectionStatus::Stopping, palette.tertiary),
        (AgentProjectionStatus::CleanupBlocked, palette.tertiary),
    ] {
        assert_eq!(
            agent_status_dot_style(status, &palette).fg,
            Some(expected),
            "status {status:?} should map to its semantic palette slot"
        );
    }
}

#[test]
fn terminal_default_dot_styles_degrade_to_distinct_named_colors() {
    let palette = terminal_default_palette();

    // 带色状态退化为互不相同的 ANSI 命名色，颜色区分度保留。
    let mut colored_fgs = std::collections::HashSet::new();
    for status in [
        AgentProjectionStatus::Working,
        AgentProjectionStatus::WaitingPermission,
        AgentProjectionStatus::Completed,
        AgentProjectionStatus::Failed,
        AgentProjectionStatus::Cancelled,
    ] {
        colored_fgs.insert(agent_status_dot_style(status, &palette).fg);
    }
    assert_eq!(colored_fgs.len(), 5);

    // 弱化槽位退化为终端默认前景（无 fg），由符号与文字保底区分。
    for status in [
        AgentProjectionStatus::Pending,
        AgentProjectionStatus::Stopping,
        AgentProjectionStatus::CleanupBlocked,
    ] {
        assert_eq!(
            agent_status_dot_style(status, &palette).fg,
            None,
            "status {status:?} should degrade to the terminal default foreground"
        );
    }
}

#[test]
fn status_dot_symbol_degrades_to_hollow_only_for_terminal_states() {
    let palette = terminal_default_palette();

    for status in [
        AgentProjectionStatus::Pending,
        AgentProjectionStatus::Working,
        AgentProjectionStatus::WaitingPermission,
        AgentProjectionStatus::Stopping,
    ] {
        assert_eq!(agent_status_dot_symbol(status, &palette), "●");
    }
    for status in [
        AgentProjectionStatus::Completed,
        AgentProjectionStatus::Failed,
        AgentProjectionStatus::Cancelled,
        AgentProjectionStatus::CleanupBlocked,
    ] {
        assert_eq!(agent_status_dot_symbol(status, &palette), "○");
    }
}

#[test]
fn status_dot_symbol_stays_solid_with_explicit_colors() {
    let palette = default_palette();

    for status in ALL_STATUSES {
        assert_eq!(agent_status_dot_symbol(status, &palette), "●");
    }
}

#[test]
fn status_labels_fit_the_status_column() {
    for status in ALL_STATUSES {
        let label = agent_status_label(status);
        assert!(!label.is_empty());
        assert!(
            display_width(label) <= AGENTS_STATUS_COLUMN_WIDTH,
            "label {label:?} must fit the fixed status column"
        );
    }
}

fn title_span<'a>(line: &'a ratatui::text::Line<'a>) -> &'a ratatui::text::Span<'a> {
    line.spans
        .iter()
        .find(|span| span.content.contains("research task"))
        .expect("row should render the title")
}

/// 长 title + 长 latest 的 fixture：截断宽度反映各列的实际分配。
fn long_content_row() -> runtime_domain::agent::AgentOverviewRow {
    let mut row = overview_row(2, &"r".repeat(60), AgentProjectionStatus::Working);
    row.latest_activity = AgentActivitySummary::UsingTool {
        title: "t".repeat(60),
    };
    row
}

#[test]
fn wide_terminal_gives_title_the_dominant_share() {
    let row = long_content_row();

    // usable 118；fixed 15 + metrics 24 + 间隔 2 → rest 78；
    // title 拿 40（max 上限），latest 只拿剩余 37。
    let layout = agents_panel_row_layout(&row, 120);
    let title_width = display_width(&layout.title);
    let latest_width = layout.latest.as_deref().map_or(0, display_width);
    assert_eq!(title_width, 40, "title should reach its max allocation");
    assert_eq!(
        latest_width, 37,
        "latest should take the leftover elastic width"
    );
    assert!(
        title_width > latest_width,
        "title must dominate the elastic budget"
    );
    assert_eq!(
        layout.metrics,
        vec![
            "1m23s".to_string(),
            "3 tools".to_string(),
            "2k tok".to_string()
        ],
        "wide terminal should keep every metric"
    );
}

#[test]
fn metrics_yield_tokens_then_tools_then_elapsed() {
    let row = overview_row(2, "research task", AgentProjectionStatus::Working);

    // fixed 15 + title 16 + latest 8 + 间隔 2：全 metrics 需 usable 65（width 67），
    // 丢 tokens 需 56（width 58），丢 tools 需 46（width 48）。
    for (width, tokens, tools, elapsed) in [
        (67, true, true, true),
        (66, false, true, true),
        (48, false, false, true),
        (47, false, false, false),
    ] {
        let layout = agents_panel_row_layout(&row, width);
        let metrics = layout.metrics.join(" · ");
        assert_eq!(
            metrics.contains("2k tok"),
            tokens,
            "width {width} tokens: {metrics}"
        );
        assert_eq!(
            metrics.contains("3 tools"),
            tools,
            "width {width} tools: {metrics}"
        );
        assert_eq!(
            metrics.contains("1m23s"),
            elapsed,
            "width {width} elapsed: {metrics}"
        );
    }

    // tokens 与 tools 之间的中界：只丢 tokens。
    let layout = agents_panel_row_layout(&row, 58);
    assert_eq!(
        layout.metrics,
        vec!["1m23s".to_string(), "3 tools".to_string()]
    );
}

#[test]
fn metrics_sequence_is_right_aligned_to_the_row_edge() {
    let row = overview_row(2, "research task", AgentProjectionStatus::Working);

    // 有 metrics 的行：整行内容正好终止在 width - AGENTS_ROW_RIGHT_PADDING。
    for width in [120, 80, 67] {
        let line = agents_panel_row_line(&row, width, false, false, default_palette());
        assert_eq!(
            line_display_width(&line),
            width - AGENTS_ROW_RIGHT_PADDING,
            "width {width} metrics must anchor at the row edge: {:?}",
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        );
    }
}

#[test]
fn extreme_narrow_keeps_prefix_status_and_title_only() {
    let row = overview_row(2, "research task", AgentProjectionStatus::Working);

    // rest = 26 - 2 - 15 = 9 ≤ title min 16：极窄回退，title 截断到 9。
    let layout = agents_panel_row_layout(&row, 26);
    assert!(layout.latest.is_none(), "latest should be hidden");
    assert!(layout.metrics.is_empty(), "metrics should be hidden");
    assert_eq!(display_width(&layout.title), 9);
    assert_eq!(display_width(&layout.status), 10);

    // 弹性预算只够 title 时，latest 低于可见下限同样整列隐藏。
    let layout = agents_panel_row_layout(&row, 36);
    assert!(
        layout.latest.is_none(),
        "latest below visible floor should hide"
    );
    assert!(layout.metrics.is_empty());
    assert_eq!(layout.title, "research task");
}

#[test]
fn status_column_width_is_shared_across_rows() {
    // 状态文字列固定 padding，所有行 title 列纵向对齐。
    let rows = [
        overview_row(2, "research task", AgentProjectionStatus::Working),
        overview_row(3, "write docs", AgentProjectionStatus::Completed),
    ];
    for row in rows {
        let layout = agents_panel_row_layout(&row, 80);
        assert_eq!(
            display_width(&layout.status),
            AGENTS_STATUS_COLUMN_WIDTH,
            "status column must stay fixed-width for vertical alignment"
        );
    }
}

#[test]
fn activity_fold_prefix_aligns_with_the_title_column_start() {
    let row = overview_row(2, "research task", AgentProjectionStatus::Working);
    let line = agents_panel_row_line(&row, 100, true, false, default_palette());

    // title span 前所有 span 的显示宽之和即 title 列起点。
    let title_column_start = line
        .spans
        .iter()
        .take_while(|span| !span.content.contains("research task"))
        .map(|span| display_width(span.content.as_ref()))
        .sum::<usize>();
    let prefix = crate::agents_panel::list_render::agents_activity_fold_prefix();
    assert_eq!(
        display_width(&prefix) - display_width("↳ "),
        title_column_start,
        "fold lines must indent to the title column start"
    );
}
