use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::{
    envinfo,
    frontend::tui::{
        Model, StyleMode, theme::tertiary_text_style, transcript::DEFAULT_RENDER_WIDTH,
    },
};

/// `StatusLineItem` 表示输入框下方状态行中的一个可配置项目。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StatusLineItem {
    GitBranch,
    CurrentDir,
}

impl StatusLineItem {
    /// `from_config_value` 把配置值映射为状态行项目。
    pub fn from_config_value(value: &str) -> Option<Self> {
        match value {
            "git-branch" => Some(Self::GitBranch),
            "current-dir" => Some(Self::CurrentDir),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct StatusLineRenderResult {
    pub(crate) line: Option<Line<'static>>,
    pub(crate) plain_line: String,
    pub(crate) has_content: bool,
    pub(crate) gap_before: usize,
}

pub(crate) const STATUS_LINE_INSET_WIDTH: usize = 2;
const STATUS_LINE_SEPARATOR: &str = " · ";
const STATUS_LINE_ELLIPSIS: &str = "...";

impl Model {
    pub(crate) fn current_status_line_cache_key(&self) -> String {
        if !self.current_status_notice_text().is_empty() {
            return format!("notice\0{}", self.current_status_notice_text());
        }

        self.current_status_line_parts().join("\0")
    }

    pub(crate) fn current_status_line_render_result(&self) -> StatusLineRenderResult {
        if !self.current_status_notice_text().is_empty() {
            return self.current_status_notice_render_result();
        }

        let parts = self.current_status_line_parts();
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
            line: Some(Line::from(vec![Span::styled(
                plain_line.clone(),
                tertiary_text_style(self.palette),
            )])),
            plain_line,
            has_content: true,
            gap_before: status_line_gap_before(self.style_mode),
        }
    }

    pub(crate) fn current_status_line_parts(&self) -> Vec<String> {
        let mut parts = Vec::with_capacity(self.status_line_items.len() + 1);
        for item in &self.status_line_items {
            match item {
                StatusLineItem::GitBranch if !self.git_branch.is_empty() => {
                    parts.push(sanitize_status_line_part(&self.git_branch));
                }
                StatusLineItem::CurrentDir if !self.current_dir.is_empty() => {
                    parts.push(sanitize_status_line_part(&self.current_dir));
                }
                _ => {}
            }
        }
        let helper = self.current_external_editor_helper_text();
        if !helper.is_empty() {
            parts.push(sanitize_status_line_part(&helper));
        }

        parts
    }

    pub(crate) fn uses_status_line_item(&self, target: StatusLineItem) -> bool {
        self.status_line_items.contains(&target)
    }

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
    }
}

pub(crate) fn status_line_gap_before(style_mode: StyleMode) -> usize {
    if matches!(style_mode.normalized(), StyleMode::Ms) {
        1
    } else {
        0
    }
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
    let separator_width = STATUS_LINE_SEPARATOR.width();
    let mut current_width = text.width();

    for part in parts.iter().skip(1) {
        let part_width = part.width();
        if current_width + separator_width + part_width > width {
            let remaining_width = width.saturating_sub(current_width + separator_width);
            if remaining_width < STATUS_LINE_ELLIPSIS.width() {
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

fn sanitize_status_line_part(text: &str) -> String {
    text.chars()
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
    if text.width() <= width {
        return text.to_string();
    }

    let mut rendered = String::new();
    let mut used_width = 0;
    for grapheme in UnicodeSegmentation::graphemes(text, true) {
        let grapheme_width = grapheme.width();
        if used_width + grapheme_width > width {
            break;
        }
        rendered.push_str(grapheme);
        used_width += grapheme_width;
    }

    rendered
}

fn truncate_display_width_with_ellipsis(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if text.width() <= width {
        return text.to_string();
    }

    let ellipsis_width = STATUS_LINE_ELLIPSIS.width();
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

    let ellipsis_width = STATUS_LINE_ELLIPSIS.width();
    if width <= ellipsis_width {
        return truncate_display_width(STATUS_LINE_ELLIPSIS, width);
    }

    format!(
        "{}{}",
        truncate_display_width(text, width - ellipsis_width),
        STATUS_LINE_ELLIPSIS
    )
}
