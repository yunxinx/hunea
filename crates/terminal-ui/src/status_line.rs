use ratatui::text::Line;
use unicode_segmentation::UnicodeSegmentation;

use runtime_domain::envinfo;

use crate::{
    Model, StyleMode,
    display_width::{display_width, grapheme_width},
    selection::SelectableLineRange,
    terminal_text::sanitize_terminal_text,
    theme::tertiary_text_style,
    transcript::DEFAULT_RENDER_WIDTH,
};

/// `StatusLineItem` 表示输入框下方状态行中的一个可配置项目。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StatusLineItem {
    GitBranch,
    CurrentDir,
    CurrentModel,
    Throughput,
    Latency,
}

impl StatusLineItem {
    /// `from_config_value` 把配置值映射为状态行项目。
    pub fn from_config_value(value: &str) -> Option<Self> {
        match value {
            "git-branch" => Some(Self::GitBranch),
            "current-dir" => Some(Self::CurrentDir),
            "current-model" => Some(Self::CurrentModel),
            "throughput" => Some(Self::Throughput),
            "latency" => Some(Self::Latency),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct StatusLineRenderResult {
    pub(crate) line: Option<Line<'static>>,
    pub(crate) plain_line: String,
    pub(crate) selectable: SelectableLineRange,
    pub(crate) has_content: bool,
    pub(crate) gap_before: usize,
}

pub(crate) const STATUS_LINE_INSET_WIDTH: usize = 2;
const STATUS_LINE_SEPARATOR: &str = " · ";
const STATUS_LINE_ELLIPSIS: &str = "...";

impl Model {
    pub(crate) fn status_line_config_bits(&self) -> u8 {
        status_line_config_bits_for_items(&self.status_line_items)
    }

    pub(crate) fn status_line_2_config_bits(&self) -> u8 {
        status_line_config_bits_for_items(&self.status_line_2_items)
    }

    pub(crate) fn current_status_line_render_result(&self) -> StatusLineRenderResult {
        if self.tool_approval_panel_active() {
            return StatusLineRenderResult::default();
        }
        if !self.current_status_notice_text().is_empty() {
            return self.current_status_notice_render_result();
        }
        if self.command_panel_active() || self.model_panel_active() || self.context_budget_active()
        {
            return StatusLineRenderResult::default();
        }

        self.render_status_line_result(
            self.current_status_line_parts(),
            status_line_gap_before(self.style_mode),
        )
    }

    pub(crate) fn current_status_line_2_render_result(&self) -> StatusLineRenderResult {
        if !self.current_status_notice_text().is_empty()
            || self.command_panel_active()
            || self.model_panel_active()
            || self.context_budget_active()
            || self.tool_approval_panel_active()
        {
            return StatusLineRenderResult::default();
        }

        self.render_status_line_result(self.current_status_line_2_parts(), 0)
    }

    fn render_status_line_result(
        &self,
        parts: Vec<String>,
        gap_before: usize,
    ) -> StatusLineRenderResult {
        if parts.is_empty() {
            return StatusLineRenderResult::default();
        }

        let width = if self.width == 0 {
            DEFAULT_RENDER_WIDTH
        } else {
            usize::from(self.width)
        };
        let text = compose_status_line_text(&parts, width.saturating_sub(STATUS_LINE_INSET_WIDTH));
        if text.is_empty() {
            return StatusLineRenderResult::default();
        }

        let plain_line = format!("{}{}", " ".repeat(STATUS_LINE_INSET_WIDTH), text);
        StatusLineRenderResult {
            line: Some(Line::styled(
                plain_line.clone(),
                tertiary_text_style(self.palette),
            )),
            plain_line,
            selectable: status_line_selectable_range(&text),
            has_content: true,
            gap_before,
        }
    }

    pub(crate) fn current_status_line_parts(&self) -> Vec<String> {
        let mut parts = self.status_line_parts_for_items(&self.status_line_items);
        let helper = self.current_external_editor_helper_text();
        if !helper.is_empty() {
            parts.push(sanitize_status_line_part(&helper));
        }

        parts
    }

    pub(crate) fn current_status_line_2_parts(&self) -> Vec<String> {
        let items = self
            .status_line_2_items
            .iter()
            .copied()
            .filter(|item| !self.status_line_items.contains(item))
            .collect::<Vec<_>>();
        self.status_line_parts_for_items(&items)
    }

    fn status_line_parts_for_items(&self, items: &[StatusLineItem]) -> Vec<String> {
        let mut parts = Vec::with_capacity(items.len());
        for item in items {
            match item {
                StatusLineItem::GitBranch if !self.git_branch.is_empty() => {
                    parts.push(sanitize_status_line_part(&self.git_branch));
                }
                StatusLineItem::CurrentDir if !self.current_dir.is_empty() => {
                    parts.push(sanitize_status_line_part(&self.current_dir));
                }
                StatusLineItem::CurrentModel => {
                    if let Some(selection) = self.selected_model.selection() {
                        parts.push(sanitize_status_line_part(
                            &self.model_selection_display_name(
                                selection.provider_id.as_str(),
                                selection.model_id.as_str(),
                            ),
                        ));
                    }
                }
                StatusLineItem::Throughput => {
                    if let Some(metrics) = self.last_request_metrics() {
                        parts.push(format_request_throughput(
                            metrics.output_tokens,
                            metrics.duration,
                        ));
                    }
                }
                StatusLineItem::Latency => {
                    if let Some(metrics) = self.last_request_metrics() {
                        parts.push(format_request_latency(metrics.latency));
                    }
                }
                _ => {}
            }
        }
        parts
    }

    pub(crate) fn uses_status_line_item(&self, target: StatusLineItem) -> bool {
        self.status_line_items.contains(&target) || self.status_line_2_items.contains(&target)
    }
}

fn status_line_config_bits_for_items(items: &[StatusLineItem]) -> u8 {
    let mut bits = 0u8;
    if items.contains(&StatusLineItem::GitBranch) {
        bits |= 1 << 0;
    }
    if items.contains(&StatusLineItem::CurrentDir) {
        bits |= 1 << 1;
    }
    if items.contains(&StatusLineItem::CurrentModel) {
        bits |= 1 << 2;
    }
    if items.contains(&StatusLineItem::Throughput) {
        bits |= 1 << 3;
    }
    if items.contains(&StatusLineItem::Latency) {
        bits |= 1 << 4;
    }
    bits
}

impl Model {
    /// `refresh_status_line_after_transcript_change` 在 transcript 真正追加后刷新动态状态项。
    pub(crate) fn refresh_status_line_after_transcript_change(&mut self) {
        if !self.uses_status_line_item(StatusLineItem::GitBranch) {
            return;
        }

        let next_branch = envinfo::git_branch();
        if next_branch == self.git_branch {
            return;
        }

        self.git_branch = next_branch;
        self.bump_status_line_revision();
    }
}

fn format_request_throughput(output_tokens: usize, duration: std::time::Duration) -> String {
    let throughput = if duration.is_zero() {
        0
    } else {
        (output_tokens as f64 / duration.as_secs_f64()) as usize
    };
    format!("{throughput}tps")
}

fn format_request_latency(latency: std::time::Duration) -> String {
    format!("{:.2}s", latency.as_secs_f64())
}

pub(crate) fn status_line_gap_before(style_mode: StyleMode) -> usize {
    if matches!(style_mode.normalized(), StyleMode::Ms) {
        1
    } else {
        0
    }
}

pub(crate) fn status_line_pair_height(
    status_line: &StatusLineRenderResult,
    status_line_2: &StatusLineRenderResult,
    first_visible_gap_before: usize,
) -> usize {
    let visible_lines =
        usize::from(status_line.has_content) + usize::from(status_line_2.has_content);
    if visible_lines == 0 {
        return 0;
    }

    first_visible_gap_before + visible_lines
}

pub(crate) fn compose_status_line_text(parts: &[String], width: usize) -> String {
    if parts.is_empty() || width == 0 {
        return String::new();
    }

    let first = truncate_display_width_with_ellipsis(&parts[0], width);
    if first.is_empty() {
        return String::new();
    }
    if first != parts[0] {
        return first;
    }

    let mut text = first;
    let separator_width = display_width(STATUS_LINE_SEPARATOR);
    let mut current_width = display_width(&text);

    for part in parts.iter().skip(1) {
        let part_width = display_width(part);
        if current_width + separator_width + part_width > width {
            let remaining_width = width.saturating_sub(current_width + separator_width);
            if remaining_width < display_width(STATUS_LINE_ELLIPSIS) {
                return force_ellipsis_at_display_width(&text, width);
            }

            let truncated_part = truncate_display_width_with_ellipsis(part, remaining_width);
            if !truncated_part.is_empty() {
                text.push_str(STATUS_LINE_SEPARATOR);
                text.push_str(&truncated_part);
                return text;
            }

            return force_ellipsis_at_display_width(&text, width);
        }

        text.push_str(STATUS_LINE_SEPARATOR);
        text.push_str(part);
        current_width += separator_width + part_width;
    }

    text
}

fn status_line_selectable_range(text: &str) -> SelectableLineRange {
    SelectableLineRange::with_hit_range(
        STATUS_LINE_INSET_WIDTH,
        STATUS_LINE_INSET_WIDTH + display_width(text),
        0,
        STATUS_LINE_INSET_WIDTH + display_width(text),
    )
}

fn sanitize_status_line_part(text: &str) -> String {
    sanitize_terminal_text(text)
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

pub(crate) fn truncate_display_width(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if display_width(text) <= width {
        return text.to_string();
    }

    let mut rendered = String::new();
    let mut used_width = 0;
    for grapheme in UnicodeSegmentation::graphemes(text, true) {
        let cluster_width = grapheme_width(grapheme);
        if used_width + cluster_width > width {
            break;
        }
        rendered.push_str(grapheme);
        used_width += cluster_width;
    }

    rendered
}

pub(crate) fn truncate_display_width_with_ellipsis(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if display_width(text) <= width {
        return text.to_string();
    }

    let ellipsis_width = display_width(STATUS_LINE_ELLIPSIS);
    if width <= ellipsis_width {
        return truncate_display_width(STATUS_LINE_ELLIPSIS, width);
    }

    format!(
        "{}{}",
        truncate_display_width(text, width - ellipsis_width),
        STATUS_LINE_ELLIPSIS
    )
}

fn force_ellipsis_at_display_width(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }

    let ellipsis_width = display_width(STATUS_LINE_ELLIPSIS);
    if width <= ellipsis_width {
        return truncate_display_width(STATUS_LINE_ELLIPSIS, width);
    }

    format!(
        "{}{}",
        truncate_display_width(text, width - ellipsis_width),
        STATUS_LINE_ELLIPSIS
    )
}
