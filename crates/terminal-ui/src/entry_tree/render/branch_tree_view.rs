use crate::theme::build_page_rule;

use super::{
    shared::{EntryTreeWidget, branch_message_count_label},
    *,
};

impl Model {
    pub(in crate::entry_tree::render) fn render_entry_tree_branch_tree(
        &self,
        frame: &mut RenderFrame<'_>,
        area: Rect,
    ) {
        let Some(branch_tree) = self
            .entry_tree
            .as_ref()
            .and_then(|state| state.branch_tree.as_ref())
        else {
            return;
        };
        frame.render_widget(Clear, area);
        let Some(chrome) = fullscreen_list_chrome_rects(area) else {
            return;
        };
        let page_size = entry_tree_branch_tree_page_size_for_height(area.height);

        frame.render_widget(
            Paragraph::new(
                self.entry_tree_branch_tree_header_line(branch_tree, usize::from(area.width)),
            ),
            chrome.header,
        );
        frame.render_widget(
            Paragraph::new(subtle_rule_line(usize::from(area.width), self.palette)),
            chrome.header_rule,
        );

        let lines = self.entry_tree_branch_tree_body_lines(
            branch_tree,
            usize::from(area.width),
            usize::from(chrome.body.height),
            page_size,
        );
        frame.render_widget(EntryTreeWidget { lines: &lines }, chrome.body);
        if let Some(selected_visible_row) = branch_tree.selected_visible_row(page_size) {
            let selected_y = chrome
                .body
                .y
                .saturating_add(1)
                .saturating_add(u16::try_from(selected_visible_row).unwrap_or(u16::MAX));
            if selected_y < chrome.body.bottom() {
                frame.buffer_mut().set_style(
                    Rect::new(chrome.body.x, selected_y, chrome.body.width, 1),
                    self.entry_tree_selected_row_style(),
                );
            }
        }

        frame.render_widget(
            Paragraph::new(build_page_rule(
                area.width,
                branch_tree.page_number(page_size),
                branch_tree.page_count(page_size),
                self.palette,
            )),
            chrome.page_rule,
        );
        frame.render_widget(
            Paragraph::new(Line::styled(
                entry_tree_branch_tree_footer_hint(
                    area.width,
                    branch_tree
                        .selected_node()
                        .is_some_and(|node| node.branch.is_current),
                ),
                tertiary_text_style(self.palette).add_modifier(Modifier::ITALIC),
            )),
            chrome.footer,
        );
    }

    fn entry_tree_branch_tree_header_line(
        &self,
        state: &EntryTreeBranchTreeState,
        width: usize,
    ) -> Line<'static> {
        let title = format!(
            "Branch Tree ({} of {})",
            state.selected_position_label(),
            state.nodes.len(),
        );
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                truncate_display_width_with_ellipsis(&title, width.saturating_sub(2).max(1)),
                primary_text_style(self.palette).bold(),
            ),
        ])
    }

    fn entry_tree_branch_tree_body_lines(
        &self,
        state: &EntryTreeBranchTreeState,
        width: usize,
        body_height: usize,
        page_size: usize,
    ) -> Vec<Line<'static>> {
        let width = width.max(1);
        let mut lines = Vec::new();
        if state.is_loading {
            lines.push(Line::styled(
                "  Loading branch tree...",
                tertiary_text_style(self.palette),
            ));
        } else if let Some(error) = state.error.as_deref() {
            lines.push(Line::styled(
                truncate_display_width_with_ellipsis(&format!("  {error}"), width),
                tertiary_text_style(self.palette),
            ));
        } else {
            lines.push(Line::from(vec![
                Span::raw(" ".repeat(ENTRY_TREE_BODY_HORIZONTAL_PADDING)),
                Span::styled(".", tertiary_text_style(self.palette)),
            ]));
            if state.nodes.is_empty() {
                lines.push(Line::styled(
                    "  No branches yet",
                    tertiary_text_style(self.palette),
                ));
            } else {
                let prefixes = branch_tree_connector_prefixes(&state.nodes);
                let page_start = state.page_start(page_size);
                for (visible_position, node_index) in state.page_indices(page_size).enumerate() {
                    let node = &state.nodes[node_index];
                    let prefix = prefixes.get(node_index).map(String::as_str).unwrap_or("");
                    lines.push(self.entry_tree_branch_tree_node_line(
                        node,
                        prefix,
                        width,
                        page_start + visible_position == state.selected,
                    ));
                }
            }
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                branch_tree_summary_label(state.nodes.len(), state.total_message_count, width),
                tertiary_text_style(self.palette),
            ));
        }

        lines.truncate(body_height);
        lines
    }

    fn entry_tree_branch_tree_node_line(
        &self,
        node: &SessionBranchTreeNode,
        connector_prefix: &str,
        width: usize,
        is_selected: bool,
    ) -> Line<'static> {
        let left_padding = " ".repeat(ENTRY_TREE_BODY_HORIZONTAL_PADDING);
        let prefix_width = display_width(&left_padding) + display_width(connector_prefix);
        let text_width = width
            .saturating_sub(prefix_width)
            .saturating_sub(ENTRY_TREE_BODY_HORIZONTAL_PADDING);
        let node_text = branch_tree_node_text(node, text_width);
        let node_style = if node.branch.is_current {
            command_accent_text_style(self.palette)
        } else if is_selected {
            primary_text_style(self.palette).bold()
        } else {
            secondary_text_style(self.palette)
        };

        Line::from(vec![
            Span::raw(left_padding),
            Span::styled(
                connector_prefix.to_string(),
                tertiary_text_style(self.palette),
            ),
            Span::styled(node_text, node_style),
        ])
    }
}

fn branch_tree_node_text(node: &SessionBranchTreeNode, width: usize) -> String {
    let width = width.max(1);
    let branch = &node.branch;
    let message_count = branch_message_count_text(branch.message_count);
    let branch_content = if branch.is_current {
        format!("{message_count} (current) {}", branch.display_summary)
    } else {
        format!("{message_count} {}", branch.display_summary)
    };

    truncate_display_width_with_ellipsis(&branch_content, width)
}

fn branch_tree_summary_label(
    branch_count: usize,
    total_message_count: usize,
    width: usize,
) -> String {
    truncate_display_width_with_ellipsis(
        &format!("  {branch_count} branches, {total_message_count} messages"),
        width.max(1),
    )
}

fn branch_message_count_text(message_count: usize) -> String {
    format!("{} msgs", branch_message_count_label(message_count))
}

fn entry_tree_branch_tree_footer_hint(width: u16, is_current_branch: bool) -> &'static str {
    match (width < 90, is_current_branch) {
        (true, true) => "  Esc back · Enter switch · j/k · h/l page",
        (true, false) => "  Esc back · Enter switch · Space preview branch · j/k · h/l page",
        (false, true) => "  Esc back · Enter switch · ↑/↓/j/k move · ←/→/h/l page",
        (false, false) => {
            "  Esc back · Enter switch · Space preview branch · ↑/↓/j/k move · ←/→/h/l page"
        }
    }
}
