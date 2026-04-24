use std::collections::VecDeque;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::prompt_wrap::wrap_prompt_visual_lines;

pub(crate) const DEFAULT_RENDER_WIDTH: usize = 80;

pub(super) const DISPLAY_TAB_WIDTH: usize = 8;
const ASSISTANT_LITERAL_COMMANDS: &[&str] = &[
    "go", "git", "make", "npm", "pnpm", "yarn", "uv", "python", "python3", "pip", "cargo",
    "docker", "kubectl", "curl", "wget", "bash", "sh",
];
const ASSISTANT_PROSE_LEAD_WORDS: &[&str] = &[
    "a", "an", "the", "this", "that", "these", "those", "to", "into", "from", "of", "for", "with",
    "without", "in", "on", "at", "by", "and", "or", "but", "sure", "contains", "contain",
    "returns", "return", "shows", "show", "opens", "open", "uses", "use", "sets", "set",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WrapMode {
    Assistant,
}

#[derive(Debug, Clone, Default)]
struct WrapSegment {
    text: String,
    width: usize,
    is_space: bool,
}

/// `wrap_prompt_text` 按 prompt transcript 的展示需求换行。
pub(crate) fn wrap_prompt_text(
    value: impl AsRef<str>,
    width: usize,
    line_prefix_width: usize,
) -> Vec<String> {
    wrap_prompt_visual_lines(value.as_ref(), width, line_prefix_width)
        .into_iter()
        .map(|line| line.text)
        .collect()
}

/// `wrap_assistant_text` 按 assistant transcript 的展示需求换行。
pub(crate) fn wrap_assistant_text(
    value: impl AsRef<str>,
    width: usize,
    line_prefix_width: usize,
) -> Vec<String> {
    wrap_text(
        value.as_ref(),
        width,
        line_prefix_width,
        WrapMode::Assistant,
    )
}

fn wrap_text(value: &str, width: usize, line_prefix_width: usize, mode: WrapMode) -> Vec<String> {
    if value.is_empty() {
        return vec![String::new()];
    }
    if width == 0 {
        return value.split('\n').map(ToOwned::to_owned).collect();
    }

    let mut logical_lines = Vec::new();
    let mut in_fence = false;
    let mut fence_marker = "";

    for raw_line in value.split('\n') {
        if mode == WrapMode::Assistant {
            if let Some(marker) = detect_fence_marker(raw_line) {
                logical_lines.extend(hard_wrap_line(raw_line, width, line_prefix_width));
                if in_fence && marker == fence_marker {
                    in_fence = false;
                    fence_marker = "";
                } else if !in_fence {
                    in_fence = true;
                    fence_marker = marker;
                }
                continue;
            }

            if in_fence {
                logical_lines.extend(hard_wrap_line(raw_line, width, line_prefix_width));
                continue;
            }
        }

        if raw_line.contains('\t') {
            logical_lines.extend(hard_wrap_line(raw_line, width, line_prefix_width));
            continue;
        }

        logical_lines.extend(wrap_display_line(raw_line, width, line_prefix_width, mode));
    }

    if logical_lines.is_empty() {
        return vec![String::new()];
    }

    logical_lines
}

fn wrap_display_line(
    line: &str,
    width: usize,
    line_prefix_width: usize,
    mode: WrapMode,
) -> Vec<String> {
    if line.is_empty() || width == 0 {
        return vec![line.to_string()];
    }

    if should_hard_wrap_line(line, mode) {
        return hard_wrap_line(line, width, line_prefix_width);
    }

    let (prefix, remainder) = split_short_indent(line);
    if !prefix.is_empty() {
        return wrap_indented_prose_line(prefix, remainder, width, line_prefix_width, mode);
    }

    wrap_prose_line(line, width, mode)
}

fn should_hard_wrap_line(line: &str, mode: WrapMode) -> bool {
    if leading_space_count(line) >= 4 {
        return true;
    }

    mode == WrapMode::Assistant && looks_like_assistant_literal_line(line)
}

pub(in crate::frontend::tui::transcript) fn split_short_indent(line: &str) -> (String, &str) {
    let prefix_len = line
        .chars()
        .take_while(|character| *character == ' ')
        .count();
    if prefix_len == 0 || prefix_len >= 4 {
        return (String::new(), line);
    }

    let byte_len = line
        .char_indices()
        .nth(prefix_len)
        .map(|(byte_index, _)| byte_index)
        .unwrap_or(line.len());

    (line[..byte_len].to_string(), &line[byte_len..])
}

fn wrap_indented_prose_line(
    prefix: String,
    remainder: &str,
    width: usize,
    line_prefix_width: usize,
    mode: WrapMode,
) -> Vec<String> {
    let prefix_width = measure_width(&prefix);
    if prefix_width >= width || remainder.is_empty() {
        return hard_wrap_line(&(prefix + remainder), width, line_prefix_width);
    }

    let mut lines = wrap_prose_segments(
        tokenize_wrap_segments(remainder),
        width - prefix_width,
        width,
        mode,
    );
    if let Some(first_line) = lines.first_mut() {
        first_line.insert_str(0, &prefix);
    }
    lines
}

fn wrap_prose_line(line: &str, width: usize, mode: WrapMode) -> Vec<String> {
    if line.is_empty() || width == 0 {
        return vec![line.to_string()];
    }

    wrap_prose_segments(tokenize_wrap_segments(line), width, width, mode)
}

fn wrap_prose_segments(
    segments: Vec<WrapSegment>,
    first_width: usize,
    continuation_width: usize,
    mode: WrapMode,
) -> Vec<String> {
    if segments.is_empty() {
        return vec![String::new()];
    }

    let mut cursor = SegmentCursor::new(segments);
    let mut lines = Vec::new();
    let mut current_width = if first_width == 0 {
        continuation_width
    } else {
        first_width
    };

    while cursor.has_more() {
        lines.push(consume_prose_line(&mut cursor, current_width, mode));
        current_width = continuation_width;
    }

    lines
}

fn consume_prose_line(cursor: &mut SegmentCursor, width: usize, mode: WrapMode) -> String {
    let mut current = String::new();
    let mut current_width = 0;
    let mut pending_spaces = WrapSegment::default();

    while let Some(segment) = cursor.next() {
        if segment.is_space {
            if current_width == 0 {
                if segment.width <= width {
                    current.push_str(&segment.text);
                    current_width += segment.width;
                    continue;
                }

                let (fitted, overflow) = split_segment_to_width(segment, width);
                current.push_str(&fitted.text);
                if !overflow.text.is_empty() {
                    cursor.push_front([overflow]);
                }
                return current;
            }

            pending_spaces.text.push_str(&segment.text);
            pending_spaces.width += segment.width;
            pending_spaces.is_space = true;
            continue;
        }

        if current_width == 0 {
            if segment.width <= width {
                current.push_str(&segment.text);
                current_width += segment.width;
                continue;
            }

            let (fitted, overflow) = split_segment_to_width(segment, width);
            current.push_str(&fitted.text);
            if !overflow.text.is_empty() {
                cursor.push_front([overflow]);
            }
            return current;
        }

        if current_width + pending_spaces.width + segment.width <= width {
            current.push_str(&pending_spaces.text);
            current.push_str(&segment.text);
            current_width += pending_spaces.width + segment.width;
            pending_spaces = WrapSegment::default();
            continue;
        }

        if !pending_spaces.text.is_empty()
            && should_preserve_wrapped_spaces(mode, pending_spaces.width)
        {
            cursor.push_front([pending_spaces, segment]);
            return current;
        }

        cursor.push_front([segment]);
        return current;
    }

    if !pending_spaces.text.is_empty() {
        if current_width + pending_spaces.width <= width {
            current.push_str(&pending_spaces.text);
            return current;
        }

        cursor.push_front([pending_spaces]);
    }

    current
}

fn should_preserve_wrapped_spaces(mode: WrapMode, pending_space_width: usize) -> bool {
    mode == WrapMode::Assistant && pending_space_width > 1
}

fn tokenize_wrap_segments(line: &str) -> Vec<WrapSegment> {
    if line.is_empty() {
        return Vec::new();
    }

    let mut segments = Vec::new();
    let mut current = String::new();
    let mut current_width = 0;
    let mut current_is_space = false;
    let mut has_current = false;

    let flush_current = |segments: &mut Vec<WrapSegment>,
                         current: &mut String,
                         current_width: &mut usize,
                         current_is_space: &mut bool,
                         has_current: &mut bool| {
        if !*has_current {
            return;
        }

        segments.push(WrapSegment {
            text: std::mem::take(current),
            width: *current_width,
            is_space: *current_is_space,
        });
        *current_width = 0;
        *has_current = false;
    };

    for cluster in UnicodeSegmentation::graphemes(line, true) {
        let cluster_is_space = is_space_cluster(cluster);
        if !has_current {
            has_current = true;
            current_is_space = cluster_is_space;
        } else if current_is_space != cluster_is_space {
            flush_current(
                &mut segments,
                &mut current,
                &mut current_width,
                &mut current_is_space,
                &mut has_current,
            );
            has_current = true;
            current_is_space = cluster_is_space;
        }

        current.push_str(cluster);
        current_width += measure_width(cluster);
    }

    flush_current(
        &mut segments,
        &mut current,
        &mut current_width,
        &mut current_is_space,
        &mut has_current,
    );

    segments
}

#[derive(Debug, Clone)]
struct SegmentCursor {
    segments: Vec<WrapSegment>,
    buffered: VecDeque<WrapSegment>,
    index: usize,
}

impl SegmentCursor {
    fn new(segments: Vec<WrapSegment>) -> Self {
        Self {
            segments,
            buffered: VecDeque::new(),
            index: 0,
        }
    }

    fn has_more(&self) -> bool {
        !self.buffered.is_empty() || self.index < self.segments.len()
    }

    fn next(&mut self) -> Option<WrapSegment> {
        if let Some(segment) = self.buffered.pop_front() {
            return Some(segment);
        }

        let segment = self.segments.get(self.index)?.clone();
        self.index += 1;
        Some(segment)
    }

    fn push_front<const N: usize>(&mut self, segments: [WrapSegment; N]) {
        for segment in segments.into_iter().rev() {
            if !segment.text.is_empty() {
                self.buffered.push_front(segment);
            }
        }
    }
}

fn split_segment_to_width(segment: WrapSegment, width: usize) -> (WrapSegment, WrapSegment) {
    let (fitted_text, overflow_text) = split_text_to_width(&segment.text, width);

    (
        WrapSegment {
            width: measure_width(&fitted_text),
            text: fitted_text,
            is_space: segment.is_space,
        },
        WrapSegment {
            width: measure_width(&overflow_text),
            text: overflow_text,
            is_space: segment.is_space,
        },
    )
}

fn hard_wrap_line(line: &str, width: usize, line_prefix_width: usize) -> Vec<String> {
    if line.is_empty() || width == 0 {
        return vec![line.to_string()];
    }

    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0;

    let flush_current =
        |lines: &mut Vec<String>, current: &mut String, current_width: &mut usize| {
            lines.push(std::mem::take(current));
            *current_width = 0;
        };

    for cluster in UnicodeSegmentation::graphemes(line, true) {
        let mut remaining = cluster.to_string();
        while !remaining.is_empty() {
            if current_width >= width && current_width > 0 {
                flush_current(&mut lines, &mut current, &mut current_width);
            }

            let (rendered_cluster, _) =
                render_cluster_for_display(&remaining, line_prefix_width + current_width);
            let available_width = width.saturating_sub(current_width);
            let (fitted, overflow) = split_text_to_width(&rendered_cluster, available_width);

            current.push_str(&fitted);
            current_width += measure_width(&fitted);

            if overflow.is_empty() {
                break;
            }

            flush_current(&mut lines, &mut current, &mut current_width);
            if cluster == "\t" {
                remaining = overflow;
            } else {
                remaining.clear();
            }
        }
    }

    if !current.is_empty() || lines.is_empty() {
        flush_current(&mut lines, &mut current, &mut current_width);
    }

    lines
}

pub(super) fn split_text_to_width(text: &str, width: usize) -> (String, String) {
    if text.is_empty() || width == 0 {
        return (String::new(), text.to_string());
    }

    let mut fitted = String::new();
    let mut current_width = 0;
    let mut byte_offset = 0;

    for cluster in UnicodeSegmentation::graphemes(text, true) {
        let cluster_width = measure_width(cluster);
        if current_width + cluster_width > width && current_width > 0 {
            break;
        }

        fitted.push_str(cluster);
        current_width += cluster_width;
        byte_offset += cluster.len();
    }

    if byte_offset == 0 {
        return (text.to_string(), String::new());
    }

    (fitted, text[byte_offset..].to_string())
}

pub(super) fn leading_space_count(line: &str) -> usize {
    line.chars()
        .take_while(|character| *character == ' ')
        .count()
}

pub(in crate::frontend::tui::transcript) fn looks_like_assistant_literal_line(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }

    if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
        return true;
    }

    if has_assistant_literal_command_prefix(trimmed) {
        return true;
    }

    if has_likely_aligned_columns(trimmed) {
        return true;
    }

    if trimmed.contains(":=")
        || trimmed.contains(" == ")
        || trimmed.contains(" != ")
        || trimmed.contains(" <= ")
        || trimmed.contains(" >= ")
        || trimmed.contains(" => ")
        || trimmed.contains(" -> ")
    {
        return true;
    }

    trimmed.ends_with('{') || trimmed.ends_with('}')
}

fn has_assistant_literal_command_prefix(line: &str) -> bool {
    let mut fields = line.split_whitespace();
    let Some(command) = fields.next() else {
        return false;
    };
    let arguments = fields.collect::<Vec<_>>();

    if command.starts_with("./") || command.starts_with("../") {
        return arguments.is_empty() || looks_like_literal_command_arguments(&arguments);
    }

    if !is_known_assistant_command(command) {
        return false;
    }

    arguments.is_empty() || looks_like_literal_command_arguments(&arguments)
}

fn is_known_assistant_command(command: &str) -> bool {
    ASSISTANT_LITERAL_COMMANDS.contains(&command)
}

fn looks_like_literal_command_arguments(arguments: &[&str]) -> bool {
    if arguments.is_empty() {
        return false;
    }

    let first = normalize_command_token(arguments[0]);
    if first.is_empty() || is_assistant_prose_lead_word(&first) {
        return false;
    }

    if looks_shell_like_token(arguments[0]) {
        return true;
    }

    if arguments.len() > 1 {
        let second = normalize_command_token(arguments[1]);
        if is_assistant_prose_lead_word(&second) {
            return false;
        }
    }

    is_simple_command_token(&first)
}

fn is_assistant_prose_lead_word(token: &str) -> bool {
    ASSISTANT_PROSE_LEAD_WORDS.contains(&token)
}

fn has_likely_aligned_columns(line: &str) -> bool {
    let mut space_runs = 0;
    let mut current_run = 0;

    for character in line.chars() {
        if character == ' ' {
            current_run += 1;
            continue;
        }

        if current_run >= 2 {
            space_runs += 1;
            if space_runs >= 2 {
                return true;
            }
        }
        current_run = 0;
    }

    current_run >= 2 && space_runs >= 1
}

fn looks_shell_like_token(token: &str) -> bool {
    token.starts_with('-')
        || token.starts_with("./")
        || token.starts_with("../")
        || token.starts_with('/')
        || token.starts_with('~')
        || token.contains('=')
        || token.contains('/')
        || token.contains(':')
        || token.contains('|')
        || token.contains('>')
        || token.contains('<')
}

fn is_simple_command_token(token: &str) -> bool {
    !token.is_empty()
        && token.chars().all(|character| {
            character.is_alphanumeric() || character == '_' || character == '-' || character == '.'
        })
}

fn normalize_command_token(token: &str) -> String {
    token
        .trim()
        .trim_matches(|character| "\"'`()[]{}.,:;".contains(character))
        .to_ascii_lowercase()
}

pub(in crate::frontend::tui::transcript) fn render_cluster_for_display(
    cluster: &str,
    absolute_column: usize,
) -> (String, usize) {
    if cluster != "\t" {
        return (cluster.to_string(), measure_width(cluster));
    }

    let tab_width = display_tab_width(absolute_column);
    (" ".repeat(tab_width), tab_width)
}

pub(crate) fn display_tab_width(absolute_column: usize) -> usize {
    let mut tab_width = DISPLAY_TAB_WIDTH - (absolute_column % DISPLAY_TAB_WIDTH);
    if tab_width == 0 {
        tab_width = DISPLAY_TAB_WIDTH;
    }

    tab_width
}

pub(in crate::frontend::tui::transcript) fn detect_fence_marker(
    line: &str,
) -> Option<&'static str> {
    let trimmed = line.trim();
    if trimmed.starts_with("```") {
        return Some("```");
    }
    if trimmed.starts_with("~~~") {
        return Some("~~~");
    }
    None
}

pub(super) fn is_space_cluster(cluster: &str) -> bool {
    !cluster.is_empty() && cluster.chars().all(char::is_whitespace)
}

pub(super) fn measure_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

#[cfg(test)]
mod tests {
    use super::{wrap_assistant_text, wrap_prompt_text};

    #[test]
    fn wrap_prompt_text_word_wraps_plain_prose() {
        assert_eq!(
            wrap_prompt_text("hello world", 6, 0),
            vec!["hello", "world"]
        );
    }

    #[test]
    fn wrap_prompt_text_keeps_leading_space_but_not_mid_word_hard_wrap() {
        assert_eq!(
            wrap_prompt_text(" Hello world", 8, 0),
            vec![" Hello", "world"]
        );
    }

    #[test]
    fn wrap_prompt_text_preserves_wrapped_double_spaces() {
        assert_eq!(
            wrap_prompt_text("Hello.  Another sentence", 10, 0),
            vec!["Hello.", "  Another", "sentence"]
        );
    }

    #[test]
    fn wrap_prompt_text_uses_full_width_after_short_indent() {
        assert_eq!(
            wrap_prompt_text(" - nested list item that wraps", 10, 0),
            vec![" - nested", "list item", "that wraps"]
        );
    }

    #[test]
    fn wrap_prompt_text_reflows_boundary_word_when_trailing_space_overflows() {
        assert_eq!(
            wrap_prompt_text("aaaaaaaaaaaaaaaaaa b ", 20, 0),
            vec!["aaaaaaaaaaaaaaaaaa", "b "]
        );
    }

    #[test]
    fn wrap_prompt_text_preserves_long_leading_spaces_on_wrapped_line() {
        assert_eq!(wrap_prompt_text("abc d    e", 5, 0), vec!["abc d", "    e"]);
    }

    #[test]
    fn wrap_assistant_text_preserves_indented_code() {
        assert_eq!(wrap_assistant_text("    code", 6, 0), vec!["    co", "de"]);
    }

    #[test]
    fn wrap_assistant_text_preserves_wrapped_spaces() {
        assert_eq!(wrap_assistant_text("foo  bar", 5, 0), vec!["foo", "  bar"]);
    }

    #[test]
    fn wrap_assistant_text_does_not_add_blank_line_before_wide_glyph() {
        assert_eq!(wrap_assistant_text("中", 1, 0), vec!["中"]);
    }

    #[test]
    fn wrap_prompt_text_expands_tabs_for_display_width() {
        assert_eq!(wrap_prompt_text("a\tb", 9, 0), vec!["a       b"]);
    }

    #[test]
    fn wrap_assistant_text_keeps_command_lines_literal() {
        assert_eq!(
            wrap_assistant_text("go test ./...", 6, 0),
            vec!["go tes", "t ./..", "."]
        );
    }

    #[test]
    fn wrap_assistant_text_keeps_code_lines_literal() {
        assert_eq!(
            wrap_assistant_text("if err != nil {", 6, 0),
            vec!["if err", " != ni", "l {"]
        );
    }

    #[test]
    fn wrap_assistant_text_keeps_fenced_block_literal() {
        assert_eq!(
            wrap_assistant_text("```\nhello world\n```", 6, 0),
            vec!["```", "hello ", "world", "```"]
        );
    }

    #[test]
    fn wrap_assistant_text_treats_markdown_heading_as_prose() {
        assert_eq!(
            wrap_assistant_text("# Overview of the API", 8, 0),
            vec!["#", "Overview", "of the", "API"]
        );
    }

    #[test]
    fn wrap_assistant_text_keeps_path_mentions_on_prose_path() {
        assert_eq!(
            wrap_assistant_text("Open ./cmd/main.go and rerun", 8, 0),
            vec!["Open", "./cmd/ma", "in.go", "and", "rerun"]
        );
    }

    #[test]
    fn wrap_assistant_text_treats_leading_path_explanation_as_prose() {
        assert_eq!(
            wrap_assistant_text("./cmd/main.go contains the entry point", 20, 0),
            vec!["./cmd/main.go", "contains the entry", "point"]
        );
    }

    #[test]
    fn wrap_assistant_text_keeps_parenthesized_prose_on_word_wrap_path() {
        assert_eq!(
            wrap_assistant_text("Use the cache (when warm)", 10, 0),
            vec!["Use the", "cache", "(when", "warm)"]
        );
    }

    #[test]
    fn wrap_assistant_text_treats_leading_make_explanation_as_prose() {
        assert_eq!(
            wrap_assistant_text("make the handler return early", 20, 0),
            vec!["make the handler", "return early"]
        );
    }

    #[test]
    fn wrap_assistant_text_treats_single_triple_space_run_as_prose() {
        assert_eq!(
            wrap_assistant_text("This   example should stay prose", 12, 0),
            vec!["This", "   example", "should stay", "prose"]
        );
    }

    #[test]
    fn wrap_prompt_text_accounts_for_prefix_width_when_expanding_tabs() {
        assert_eq!(wrap_prompt_text("a\tb", 8, 2), vec!["a     b"]);
    }

    #[test]
    fn wrap_prompt_text_splits_expanded_tabs_across_narrow_lines() {
        assert_eq!(
            wrap_prompt_text("a\tb", 1, 2),
            vec!["a", " ", " ", " ", " ", " ", " ", "b"]
        );
    }
}
