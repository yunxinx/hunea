use std::{borrow::Cow, ops::Range};

#[derive(Clone, Copy)]
struct Fence {
    marker: char,
    length: usize,
    is_blockquoted: bool,
}

/// 去掉包含表格的 `md` / `markdown` fence，让 parser 按原生表格处理。
///
/// 该处理是保守的：只有 markdown fence 内出现连续 header + delimiter
/// 表格结构时才 unwrap；其它 fence、非表格 markdown fence、未闭合 fence 均保持原样。
pub(crate) fn unwrap_markdown_table_fences(markdown_source: &str) -> Cow<'_, str> {
    if !markdown_source.contains("```") && !markdown_source.contains("~~~") {
        return Cow::Borrowed(markdown_source);
    }

    struct MarkdownCandidateData {
        fence: Fence,
        opening_range: Range<usize>,
        content_ranges: Vec<Range<usize>>,
    }

    enum ActiveFence {
        Passthrough(Fence),
        MarkdownCandidate(Box<MarkdownCandidateData>),
    }

    let mut output = String::with_capacity(markdown_source.len());
    let mut active_fence = None;
    let mut source_offset = 0usize;

    let mut push_source_range = |range: Range<usize>| {
        if !range.is_empty() {
            output.push_str(&markdown_source[range]);
        }
    };

    for line in markdown_source.split_inclusive('\n') {
        let line_start = source_offset;
        source_offset += line.len();
        let line_range = line_start..source_offset;

        if let Some(active) = active_fence.take() {
            match active {
                ActiveFence::Passthrough(fence) => {
                    push_source_range(line_range);
                    if !is_close_fence(line, fence) {
                        active_fence = Some(ActiveFence::Passthrough(fence));
                    }
                }
                ActiveFence::MarkdownCandidate(mut data) => {
                    if is_close_fence(line, data.fence) {
                        let content = content_from_ranges(markdown_source, &data.content_ranges);
                        if markdown_fence_contains_table(&content, data.fence.is_blockquoted) {
                            for range in data.content_ranges {
                                push_source_range(range);
                            }
                        } else {
                            push_source_range(data.opening_range);
                            for range in data.content_ranges {
                                push_source_range(range);
                            }
                            push_source_range(line_range);
                        }
                    } else {
                        data.content_ranges.push(line_range);
                        active_fence = Some(ActiveFence::MarkdownCandidate(data));
                    }
                }
            }
            continue;
        }

        if let Some((fence, is_markdown)) = parse_open_fence(line) {
            if is_markdown {
                active_fence = Some(ActiveFence::MarkdownCandidate(Box::new(
                    MarkdownCandidateData {
                        fence,
                        opening_range: line_range,
                        content_ranges: Vec::new(),
                    },
                )));
            } else {
                push_source_range(line_range);
                active_fence = Some(ActiveFence::Passthrough(fence));
            }
            continue;
        }

        push_source_range(line_range);
    }

    if let Some(active) = active_fence {
        match active {
            ActiveFence::Passthrough(_) => {}
            ActiveFence::MarkdownCandidate(data) => {
                push_source_range(data.opening_range);
                for range in data.content_ranges {
                    push_source_range(range);
                }
            }
        }
    }

    Cow::Owned(output)
}

fn strip_line_indent(line: &str) -> Option<&str> {
    let without_newline = line.strip_suffix('\n').unwrap_or(line);
    let mut byte_index = 0usize;
    let mut column = 0usize;

    for byte in without_newline.as_bytes() {
        match byte {
            b' ' => {
                byte_index += 1;
                column += 1;
            }
            b'\t' => {
                byte_index += 1;
                column += 4;
            }
            _ => break,
        }

        if column >= 4 {
            return None;
        }
    }

    Some(&without_newline[byte_index..])
}

fn parse_open_fence(line: &str) -> Option<(Fence, bool)> {
    let trimmed = strip_line_indent(line)?;
    let is_blockquoted = trimmed.trim_start().starts_with('>');
    let fence_scan_text = strip_blockquote_prefix(trimmed);
    let (marker, length) = parse_fence_marker(fence_scan_text)?;
    let is_markdown = is_markdown_fence_info(fence_scan_text, length);

    Some((
        Fence {
            marker,
            length,
            is_blockquoted,
        },
        is_markdown,
    ))
}

fn is_close_fence(line: &str, fence: Fence) -> bool {
    let Some(trimmed) = strip_line_indent(line) else {
        return false;
    };
    let fence_scan_text = if fence.is_blockquoted {
        if !trimmed.trim_start().starts_with('>') {
            return false;
        }
        strip_blockquote_prefix(trimmed)
    } else {
        trimmed
    };

    parse_fence_marker(fence_scan_text).is_some_and(|(marker, length)| {
        marker == fence.marker
            && length >= fence.length
            && fence_scan_text[length..].trim().is_empty()
    })
}

fn markdown_fence_contains_table(content: &str, is_blockquoted_fence: bool) -> bool {
    let mut previous_line = None;

    for line in content.lines() {
        let text = if is_blockquoted_fence {
            strip_blockquote_prefix(line)
        } else {
            line
        };
        let trimmed = text.trim();
        if trimmed.is_empty() {
            previous_line = None;
            continue;
        }

        if let Some(previous) = previous_line
            && is_table_header_line(previous)
            && !is_table_delimiter_line(previous)
            && is_table_delimiter_line(trimmed)
        {
            return true;
        }

        previous_line = Some(trimmed);
    }

    false
}

fn content_from_ranges(source: &str, ranges: &[Range<usize>]) -> String {
    let total_len = ranges
        .iter()
        .map(|range| range.end.saturating_sub(range.start))
        .sum();
    let mut content = String::with_capacity(total_len);
    for range in ranges {
        content.push_str(&source[range.start..range.end]);
    }
    content
}

/// 将一行 pipe-table 源文本拆成去除首尾空白的 cell 片段。
///
/// 该函数只做结构识别，不消费转义字符；`\|` 会保留在片段内。
pub(crate) fn parse_table_segments(line: &str) -> Option<Vec<&str>> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    let has_outer_pipe = trimmed.starts_with('|') || trimmed.ends_with('|');
    let content = trimmed.strip_prefix('|').unwrap_or(trimmed);
    let content = content.strip_suffix('|').unwrap_or(content);
    let raw_segments = split_unescaped_pipe(content);
    if !has_outer_pipe && raw_segments.len() <= 1 {
        return None;
    }

    let segments = raw_segments.into_iter().map(str::trim).collect::<Vec<_>>();
    (!segments.is_empty()).then_some(segments)
}

fn split_unescaped_pipe(content: &str) -> Vec<&str> {
    let mut segments = Vec::with_capacity(8);
    let mut start = 0;
    let bytes = content.as_bytes();
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'\\' {
            index += 2;
        } else if bytes[index] == b'|' {
            segments.push(&content[start..index]);
            start = index + 1;
            index += 1;
        } else {
            index += 1;
        }
    }

    segments.push(&content[start..]);
    segments
}

/// 判断一行是否具备 GFM table header 的结构形状。
pub(crate) fn is_table_header_line(line: &str) -> bool {
    parse_table_segments(line)
        .is_some_and(|segments| segments.iter().any(|segment| !segment.is_empty()))
}

/// 判断一行是否是 GFM table delimiter 行。
pub(crate) fn is_table_delimiter_line(line: &str) -> bool {
    parse_table_segments(line)
        .is_some_and(|segments| segments.into_iter().all(is_table_delimiter_segment))
}

/// 判断 Markdown 源文本中是否包含可确认的 pipe-table 结构。
pub(crate) fn contains_table_structure(markdown: &str) -> bool {
    let mut tracker = FenceTracker::new();
    let mut previous_candidate = None;

    for raw_line in markdown.lines() {
        if tracker.kind() == FenceKind::Other {
            previous_candidate = None;
            tracker.advance(raw_line);
            continue;
        }

        let candidate = strip_blockquote_prefix(raw_line);
        if previous_candidate.is_some_and(is_table_header_line)
            && is_table_delimiter_line(candidate)
        {
            return true;
        }

        previous_candidate = Some(candidate);
        tracker.advance(raw_line);
    }

    false
}

fn is_table_delimiter_segment(segment: &str) -> bool {
    let trimmed = segment.trim();
    if trimmed.is_empty() {
        return false;
    }

    let without_leading_colon = trimmed.strip_prefix(':').unwrap_or(trimmed);
    let core = without_leading_colon
        .strip_suffix(':')
        .unwrap_or(without_leading_colon);

    core.len() >= 3 && core.chars().all(|character| character == '-')
}

/// 描述当前源码行处于哪类 fenced code block 语境。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FenceKind {
    /// 不在 fenced code block 内。
    Outside,
    /// 在 `md` / `markdown` fenced code block 内。
    Markdown,
    /// 在其它语言 fenced code block 内。
    Other,
}

/// 增量跟踪 fenced code block，供 table source scanner 避免误判代码中的 `|`。
pub(crate) struct FenceTracker {
    state: Option<(char, usize, FenceKind)>,
}

impl FenceTracker {
    /// 创建空的 fence tracker。
    pub(crate) fn new() -> Self {
        Self { state: None }
    }

    /// 消费一行源码并更新 fence 状态。
    pub(crate) fn advance(&mut self, raw_line: &str) {
        let leading_spaces = raw_line
            .as_bytes()
            .iter()
            .take_while(|byte| **byte == b' ')
            .count();
        if leading_spaces > 3 {
            return;
        }

        let trimmed = &raw_line[leading_spaces..];
        let fence_scan_text = strip_blockquote_prefix(trimmed);
        let Some((marker, length)) = parse_fence_marker(fence_scan_text) else {
            return;
        };

        if let Some((open_marker, open_length, _)) = self.state {
            if marker == open_marker
                && length >= open_length
                && fence_scan_text[length..].trim().is_empty()
            {
                self.state = None;
            }
        } else {
            let kind = if is_markdown_fence_info(fence_scan_text, length) {
                FenceKind::Markdown
            } else {
                FenceKind::Other
            };
            self.state = Some((marker, length, kind));
        }
    }

    /// 返回当前 fence 语境。
    pub(crate) fn kind(&self) -> FenceKind {
        self.state.map_or(FenceKind::Outside, |(_, _, kind)| kind)
    }
}

/// 解析一行开头的 backtick / tilde fence 标记。
pub(crate) fn parse_fence_marker(line: &str) -> Option<(char, usize)> {
    let first = line.as_bytes().first().copied()?;
    if first != b'`' && first != b'~' {
        return None;
    }

    let length = line.bytes().take_while(|byte| *byte == first).count();
    (length >= 3).then_some((first as char, length))
}

/// 判断 fence info string 是否表示 Markdown 内容。
pub(crate) fn is_markdown_fence_info(trimmed_line: &str, marker_length: usize) -> bool {
    let info = trimmed_line[marker_length..]
        .split_whitespace()
        .next()
        .unwrap_or_default();
    info.eq_ignore_ascii_case("md") || info.eq_ignore_ascii_case("markdown")
}

/// 去掉一行开头的嵌套 blockquote 标记。
pub(crate) fn strip_blockquote_prefix(line: &str) -> &str {
    let mut rest = line.trim_start();
    loop {
        let Some(stripped) = rest.strip_prefix('>') else {
            return rest;
        };
        rest = stripped.strip_prefix(' ').unwrap_or(stripped).trim_start();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_table_segments_handles_outer_pipes() {
        assert_eq!(
            parse_table_segments("| A | B | C |"),
            Some(vec!["A", "B", "C"])
        );
    }

    #[test]
    fn parse_table_segments_accepts_missing_outer_pipes() {
        assert_eq!(parse_table_segments("A | B | C"), Some(vec!["A", "B", "C"]));
        assert_eq!(
            parse_table_segments("A | B | C |"),
            Some(vec!["A", "B", "C"])
        );
        assert_eq!(
            parse_table_segments("| A | B | C"),
            Some(vec!["A", "B", "C"])
        );
    }

    #[test]
    fn parse_table_segments_ignores_escaped_pipes() {
        assert_eq!(
            parse_table_segments(r"| A \| B | C |"),
            Some(vec![r"A \| B", "C"])
        );
    }

    #[test]
    fn parse_table_segments_rejects_plain_text_without_separator() {
        assert_eq!(parse_table_segments("just text"), None);
        assert_eq!(parse_table_segments("   "), None);
    }

    #[test]
    fn table_header_and_delimiter_detection_follow_gfm_shape() {
        assert!(is_table_header_line("| Name | Value |"));
        assert!(is_table_header_line("Name | Value"));
        assert!(!is_table_header_line("| | |"));

        assert!(is_table_delimiter_line("| --- | :---: | ---: |"));
        assert!(is_table_delimiter_line("--- | --- | ---"));
        assert!(!is_table_delimiter_line("| -- | --- |"));
        assert!(!is_table_delimiter_line("| Name | Value |"));
    }

    #[test]
    fn contains_table_structure_ignores_non_markdown_fences() {
        let markdown = "```rust\n| not | a table |\n| --- | --- |\n```\n\nName | Value\n--- | ---";

        assert!(contains_table_structure(markdown));
        assert!(!contains_table_structure(
            "```rust\n| not | a table |\n| --- | --- |\n```"
        ));
    }

    #[test]
    fn unwrap_markdown_table_fences_removes_markdown_fence_with_table() {
        let markdown = "```markdown\n| A | B |\n|---|---|\n| 1 | 2 |\n```\n";

        assert_eq!(
            unwrap_markdown_table_fences(markdown),
            "| A | B |\n|---|---|\n| 1 | 2 |\n"
        );
    }

    #[test]
    fn unwrap_markdown_table_fences_keeps_non_table_markdown_fence() {
        let markdown = "```markdown\n**bold**\n```\n";

        assert_eq!(unwrap_markdown_table_fences(markdown), markdown);
    }

    #[test]
    fn unwrap_markdown_table_fences_keeps_non_markdown_fence_with_table() {
        let markdown = "```rust\n| A | B |\n|---|---|\n| 1 | 2 |\n```\n";

        assert_eq!(unwrap_markdown_table_fences(markdown), markdown);
    }

    #[test]
    fn unwrap_markdown_table_fences_keeps_unclosed_markdown_fence() {
        let markdown = "```md\n| A | B |\n|---|---|\n| 1 | 2 |\n";

        assert_eq!(unwrap_markdown_table_fences(markdown), markdown);
    }

    #[test]
    fn unwrap_markdown_table_fences_keeps_blockquote_table_example_inside_plain_fence() {
        let markdown = "```markdown\n> | A | B |\n> |---|---|\n> | 1 | 2 |\n```\n";

        assert_eq!(unwrap_markdown_table_fences(markdown), markdown);
    }

    #[test]
    fn unwrap_markdown_table_fences_removes_blockquoted_markdown_fence_with_table() {
        let markdown = "> ```markdown\n> | A | B |\n> |---|---|\n> | 1 | 2 |\n> ```\n";

        assert_eq!(
            unwrap_markdown_table_fences(markdown),
            "> | A | B |\n> |---|---|\n> | 1 | 2 |\n"
        );
    }

    #[test]
    fn strip_blockquote_prefix_peels_nested_markers() {
        assert_eq!(strip_blockquote_prefix("> | A | B |"), "| A | B |");
        assert_eq!(strip_blockquote_prefix("> > | A | B |"), "| A | B |");
        assert_eq!(strip_blockquote_prefix("  >  > text"), "text");
    }

    #[test]
    fn fence_tracker_classifies_markdown_fences() {
        let mut tracker = FenceTracker::new();

        assert_eq!(tracker.kind(), FenceKind::Outside);
        tracker.advance("```md");
        assert_eq!(tracker.kind(), FenceKind::Markdown);
        tracker.advance("| A | B |");
        assert_eq!(tracker.kind(), FenceKind::Markdown);
        tracker.advance("```");
        assert_eq!(tracker.kind(), FenceKind::Outside);
    }

    #[test]
    fn fence_tracker_classifies_non_markdown_fences() {
        let mut tracker = FenceTracker::new();

        tracker.advance("```rust");
        assert_eq!(tracker.kind(), FenceKind::Other);
        tracker.advance("| not | markdown |");
        assert_eq!(tracker.kind(), FenceKind::Other);
        tracker.advance("```");
        assert_eq!(tracker.kind(), FenceKind::Outside);
    }

    #[test]
    fn fence_tracker_handles_blockquoted_fences() {
        let mut tracker = FenceTracker::new();

        tracker.advance("> ```markdown");
        assert_eq!(tracker.kind(), FenceKind::Markdown);
        tracker.advance("> ```");
        assert_eq!(tracker.kind(), FenceKind::Outside);
    }

    #[test]
    fn fence_tracker_ignores_deeply_indented_fences() {
        let mut tracker = FenceTracker::new();

        tracker.advance("    ```markdown");
        assert_eq!(tracker.kind(), FenceKind::Outside);
    }
}
