//! Transcript prose 的统一换行规划。
//!
//! planner 先在完整可见文本上计算 grapheme 与 UAX #14 边界，再返回最终源文本范围。
//! 调用方只负责切片或投影样式，因此样式边界不会改变宽度、grapheme 或换行语义。

mod url_token;

use std::ops::Range;

use unicode_linebreak::{BreakClass, BreakOpportunity, break_property, linebreaks};
use unicode_segmentation::UnicodeSegmentation;
use url_token::{UrlTokenRange, url_token_ranges};

use crate::display_width::grapheme_width;

/// 软换行发生在空白后时，下一行如何展示该空白。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WrappedWhitespace {
    /// 隐藏整个换行分隔空白。
    Discard,
    /// 隐藏单个空格，但把多空格作为下一行缩进保留。
    PreserveMultiple,
}

/// Prose planner 的宽度与空白策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProseWrapOptions {
    pub(crate) first_width: usize,
    pub(crate) continuation_width: usize,
    pub(crate) wrapped_whitespace: WrappedWhitespace,
    pub(crate) trim_trailing_whitespace: bool,
}

/// 一条视觉行在原文本中的消费范围与可见范围。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WrappedTextRange {
    /// 归属于该视觉行的完整源范围，可能包含被折叠的空白或 mandatory separator。
    pub(crate) consumed: Range<usize>,
    /// 实际渲染的连续源范围。
    pub(crate) visible: Range<usize>,
}

/// 投影后的同样式文本范围。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StyledTextRange<S> {
    pub(crate) range: Range<usize>,
    pub(crate) style: S,
}

/// 按 LF 拆分逻辑行，并把 CRLF 中不应渲染的 CR 单独标记出来。
pub(crate) fn split_text_lines(text: &str) -> impl Iterator<Item = (&str, bool)> {
    let mut lines = text.split('\n').peekable();
    std::iter::from_fn(move || {
        let raw_line = lines.next()?;
        let has_line_feed = lines.peek().is_some();
        let display_line = has_line_feed
            .then(|| raw_line.strip_suffix('\r'))
            .flatten()
            .unwrap_or(raw_line);
        Some((display_line, display_line.len() != raw_line.len()))
    })
}

/// 判断普通 prose 的断点集合是否与宽度变化保持单调。
///
/// 调用方应在进入这里前完成 terminal text sanitization；这里仅处理由
/// UAX/URL/空白策略决定的宽度敏感特征。prompt 的前导缩进由 prompt 层另行判断。
pub(crate) fn prose_wrap_is_monotone_when_widening(text: &str) -> bool {
    split_text_lines(text).all(|(line, hides_carriage_return)| {
        !hides_carriage_return
            && !has_width_sensitive_whitespace(line)
            && !url_token::has_url_like_token(line)
    })
}

fn has_width_sensitive_whitespace(line: &str) -> bool {
    let mut previous_is_whitespace = false;
    for character in line.chars() {
        let is_whitespace = character.is_whitespace();
        if is_whitespace && (character != ' ' || previous_is_whitespace) {
            return true;
        }
        previous_is_whitespace = is_whitespace;
    }
    false
}

#[derive(Debug, Clone)]
struct GraphemeInfo {
    range: Range<usize>,
    width: usize,
    is_whitespace: bool,
    is_mandatory_separator: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoundaryKind {
    Prohibited,
    Emergency,
    Allowed,
    Mandatory,
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Boundary {
    kind: BoundaryKind,
    protected_url_token_width: Option<usize>,
}

impl Boundary {
    const fn new(kind: BoundaryKind) -> Self {
        Self {
            kind,
            protected_url_token_width: None,
        }
    }

    fn kind_for_width(self, width: usize) -> BoundaryKind {
        // 只有完整 raw token（包括包裹标点）能放下时，URL body 才保持原子；
        // 否则回退到受禁则约束的 emergency wrap，避免把标点单独留在行首。
        if self
            .protected_url_token_width
            .is_some_and(|token_width| token_width <= width)
            && !matches!(self.kind, BoundaryKind::Mandatory | BoundaryKind::End)
        {
            BoundaryKind::Prohibited
        } else {
            self.kind
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct BreakCandidate {
    consumed_end: usize,
    visible_end: usize,
    next_start: usize,
    next_visible_start: usize,
}

#[derive(Debug, Clone, Copy)]
struct ReflowContext<'a> {
    graphemes: &'a [GraphemeInfo],
    prefix_widths: &'a [usize],
    boundaries: &'a [Boundary],
    visible_start: usize,
    width: usize,
    continuation_width: usize,
}

/// 按 UAX #14、grapheme 和终端显示宽度规划 prose 的最终视觉行范围。
///
/// 性能剖面（release，2026-07 实测）：单逻辑行约 1–11µs（33B–500B 文本），
/// 其中约 90% 花在与宽度无关的分析（grapheme / 前缀宽度 / URL / UAX 边界）。
/// resize 会对全部 transcript item 重估；若 benchmark 的 user_estimate_time
/// 显示该路径成为热点，优先把分析半场提取为可按文本缓存的结构，而非微调主循环。
pub(crate) fn wrap_prose_ranges(text: &str, options: ProseWrapOptions) -> Vec<WrappedTextRange> {
    if text.is_empty() {
        return vec![WrappedTextRange {
            consumed: 0..0,
            visible: 0..0,
        }];
    }

    let graphemes = grapheme_infos(text);
    let prefix_widths = prefix_widths(&graphemes);
    let url_ranges = url_token_ranges(text);
    let boundaries = boundary_kinds(text, &graphemes, &url_ranges);
    let mut lines = Vec::new();
    let mut consumed_start = 0usize;
    let mut visible_start = 0usize;
    let mut width = options.first_width.max(1);
    let continuation_width = options.continuation_width.max(1);
    let mut last_line_ended_with_mandatory = false;

    while consumed_start < graphemes.len() {
        let mut best_allowed = None;
        let mut best_emergency = None;
        let mut boundary = visible_start + 1;
        let mut restarted = false;

        while boundary <= graphemes.len() {
            let kind = boundaries[boundary].kind_for_width(width);

            if range_width(&prefix_widths, visible_start, boundary) > width
                && should_defer_leading_url_to_continuation(
                    &graphemes,
                    consumed_start,
                    visible_start,
                    width,
                    continuation_width,
                    &url_ranges,
                )
            {
                // prefix 由调用方在范围外渲染，空内容行让它独占首行并保持 URL 原子性。
                lines.push(prefix_only_range(&graphemes, consumed_start, visible_start));
                width = continuation_width;
                last_line_ended_with_mandatory = false;
                restarted = true;
                break;
            }

            if kind == BoundaryKind::Mandatory {
                let separator_start = boundary - 1;
                let visible_end = if options.trim_trailing_whitespace {
                    trim_trailing_whitespace(&graphemes, visible_start, separator_start)
                } else {
                    separator_start
                };

                // forced wrap 可能只把 separator（或会被裁剪的尾随空白）留给下一轮；
                // 它们仍属于前一逻辑行，除非前一行本身已经由 mandatory break 结束。
                let can_attach_to_previous_line = visible_end == visible_start
                    && !last_line_ended_with_mandatory
                    && lines.last().is_some_and(|previous| {
                        previous.visible.start < previous.visible.end
                            && previous.consumed.end == byte_offset(&graphemes, consumed_start)
                    });
                if can_attach_to_previous_line {
                    if let Some(previous) = lines.last_mut() {
                        previous.consumed.end = byte_offset(&graphemes, boundary);
                    }
                    consumed_start = boundary;
                    visible_start = boundary;
                    width = continuation_width;
                    last_line_ended_with_mandatory = true;
                    restarted = true;
                    break;
                }

                if range_width(&prefix_widths, visible_start, visible_end) > width {
                    (consumed_start, visible_start) = close_overflowed_line(
                        &mut lines,
                        &graphemes,
                        &prefix_widths,
                        consumed_start,
                        visible_start,
                        width,
                        best_allowed.or(best_emergency),
                    );
                    width = continuation_width;
                    last_line_ended_with_mandatory = false;
                    restarted = true;
                    break;
                }

                lines.push(WrappedTextRange {
                    consumed: byte_offset(&graphemes, consumed_start)
                        ..byte_offset(&graphemes, boundary),
                    visible: byte_offset(&graphemes, visible_start)
                        ..byte_offset(&graphemes, visible_end),
                });
                consumed_start = boundary;
                visible_start = boundary;
                width = continuation_width;
                last_line_ended_with_mandatory = true;
                restarted = true;
                break;
            }

            if kind == BoundaryKind::End {
                let visible_end = if options.trim_trailing_whitespace {
                    trim_trailing_whitespace(&graphemes, visible_start, boundary)
                } else {
                    boundary
                };

                if range_width(&prefix_widths, visible_start, visible_end) <= width {
                    lines.push(WrappedTextRange {
                        consumed: byte_offset(&graphemes, consumed_start)..text.len(),
                        visible: byte_offset(&graphemes, visible_start)
                            ..byte_offset(&graphemes, visible_end),
                    });
                    consumed_start = graphemes.len();
                    last_line_ended_with_mandatory = false;
                    break;
                }

                (consumed_start, visible_start) = close_overflowed_line(
                    &mut lines,
                    &graphemes,
                    &prefix_widths,
                    consumed_start,
                    visible_start,
                    width,
                    best_allowed.or(best_emergency),
                );
                width = continuation_width;
                last_line_ended_with_mandatory = false;
                restarted = true;
                break;
            }

            let candidate = match kind {
                BoundaryKind::Allowed => Some(allowed_candidate(
                    &graphemes,
                    &prefix_widths,
                    visible_start,
                    boundary,
                    options.wrapped_whitespace,
                )),
                BoundaryKind::Emergency => Some(emergency_candidate(boundary)),
                BoundaryKind::Prohibited | BoundaryKind::Mandatory | BoundaryKind::End => None,
            };

            if let Some(candidate) = candidate
                && candidate.consumed_end > consumed_start
                && range_width(&prefix_widths, visible_start, candidate.visible_end) <= width
            {
                match kind {
                    BoundaryKind::Allowed => {
                        let should_reflow = best_allowed.is_some_and(|previous| {
                            should_reflow_exact_fit(
                                ReflowContext {
                                    graphemes: &graphemes,
                                    prefix_widths: &prefix_widths,
                                    boundaries: &boundaries,
                                    visible_start,
                                    width,
                                    continuation_width,
                                },
                                previous,
                                candidate,
                                boundary,
                            )
                        });
                        if !should_reflow {
                            best_allowed = Some(candidate);
                        }
                    }
                    BoundaryKind::Emergency => best_emergency = Some(candidate),
                    _ => {}
                }
            }

            if range_width(&prefix_widths, visible_start, boundary) > width {
                // UAX 的空白断点位于整个 space run 之后；先扫描到 run 末尾，才能
                // 判断这些空白应折叠还是作为下一行缩进保留。
                if graphemes[boundary - 1].is_whitespace
                    && boundary < graphemes.len()
                    && graphemes[boundary].is_whitespace
                {
                    boundary += 1;
                    continue;
                }

                (consumed_start, visible_start) = close_overflowed_line(
                    &mut lines,
                    &graphemes,
                    &prefix_widths,
                    consumed_start,
                    visible_start,
                    width,
                    best_allowed.or(best_emergency),
                );
                width = continuation_width;
                last_line_ended_with_mandatory = false;
                restarted = true;
                break;
            }

            boundary += 1;
        }

        if consumed_start >= graphemes.len() {
            break;
        }
        debug_assert!(restarted, "wrap planner must either finish or advance");
    }

    if last_line_ended_with_mandatory {
        lines.push(WrappedTextRange {
            consumed: text.len()..text.len(),
            visible: text.len()..text.len(),
        });
    }

    lines
}

/// 把最终可见范围单调投影回样式区间。
///
/// 若一个 grapheme 跨越多个样式区间，完整 grapheme 采用首个 code point 的样式，
/// 避免终端侧再次把它拆开。
///
/// visible 范围始终落在全文 grapheme 边界上，而 grapheme 边界规则在真边界处
/// 重新分段结果不变，因此逐行对切片分段与对全文分段等价，无需重建全文索引。
pub(crate) fn project_wrapped_styles<S: Copy + PartialEq>(
    text: &str,
    style_ranges: &[(Range<usize>, S)],
    wrapped: &[WrappedTextRange],
) -> Vec<Vec<StyledTextRange<S>>> {
    let mut style_cursor = 0usize;
    let mut projected = Vec::with_capacity(wrapped.len());

    for wrapped_line in wrapped {
        let mut line: Vec<StyledTextRange<S>> = Vec::new();
        for (offset, grapheme) in text[wrapped_line.visible.clone()].grapheme_indices(true) {
            let start = wrapped_line.visible.start + offset;
            let range = start..start + grapheme.len();
            while style_cursor < style_ranges.len()
                && style_ranges[style_cursor].0.end <= range.start
            {
                style_cursor += 1;
            }

            let (_, style) = style_ranges
                .get(style_cursor)
                .expect("style ranges must cover the flattened text");
            if let Some(last) = line.last_mut()
                && last.style == *style
                && last.range.end == range.start
            {
                last.range.end = range.end;
            } else {
                line.push(StyledTextRange {
                    range,
                    style: *style,
                });
            }
        }
        projected.push(line);
    }

    projected
}

/// 把带样式的连续文本片段拼成整段可见文本，并记录每段字节区间对应的样式。
///
/// 输出正好是 [`wrap_prose_ranges`] 与 [`project_wrapped_styles`] 需要的输入形态；
/// 空片段被跳过，保证样式区间非空且连续覆盖全文。
pub(crate) fn flatten_styled_text<'a, S: Copy>(
    chunks: impl IntoIterator<Item = (&'a str, S)>,
) -> (String, Vec<(Range<usize>, S)>) {
    let mut flat = String::new();
    let mut style_ranges = Vec::new();
    for (text, style) in chunks {
        let start = flat.len();
        flat.push_str(text);
        if flat.len() > start {
            style_ranges.push((start..flat.len(), style));
        }
    }
    (flat, style_ranges)
}

fn grapheme_infos(text: &str) -> Vec<GraphemeInfo> {
    text.grapheme_indices(true)
        .map(|(start, grapheme)| GraphemeInfo {
            range: start..start + grapheme.len(),
            width: grapheme_width(grapheme),
            is_whitespace: !grapheme.is_empty() && grapheme.chars().all(char::is_whitespace),
            is_mandatory_separator: grapheme.chars().any(|character| {
                matches!(
                    break_property(character as u32),
                    BreakClass::Mandatory
                        | BreakClass::CarriageReturn
                        | BreakClass::LineFeed
                        | BreakClass::NextLine
                )
            }),
        })
        .collect()
}

fn prefix_widths(graphemes: &[GraphemeInfo]) -> Vec<usize> {
    let mut widths = Vec::with_capacity(graphemes.len() + 1);
    widths.push(0usize);
    for grapheme in graphemes {
        widths.push(
            widths
                .last()
                .copied()
                .unwrap_or(0)
                .saturating_add(grapheme.width),
        );
    }
    widths
}

fn boundary_kinds(
    text: &str,
    graphemes: &[GraphemeInfo],
    url_ranges: &[UrlTokenRange],
) -> Vec<Boundary> {
    let mut boundaries = vec![Boundary::new(BoundaryKind::Prohibited); graphemes.len() + 1];
    for boundary in 1..graphemes.len() {
        boundaries[boundary].kind = if emergency_break_is_legal(
            &text[graphemes[boundary - 1].range.clone()],
            &text[graphemes[boundary].range.clone()],
        ) {
            BoundaryKind::Emergency
        } else {
            BoundaryKind::Prohibited
        };
    }
    boundaries[graphemes.len()].kind = BoundaryKind::End;

    let mut url_cursor = 0usize;
    let mut opportunities = linebreaks(text).peekable();
    for boundary in 1..=graphemes.len() {
        let offset = graphemes[boundary - 1].range.end;
        while url_cursor < url_ranges.len() && url_ranges[url_cursor].raw.end <= offset {
            url_cursor += 1;
        }
        let url_range = url_ranges
            .get(url_cursor)
            .filter(|range| range.raw.start < offset && offset < range.raw.end);
        let protected_url_token_width = url_range
            .filter(|range| range.body.start < offset && offset < range.body.end)
            .map(|range| range.raw_width);
        let inside_url_body = protected_url_token_width.is_some();
        boundaries[boundary].protected_url_token_width = protected_url_token_width;

        while opportunities
            .peek()
            .is_some_and(|(opportunity_offset, _)| *opportunity_offset < offset)
        {
            opportunities.next();
        }
        if let Some(&(opportunity_offset, opportunity)) = opportunities.peek()
            && opportunity_offset == offset
        {
            opportunities.next();

            boundaries[boundary].kind = match opportunity {
                BreakOpportunity::Allowed => {
                    let previous = &text[graphemes[boundary - 1].range.clone()];
                    let next = graphemes
                        .get(boundary)
                        .map(|grapheme| &text[grapheme.range.clone()])
                        .unwrap_or("");
                    if !uax_allowed_break_is_legal(previous, next) {
                        BoundaryKind::Prohibited
                    } else if inside_url_body {
                        BoundaryKind::Emergency
                    } else {
                        BoundaryKind::Allowed
                    }
                }
                BreakOpportunity::Mandatory
                    if boundary < graphemes.len()
                        || graphemes[boundary - 1].is_mandatory_separator =>
                {
                    BoundaryKind::Mandatory
                }
                BreakOpportunity::Mandatory => BoundaryKind::End,
            };
        }
    }

    boundaries
}

fn emergency_break_is_legal(previous: &str, next: &str) -> bool {
    kinsoku_break_is_legal(previous, next, true)
}

fn uax_allowed_break_is_legal(previous: &str, next: &str) -> bool {
    // UAX 已依据上下文区分 opening/closing quotation，不能再把合法 opening quote
    // 当作一律禁止起行的标点；其他 Hunea 禁则 tailoring 仍然生效。
    kinsoku_break_is_legal(previous, next, false)
}

fn kinsoku_break_is_legal(previous: &str, next: &str, prohibit_ambiguous_quotation: bool) -> bool {
    !previous.chars().next().is_some_and(|character| {
        let class = break_property(character as u32);
        matches!(
            class,
            BreakClass::OpenPunctuation
                | BreakClass::Prefix
                | BreakClass::NonBreakingGlue
                | BreakClass::WordJoiner
        ) || prohibit_ambiguous_quotation && class == BreakClass::Quotation
    }) && !next.chars().next().is_some_and(|character| {
        let class = break_property(character as u32);
        matches!(
            class,
            BreakClass::ClosePunctuation
                | BreakClass::CloseParenthesis
                | BreakClass::Exclamation
                | BreakClass::Inseparable
                | BreakClass::NonStarter
                | BreakClass::ConditionalJapaneseStarter
                | BreakClass::InfixSeparator
                | BreakClass::Postfix
                | BreakClass::NonBreakingGlue
                | BreakClass::WordJoiner
                | BreakClass::CombiningMark
                | BreakClass::ZeroWidthJoiner
                | BreakClass::EmojiModifier
        ) || prohibit_ambiguous_quotation && class == BreakClass::Quotation
    })
}

fn allowed_candidate(
    graphemes: &[GraphemeInfo],
    prefix_widths: &[usize],
    visible_start: usize,
    boundary: usize,
    wrapped_whitespace: WrappedWhitespace,
) -> BreakCandidate {
    let whitespace_start = trim_trailing_whitespace(graphemes, visible_start, boundary);
    if whitespace_start == visible_start || whitespace_start == boundary {
        return emergency_candidate(boundary);
    }

    let whitespace_width = range_width(prefix_widths, whitespace_start, boundary);
    let preserve =
        wrapped_whitespace == WrappedWhitespace::PreserveMultiple && whitespace_width > 1;
    BreakCandidate {
        consumed_end: whitespace_start,
        visible_end: whitespace_start,
        next_start: whitespace_start,
        next_visible_start: if preserve { whitespace_start } else { boundary },
    }
}

fn emergency_candidate(boundary: usize) -> BreakCandidate {
    BreakCandidate {
        consumed_end: boundary,
        visible_end: boundary,
        next_start: boundary,
        next_visible_start: boundary,
    }
}

fn should_reflow_exact_fit(
    context: ReflowContext<'_>,
    previous: BreakCandidate,
    current: BreakCandidate,
    boundary: usize,
) -> bool {
    let preserves_multi_space =
        current.next_start < boundary && current.next_visible_start == current.next_start;
    if !preserves_multi_space
        || range_width(
            context.prefix_widths,
            context.visible_start,
            current.visible_end,
        ) != context.width
    {
        return false;
    }

    let next_visible_end = next_unit_visible_end(
        context.graphemes,
        context.prefix_widths,
        context.boundaries,
        boundary,
        context.continuation_width,
    );
    next_visible_end > boundary
        && range_width(
            context.prefix_widths,
            previous.next_visible_start,
            next_visible_end,
        ) <= context.continuation_width
}

fn next_unit_visible_end(
    graphemes: &[GraphemeInfo],
    prefix_widths: &[usize],
    boundaries: &[Boundary],
    start: usize,
    width: usize,
) -> usize {
    let mut trimmed_end = start;
    for (boundary, boundary_info) in boundaries.iter().copied().enumerate().skip(start + 1) {
        // trimmed_end 即"到当前扫描位置为止、裁剪尾随空白后的可见终点"，随扫描
        // 单调前移。一旦它超出行宽，后续任何断点产出的单元都放不进 continuation
        // line、调用方的 reflow 检查必然失败（mandatory separator 属空白，不会
        // 推进 trimmed_end），提前返回可避免在无断点的长 token 上扫描到行尾。
        if !graphemes[boundary - 1].is_whitespace {
            trimmed_end = boundary;
            if range_width(prefix_widths, start, trimmed_end) > width {
                return start;
            }
        }

        match boundary_info.kind_for_width(width) {
            BoundaryKind::Allowed => {
                if trimmed_end > start {
                    return trimmed_end;
                }
            }
            BoundaryKind::Mandatory => return boundary - 1,
            BoundaryKind::End => return trimmed_end,
            BoundaryKind::Prohibited | BoundaryKind::Emergency => {}
        }
    }
    start
}

fn should_defer_leading_url_to_continuation(
    graphemes: &[GraphemeInfo],
    consumed_start: usize,
    visible_start: usize,
    width: usize,
    continuation_width: usize,
    url_ranges: &[UrlTokenRange],
) -> bool {
    if consumed_start != visible_start || width >= continuation_width {
        return false;
    }

    let start = byte_offset(graphemes, visible_start);
    let Some(url_range) = url_ranges.iter().find(|range| range.raw.start == start) else {
        return false;
    };

    url_range.raw_width > width && url_range.raw_width <= continuation_width
}

fn prefix_only_range(
    graphemes: &[GraphemeInfo],
    consumed_start: usize,
    visible_start: usize,
) -> WrappedTextRange {
    debug_assert_eq!(consumed_start, visible_start);
    let consumed = byte_offset(graphemes, consumed_start);
    let visible = byte_offset(graphemes, visible_start);
    WrappedTextRange {
        consumed: consumed..consumed,
        visible: visible..visible,
    }
}

fn forced_width_candidate(
    prefix_widths: &[usize],
    visible_start: usize,
    width: usize,
) -> BreakCandidate {
    let mut boundary = visible_start + 1;
    if range_width(prefix_widths, visible_start, boundary) > width {
        // 单个 grapheme 自身超宽时无法同时满足 grapheme 完整性与行宽限制。
        return emergency_candidate(boundary);
    }

    while boundary + 1 < prefix_widths.len()
        && range_width(prefix_widths, visible_start, boundary + 1) <= width
    {
        boundary += 1;
    }

    emergency_candidate(boundary)
}

/// 当前行放不下更多内容时收束该行：优先用已记录的最佳断点，否则按行宽强制截断。
///
/// 返回下一行的 (consumed_start, visible_start)。
fn close_overflowed_line(
    lines: &mut Vec<WrappedTextRange>,
    graphemes: &[GraphemeInfo],
    prefix_widths: &[usize],
    consumed_start: usize,
    visible_start: usize,
    width: usize,
    best_candidate: Option<BreakCandidate>,
) -> (usize, usize) {
    let candidate = best_candidate
        .unwrap_or_else(|| forced_width_candidate(prefix_widths, visible_start, width));
    lines.push(WrappedTextRange {
        consumed: byte_offset(graphemes, consumed_start)
            ..byte_offset(graphemes, candidate.consumed_end),
        visible: byte_offset(graphemes, visible_start)
            ..byte_offset(graphemes, candidate.visible_end),
    });
    (candidate.next_start, candidate.next_visible_start)
}

fn trim_trailing_whitespace(graphemes: &[GraphemeInfo], start: usize, mut end: usize) -> usize {
    while end > start && graphemes[end - 1].is_whitespace {
        end -= 1;
    }
    end
}

fn range_width(prefix_widths: &[usize], start: usize, end: usize) -> usize {
    prefix_widths[end].saturating_sub(prefix_widths[start])
}

fn byte_offset(graphemes: &[GraphemeInfo], boundary: usize) -> usize {
    if boundary == 0 {
        0
    } else {
        graphemes[boundary - 1].range.end
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display_width::display_width;

    fn wrapped(text: &str, width: usize) -> Vec<&str> {
        wrap_prose_ranges(
            text,
            ProseWrapOptions {
                first_width: width,
                continuation_width: width,
                wrapped_whitespace: WrappedWhitespace::Discard,
                trim_trailing_whitespace: false,
            },
        )
        .into_iter()
        .map(|range| &text[range.visible])
        .collect()
    }

    fn wrapped_preserving_indent(text: &str, width: usize) -> Vec<&str> {
        wrap_prose_ranges(
            text,
            ProseWrapOptions {
                first_width: width,
                continuation_width: width,
                wrapped_whitespace: WrappedWhitespace::PreserveMultiple,
                trim_trailing_whitespace: false,
            },
        )
        .into_iter()
        .map(|range| &text[range.visible])
        .collect()
    }

    fn wrapped_trimming_trailing_whitespace(text: &str, width: usize) -> Vec<&str> {
        wrap_prose_ranges(
            text,
            ProseWrapOptions {
                first_width: width,
                continuation_width: width,
                wrapped_whitespace: WrappedWhitespace::Discard,
                trim_trailing_whitespace: true,
            },
        )
        .into_iter()
        .map(|range| &text[range.visible])
        .collect()
    }

    /// 禁止起行的标点集合（部分代表），用于断言不出现在单元行首。
    const LEADING_PROHIBITED: &[&str] = &[
        "，", "。", "、", "！", "？", "：", "；", "）", "】", "」", "』", "%",
    ];

    #[test]
    fn empty_text_yields_one_empty_visual_line() {
        assert_eq!(wrapped("", 10), vec![""]);
    }

    #[test]
    fn ascii_prose_wraps_at_spaces() {
        assert_eq!(wrapped("hello world", 5), vec!["hello", "world"]);
    }

    #[test]
    fn uax_allowed_break_before_opening_quote_discards_separator_space() {
        assert_eq!(wrapped("ab \"cd\"", 4), vec!["ab", "\"cd\""]);
    }

    #[test]
    fn cjk_breaks_between_ideographs() {
        assert_eq!(wrapped("你好世界", 2), vec!["你", "好", "世", "界"]);
    }

    #[test]
    fn no_wrapped_line_starts_with_leading_prohibited_punctuation() {
        let text = "你好，请阅读一下。然后（附注）继续！好吗？";
        for line in wrapped(text, 6) {
            assert!(
                !LEADING_PROHIBITED.iter().any(|mark| line.starts_with(mark)),
                "wrapped line unexpectedly starts with a leading-prohibited mark: {line:?}"
            );
        }
    }

    #[test]
    fn uax_allowed_break_does_not_override_leading_prohibited_punctuation() {
        assert_eq!(wrapped("abc/%def", 5), vec!["abc/%", "def"]);
    }

    #[test]
    fn forced_wrap_prefers_width_bound_when_kinsoku_pair_cannot_fit() {
        assert_eq!(wrapped("你，好", 2), vec!["你", "，", "好"]);
    }

    #[test]
    fn emergency_wrap_bounds_long_kinsoku_runs() {
        let text = "好！！！！！！！！继续";
        let lines = wrapped(text, 4);

        assert_eq!(lines.concat(), text);
        assert!(lines.iter().all(|line| display_width(line) <= 4));
    }

    #[test]
    fn keeps_family_emoji_grapheme_intact() {
        let text = "a👨‍👩‍👧b";
        let lines = wrapped(text, 2);

        assert_eq!(lines.concat(), text);
        assert!(lines.contains(&"👨‍👩‍👧"));
    }

    #[test]
    fn keeps_keycap_emoji_grapheme_intact() {
        let text = "x2️⃣y";
        let lines = wrapped(text, 2);

        assert_eq!(lines.concat(), text);
        assert!(lines.contains(&"2️⃣"));
    }

    #[test]
    fn keeps_url_token_intact_when_it_fits_a_line() {
        let text = "see https://example.com/a/b end";
        let url = "https://example.com/a/b";

        assert_eq!(wrapped(text, url.len()), vec!["see", url, "end"]);
    }

    #[test]
    fn url_wrapper_punctuation_stays_attached_during_emergency_wrap() {
        let url = "https://example.com/a";
        let trailing_punctuation = format!("{url}.");
        let wrapped_in_parentheses = format!("({url})");

        assert_eq!(
            wrapped(&trailing_punctuation, display_width(url)),
            vec!["https://example.com/", "a."]
        );
        assert_eq!(
            wrapped(&wrapped_in_parentheses, display_width(url)),
            vec!["(https://example.com/", "a)"]
        );
    }

    #[test]
    fn overlong_url_uses_emergency_grapheme_breaks() {
        let text = "https://example.com/a/b";
        let lines = wrapped(text, 8);

        assert_eq!(lines.concat(), text);
        assert!(lines.iter().all(|line| display_width(line) <= 8));
    }

    #[test]
    fn file_path_wraps_without_url_atomicity() {
        assert_eq!(wrapped("src/main.rs", 4)[0], "src/");
    }

    #[test]
    fn path_heavy_prose_uses_uax_breaks_without_retaining_separator_spaces() {
        assert_eq!(wrapped("go test ./...", 6), vec!["go", "test .", "/..."]);
    }

    #[test]
    fn mandatory_separator_forces_a_line_and_is_not_rendered() {
        assert_eq!(wrapped("a\u{2028}b", 10), vec!["a", "b"]);
        assert_eq!(wrapped("a\u{2028}", 10), vec!["a", ""]);
    }

    #[test]
    fn mandatory_separator_after_overwide_grapheme_does_not_add_empty_line() {
        assert_eq!(wrapped("中\u{2028}b", 1), vec!["中", "b"]);
        assert_eq!(wrapped("中\u{2028}", 1), vec!["中", ""]);
    }

    #[test]
    fn trimmed_whitespace_before_mandatory_separator_does_not_add_empty_line() {
        assert_eq!(
            wrapped_trimming_trailing_whitespace("中 \u{2028}b", 1),
            vec!["中", "b"]
        );
        assert_eq!(
            wrapped_trimming_trailing_whitespace("中 \u{2028}", 1),
            vec!["中", ""]
        );
    }

    #[test]
    fn consecutive_mandatory_separators_preserve_the_empty_logical_line() {
        assert_eq!(wrapped("a\u{2028}\u{2028}b", 10), vec!["a", "", "b"]);
        assert_eq!(wrapped("a\u{2028}\u{2028}", 10), vec!["a", "", ""]);
    }

    #[test]
    fn wrapped_whitespace_policy_preserves_only_multi_space_indent() {
        assert_eq!(
            wrapped_preserving_indent("hello world", 5),
            vec!["hello", "world"]
        );
        assert_eq!(
            wrapped_preserving_indent("abc d    e", 5),
            vec!["abc d", "    e"]
        );
    }

    #[test]
    fn overlong_token_without_uax_breaks_uses_emergency_breaks() {
        let text = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let lines = wrapped(text, 8);

        assert_eq!(lines.concat(), text);
        assert!(lines.iter().all(|line| display_width(line) <= 8));
    }

    #[test]
    fn overflow_fallback_does_not_render_mandatory_separator() {
        assert_eq!(wrapped("((\u{2028}x", 1), vec!["(", "(", "x"]);
    }

    #[test]
    fn overflow_before_terminal_mandatory_separator_keeps_one_trailing_empty_line() {
        assert_eq!(wrapped("((\u{2028}", 1), vec!["(", "(", ""]);
    }
}
