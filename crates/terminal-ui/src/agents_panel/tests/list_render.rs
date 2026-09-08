use ratatui::style::Modifier;
use runtime_domain::agent::{AgentActivitySummary, AgentProjectionStatus};

use crate::agents_panel::{
    AGENTS_ELAPSED_COLUMN_WIDTH, AGENTS_STATUS_COLUMN_WIDTH, AGENTS_TOKENS_COLUMN_WIDTH,
    AGENTS_TOOLS_COLUMN_WIDTH, agent_status_dot_style, agent_status_dot_symbol, agent_status_label,
    format_agent_token_usage, format_agent_tool_uses, list_render::AGENTS_ROW_RIGHT_PADDING,
    list_render::AGENTS_STOP_CONFIRM_HINT, list_render::agents_panel_column_header_line,
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
fn cursor_row_marks_selection_with_block_marker_and_bold_title_only() {
    let row = overview_row(2, "research task", AgentProjectionStatus::Working);
    let palette = default_palette();

    let cursor_line = agents_panel_row_line(&row, 80, true, None, palette);
    let plain_line = agents_panel_row_line(&row, 80, false, None, palette);

    // 选中 marker 与未选中空白 marker 等宽，列几何不随选中变化。
    let cursor_marker = cursor_line.spans.first().expect("marker span");
    assert_eq!(cursor_marker.content.as_ref(), "█ ");
    assert_eq!(cursor_marker.style.fg, Some(palette.command_accent));
    assert_eq!(
        plain_line.spans.first().map(|span| span.content.as_ref()),
        Some("  ")
    );
    assert_eq!(
        line_display_width(&cursor_line),
        line_display_width(&plain_line)
    );

    // 无斑马纹：行样式与所有 span 都不携带背景色。
    for line in [&cursor_line, &plain_line] {
        assert!(line.style.bg.is_none());
        for span in &line.spans {
            assert!(
                span.style.bg.is_none(),
                "row spans must not paint a background"
            );
        }
    }

    let cursor_title = title_span(&cursor_line);
    assert!(cursor_title.style.add_modifier.contains(Modifier::BOLD));

    let plain_title = title_span(&plain_line);
    assert!(!plain_title.style.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn status_dot_and_label_align_with_the_status_column_header() {
    let palette = default_palette();
    let row = overview_row(2, "research task", AgentProjectionStatus::Working);
    let line = agents_panel_row_line(&row, 80, false, None, palette);
    let header = agents_panel_column_header_line(80, palette);

    // 状态点归入 status 列：紧随 marker 前缀（marker 2 列）之后。
    assert_eq!(display_width_before(&line, "●"), 2);
    // 状态文字与列头 Status 共用同一列起点（dot 位计入 status 列推导）。
    assert_eq!(display_width_before(&line, "Working"), 4);
    assert_eq!(display_width_before(&header, "Status"), 4);
}

#[test]
fn status_column_renders_dot_symbol_with_label_text() {
    let row = overview_row(2, "research task", AgentProjectionStatus::Failed);
    let line = agents_panel_row_line(&row, 80, false, None, default_palette());

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

    // usable 118；fixed 15 + metrics_gap 1 + metrics 23 → rest 79；
    // title 拿 40（max 上限），latest 只拿剩余 38。
    let layout = agents_panel_row_layout(&row, 120, None);
    let title_width = display_width(&layout.title);
    let latest_width = layout.latest.as_deref().map_or(0, display_width);
    assert_eq!(title_width, 40, "title should reach its max allocation");
    assert_eq!(
        latest_width, 38,
        "latest should take the leftover elastic width"
    );
    assert!(
        title_width > latest_width,
        "title must dominate the elastic budget"
    );
    assert_eq!(
        layout.metrics,
        vec![
            "   1m23s".to_string(),
            "     3".to_string(),
            "   2.0K".to_string()
        ],
        "wide terminal should keep every metric at its fixed column width"
    );
}

#[test]
fn metrics_yield_tokens_then_tools_then_elapsed() {
    let row = overview_row(2, "research task", AgentProjectionStatus::Working);

    // fixed 15 + title 16 + latest 8 + 间隔 2 + metrics_gap 1：全 metrics 序列 23
    // 需 usable 64（width 66），丢 tokens（15）需 56（width 58），丢 tools（8）
    // 需 49（width 51）。
    for (width, expected) in [
        (66, vec!["   1m23s", "     3", "   2.0K"]),
        (58, vec!["   1m23s", "     3"]),
        (51, vec!["   1m23s"]),
    ] {
        let layout = agents_panel_row_layout(&row, width, None);
        assert_eq!(
            layout.metrics,
            expected.into_iter().map(String::from).collect::<Vec<_>>(),
            "width {width} metrics"
        );
    }
    let layout = agents_panel_row_layout(&row, 50, None);
    assert!(
        layout.metrics.is_empty(),
        "width 50 should drop every metric column"
    );
}

#[test]
fn metrics_sequence_is_right_aligned_to_the_row_edge() {
    let row = overview_row(2, "research task", AgentProjectionStatus::Working);

    // 有 metrics 的行：整行内容正好终止在 width - AGENTS_ROW_RIGHT_PADDING。
    for width in [120, 80, 74] {
        let line = agents_panel_row_line(&row, width, false, None, default_palette());
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
fn metric_columns_align_vertically_across_rows() {
    let palette = default_palette();
    let mut small = overview_row(2, "research task", AgentProjectionStatus::Working);
    small.elapsed_ms = Some(5_000);
    small.tool_uses = Some(1);
    small.token_usage = Some(8);
    let mut large = overview_row(3, "research task", AgentProjectionStatus::Working);
    large.elapsed_ms = Some(83_000);
    large.tool_uses = Some(99);
    large.token_usage = Some(2_048);

    let small_line = agents_panel_row_line(&small, 100, false, None, palette);
    let large_line = agents_panel_row_line(&large, 100, false, None, palette);

    // 同列固定宽度右对齐：tokens 列 span 的起点在两行中处于同一显示列。
    let small_prefix = display_width_before(&small_line, "8");
    let large_prefix = display_width_before(&large_line, "2.0K");
    assert_eq!(
        small_prefix, large_prefix,
        "tokens column must start at the same display column in both rows"
    );

    // 状态列宽共享 + title 列起点一致（title 相同）。
    let layouts = [
        agents_panel_row_layout(&small, 100, None),
        agents_panel_row_layout(&large, 100, None),
    ];
    for layout in layouts {
        assert_eq!(display_width(&layout.status), AGENTS_STATUS_COLUMN_WIDTH);
        assert_eq!(display_width(&layout.title), 13);
    }
}

/// 计算 line 中首个包含 `needle` 的 span 之前的累计显示宽（列起点）。
fn display_width_before(line: &ratatui::text::Line<'_>, needle: &str) -> usize {
    let mut width = 0;
    for span in &line.spans {
        if span.content.contains(needle) {
            return width;
        }
        width += display_width(span.content.as_ref());
    }
    panic!("row should contain {needle}");
}

#[test]
fn column_header_line_aligns_labels_to_row_geometry() {
    let palette = default_palette();

    // 宽屏：列名齐全，metrics 列名与行数据共用右端锚点。
    let header = agents_panel_column_header_line(100, palette);
    let header_text: String = header.spans.iter().map(|s| s.content.as_ref()).collect();
    for label in ["Status", "Title", "Latest", "Time", "Use Tools", "Tokens"] {
        assert!(
            header_text.contains(label),
            "header missing {label}: {header_text}"
        );
    }
    assert_eq!(
        line_display_width(&header),
        100 - AGENTS_ROW_RIGHT_PADDING,
        "header must anchor at the same right edge as rows: {header_text}"
    );
    assert!(
        header
            .spans
            .iter()
            .any(|span| span.style.fg == Some(palette.table_header)),
        "column labels must use the table header palette slot"
    );

    // 收窄时列名与行数据共用同一让位顺序：丢 tokens 后表头只剩 Time/Use Tools。
    let narrow = agents_panel_column_header_line(61, palette);
    let narrow_text: String = narrow.spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(
        !narrow_text.contains("Tokens"),
        "header tokens: {narrow_text}"
    );
    assert!(narrow_text.contains("Time") && narrow_text.contains("Use Tools"));
}

#[test]
fn token_usage_labels_drop_units_and_keep_bounded_width() {
    for (usage, expected) in [
        (0, "0"),
        (8, "8"),
        (999, "999"),
        (1_024, "1.0K"),
        (2_048, "2.0K"),
        (99_940, "99.9K"),
        (123_456, "123K"),
        (1_500_000, "1.5M"),
        (99_940_000, "99.9M"),
        (123_456_789, "123M"),
        (u64::MAX as usize, "999M"),
    ] {
        assert_eq!(format_agent_token_usage(usage), expected.to_string());
    }
}

#[test]
fn tool_use_labels_are_bare_numbers() {
    for (uses, expected) in [(0, "0"), (1, "1"), (3, "3"), (99_999, "99999")] {
        assert_eq!(format_agent_tool_uses(uses), expected.to_string());
    }
}

#[test]
fn token_usage_labels_fit_the_tokens_column() {
    for usage in [
        0,
        8,
        999,
        1_024,
        99_940,
        123_456,
        999_999,
        99_940_000,
        usize::MAX,
    ] {
        let label = format_agent_token_usage(usage);
        assert!(
            display_width(&label) <= AGENTS_TOKENS_COLUMN_WIDTH,
            "label {label:?} must fit the tokens column"
        );
    }
}

#[test]
fn metric_labels_pad_to_their_fixed_column_widths() {
    let row = overview_row(2, "research task", AgentProjectionStatus::Working);
    let layout = agents_panel_row_layout(&row, 100, None);
    let widths: Vec<usize> = layout.metrics.iter().map(|m| display_width(m)).collect();
    assert_eq!(
        widths,
        vec![
            AGENTS_ELAPSED_COLUMN_WIDTH,
            AGENTS_TOOLS_COLUMN_WIDTH,
            AGENTS_TOKENS_COLUMN_WIDTH
        ],
        "each metric label must pad to its fixed column width"
    );
}

#[test]
fn extreme_narrow_keeps_prefix_status_and_title_only() {
    let row = overview_row(2, "research task", AgentProjectionStatus::Working);

    // rest = 26 - 2 - 15 = 9 ≤ title min 16：极窄回退，title 截断到 9。
    let layout = agents_panel_row_layout(&row, 26, None);
    assert!(layout.latest.is_none(), "latest should be hidden");
    assert!(layout.metrics.is_empty(), "metrics should be hidden");
    assert_eq!(display_width(&layout.title), 9);
    assert_eq!(display_width(&layout.status), 10);

    // 弹性预算只够 title 时，latest 低于可见下限同样整列隐藏。
    let layout = agents_panel_row_layout(&row, 36, None);
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
        let layout = agents_panel_row_layout(&row, 80, None);
        assert_eq!(
            display_width(&layout.status),
            AGENTS_STATUS_COLUMN_WIDTH,
            "status column must stay fixed-width for vertical alignment"
        );
    }
}

#[test]
fn stop_confirm_hint_takes_the_latest_column_slot() {
    let row = overview_row(2, "research task", AgentProjectionStatus::Working);
    let palette = default_palette();

    let plain = agents_panel_row_line(&row, 100, true, None, palette);
    let hinted = agents_panel_row_line(&row, 100, true, Some(AGENTS_STOP_CONFIRM_HINT), palette);

    // 提示接管 latest 列槽位：列几何不变，行仍锚定同一右端。
    assert_eq!(line_display_width(&plain), line_display_width(&hinted));
    let hint_span = hinted
        .spans
        .iter()
        .find(|span| span.content.contains("press x again"))
        .expect("hinted row should render the stop hint");
    assert_eq!(hint_span.style.fg, Some(palette.command_accent));
    assert!(
        plain
            .spans
            .iter()
            .all(|span| !span.content.contains("press x again"))
    );
}

#[test]
fn stop_confirm_hint_replaces_idle_latest_activity() {
    let mut row = overview_row(2, "research task", AgentProjectionStatus::Working);
    row.latest_activity = AgentActivitySummary::Idle;

    // 提示优先于活动文本：Idle 行的 latest 列仍显示确认提示。
    let hinted = agents_panel_row_layout(&row, 100, Some(AGENTS_STOP_CONFIRM_HINT));
    assert_eq!(
        hinted.latest.as_deref(),
        Some(crate::agents_panel::list_render::AGENTS_STOP_CONFIRM_HINT)
    );
    let plain = agents_panel_row_layout(&row, 100, None);
    assert!(plain.latest.is_none());
}

#[test]
fn activity_fold_prefix_aligns_with_the_title_column_start() {
    let row = overview_row(2, "research task", AgentProjectionStatus::Working);
    let line = agents_panel_row_line(&row, 100, true, None, default_palette());

    // title span 前所有 span 的显示宽之和即 title 列起点。
    let title_column_start = line
        .spans
        .iter()
        .take_while(|span| !span.content.contains("research task"))
        .map(|span| display_width(span.content.as_ref()))
        .sum::<usize>();
    let prefix = crate::agents_panel::list_render::agents_activity_fold_prefix("│");
    assert_eq!(
        display_width(&prefix) - display_width("│ "),
        title_column_start,
        "fold lines must indent to the title column start"
    );
}
