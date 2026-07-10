use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    path::Path,
    rc::Rc,
    time::Duration,
};

use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::{
    display_width::display_width,
    markdown_display::markdown_display_content,
    message::assistant_message_content_width,
    styled_text::{line_to_plain_text, lines_to_ansi_text, lines_to_plain_text},
    theme::{TerminalPalette, tertiary_text_style},
    transcript::{
        ItemLineAnchor, LineAnchorKind, TRANSCRIPT_DETAIL_HINT, TranscriptEstimateKind,
        TranscriptFastEstimate, TranscriptItemMetrics,
        markdown_render::{render_reasoning_markdown_lines, render_reasoning_markdown_metrics},
        wrap_assistant_text,
    },
};

const REASONING_ACTION_SHOW: &str = "Show reasoning";
const REASONING_ACTION_HIDE: &str = "Hide reasoning";
const REASONING_SIMPLIFIED_EDGE_LINES: usize = 4;
const REASONING_SIMPLIFIED_HINT_OVERHEAD_LINES: usize = 3;

/// `ReasoningDisplayMode` 表示思维链消息进入 transcript 时的默认展示状态。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum ReasoningDisplayMode {
    #[default]
    Collapsed,
    Expanded,
    /// `ExpandedSimplified` 是 expanded 的主界面 compact 变体：
    /// 短内容完整展示，长内容保留前后行并由 Ctrl+T overlay 还原全文。
    ExpandedSimplified,
    Snippet,
}

/// `ReasoningRenderMode` 控制 `ExpandedSimplified` 在主界面与 overlay 中的详略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ReasoningRenderMode {
    Compact,
    Detailed,
}

/// `ReasoningMessageItem` 表示只用于展示的模型思维链，不参与后续模型上下文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReasoningMessageItem {
    content: String,
    display_mode: ReasoningDisplayMode,
    render_mode: ReasoningRenderMode,
    has_toggle_header: bool,
    duration: Option<Duration>,
    working_dir: Option<Rc<Path>>,
    render_cache_key: u64,
}

impl ReasoningMessageItem {
    /// `new` 创建一条思维链展示项。
    pub(crate) fn new(
        content: impl Into<String>,
        display_mode: ReasoningDisplayMode,
        duration: Option<Duration>,
        working_dir: Option<Rc<Path>>,
    ) -> Self {
        let content = if matches!(display_mode, ReasoningDisplayMode::Snippet) {
            String::new()
        } else {
            content.into()
        };
        let has_toggle_header = matches!(display_mode, ReasoningDisplayMode::Collapsed);
        let render_mode = ReasoningRenderMode::Compact;
        let render_cache_key = reasoning_message_render_cache_key(
            &content,
            display_mode,
            render_mode,
            has_toggle_header,
            duration,
        );

        Self {
            content,
            display_mode,
            render_mode,
            has_toggle_header,
            duration,
            working_dir,
            render_cache_key,
        }
    }

    /// `render_lines` 将思维链渲染为淡色斜体文本行。
    pub fn render_lines(&self, width: u16, palette: TerminalPalette) -> Vec<Line<'static>> {
        let style = reasoning_content_style(palette);

        match self.display_mode {
            ReasoningDisplayMode::Collapsed | ReasoningDisplayMode::Snippet => {
                vec![styled_reasoning_line(self.header_label(), style)]
            }
            ReasoningDisplayMode::Expanded | ReasoningDisplayMode::ExpandedSimplified => {
                let mut lines = Vec::new();
                if self.has_toggle_header {
                    lines.push(styled_reasoning_line(self.header_label(), style));
                }
                lines.extend(self.render_content_lines(width, palette, style));
                lines
            }
        }
    }

    pub(crate) fn is_header_line(&self, line_index: usize) -> bool {
        line_index == 0 && self.is_toggleable()
    }

    pub(crate) fn uses_assistant_visual_inset(&self) -> bool {
        !matches!(self.display_mode, ReasoningDisplayMode::Snippet)
    }

    pub(crate) fn header_display_width(&self) -> usize {
        if self.is_toggleable() {
            display_width(&self.header_label())
        } else {
            0
        }
    }

    pub(crate) fn toggle(&mut self) {
        if !self.is_toggleable() {
            return;
        }

        self.display_mode = match self.display_mode {
            ReasoningDisplayMode::Collapsed => ReasoningDisplayMode::Expanded,
            ReasoningDisplayMode::Expanded => ReasoningDisplayMode::Collapsed,
            ReasoningDisplayMode::ExpandedSimplified | ReasoningDisplayMode::Snippet => return,
        };
        self.render_cache_key = reasoning_message_render_cache_key(
            &self.content,
            self.display_mode,
            self.render_mode,
            self.has_toggle_header,
            self.duration,
        );
    }

    pub(crate) fn set_render_mode(&mut self, mode: ReasoningRenderMode) -> bool {
        if self.render_mode == mode
            || !matches!(self.display_mode, ReasoningDisplayMode::ExpandedSimplified)
        {
            return false;
        }

        self.render_mode = mode;
        self.render_cache_key = reasoning_message_render_cache_key(
            &self.content,
            self.display_mode,
            self.render_mode,
            self.has_toggle_header,
            self.duration,
        );
        true
    }

    /// `render_for_terminal_replay` 返回适合退出 AltScreen 后回放到终端的文本。
    pub fn render_for_terminal_replay(
        &self,
        width: u16,
        palette: TerminalPalette,
        preserve_ansi: bool,
    ) -> String {
        let lines = self.render_lines(width, palette);
        if preserve_ansi {
            lines_to_ansi_text(&lines)
        } else {
            lines_to_plain_text(&lines)
        }
    }

    /// `render_plain_text` 返回不带 ANSI 的纯文本内容。
    pub fn render_plain_text(&self, width: u16, palette: TerminalPalette) -> String {
        lines_to_plain_text(&self.render_lines(width, palette))
    }

    pub(crate) fn render_cache_key(&self) -> u64 {
        self.render_cache_key
    }

    pub(crate) fn source_text_byte_len(&self) -> usize {
        self.content.len()
    }

    pub(crate) fn measure_render_metrics(
        &self,
        width: u16,
        palette: TerminalPalette,
    ) -> (usize, usize) {
        match self.display_mode {
            ReasoningDisplayMode::Collapsed | ReasoningDisplayMode::Snippet => {
                let label = self.header_label();
                (1, label.len())
            }
            ReasoningDisplayMode::Expanded | ReasoningDisplayMode::ExpandedSimplified => {
                let mut line_count = 0usize;
                let mut content_char_len = 0usize;

                if self.has_toggle_header {
                    let label = self.header_label();
                    line_count = line_count.saturating_add(1);
                    content_char_len = content_char_len.saturating_add(label.len());
                }

                let body_metrics = self.measure_content_metrics(width, palette);
                line_count = line_count.saturating_add(body_metrics.0);
                content_char_len = content_char_len.saturating_add(body_metrics.1);

                (line_count, content_char_len)
            }
        }
    }

    pub(crate) fn estimate_render_metrics_fast(
        &self,
        width: u16,
        palette: TerminalPalette,
        previous_metrics: Option<TranscriptItemMetrics>,
    ) -> TranscriptFastEstimate {
        let previous_metrics =
            previous_metrics.filter(|metrics| metrics.cache_key == self.render_cache_key);
        if let Some(metrics) = previous_metrics
            && metrics.is_valid
            && metrics.width == width
        {
            return TranscriptFastEstimate {
                content_line_count: metrics.content_line_count,
                content_char_len: metrics.content_char_len,
                kind: TranscriptEstimateKind::Assistant,
                ..TranscriptFastEstimate::default()
            };
        }

        let (content_line_count, content_char_len) = self.measure_render_metrics(width, palette);
        TranscriptFastEstimate {
            content_line_count,
            content_char_len,
            kind: TranscriptEstimateKind::Assistant,
            ..TranscriptFastEstimate::default()
        }
    }

    pub(crate) fn render_line_anchors(
        &self,
        width: u16,
        palette: TerminalPalette,
    ) -> Vec<ItemLineAnchor> {
        let full_lines =
            self.render_full_content_lines(width, palette, reasoning_content_style(palette));
        reasoning_content_line_anchors(
            full_lines.len(),
            self.should_compact_content_lines(full_lines.len()),
        )
    }

    fn render_content_lines(
        &self,
        width: u16,
        palette: TerminalPalette,
        style: Style,
    ) -> Vec<Line<'static>> {
        let lines = self.render_full_content_lines(width, palette, style);
        if self.should_compact_content_lines(lines.len()) {
            compact_reasoning_content_lines(lines, style)
        } else {
            lines
        }
    }

    fn render_full_content_lines(
        &self,
        width: u16,
        palette: TerminalPalette,
        style: Style,
    ) -> Vec<Line<'static>> {
        let content = self.display_content();
        let content_width = assistant_message_content_width(width);
        let markdown_lines = render_reasoning_markdown_lines(
            content,
            content_width,
            palette,
            self.working_dir.as_deref(),
        );
        let lines = if markdown_lines.is_empty() {
            self.wrapped_lines(width)
                .into_iter()
                .map(|line| Line::from(Span::raw(line)))
                .collect()
        } else {
            markdown_lines
        };

        apply_reasoning_content_style(lines, style)
    }

    fn measure_content_metrics(&self, width: u16, palette: TerminalPalette) -> (usize, usize) {
        if matches!(self.display_mode, ReasoningDisplayMode::ExpandedSimplified) {
            let lines = self.render_content_lines(width, palette, reasoning_content_style(palette));
            return (
                lines.len(),
                lines
                    .iter()
                    .map(line_to_plain_text)
                    .map(|line| line.len())
                    .sum(),
            );
        }

        let content = self.display_content();
        let content_width = assistant_message_content_width(width);
        let markdown_metrics = render_reasoning_markdown_metrics(
            content,
            content_width,
            palette,
            self.working_dir.as_deref(),
        );
        if markdown_metrics.0 > 0 {
            return markdown_metrics;
        }

        let lines = self.wrapped_lines(width);
        (lines.len(), lines.iter().map(String::len).sum())
    }

    fn wrapped_lines(&self, width: u16) -> Vec<String> {
        wrap_assistant_text(
            self.display_content(),
            assistant_message_content_width(width),
            0,
        )
    }

    fn display_content(&self) -> &str {
        markdown_display_content(&self.content)
    }

    fn header_label(&self) -> String {
        match self.display_mode {
            ReasoningDisplayMode::Collapsed | ReasoningDisplayMode::Expanded => {
                let action = if matches!(self.display_mode, ReasoningDisplayMode::Collapsed) {
                    REASONING_ACTION_SHOW
                } else {
                    REASONING_ACTION_HIDE
                };
                let Some(duration) = self.duration.map(format_reasoning_duration) else {
                    return format!("[{action}]");
                };
                format!("[{action} · thoughts {duration}]")
            }
            ReasoningDisplayMode::ExpandedSimplified => String::new(),
            ReasoningDisplayMode::Snippet => {
                let Some(duration) = self.duration.map(format_reasoning_duration) else {
                    return "• thoughts".to_string();
                };
                format!("• thoughts {duration}")
            }
        }
    }

    fn is_toggleable(&self) -> bool {
        self.has_toggle_header
    }

    fn should_compact_content_lines(&self, line_count: usize) -> bool {
        // `expanded-simplified` 的省略块包含上方空行、提示行、下方空行。
        // 只有 compact 后严格更短才折叠，避免 9-11 行内容被“简化”成同样长甚至更长。
        let compacted_line_count = REASONING_SIMPLIFIED_EDGE_LINES
            .saturating_mul(2)
            .saturating_add(REASONING_SIMPLIFIED_HINT_OVERHEAD_LINES);
        matches!(self.display_mode, ReasoningDisplayMode::ExpandedSimplified)
            && self.render_mode == ReasoningRenderMode::Compact
            && line_count > compacted_line_count
    }
}

fn compact_reasoning_content_lines(lines: Vec<Line<'static>>, style: Style) -> Vec<Line<'static>> {
    let edge = REASONING_SIMPLIFIED_EDGE_LINES;
    let limit = edge.saturating_mul(2);
    let omitted = lines.len().saturating_sub(limit);
    let mut compacted = Vec::with_capacity(limit + 3);
    compacted.extend(lines.iter().take(edge).cloned());
    compacted.push(styled_reasoning_line(String::new(), style));
    compacted.push(styled_reasoning_line(
        format!("… +{omitted} lines ({TRANSCRIPT_DETAIL_HINT})"),
        style,
    ));
    compacted.push(styled_reasoning_line(String::new(), style));
    compacted.extend(lines.iter().skip(lines.len().saturating_sub(edge)).cloned());
    compacted
}

fn reasoning_content_line_anchors(
    full_line_count: usize,
    should_compact: bool,
) -> Vec<ItemLineAnchor> {
    if !should_compact {
        return (0..full_line_count)
            .map(reasoning_source_line_anchor)
            .collect();
    }

    let edge = REASONING_SIMPLIFIED_EDGE_LINES;
    let tail_start = full_line_count.saturating_sub(edge);
    let omitted_start = edge.min(full_line_count.saturating_sub(1));
    let mut anchors = Vec::with_capacity(edge.saturating_mul(2).saturating_add(3));
    anchors.extend((0..edge).map(reasoning_source_line_anchor));

    // 省略提示和两侧空行都锚定到第一条被省略的 source 行。
    // Ctrl+T overlay 因此会进入被隐藏区域，而不是停在 compact 提示本身。
    let omitted_anchor = reasoning_source_line_anchor(omitted_start);
    anchors.push(omitted_anchor);
    anchors.push(omitted_anchor);
    anchors.push(omitted_anchor);

    anchors.extend((tail_start..full_line_count).map(reasoning_source_line_anchor));
    anchors
}

fn reasoning_source_line_anchor(source_line: usize) -> ItemLineAnchor {
    ItemLineAnchor {
        kind: LineAnchorKind::LogicalPosition,
        logical_line: source_line,
        range_start: source_line,
        range_end: source_line.saturating_add(1),
        rendered_line: source_line,
        ..ItemLineAnchor::default()
    }
}

fn reasoning_content_style(palette: TerminalPalette) -> Style {
    tertiary_text_style(palette).italic()
}

fn styled_reasoning_line(content: String, style: Style) -> Line<'static> {
    Line::from(Span::styled(content, style))
}

fn apply_reasoning_content_style(
    mut lines: Vec<Line<'static>>,
    style: Style,
) -> Vec<Line<'static>> {
    for line in &mut lines {
        line.style = line.style.patch(style);
        for span in &mut line.spans {
            // Reasoning Content 的外层视觉语义优先；Markdown 只提供结构和 modifier。
            // 因此这里保留 bold/strike 等 modifier，但统一覆盖为 reasoning 的低优先级颜色。
            span.style = span.style.patch(style);
        }
    }
    lines
}

fn reasoning_message_render_cache_key(
    content: &str,
    display_mode: ReasoningDisplayMode,
    render_mode: ReasoningRenderMode,
    has_toggle_header: bool,
    duration: Option<Duration>,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    "reasoning_message".hash(&mut hasher);
    display_mode.hash(&mut hasher);
    if matches!(display_mode, ReasoningDisplayMode::ExpandedSimplified) {
        render_mode.hash(&mut hasher);
    }
    has_toggle_header.hash(&mut hasher);
    duration.hash(&mut hasher);
    if !matches!(display_mode, ReasoningDisplayMode::Snippet) {
        content.hash(&mut hasher);
    }
    hasher.finish()
}

fn format_reasoning_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds == 0 && duration > Duration::ZERO {
        return "<1s".to_string();
    }
    if seconds < 60 {
        return format!("{seconds}s");
    }
    if seconds < 3600 {
        let minutes = seconds / 60;
        let seconds = seconds % 60;
        return format!("{minutes}m{seconds:02}s");
    }

    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let seconds = seconds % 60;
    format!("{hours}h{minutes:02}m{seconds:02}s")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ratatui::style::Modifier;

    use super::*;
    use crate::{
        styled_text::line_to_plain_text,
        theme::{default_palette, tertiary_text_style},
    };

    #[test]
    fn reasoning_message_renders_expanded_content_without_header() {
        let palette = default_palette();
        let item = ReasoningMessageItem::new("先分析", ReasoningDisplayMode::Expanded, None, None);
        let lines = item.render_lines(80, palette);

        assert_eq!(
            lines.iter().map(line_to_plain_text).collect::<Vec<_>>(),
            vec!["先分析".to_string()]
        );
        assert_eq!(lines[0].spans[0].style.fg, tertiary_text_style(palette).fg);
        assert!(
            lines[0].spans[0]
                .style
                .add_modifier
                .contains(Modifier::ITALIC)
        );
        assert!(!item.is_header_line(0));
        assert_eq!(item.header_display_width(), 0);
    }

    #[test]
    fn reasoning_message_collapses_to_clickable_header() {
        let item = ReasoningMessageItem::new("先分析", ReasoningDisplayMode::Collapsed, None, None);
        let lines = item.render_lines(80, default_palette());

        assert_eq!(
            lines.iter().map(line_to_plain_text).collect::<Vec<_>>(),
            vec!["[Show reasoning]".to_string()]
        );
        assert!(item.is_header_line(0));
        assert!(!item.is_header_line(1));
    }

    #[test]
    fn reasoning_message_toggle_changes_rendered_lines_and_cache_key() {
        let mut item =
            ReasoningMessageItem::new("先分析", ReasoningDisplayMode::Collapsed, None, None);
        let collapsed_key = item.render_cache_key();

        item.toggle();

        assert_ne!(item.render_cache_key(), collapsed_key);
        assert_eq!(
            item.render_lines(80, default_palette())
                .iter()
                .map(line_to_plain_text)
                .collect::<Vec<_>>(),
            vec!["[Hide reasoning]".to_string(), "先分析".to_string()]
        );
    }

    #[test]
    fn reasoning_message_header_includes_duration_when_available() {
        let item = ReasoningMessageItem::new(
            "先分析",
            ReasoningDisplayMode::Collapsed,
            Some(Duration::from_secs(3)),
            None,
        );

        assert_eq!(
            item.render_lines(80, default_palette())
                .iter()
                .map(line_to_plain_text)
                .collect::<Vec<_>>(),
            vec!["[Show reasoning · thoughts 3s]".to_string()]
        );
    }

    #[test]
    fn toggled_reasoning_message_header_uses_subsecond_duration_label() {
        let mut item = ReasoningMessageItem::new(
            "先分析",
            ReasoningDisplayMode::Collapsed,
            Some(Duration::from_millis(120)),
            None,
        );

        item.toggle();

        assert_eq!(
            item.render_lines(80, default_palette())
                .iter()
                .map(line_to_plain_text)
                .collect::<Vec<_>>(),
            vec![
                "[Hide reasoning · thoughts <1s]".to_string(),
                "先分析".to_string()
            ]
        );
    }

    #[test]
    fn reasoning_message_snippet_renders_only_duration_hint() {
        let item = ReasoningMessageItem::new(
            "这段内容不能进入 transcript",
            ReasoningDisplayMode::Snippet,
            Some(Duration::from_secs(16)),
            None,
        );

        assert_eq!(
            item.render_lines(80, default_palette())
                .iter()
                .map(line_to_plain_text)
                .collect::<Vec<_>>(),
            vec!["• thoughts 16s".to_string()]
        );
        assert_eq!(item.source_text_byte_len(), 0);
        assert_eq!(item.header_display_width(), 0);
    }

    #[test]
    fn reasoning_message_snippet_uses_subsecond_duration_label() {
        let item = ReasoningMessageItem::new(
            "这段内容不能进入 transcript",
            ReasoningDisplayMode::Snippet,
            Some(Duration::from_millis(120)),
            None,
        );

        assert_eq!(
            item.render_lines(80, default_palette())
                .iter()
                .map(line_to_plain_text)
                .collect::<Vec<_>>(),
            vec!["• thoughts <1s".to_string()]
        );
    }

    #[test]
    fn reasoning_message_snippet_does_not_toggle_or_hash_content() {
        let mut item = ReasoningMessageItem::new(
            "第一段思维链",
            ReasoningDisplayMode::Snippet,
            Some(Duration::from_secs(3)),
            None,
        );
        let same_hint_item = ReasoningMessageItem::new(
            "第二段思维链",
            ReasoningDisplayMode::Snippet,
            Some(Duration::from_secs(3)),
            None,
        );
        let original_key = item.render_cache_key();

        item.toggle();

        assert_eq!(item.render_cache_key(), original_key);
        assert_eq!(item.render_cache_key(), same_hint_item.render_cache_key());
        assert_eq!(
            item.render_lines(80, default_palette())
                .iter()
                .map(line_to_plain_text)
                .collect::<Vec<_>>(),
            vec!["• thoughts 3s".to_string()]
        );
    }

    #[test]
    fn reasoning_render_cache_key_ignores_transcript_owned_working_directory() {
        let item_in_first_transcript = ReasoningMessageItem::new(
            "[report](reports/current.md)",
            ReasoningDisplayMode::Expanded,
            None,
            Some(Rc::from(PathBuf::from("/workspace/first"))),
        );
        let item_in_second_transcript = ReasoningMessageItem::new(
            "[report](reports/current.md)",
            ReasoningDisplayMode::Expanded,
            None,
            Some(Rc::from(PathBuf::from("/workspace/second"))),
        );

        assert_eq!(
            item_in_first_transcript.render_cache_key(),
            item_in_second_transcript.render_cache_key(),
            "working_dir is immutable transcript context, not reasoning item identity"
        );
    }

    #[test]
    fn reasoning_message_expanded_content_renders_markdown_with_reasoning_style() {
        let palette = default_palette();
        let item = ReasoningMessageItem::new(
            "# Plan\n\nUse **bold** and `code`.\n\n| Key | Value |\n| --- | --- |\n| a | b |",
            ReasoningDisplayMode::Expanded,
            None,
            None,
        );

        let lines = item.render_lines(80, palette);
        let plain_lines = lines.iter().map(line_to_plain_text).collect::<Vec<_>>();

        assert!(plain_lines.iter().any(|line| line == "# Plan"));
        assert!(
            plain_lines
                .iter()
                .any(|line| line.contains("Use bold and code.")),
            "reasoning Markdown 应移除行内 marker 并保留文本内容: {plain_lines:?}"
        );
        assert!(
            plain_lines
                .iter()
                .any(|line| line.contains("Key") && line.contains("Value")),
            "reasoning Markdown 应渲染 pipe table，而不是保持原始源码: {plain_lines:?}"
        );
        assert!(
            lines
                .iter()
                .flat_map(|line| line.spans.iter())
                .all(|span| span.style.fg == tertiary_text_style(palette).fg),
            "reasoning 外层颜色必须覆盖所有 Markdown span"
        );
        assert!(
            lines
                .iter()
                .flat_map(|line| line.spans.iter())
                .all(|span| span.style.add_modifier.contains(Modifier::ITALIC)),
            "reasoning 外层 italic 必须覆盖所有 Markdown span"
        );
        assert!(
            lines.iter().flat_map(|line| line.spans.iter()).any(|span| {
                span.content.as_ref() == "bold" && span.style.add_modifier.contains(Modifier::BOLD)
            }),
            "Markdown bold modifier 应在 reasoning 外层样式叠加后保留"
        );
    }

    #[test]
    fn reasoning_display_trims_outer_blank_lines_without_mutating_source_content() {
        let palette = default_palette();
        let source_content = "\n\nthink\n\n";

        for display_mode in [
            ReasoningDisplayMode::Expanded,
            ReasoningDisplayMode::ExpandedSimplified,
        ] {
            let item = ReasoningMessageItem::new(source_content, display_mode, None, None);

            assert_eq!(rendered_plain_lines(&item), vec!["think".to_string()]);
            assert_eq!(item.measure_render_metrics(80, palette), (1, "think".len()));
            assert_eq!(item.source_text_byte_len(), source_content.len());
        }
    }

    #[test]
    fn expanded_simplified_short_reasoning_renders_like_expanded() {
        let item = ReasoningMessageItem::new(
            "line 1\nline 2",
            ReasoningDisplayMode::ExpandedSimplified,
            None,
            None,
        );

        assert_eq!(
            item.render_lines(80, default_palette())
                .iter()
                .map(line_to_plain_text)
                .collect::<Vec<_>>(),
            vec!["line 1".to_string(), "line 2".to_string()]
        );
    }

    #[test]
    fn expanded_simplified_compacts_only_when_output_is_strictly_shorter() {
        let twelve_line_item = ReasoningMessageItem::new(
            numbered_lines(12),
            ReasoningDisplayMode::ExpandedSimplified,
            None,
            None,
        );

        for line_count in 9..=11 {
            let item = ReasoningMessageItem::new(
                numbered_lines(line_count),
                ReasoningDisplayMode::ExpandedSimplified,
                None,
                None,
            );
            assert_eq!(
                rendered_plain_lines(&item),
                numbered_plain_lines(line_count),
                "{line_count} 行内容 compact 后不会更短，主界面应保持完整展示"
            );
        }

        let twelve_lines = rendered_plain_lines(&twelve_line_item);

        assert_eq!(
            twelve_lines,
            vec![
                "line 1".to_string(),
                "line 2".to_string(),
                "line 3".to_string(),
                "line 4".to_string(),
                String::new(),
                "… +4 lines (ctrl + t to view transcript)".to_string(),
                String::new(),
                "line 9".to_string(),
                "line 10".to_string(),
                "line 11".to_string(),
                "line 12".to_string(),
            ],
            "12 行内容 compact 后比原文少 1 行，应进入简化展示"
        );
    }

    #[test]
    fn expanded_simplified_long_reasoning_compacts_in_default_render_mode() {
        let item = ReasoningMessageItem::new(
            (1..=14)
                .map(|line| format!("line {line}"))
                .collect::<Vec<_>>()
                .join("\n"),
            ReasoningDisplayMode::ExpandedSimplified,
            Some(Duration::from_secs(8)),
            None,
        );

        let plain_lines = item
            .render_lines(80, default_palette())
            .iter()
            .map(line_to_plain_text)
            .collect::<Vec<_>>();

        assert_eq!(
            plain_lines,
            vec![
                "line 1".to_string(),
                "line 2".to_string(),
                "line 3".to_string(),
                "line 4".to_string(),
                String::new(),
                "… +6 lines (ctrl + t to view transcript)".to_string(),
                String::new(),
                "line 11".to_string(),
                "line 12".to_string(),
                "line 13".to_string(),
                "line 14".to_string(),
            ]
        );
    }

    #[test]
    fn expanded_simplified_long_reasoning_renders_full_content_in_detailed_mode() {
        let mut item = ReasoningMessageItem::new(
            (1..=14)
                .map(|line| format!("line {line}"))
                .collect::<Vec<_>>()
                .join("\n"),
            ReasoningDisplayMode::ExpandedSimplified,
            Some(Duration::from_secs(8)),
            None,
        );

        assert!(item.set_render_mode(ReasoningRenderMode::Detailed));

        let plain_lines = item
            .render_lines(80, default_palette())
            .iter()
            .map(line_to_plain_text)
            .collect::<Vec<_>>();

        assert!(plain_lines.contains(&"line 7".to_string()));
        assert!(
            !plain_lines
                .iter()
                .any(|line| line.contains("ctrl + t to view transcript"))
        );
    }

    #[test]
    fn reasoning_message_markdown_profile_matches_codex_reasoning_summary() {
        let item = ReasoningMessageItem::new(
            "- [x] task\n\nenergy $E = mc^2$ now\n\n![diagram](image.png)\n\n<kbd>Ctrl</kbd>",
            ReasoningDisplayMode::Expanded,
            None,
            None,
        );

        let plain_text = item.render_plain_text(80, default_palette());

        assert!(
            plain_text.contains("[x] task"),
            "Reasoning Content 不启用 task list 扩展，应保留原始 marker"
        );
        assert!(
            plain_text.contains("$E = mc^2$"),
            "Reasoning Content 不启用 math 扩展，应保留 dollar-delimited 文本"
        );
        assert!(
            plain_text.contains("diagram"),
            "Image alt text 应作为普通文本保留"
        );
        assert!(
            !plain_text.contains("image.png"),
            "Image target 不应按 link 或媒体语义渲染"
        );
        assert!(
            plain_text.contains("<kbd>Ctrl</kbd>"),
            "HTML 应作为 literal text 渲染"
        );
    }

    fn numbered_lines(count: usize) -> String {
        numbered_plain_lines(count).join("\n")
    }

    fn numbered_plain_lines(count: usize) -> Vec<String> {
        (1..=count).map(|line| format!("line {line}")).collect()
    }

    fn rendered_plain_lines(item: &ReasoningMessageItem) -> Vec<String> {
        item.render_lines(80, default_palette())
            .iter()
            .map(line_to_plain_text)
            .collect()
    }
}
