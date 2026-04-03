/// `SlotFrame` 描述统一文档中某个带可选上下装饰行的内容块。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct SlotFrame {
    pub(crate) frame_start_line: usize,
    pub(crate) frame_line_count: usize,
    pub(crate) content_start_line: usize,
    pub(crate) content_line_count: usize,
}

impl SlotFrame {
    /// `new` 根据是否带装饰行计算 slot 的全文坐标。
    pub(crate) fn new(
        frame_start_line: usize,
        has_padding: bool,
        content_line_count: usize,
    ) -> Self {
        let content_line_count = content_line_count.max(1);
        let mut content_start_line = frame_start_line;
        let mut frame_line_count = content_line_count;

        if has_padding {
            content_start_line += 1;
            frame_line_count += 2;
        }

        Self {
            frame_start_line,
            frame_line_count,
            content_start_line,
            content_line_count,
        }
    }

    pub(crate) fn has_padding(self) -> bool {
        self.frame_line_count > self.content_line_count
    }

    pub(crate) fn frame_bottom_line(self) -> usize {
        self.frame_start_line + self.frame_line_count - 1
    }
}
