use std::rc::Rc;

use crate::markdown_source::markdown_display_content_slice;

/// `markdown_display_content` 返回仅用于显示的 Markdown 内容切片。
///
/// 这里只剥离外层空白行，不改变正文行内空白；否则会破坏 CommonMark 中依赖
/// 缩进、尾随空格或反斜杠 hard break 的语义。
pub(crate) fn markdown_display_content(content: &str) -> &str {
    markdown_display_content_slice(content)
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
