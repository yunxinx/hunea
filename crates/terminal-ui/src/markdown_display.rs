use std::rc::Rc;

/// `markdown_display_content` 返回仅用于显示的 Markdown 内容切片。
///
/// 这里只剥离外层空白行，不改变正文行内空白；否则会破坏 CommonMark 中依赖
/// 缩进、尾随空格或反斜杠 hard break 的语义。
pub(crate) fn markdown_display_content(content: &str) -> &str {
    let start = markdown_content_start_after_outer_blank_lines(content);
    let end = markdown_content_end_before_outer_blank_lines(content, start);
    &content[start..end]
}

/// `markdown_display_content_rc` 在内容未变时复用原始 `Rc`。
pub(crate) fn markdown_display_content_rc(content: &Rc<str>) -> Rc<str> {
    let display_content = markdown_display_content(content.as_ref());
    if display_content.len() == content.len() {
        Rc::clone(content)
    } else {
        Rc::from(display_content)
    }
}

fn markdown_content_start_after_outer_blank_lines(content: &str) -> usize {
    let mut start = 0usize;
    for segment in content.split_inclusive('\n') {
        if !is_markdown_blank_line(markdown_line_text(segment)) {
            break;
        }
        start += segment.len();
    }
    start
}

fn markdown_content_end_before_outer_blank_lines(content: &str, start: usize) -> usize {
    let mut cursor = start;
    let mut end = start;
    for segment in content[start..].split_inclusive('\n') {
        let segment_start = cursor;
        let segment_end = segment_start + segment.len();
        let line_text = markdown_line_text(segment);
        if !is_markdown_blank_line(line_text) {
            let line_text_end = segment_start + line_text.len();
            end = if segment.ends_with('\n') && markdown_line_ends_with_hard_break(line_text) {
                segment_end
            } else {
                line_text_end
            };
        }
        cursor = segment_end;
    }
    end
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
