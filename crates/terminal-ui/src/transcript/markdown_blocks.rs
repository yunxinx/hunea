/// Markdown eager renderer 识别出的顶层 block 类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MarkdownBlockKind {
    /// CommonMark heading block，例如 `# Title`。
    Heading,
    /// 顶层 list block；具体 marker 类型只在逐行快路径里参与同一 block 判定。
    List,
    /// 普通段落 block；连续段落源码行在 eager renderer 中属于同一个 block。
    Paragraph,
    /// fenced code block 或其它按 literal block 处理的内容。
    Code,
}

/// 逐行快路径识别出的 Markdown 源码行 block 类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MarkdownLineBlockKind {
    /// heading 源码行。
    Heading,
    /// 顶层 list item 源码行，携带 marker 类型以区分不同 CommonMark list。
    List(MarkdownListKind),
    /// paragraph 源码行。
    Paragraph,
    /// fenced code block 源码行。
    Code,
}

impl MarkdownLineBlockKind {
    /// 返回 eager renderer 使用的顶层 block 类型。
    pub(crate) const fn block_kind(self) -> MarkdownBlockKind {
        match self {
            Self::Heading => MarkdownBlockKind::Heading,
            Self::List(_) => MarkdownBlockKind::List,
            Self::Paragraph => MarkdownBlockKind::Paragraph,
            Self::Code => MarkdownBlockKind::Code,
        }
    }
}

/// 顶层 Markdown list 的 marker 类型，用于逐行快路径判断是否仍在同一个 list。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MarkdownListKind {
    /// `-` 开头的 bullet list。
    Dash,
    /// `*` 开头的 bullet list。
    Star,
    /// `+` 开头的 bullet list。
    Plus,
    /// `1.` 这类 ordered list。
    OrderedPeriod,
    /// `1)` 这类 ordered list。
    OrderedParen,
}

/// 判断去掉前导空白后的源码行是否是顶层 Markdown list item。
///
/// 调用方负责先排除 nested/indented list；这里仅解析 marker 类型，让
/// projection 与 metrics 的逐行快路径复用同一套 list block identity。
pub(crate) fn markdown_list_line_kind(trimmed_line: &str) -> Option<MarkdownListKind> {
    let mut chars = trimmed_line.chars();
    match (chars.next(), chars.next()) {
        (Some('-'), Some(next)) if next.is_whitespace() => {
            return Some(MarkdownListKind::Dash);
        }
        (Some('*'), Some(next)) if next.is_whitespace() => {
            return Some(MarkdownListKind::Star);
        }
        (Some('+'), Some(next)) if next.is_whitespace() => {
            return Some(MarkdownListKind::Plus);
        }
        _ => {}
    }

    let mut digit_count = 0usize;
    let mut chars = trimmed_line.chars();
    while matches!(chars.clone().next(), Some(ch) if ch.is_ascii_digit()) {
        digit_count += 1;
        chars.next();
        if digit_count > 9 {
            return None;
        }
    }

    if !(1..=9).contains(&digit_count) {
        return None;
    }

    let delimiter = match chars.next()? {
        '.' => MarkdownListKind::OrderedPeriod,
        ')' => MarkdownListKind::OrderedParen,
        _ => return None,
    };
    chars
        .next()
        .is_some_and(char::is_whitespace)
        .then_some(delimiter)
}

/// Markdown 快路径扫描时，相邻源码行在顶层 block 语义上的关系。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MarkdownBlockTransition {
    /// 当前源码行延续上一个 eager renderer 顶层 block，不额外插入 block 间距。
    SameBlock,
    /// 当前源码行开始新的 eager renderer 顶层 block，需要套用共享间距策略。
    NewBlock,
}

/// 轻量逐行扫描中，当前非空源码行与上一条非空源码行之间的分隔状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MarkdownLineSeparator {
    /// 两条非空源码行直接相邻。
    Direct,
    /// 两条非空源码行之间至少出现过一条 Markdown blank line。
    Blank,
}

/// 判断两个顶层 Markdown block 之间是否应插入 eager renderer 使用的空行。
///
/// 这是 Markdown 渲染链路的共享间距策略；eager renderer、projection 与
/// metrics 快路径都应通过这里判断 block transition，避免各自复制空行规则。
pub(crate) fn should_insert_markdown_block_spacing(
    previous_block: Option<MarkdownBlockKind>,
) -> bool {
    previous_block.is_some()
}

/// 返回 projection/metrics 在下一个顶层 Markdown block 前应补的空行数。
///
/// eager renderer 直接 push 空行，projection/metrics 只需要等价的空行数量。
pub(crate) fn markdown_block_spacing_before(previous_block: Option<MarkdownBlockKind>) -> usize {
    usize::from(should_insert_markdown_block_spacing(previous_block))
}

/// 判断轻量逐行扫描时，当前行是否延续上一个顶层 Markdown block。
///
/// pulldown-cmark 会把连续 list item 或段落源码行归入同一个顶层 block；
/// 快路径逐行扫描时需要复用这个语义，避免在同一个 block 内插入空行。
/// blank line 会结束 paragraph block；该状态必须由调用方显式传入，不能只靠
/// 前后 block kind 推断。
pub(crate) fn markdown_line_block_transition(
    previous_block: Option<MarkdownLineBlockKind>,
    next_block: MarkdownLineBlockKind,
    separator: MarkdownLineSeparator,
) -> MarkdownBlockTransition {
    match (previous_block, next_block, separator) {
        (Some(MarkdownLineBlockKind::List(previous)), MarkdownLineBlockKind::List(next), _)
            if previous == next =>
        {
            MarkdownBlockTransition::SameBlock
        }
        (
            Some(MarkdownLineBlockKind::Paragraph),
            MarkdownLineBlockKind::Paragraph,
            MarkdownLineSeparator::Direct,
        ) => MarkdownBlockTransition::SameBlock,
        _ => MarkdownBlockTransition::NewBlock,
    }
}

/// 返回逐行扫描路径在当前非空源码行前应补的空行数。
///
/// 这把 “同一顶层 block 的连续源码行” 和 “新 block 前的 eager renderer 间距”
/// 绑定到同一个判定入口，避免 projection 与 metrics 各自复刻 transition 逻辑。
pub(crate) fn markdown_line_spacing_before(
    previous_block: Option<MarkdownLineBlockKind>,
    next_block: MarkdownLineBlockKind,
    separator: MarkdownLineSeparator,
) -> usize {
    if markdown_line_block_transition(previous_block, next_block, separator)
        == MarkdownBlockTransition::NewBlock
    {
        markdown_block_spacing_before(previous_block.map(MarkdownLineBlockKind::block_kind))
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_spacing_policy_inserts_before_every_non_first_top_level_block() {
        let blocks = [
            MarkdownBlockKind::Heading,
            MarkdownBlockKind::List,
            MarkdownBlockKind::Paragraph,
            MarkdownBlockKind::Code,
        ];

        for block in blocks {
            assert!(
                !should_insert_markdown_block_spacing(None),
                "first block should not receive synthetic leading spacing: {block:?}"
            );
        }

        for previous_block in blocks {
            assert!(
                should_insert_markdown_block_spacing(Some(previous_block)),
                "non-first top-level block should preserve renderer spacing after: {previous_block:?}"
            );
        }
    }

    #[test]
    fn markdown_line_transition_keeps_contiguous_lines_inside_list_and_paragraph_blocks() {
        assert_eq!(
            markdown_line_block_transition(
                Some(MarkdownLineBlockKind::List(MarkdownListKind::Dash)),
                MarkdownLineBlockKind::List(MarkdownListKind::Dash),
                MarkdownLineSeparator::Blank
            ),
            MarkdownBlockTransition::SameBlock
        );
        assert_eq!(
            markdown_line_block_transition(
                Some(MarkdownLineBlockKind::List(MarkdownListKind::Dash)),
                MarkdownLineBlockKind::List(MarkdownListKind::Star),
                MarkdownLineSeparator::Blank
            ),
            MarkdownBlockTransition::NewBlock
        );
        assert_eq!(
            markdown_line_block_transition(
                Some(MarkdownLineBlockKind::List(MarkdownListKind::Dash)),
                MarkdownLineBlockKind::List(MarkdownListKind::OrderedPeriod),
                MarkdownLineSeparator::Blank
            ),
            MarkdownBlockTransition::NewBlock
        );
        assert_eq!(
            markdown_line_block_transition(
                Some(MarkdownLineBlockKind::Paragraph),
                MarkdownLineBlockKind::Paragraph,
                MarkdownLineSeparator::Direct
            ),
            MarkdownBlockTransition::SameBlock
        );
        assert_eq!(
            markdown_line_block_transition(
                Some(MarkdownLineBlockKind::Paragraph),
                MarkdownLineBlockKind::Paragraph,
                MarkdownLineSeparator::Blank
            ),
            MarkdownBlockTransition::NewBlock
        );
        assert_eq!(
            markdown_line_spacing_before(
                Some(MarkdownLineBlockKind::Paragraph),
                MarkdownLineBlockKind::Paragraph,
                MarkdownLineSeparator::Blank
            ),
            1
        );
        assert_eq!(
            markdown_line_spacing_before(
                Some(MarkdownLineBlockKind::Paragraph),
                MarkdownLineBlockKind::Paragraph,
                MarkdownLineSeparator::Direct
            ),
            0
        );
        assert_eq!(
            markdown_line_block_transition(
                Some(MarkdownLineBlockKind::Heading),
                MarkdownLineBlockKind::List(MarkdownListKind::Dash),
                MarkdownLineSeparator::Direct
            ),
            MarkdownBlockTransition::NewBlock
        );
    }

    #[test]
    fn markdown_list_line_kind_distinguishes_commonmark_marker_identity() {
        assert_eq!(
            markdown_list_line_kind("- item"),
            Some(MarkdownListKind::Dash)
        );
        assert_eq!(
            markdown_list_line_kind("* item"),
            Some(MarkdownListKind::Star)
        );
        assert_eq!(
            markdown_list_line_kind("+ item"),
            Some(MarkdownListKind::Plus)
        );
        assert_eq!(
            markdown_list_line_kind("12. item"),
            Some(MarkdownListKind::OrderedPeriod)
        );
        assert_eq!(
            markdown_list_line_kind("12) item"),
            Some(MarkdownListKind::OrderedParen)
        );
        assert_eq!(markdown_list_line_kind("1234567890. item"), None);
        assert_eq!(markdown_list_line_kind("-item"), None);
    }
}
