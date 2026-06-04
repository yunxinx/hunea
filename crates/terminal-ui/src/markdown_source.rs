/// Markdown 源文本的外层空白行与正文边界。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MarkdownSourceBounds {
    /// 源文本开头连续空白行数量。
    pub(crate) leading_blank_lines: usize,
    /// renderer 需要保留的显式尾部空行数量。
    ///
    /// 这包含最后一条正文行末尾换行形成的空行，以及其后的空白行。
    pub(crate) trailing_blank_lines: usize,
    /// 第一条非空白正文内容的 byte offset。
    pub(crate) content_start: usize,
    /// 最后一条非空白正文内容后的 byte offset。
    pub(crate) content_end: usize,
}

impl MarkdownSourceBounds {
    /// 返回外层空白行总数。
    pub(crate) const fn outer_blank_line_count(self) -> usize {
        self.leading_blank_lines + self.trailing_blank_lines
    }

    /// 返回正文是否为空。
    pub(crate) const fn is_empty(self) -> bool {
        self.content_start == self.content_end
    }
}

/// 计算 Markdown 源文本的外层空白行和正文 byte 边界。
///
/// 空白行只包含空格或 tab；CRLF 的 `\r` 属于行结尾，不参与正文边界。
/// `content_end` 服务 display trim；如果最后一条正文行以 Markdown hard break 结束，
/// 则保留其行尾换行。`trailing_blank_lines` 服务 renderer/metrics，保留 Markdown
/// 源文本末尾显式换行对应的空行。
pub(crate) fn markdown_source_bounds(content: &str) -> MarkdownSourceBounds {
    let mut leading_blank_lines = 0usize;
    let mut content_start = 0usize;
    let mut has_seen_content = false;
    let mut cursor = 0usize;
    let mut content_end = 0usize;
    let mut trailing_blank_lines = 0usize;

    // `split_inclusive('\n')` 会返回没有结尾换行的最后片段，因此整个扫描保持
    // 单一路径，不需要循环后的尾段兜底。
    for segment in content.split_inclusive('\n') {
        let segment_start = cursor;
        let segment_end = segment_start + segment.len();
        let line_text = markdown_line_text(segment);

        if is_markdown_blank_line(line_text) {
            if !has_seen_content {
                leading_blank_lines = leading_blank_lines.saturating_add(1);
                content_start = segment_end;
            } else {
                trailing_blank_lines = trailing_blank_lines.saturating_add(1);
            }
        } else {
            has_seen_content = true;
            let line_text_end = segment_start + line_text.len();
            content_end =
                if segment.ends_with('\n') && markdown_line_ends_with_hard_break(line_text) {
                    segment_end
                } else {
                    line_text_end
                };
            trailing_blank_lines = usize::from(segment.ends_with('\n'));
        }

        cursor = segment_end;
    }
    if has_seen_content {
        MarkdownSourceBounds {
            leading_blank_lines,
            trailing_blank_lines,
            content_start,
            content_end,
        }
    } else {
        MarkdownSourceBounds {
            leading_blank_lines,
            trailing_blank_lines: 0,
            content_start: content.len(),
            content_end: content.len(),
        }
    }
}

/// 返回去除外层空白行后的 Markdown 显示切片。
pub(crate) fn markdown_display_content_slice(content: &str) -> &str {
    let bounds = markdown_source_bounds(content);
    &content[bounds.content_start..bounds.content_end]
}

fn markdown_line_text(segment: &str) -> &str {
    let segment = segment.strip_suffix('\n').unwrap_or(segment);
    segment.strip_suffix('\r').unwrap_or(segment)
}

fn is_markdown_blank_line(line: &str) -> bool {
    line.bytes().all(|byte| matches!(byte, b' ' | b'\t'))
}

fn markdown_line_ends_with_hard_break(line: &str) -> bool {
    line.ends_with('\\')
        || line
            .as_bytes()
            .iter()
            .rev()
            .take_while(|byte| **byte == b' ')
            .count()
            >= 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_bounds_count_outer_blank_lines_with_crlf() {
        let content = "\r\n\t\r\nbody\r\n \t\r\n";

        assert_eq!(
            markdown_source_bounds(content),
            MarkdownSourceBounds {
                leading_blank_lines: 2,
                trailing_blank_lines: 2,
                content_start: "\r\n\t\r\n".len(),
                content_end: "\r\n\t\r\nbody".len(),
            }
        );
    }

    #[test]
    fn display_slice_preserves_content_whitespace_and_markdown_hard_break() {
        assert_eq!(markdown_display_content_slice("\nline  \n\n"), "line  \n");
        assert_eq!(
            markdown_display_content_slice("\n    code  \n"),
            "    code  \n"
        );
    }

    #[test]
    fn blank_only_source_has_empty_content_bounds() {
        let content = " \n\t\n";

        assert_eq!(
            markdown_source_bounds(content),
            MarkdownSourceBounds {
                leading_blank_lines: 2,
                trailing_blank_lines: 0,
                content_start: content.len(),
                content_end: content.len(),
            }
        );
    }
}
