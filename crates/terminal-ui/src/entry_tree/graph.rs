use std::collections::{HashMap, HashSet};

use ratatui::style::Style;
use runtime_domain::session::{SessionTreeRow, SessionTreeRowKind};

use crate::{
    display_width::display_width,
    status_line::truncate_display_width_with_ellipsis,
    theme::{TerminalPalette, accent_text_style, tertiary_text_style},
};

use super::{
    ENTRY_TREE_GRAPH_CELL_WIDTH, ENTRY_TREE_GRAPH_FLAT_WIDTH, ENTRY_TREE_GRAPH_LANE_WIDTH,
    ENTRY_TREE_GRAPH_MAX_WIDTH, ENTRY_TREE_GRAPH_MIN_WIDTH, ENTRY_TREE_KIND_PREFIX_WIDTH,
    ENTRY_TREE_MIN_SUMMARY_WIDTH,
};

#[derive(Debug, Clone, PartialEq, Eq)]
struct EntryTreeGraphParts {
    ancestor_segments: Vec<EntryTreeGraphSpan>,
    own_segment: EntryTreeGraphSpan,
    compact_node_segment: String,
    pub(super) is_selected_branch: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct EntryTreeGraphLine {
    pub(super) spans: Vec<EntryTreeGraphSpan>,
}

impl EntryTreeGraphLine {
    pub(super) fn display_width(&self) -> usize {
        self.spans
            .iter()
            .map(|span| display_width(&span.text))
            .sum()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct EntryTreeGraphSpan {
    pub(super) text: String,
    pub(super) is_selected_branch: bool,
}

/// `/tree` 左侧分支图的布局状态：拓扑、深度、选中投影与 lane 连续性。
#[derive(Debug, Clone, PartialEq, Eq)]
struct EntryTreeGraphLayout {
    graph_parent_by_row: Vec<Option<usize>>,
    graph_children_by_parent: HashMap<Option<usize>, Vec<usize>>,
    branch_parent_by_row: Vec<bool>,
    layout_depth_by_row: Vec<usize>,
    on_projection_by_row: Vec<bool>,
    show_lane_by_row: Vec<bool>,
    projection_first_graph_index: Option<usize>,
    projection_last_graph_index: Option<usize>,
}

pub(super) fn entry_tree_graph_lines(
    rows: &[SessionTreeRow],
    selected_index: usize,
    width: usize,
) -> Vec<EntryTreeGraphLine> {
    let graph_width_budget = entry_tree_graph_width_budget(width);
    if graph_width_budget == 0 {
        return vec![EntryTreeGraphLine::default(); rows.len()];
    }

    let layout = EntryTreeGraphLayout::new(rows, selected_index);
    rows.iter()
        .enumerate()
        .map(|(row_index, _)| entry_tree_graph_line(rows, &layout, row_index, graph_width_budget))
        .collect()
}

impl EntryTreeGraphLayout {
    fn new(rows: &[SessionTreeRow], selected_index: usize) -> Self {
        let row_index_by_id = entry_tree_row_index_by_id(rows);
        let graph_parent_by_row = entry_tree_graph_parent_by_row(rows, &row_index_by_id);
        let graph_children_by_parent =
            entry_tree_graph_children_by_parent(rows, &graph_parent_by_row);
        let branch_parent_by_row = entry_tree_branch_parent_by_row(rows, &graph_children_by_parent);
        let graph_depth_by_row = entry_tree_graph_lane_depth_by_row(
            rows,
            &graph_parent_by_row,
            &graph_children_by_parent,
            &branch_parent_by_row,
        );
        let selected_projection_path = entry_tree_selected_projection_path(
            rows,
            selected_index,
            &graph_parent_by_row,
            &graph_children_by_parent,
            &row_index_by_id,
        );
        let selected_projection_nodes = selected_projection_path
            .iter()
            .copied()
            .collect::<HashSet<_>>();
        let on_projection_by_row =
            entry_tree_on_projection_by_row(rows, &selected_projection_nodes, &row_index_by_id);
        let layout_depth_by_row = entry_tree_layout_depth_by_row(
            rows,
            &graph_depth_by_row,
            &graph_parent_by_row,
            &graph_children_by_parent,
            &on_projection_by_row,
            &row_index_by_id,
        );
        let show_lane_by_row =
            entry_tree_show_lane_by_row(rows, &on_projection_by_row, &row_index_by_id);
        let (projection_first_graph_index, projection_last_graph_index) =
            entry_tree_projection_graph_endpoints(rows, &selected_projection_path);

        Self {
            graph_parent_by_row,
            graph_children_by_parent,
            branch_parent_by_row,
            layout_depth_by_row,
            on_projection_by_row,
            show_lane_by_row,
            projection_first_graph_index,
            projection_last_graph_index,
        }
    }
}

fn entry_tree_graph_width_budget(width: usize) -> usize {
    let right_side_min_width = ENTRY_TREE_KIND_PREFIX_WIDTH + ENTRY_TREE_MIN_SUMMARY_WIDTH;
    let available = width.saturating_sub(right_side_min_width);
    if available >= ENTRY_TREE_GRAPH_MIN_WIDTH {
        return available.min(ENTRY_TREE_GRAPH_MAX_WIDTH);
    }

    width
        .saturating_sub(ENTRY_TREE_KIND_PREFIX_WIDTH)
        .min(ENTRY_TREE_GRAPH_MIN_WIDTH)
}

fn entry_tree_graph_line(
    rows: &[SessionTreeRow],
    layout: &EntryTreeGraphLayout,
    row_index: usize,
    graph_width_budget: usize,
) -> EntryTreeGraphLine {
    let graph_parts = entry_tree_graph_parts(rows, layout, row_index);
    let has_selected_graph_span = graph_parts.is_selected_branch
        || graph_parts.own_segment.is_selected_branch
        || graph_parts
            .ancestor_segments
            .iter()
            .any(|span| span.is_selected_branch);
    // path-only tree 不再用横向缩进表达层级；保留 lane 计算只为决定符号与高亮。
    let flat_prefix = entry_tree_flat_graph_prefix(rows, layout, row_index, &graph_parts);
    if display_width(&flat_prefix) <= graph_width_budget {
        return EntryTreeGraphLine {
            spans: vec![EntryTreeGraphSpan {
                text: flat_prefix,
                is_selected_branch: has_selected_graph_span,
            }],
        };
    }

    EntryTreeGraphLine {
        spans: vec![EntryTreeGraphSpan {
            text: collapsed_entry_tree_graph_prefix(&graph_parts, graph_width_budget),
            is_selected_branch: has_selected_graph_span,
        }],
    }
}

fn entry_tree_graph_parts(
    rows: &[SessionTreeRow],
    layout: &EntryTreeGraphLayout,
    row_index: usize,
) -> EntryTreeGraphParts {
    let depth = layout.layout_depth_by_row[row_index];
    let own_segment = entry_tree_graph_own_segment(rows, layout, row_index);
    let on_projection = layout.on_projection_by_row[row_index];
    let row_index_by_id = entry_tree_row_index_by_id(rows);

    // depth 为 D 时：D 个祖先列提供相对主干的缩进，最后一列祖先仅作垫层；
    // 分支内的竖线/节点只画在 own 列，避免同一行出现两条 │。
    let mut ancestor_segments = (0..depth)
        .map(|lane_index| {
            let fork_lane_continues = entry_tree_fork_lane_continues_at_row(
                layout,
                rows,
                row_index,
                lane_index,
                &row_index_by_id,
            );
            let outer_lane_active = lane_index + 1 < depth
                && entry_tree_lane_is_active(
                    lane_index,
                    row_index,
                    &layout.layout_depth_by_row,
                    &layout.show_lane_by_row,
                );
            if outer_lane_active || fork_lane_continues {
                let lane_highlighted = entry_tree_lane_is_active(
                    lane_index,
                    row_index,
                    &layout.layout_depth_by_row,
                    &layout.on_projection_by_row,
                ) || fork_lane_continues;
                EntryTreeGraphSpan {
                    text: entry_tree_graph_lane_column(),
                    is_selected_branch: lane_highlighted,
                }
            } else {
                EntryTreeGraphSpan {
                    text: entry_tree_graph_empty_lane_column(),
                    is_selected_branch: false,
                }
            }
        })
        .collect::<Vec<_>>();

    if depth > 0 && on_projection && entry_tree_row_is_branch_choice(layout, row_index) {
        let connector = entry_tree_branch_connector(layout, row_index);
        if let Some(parent_lane) = ancestor_segments.get_mut(depth - 1) {
            *parent_lane = EntryTreeGraphSpan {
                text: entry_tree_pad_graph_cell(connector, ENTRY_TREE_GRAPH_LANE_WIDTH),
                is_selected_branch: on_projection,
            };
        }
    }

    let has_active_ancestor_lane = ancestor_segments
        .iter()
        .any(|segment| segment.is_selected_branch);

    EntryTreeGraphParts {
        ancestor_segments,
        compact_node_segment: entry_tree_graph_compact_segment(rows, layout, row_index).to_string(),
        own_segment,
        is_selected_branch: on_projection || has_active_ancestor_lane,
    }
}

fn entry_tree_flat_graph_prefix(
    rows: &[SessionTreeRow],
    layout: &EntryTreeGraphLayout,
    row_index: usize,
    graph_parts: &EntryTreeGraphParts,
) -> String {
    let segment = entry_tree_graph_node(rows, layout, row_index)
        .map(str::to_string)
        .unwrap_or_else(|| graph_parts.compact_node_segment.trim_end().to_string());

    entry_tree_pad_graph_cell(&segment, ENTRY_TREE_GRAPH_FLAT_WIDTH)
}

fn entry_tree_graph_lane_column() -> String {
    "│  ".to_string()
}

fn entry_tree_graph_empty_lane_column() -> String {
    " ".repeat(ENTRY_TREE_GRAPH_LANE_WIDTH)
}

fn entry_tree_graph_own_cell_width(_depth: usize) -> usize {
    ENTRY_TREE_GRAPH_CELL_WIDTH
}

fn entry_tree_pad_graph_cell(content: &str, width: usize) -> String {
    let current_width = display_width(content);
    if current_width >= width {
        return content.to_string();
    }
    let mut cell = content.to_string();
    cell.push_str(&" ".repeat(width - current_width));
    cell
}

fn entry_tree_graph_empty_own_cell(depth: usize) -> String {
    " ".repeat(entry_tree_graph_own_cell_width(depth))
}

/// 判断外层 lane 列是否应画竖线（0 起始 lane 索引）。
fn entry_tree_lane_is_active(
    lane_index: usize,
    row_index: usize,
    layout_depth_by_row: &[usize],
    lane_path_by_row: &[bool],
) -> bool {
    let row_depth = layout_depth_by_row[row_index];
    if row_depth <= lane_index {
        return false;
    }

    let has_deeper_lane_below = (row_index..layout_depth_by_row.len())
        .any(|index| lane_path_by_row[index] && layout_depth_by_row[index] > lane_index);
    if !has_deeper_lane_below {
        return false;
    }

    (0..=row_index).any(|index| {
        if !lane_path_by_row[index] {
            return false;
        }
        let depth = layout_depth_by_row[index];
        depth > lane_index || (depth == lane_index && has_deeper_lane_below)
    })
}

fn entry_tree_graph_own_segment(
    rows: &[SessionTreeRow],
    layout: &EntryTreeGraphLayout,
    row_index: usize,
) -> EntryTreeGraphSpan {
    let depth = layout.layout_depth_by_row[row_index];
    let on_projection = layout.on_projection_by_row[row_index];
    let show_lane = layout.show_lane_by_row[row_index];
    let own_cell_width = entry_tree_graph_own_cell_width(depth);
    let Some(node) = entry_tree_graph_node(rows, layout, row_index) else {
        let text = if show_lane {
            entry_tree_pad_graph_cell("│", own_cell_width)
        } else {
            entry_tree_graph_empty_own_cell(depth)
        };
        return EntryTreeGraphSpan {
            text,
            is_selected_branch: on_projection,
        };
    };
    if depth == 0 {
        return EntryTreeGraphSpan {
            text: entry_tree_pad_graph_cell(node, own_cell_width),
            is_selected_branch: true,
        };
    }

    if !entry_tree_row_is_branch_choice(layout, row_index) {
        return EntryTreeGraphSpan {
            text: entry_tree_pad_graph_cell(node, own_cell_width),
            is_selected_branch: true,
        };
    }

    // 弯折连接器画在父列（@ 正下方）；own 列只放节点符号。
    EntryTreeGraphSpan {
        text: entry_tree_pad_graph_cell(node, own_cell_width),
        is_selected_branch: true,
    }
}

fn entry_tree_branch_connector(layout: &EntryTreeGraphLayout, row_index: usize) -> &'static str {
    // 仅当选中投影路径上还有后续兄弟分支时用 ├─；跨过的 inactive 兄弟不影响连接符。
    if entry_tree_row_has_later_projection_graph_sibling(layout, row_index) {
        "├─"
    } else {
        "╰─"
    }
}

fn entry_tree_fork_lane_continues_at_row(
    layout: &EntryTreeGraphLayout,
    rows: &[SessionTreeRow],
    row_index: usize,
    lane_index: usize,
    row_index_by_id: &HashMap<&str, usize>,
) -> bool {
    let depth = layout.layout_depth_by_row[row_index];
    // 弯折垫层只在父列（depth - 1）延续，避免外层 lane 误加缩进。
    if lane_index != depth.saturating_sub(1) {
        return false;
    }
    if lane_index >= depth {
        return false;
    }
    if layout.on_projection_by_row[row_index] && entry_tree_row_is_branch_choice(layout, row_index)
    {
        return false;
    }

    let Some(graph_owner_index) =
        entry_tree_row_graph_owner_index(rows, row_index, row_index_by_id)
    else {
        return false;
    };
    let Some(fork_parent_index) = entry_tree_graph_ancestor_at_lane_depth(
        graph_owner_index,
        lane_index,
        &layout.layout_depth_by_row,
        &layout.graph_parent_by_row,
    ) else {
        return false;
    };
    if !entry_tree_row_is_branch_parent_at(layout, fork_parent_index) {
        return false;
    }
    let Some(branch_entry_index) = entry_tree_projection_branch_child_of_fork(
        fork_parent_index,
        &layout.layout_depth_by_row,
        &layout.graph_children_by_parent,
        &layout.on_projection_by_row,
    ) else {
        return false;
    };

    row_index > fork_parent_index && row_index < branch_entry_index
}

fn entry_tree_graph_ancestor_at_lane_depth(
    graph_owner_index: usize,
    target_depth: usize,
    layout_depth_by_row: &[usize],
    graph_parent_by_row: &[Option<usize>],
) -> Option<usize> {
    let mut current_index = graph_owner_index;
    loop {
        if layout_depth_by_row[current_index] == target_depth {
            return Some(current_index);
        }
        if layout_depth_by_row[current_index] < target_depth {
            return None;
        }
        current_index = graph_parent_by_row[current_index]?;
    }
}

fn entry_tree_projection_branch_child_of_fork(
    fork_parent_index: usize,
    layout_depth_by_row: &[usize],
    graph_children_by_parent: &HashMap<Option<usize>, Vec<usize>>,
    on_projection_by_row: &[bool],
) -> Option<usize> {
    let fork_depth = layout_depth_by_row[fork_parent_index];
    let children = graph_children_by_parent.get(&Some(fork_parent_index))?;
    children.iter().copied().find(|&child_index| {
        on_projection_by_row[child_index]
            && layout_depth_by_row[child_index] == fork_depth.saturating_add(1)
    })
}

fn entry_tree_row_is_branch_parent_at(layout: &EntryTreeGraphLayout, row_index: usize) -> bool {
    layout
        .branch_parent_by_row
        .get(row_index)
        .copied()
        .unwrap_or(false)
}

fn entry_tree_graph_compact_segment(
    rows: &[SessionTreeRow],
    layout: &EntryTreeGraphLayout,
    row_index: usize,
) -> &'static str {
    let row_index_by_id = entry_tree_row_index_by_id(rows);
    match entry_tree_graph_node(rows, layout, row_index) {
        Some("@") => "@ ",
        Some("●") => "● ",
        Some("·") => "· ",
        _ if layout.show_lane_by_row[row_index] => "│ ",
        _ if (0..layout.layout_depth_by_row[row_index]).any(|lane_index| {
            entry_tree_lane_is_active(
                lane_index,
                row_index,
                &layout.layout_depth_by_row,
                &layout.show_lane_by_row,
            ) || entry_tree_fork_lane_continues_at_row(
                layout,
                rows,
                row_index,
                lane_index,
                &row_index_by_id,
            )
        }) =>
        {
            "│ "
        }
        _ => "  ",
    }
}

fn collapsed_entry_tree_graph_prefix(
    graph_parts: &EntryTreeGraphParts,
    graph_width_budget: usize,
) -> String {
    let ancestor_count = graph_parts.ancestor_segments.len();

    for kept_ancestor_count in (0..ancestor_count).rev() {
        let hidden_count = ancestor_count.saturating_sub(kept_ancestor_count);
        let mut prefix = format!("…{hidden_count} ");
        for segment in &graph_parts.ancestor_segments[hidden_count..] {
            prefix.push_str(&segment.text);
        }
        prefix.push_str(&graph_parts.own_segment.text);

        if display_width(&prefix) <= graph_width_budget {
            return prefix;
        }
    }

    if display_width(&graph_parts.compact_node_segment) <= graph_width_budget {
        return graph_parts.compact_node_segment.clone();
    }

    truncate_display_width_with_ellipsis(&graph_parts.compact_node_segment, graph_width_budget)
}

/// 分叉连接器（├─/╰─）只看选中投影路径上的后续兄弟，忽略未画线的 inactive 分支。
fn entry_tree_row_has_later_projection_graph_sibling(
    layout: &EntryTreeGraphLayout,
    row_index: usize,
) -> bool {
    let Some(parent_index) = layout.graph_parent_by_row[row_index] else {
        return false;
    };
    let Some(siblings) = layout.graph_children_by_parent.get(&Some(parent_index)) else {
        return false;
    };
    let Some(position) = siblings
        .iter()
        .position(|sibling_index| *sibling_index == row_index)
    else {
        return false;
    };

    siblings[position + 1..]
        .iter()
        .copied()
        .any(|sibling_index| layout.on_projection_by_row[sibling_index])
}

fn entry_tree_row_is_branch_choice(layout: &EntryTreeGraphLayout, row_index: usize) -> bool {
    layout.graph_parent_by_row[row_index]
        .is_some_and(|parent_index| entry_tree_row_is_branch_parent_at(layout, parent_index))
}

fn entry_tree_graph_node(
    rows: &[SessionTreeRow],
    layout: &EntryTreeGraphLayout,
    row_index: usize,
) -> Option<&'static str> {
    let row = &rows[row_index];
    if !entry_tree_row_has_graph_node(row) || !layout.on_projection_by_row[row_index] {
        return None;
    }
    if entry_tree_row_is_branch_parent(layout, row_index) {
        return Some("@");
    }
    if entry_tree_row_is_branch_choice(layout, row_index) {
        return Some("·");
    }
    if Some(row_index) == layout.projection_first_graph_index
        || Some(row_index) == layout.projection_last_graph_index
    {
        Some("●")
    } else {
        Some("·")
    }
}

fn entry_tree_row_is_branch_parent(layout: &EntryTreeGraphLayout, row_index: usize) -> bool {
    entry_tree_row_is_branch_parent_at(layout, row_index)
}

fn entry_tree_row_has_graph_node(row: &SessionTreeRow) -> bool {
    row.kind == SessionTreeRowKind::User
        || !row.branch_choices.is_empty()
        || matches!(
            row.kind,
            SessionTreeRowKind::Assistant | SessionTreeRowKind::Tool
                if row.rewind_target_id.is_some()
        )
}

fn entry_tree_row_index_by_id(rows: &[SessionTreeRow]) -> HashMap<&str, usize> {
    rows.iter()
        .enumerate()
        .map(|(row_index, row)| (row.row_id.as_str(), row_index))
        .collect()
}

fn entry_tree_graph_parent_by_row(
    rows: &[SessionTreeRow],
    row_index_by_id: &HashMap<&str, usize>,
) -> Vec<Option<usize>> {
    rows.iter()
        .enumerate()
        .map(|(row_index, row)| {
            if entry_tree_row_has_graph_node(row) {
                nearest_graph_ancestor_index(rows, row_index, row_index_by_id)
            } else {
                None
            }
        })
        .collect()
}

fn nearest_graph_ancestor_index(
    rows: &[SessionTreeRow],
    row_index: usize,
    row_index_by_id: &HashMap<&str, usize>,
) -> Option<usize> {
    let mut parent_id = rows[row_index].parent_id.as_deref();
    let mut visited = HashSet::new();

    while let Some(current_parent_id) = parent_id {
        let parent_index = *row_index_by_id.get(current_parent_id)?;
        if !visited.insert(parent_index) {
            return None;
        }
        let parent = &rows[parent_index];
        if entry_tree_row_has_graph_node(parent) {
            return Some(parent_index);
        }
        parent_id = parent.parent_id.as_deref();
    }

    None
}

fn entry_tree_graph_children_by_parent(
    rows: &[SessionTreeRow],
    graph_parent_by_row: &[Option<usize>],
) -> HashMap<Option<usize>, Vec<usize>> {
    let mut children_by_parent: HashMap<Option<usize>, Vec<usize>> = HashMap::new();
    for (row_index, row) in rows.iter().enumerate() {
        if entry_tree_row_has_graph_node(row) {
            children_by_parent
                .entry(graph_parent_by_row[row_index])
                .or_default()
                .push(row_index);
        }
    }
    children_by_parent
}

fn entry_tree_branch_parent_by_row(
    rows: &[SessionTreeRow],
    graph_children_by_parent: &HashMap<Option<usize>, Vec<usize>>,
) -> Vec<bool> {
    rows.iter()
        .enumerate()
        .map(|(row_index, row)| {
            row.branch_choices.len() >= 2
                || graph_children_by_parent
                    .get(&Some(row_index))
                    .is_some_and(|children| children.len() > 1)
        })
        .collect()
}

/// 合并 graph lane 深度与 session `display_depth`，并沿 graph owner 继承给 reasoning 等行。
fn entry_tree_layout_depth_by_row(
    rows: &[SessionTreeRow],
    graph_depth_by_row: &[usize],
    _graph_parent_by_row: &[Option<usize>],
    _graph_children_by_parent: &HashMap<Option<usize>, Vec<usize>>,
    _on_projection_by_row: &[bool],
    row_index_by_id: &HashMap<&str, usize>,
) -> Vec<usize> {
    let mut layout_depth_by_row = rows
        .iter()
        .enumerate()
        .map(|(row_index, row)| {
            if !entry_tree_row_has_graph_node(row) {
                return 0;
            }
            graph_depth_by_row[row_index].max(row.display_depth)
        })
        .collect::<Vec<_>>();

    for (row_index, row) in rows.iter().enumerate() {
        if entry_tree_row_has_graph_node(row) {
            continue;
        }
        layout_depth_by_row[row_index] =
            nearest_graph_ancestor_index(rows, row_index, row_index_by_id)
                .map(|owner_index| layout_depth_by_row[owner_index])
                .unwrap_or_default();
    }

    layout_depth_by_row
}

fn entry_tree_graph_lane_depth_by_row(
    rows: &[SessionTreeRow],
    graph_parent_by_row: &[Option<usize>],
    graph_children_by_parent: &HashMap<Option<usize>, Vec<usize>>,
    branch_parent_by_row: &[bool],
) -> Vec<usize> {
    let row_index_by_id = entry_tree_row_index_by_id(rows);
    let branch_parent_rows =
        entry_tree_graph_branch_parent_rows(graph_children_by_parent, branch_parent_by_row);
    let mut graph_depth_by_row = vec![None; rows.len()];
    for (row_index, row) in rows.iter().enumerate() {
        if entry_tree_row_has_graph_node(row) {
            let mut visiting = HashSet::new();
            let depth = entry_tree_graph_node_lane_depth(
                row_index,
                graph_parent_by_row,
                &branch_parent_rows,
                &mut graph_depth_by_row,
                &mut visiting,
            );
            graph_depth_by_row[row_index] = Some(depth);
        }
    }

    rows.iter()
        .enumerate()
        .map(|(row_index, row)| {
            if entry_tree_row_has_graph_node(row) {
                graph_depth_by_row[row_index].unwrap_or_default()
            } else {
                nearest_graph_ancestor_index(rows, row_index, &row_index_by_id)
                    .and_then(|ancestor_index| graph_depth_by_row[ancestor_index])
                    .unwrap_or_default()
            }
        })
        .collect()
}

fn entry_tree_graph_branch_parent_rows(
    graph_children_by_parent: &HashMap<Option<usize>, Vec<usize>>,
    branch_parent_by_row: &[bool],
) -> HashSet<usize> {
    let mut branch_parent_rows = graph_children_by_parent
        .iter()
        .filter_map(|(parent_index, children)| {
            if children.len() > 1 {
                *parent_index
            } else {
                None
            }
        })
        .collect::<HashSet<_>>();
    branch_parent_rows.extend(
        branch_parent_by_row
            .iter()
            .enumerate()
            .filter_map(|(row_index, is_branch_parent)| is_branch_parent.then_some(row_index)),
    );
    branch_parent_rows
}

fn entry_tree_graph_node_lane_depth(
    row_index: usize,
    graph_parent_by_row: &[Option<usize>],
    branch_parent_rows: &HashSet<usize>,
    graph_depth_by_row: &mut [Option<usize>],
    visiting: &mut HashSet<usize>,
) -> usize {
    if let Some(depth) = graph_depth_by_row[row_index] {
        return depth;
    }
    if !visiting.insert(row_index) {
        return 0;
    }

    let depth = match graph_parent_by_row[row_index] {
        Some(parent_index) => {
            let parent_depth = entry_tree_graph_node_lane_depth(
                parent_index,
                graph_parent_by_row,
                branch_parent_rows,
                graph_depth_by_row,
                visiting,
            );
            if branch_parent_rows.contains(&parent_index) {
                parent_depth.saturating_add(1)
            } else {
                parent_depth
            }
        }
        None => 0,
    };

    visiting.remove(&row_index);
    graph_depth_by_row[row_index] = Some(depth);
    depth
}

fn entry_tree_on_projection_by_row(
    rows: &[SessionTreeRow],
    selected_projection_nodes: &HashSet<usize>,
    row_index_by_id: &HashMap<&str, usize>,
) -> Vec<bool> {
    rows.iter()
        .enumerate()
        .map(|(row_index, _)| {
            entry_tree_row_graph_owner_index(rows, row_index, row_index_by_id)
                .is_some_and(|owner_index| selected_projection_nodes.contains(&owner_index))
        })
        .collect()
}

fn entry_tree_show_lane_by_row(
    rows: &[SessionTreeRow],
    on_projection_by_row: &[bool],
    _row_index_by_id: &HashMap<&str, usize>,
) -> Vec<bool> {
    // 仅选中投影路径画分支线；session 的 active path 与 UI 选择无关。
    rows.iter()
        .enumerate()
        .map(|(row_index, _)| on_projection_by_row[row_index])
        .collect()
}

fn entry_tree_projection_graph_endpoints(
    rows: &[SessionTreeRow],
    projection_path: &[usize],
) -> (Option<usize>, Option<usize>) {
    let graph_nodes = projection_path
        .iter()
        .copied()
        .filter(|&row_index| entry_tree_row_has_graph_node(&rows[row_index]))
        .collect::<Vec<_>>();
    (graph_nodes.first().copied(), graph_nodes.last().copied())
}

fn entry_tree_selected_projection_path(
    rows: &[SessionTreeRow],
    selected_index: usize,
    graph_parent_by_row: &[Option<usize>],
    graph_children_by_parent: &HashMap<Option<usize>, Vec<usize>>,
    row_index_by_id: &HashMap<&str, usize>,
) -> Vec<usize> {
    let Some(selected_owner_index) =
        entry_tree_row_graph_owner_index(rows, selected_index, row_index_by_id)
    else {
        return Vec::new();
    };
    let mut projection_path =
        entry_tree_graph_path_to_root(selected_owner_index, graph_parent_by_row);
    let mut current_index = selected_owner_index;
    let mut visited = projection_path.iter().copied().collect::<HashSet<_>>();

    while let Some(next_index) =
        entry_tree_selected_projection_next_child(rows, graph_children_by_parent, current_index)
    {
        if !visited.insert(next_index) {
            break;
        }
        projection_path.push(next_index);
        current_index = next_index;
    }

    projection_path
}

fn entry_tree_selected_projection_next_child(
    rows: &[SessionTreeRow],
    graph_children_by_parent: &HashMap<Option<usize>, Vec<usize>>,
    parent_index: usize,
) -> Option<usize> {
    let children = graph_children_by_parent.get(&Some(parent_index))?;
    children
        .iter()
        .copied()
        .find(|child_index| {
            rows[*child_index].is_current
                || (rows[*child_index].is_active_path
                    && entry_tree_row_has_graph_node(&rows[*child_index]))
        })
        .or_else(|| (children.len() == 1).then_some(children[0]))
}

fn entry_tree_row_graph_owner_index(
    rows: &[SessionTreeRow],
    row_index: usize,
    row_index_by_id: &HashMap<&str, usize>,
) -> Option<usize> {
    let row = rows.get(row_index)?;
    if entry_tree_row_has_graph_node(row) {
        Some(row_index)
    } else {
        nearest_graph_ancestor_index(rows, row_index, row_index_by_id)
    }
}

fn entry_tree_graph_path_to_root(
    row_index: usize,
    graph_parent_by_row: &[Option<usize>],
) -> Vec<usize> {
    let mut path = Vec::new();
    let mut current_index = Some(row_index);
    let mut visited = HashSet::new();

    while let Some(row_index) = current_index {
        if !visited.insert(row_index) {
            break;
        }
        path.push(row_index);
        current_index = graph_parent_by_row[row_index];
    }

    path.reverse();
    path
}

pub(super) fn entry_tree_graph_span_style(text: &str, palette: TerminalPalette) -> Style {
    if text.contains('@') {
        accent_text_style(palette)
    } else {
        tertiary_text_style(palette)
    }
}
