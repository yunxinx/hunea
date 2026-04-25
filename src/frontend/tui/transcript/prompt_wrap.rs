use unicode_segmentation::UnicodeSegmentation;

use super::wrap::{
    is_space_cluster, leading_space_count, measure_width, render_cluster_for_display,
    split_short_indent, split_text_to_width,
};

#[cfg(test)]
thread_local! {
    static COLUMN_OFFSET_REBUILD_CALL_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// `PromptVisualLine` 描述 prompt 文本在固定宽度下的一条视觉行。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PromptVisualLine {
    pub(crate) text: String,
    pub(crate) logical_line: usize,
    pub(crate) start_char: usize,
    pub(crate) visible_start_char: usize,
    pub(crate) end_char: usize,
    pub(crate) column_offsets: Vec<usize>,
}

#[derive(Debug, Clone, Default)]
struct DetailedWrapSegment {
    text: String,
    width: usize,
    start_char: usize,
    char_count: usize,
    is_space: bool,
}

#[derive(Debug, Clone)]
struct PromptWordBlock {
    leading_spaces: DetailedWrapSegment,
    word: DetailedWrapSegment,
    trailing_spaces: DetailedWrapSegment,
    has_leading_space: bool,
    has_trailing: bool,
}

#[derive(Debug, Clone, Default)]
struct PromptLineBuilder {
    text: String,
    width: usize,
    start_char: usize,
    visible_start_char: usize,
    end_char: usize,
    column_offsets: Vec<usize>,
    has_content: bool,
}

#[derive(Debug, Clone, Default)]
struct LiteralPromptLineBuilder {
    text: String,
    width: usize,
    start_char: usize,
    visible_start_char: usize,
    end_char: usize,
    column_offsets: Vec<usize>,
    has_content: bool,
}

/// `wrap_prompt_visual_lines` 生成 prompt 与已发送用户消息共用的视觉行结果。
pub(crate) fn wrap_prompt_visual_lines(
    value: &str,
    width: usize,
    line_prefix_width: usize,
) -> Vec<PromptVisualLine> {
    if value.is_empty() {
        return vec![PromptVisualLine::default()];
    }
    if width == 0 {
        return value
            .split('\n')
            .enumerate()
            .map(|(logical_line, raw_line)| PromptVisualLine {
                text: raw_line.to_string(),
                logical_line,
                start_char: 0,
                visible_start_char: 0,
                end_char: raw_line.chars().count(),
                column_offsets: build_column_offsets(raw_line),
            })
            .collect();
    }

    let mut lines = Vec::new();
    for (logical_line, raw_line) in value.split('\n').enumerate() {
        lines.extend(wrap_prompt_logical_line(
            raw_line,
            width,
            line_prefix_width,
            logical_line,
        ));
    }

    if lines.is_empty() {
        vec![PromptVisualLine::default()]
    } else {
        lines
    }
}

fn wrap_prompt_logical_line(
    line: &str,
    width: usize,
    line_prefix_width: usize,
    logical_line: usize,
) -> Vec<PromptVisualLine> {
    if line.is_empty() || width == 0 {
        return vec![PromptVisualLine {
            text: line.to_string(),
            logical_line,
            start_char: 0,
            visible_start_char: 0,
            end_char: line.chars().count(),
            column_offsets: build_column_offsets(line),
        }];
    }

    if line.contains('\t') {
        return wrap_prompt_literal_line_with_tabs(line, width, line_prefix_width, logical_line);
    }

    if should_hard_wrap_prompt_line(line) {
        return hard_wrap_prompt_visible_text(line, width, width, 0, 0, logical_line);
    }

    let (prefix, remainder) = split_short_indent(line);
    if !prefix.is_empty() {
        return wrap_prompt_indented_line(&prefix, remainder, width, logical_line);
    }

    wrap_prompt_prose_line(line, width, width, 0, logical_line)
}

fn wrap_prompt_indented_line(
    prefix: &str,
    remainder: &str,
    width: usize,
    logical_line: usize,
) -> Vec<PromptVisualLine> {
    let prefix_width = measure_width(prefix);
    if prefix_width >= width || remainder.is_empty() {
        return hard_wrap_prompt_visible_text(
            &(prefix.to_string() + remainder),
            width,
            width,
            0,
            0,
            logical_line,
        );
    }

    let prefix_char_count = prefix.chars().count();
    let mut lines = wrap_prompt_prose_line(
        remainder,
        width - prefix_width,
        width,
        prefix_char_count,
        logical_line,
    );
    let first_line_text = format!("{prefix}{}", lines[0].text);
    if measure_width(&first_line_text) > width {
        let prefix_line = PromptVisualLine {
            text: prefix.to_string(),
            logical_line,
            start_char: 0,
            visible_start_char: 0,
            end_char: prefix_char_count,
            column_offsets: build_column_offsets(prefix),
        };
        let mut result = vec![prefix_line];
        result.extend(wrap_prompt_prose_line(
            remainder,
            width,
            width,
            prefix_char_count,
            logical_line,
        ));
        return result;
    }

    lines[0].text = first_line_text;
    lines[0].start_char = 0;
    lines[0].visible_start_char = 0;
    lines[0].column_offsets = build_column_offsets(&lines[0].text);
    lines
}

fn wrap_prompt_prose_line(
    line: &str,
    first_width: usize,
    continuation_width: usize,
    base_char_offset: usize,
    logical_line: usize,
) -> Vec<PromptVisualLine> {
    let segments = tokenize_detailed_segments(line);
    if segments.is_empty() {
        return vec![PromptVisualLine {
            logical_line,
            start_char: base_char_offset,
            visible_start_char: base_char_offset,
            end_char: base_char_offset,
            column_offsets: vec![0],
            ..PromptVisualLine::default()
        }];
    }

    let mut blocks = build_prompt_word_blocks(&segments);
    if blocks.is_empty() {
        return hard_wrap_prompt_visible_text(
            line,
            first_width,
            continuation_width,
            base_char_offset,
            base_char_offset,
            logical_line,
        );
    }
    if base_char_offset > 0 {
        blocks = offset_prompt_word_blocks(&blocks, base_char_offset);
    }

    let mut lines = Vec::with_capacity((blocks.len() / 2).max(1));
    let mut current = PromptLineBuilder::default();
    let mut current_limit = first_width.max(1);
    let continuation_width = continuation_width.max(1);

    let flush_current = |lines: &mut Vec<PromptVisualLine>, current: &mut PromptLineBuilder| {
        if !current.has_content {
            return;
        }
        lines.push(PromptVisualLine {
            text: std::mem::take(&mut current.text),
            logical_line,
            start_char: current.start_char,
            visible_start_char: current.visible_start_char,
            end_char: current.end_char,
            column_offsets: std::mem::take(&mut current.column_offsets),
        });
        *current = PromptLineBuilder::default();
    };

    for index in 0..blocks.len() {
        let block = &blocks[index];
        loop {
            let at_line_start = !current.has_content;
            let block_width = block.visible_width(at_line_start);
            if current.width + block_width <= current_limit {
                if should_reflow_exact_fit_block(
                    &current,
                    block,
                    &blocks[index + 1..],
                    current_limit,
                    at_line_start,
                    continuation_width,
                ) {
                    flush_current(&mut lines, &mut current);
                    current_limit = continuation_width;
                    continue;
                }

                current.append_block(block, at_line_start);
                break;
            }

            if !at_line_start {
                flush_current(&mut lines, &mut current);
                current_limit = continuation_width;
                continue;
            }

            let hard_wrapped = hard_wrap_prompt_visible_text(
                &block.visible_text(true),
                current_limit,
                continuation_width,
                block.raw_start_char(),
                block.visible_start_char(true),
                logical_line,
            );
            if hard_wrapped.is_empty() {
                break;
            }

            if hard_wrapped.len() > 1 {
                lines.extend(hard_wrapped[..hard_wrapped.len() - 1].iter().cloned());
            }

            let last = hard_wrapped
                .last()
                .cloned()
                .expect("hard wrap should keep the final fragment");
            current = PromptLineBuilder::from_visual_line(last);
            current_limit = continuation_width;
            break;
        }
    }

    if current.has_content {
        flush_current(&mut lines, &mut current);
    }
    if lines.is_empty() {
        return vec![PromptVisualLine {
            logical_line,
            start_char: base_char_offset,
            visible_start_char: base_char_offset,
            end_char: base_char_offset,
            column_offsets: vec![0],
            ..PromptVisualLine::default()
        }];
    }

    lines
}

fn should_reflow_exact_fit_block(
    current: &PromptLineBuilder,
    block: &PromptWordBlock,
    remaining: &[PromptWordBlock],
    current_limit: usize,
    at_line_start: bool,
    continuation_width: usize,
) -> bool {
    if at_line_start || continuation_width == 0 || remaining.is_empty() {
        return false;
    }
    if current.width + block.visible_width(false) != current_limit {
        return false;
    }

    let next_block = &remaining[0];
    if !next_block.has_leading_space || next_block.leading_spaces.width <= 1 {
        return false;
    }

    block.visible_width(true) + next_block.visible_width(false) <= continuation_width
}

impl PromptLineBuilder {
    fn append_block(&mut self, block: &PromptWordBlock, at_line_start: bool) {
        if !self.has_content {
            self.start_char = block.raw_start_char();
            self.visible_start_char = block.visible_start_char(at_line_start);
            self.end_char = self.visible_start_char;
            self.column_offsets = vec![0];
        }

        self.append_segment(
            &block.visible_text(at_line_start),
            block.visible_width(at_line_start),
        );
        self.end_char = block.raw_end_char();
        self.has_content = true;
    }

    fn append_segment(&mut self, text: &str, width: usize) {
        self.text.push_str(text);
        self.width += width;
        self.column_offsets = append_column_offset_run(
            std::mem::take(&mut self.column_offsets),
            text.chars().count(),
            width,
        );
    }

    fn from_visual_line(line: PromptVisualLine) -> Self {
        Self {
            width: measure_width(&line.text),
            start_char: line.start_char,
            visible_start_char: line.visible_start_char,
            end_char: line.end_char,
            column_offsets: line.column_offsets,
            has_content: !line.text.is_empty(),
            text: line.text,
        }
    }
}

impl PromptWordBlock {
    fn visible_width(&self, at_line_start: bool) -> usize {
        let mut width = self.word.width;
        if self.should_render_leading_spaces(at_line_start) {
            width += self.leading_spaces.width;
        }
        if self.has_trailing {
            width += self.trailing_spaces.width;
        }
        width
    }

    fn visible_text(&self, at_line_start: bool) -> String {
        let mut text = String::new();
        if self.should_render_leading_spaces(at_line_start) {
            text.push_str(&self.leading_spaces.text);
        }
        text.push_str(&self.word.text);
        if self.has_trailing {
            text.push_str(&self.trailing_spaces.text);
        }
        text
    }

    fn raw_start_char(&self) -> usize {
        if self.has_leading_space {
            self.leading_spaces.start_char
        } else {
            self.word.start_char
        }
    }

    fn visible_start_char(&self, at_line_start: bool) -> usize {
        if at_line_start && self.has_leading_space && self.leading_spaces.width == 1 {
            return self.word.start_char;
        }

        self.raw_start_char()
    }

    fn should_render_leading_spaces(&self, at_line_start: bool) -> bool {
        if !self.has_leading_space {
            return false;
        }
        if !at_line_start {
            return true;
        }

        self.leading_spaces.width > 1
    }

    fn raw_end_char(&self) -> usize {
        if self.has_trailing {
            self.trailing_spaces.start_char + self.trailing_spaces.char_count
        } else {
            self.word.start_char + self.word.char_count
        }
    }
}

fn build_prompt_word_blocks(segments: &[DetailedWrapSegment]) -> Vec<PromptWordBlock> {
    let mut blocks = Vec::with_capacity(segments.len() / 2 + 1);
    let mut pending_spaces = DetailedWrapSegment::default();
    let mut has_pending_spaces = false;
    let mut index = 0;

    while index < segments.len() {
        let segment = &segments[index];
        if segment.is_space {
            pending_spaces = segment.clone();
            has_pending_spaces = true;
            index += 1;
            continue;
        }

        let mut block = PromptWordBlock {
            leading_spaces: pending_spaces.clone(),
            word: segment.clone(),
            trailing_spaces: DetailedWrapSegment::default(),
            has_leading_space: has_pending_spaces,
            has_trailing: false,
        };
        pending_spaces = DetailedWrapSegment::default();
        has_pending_spaces = false;

        if index + 1 < segments.len() && segments[index + 1].is_space && index + 2 >= segments.len()
        {
            block.trailing_spaces = segments[index + 1].clone();
            block.has_trailing = true;
            index += 1;
        }

        blocks.push(block);
        index += 1;
    }

    blocks
}

fn offset_prompt_word_blocks(
    blocks: &[PromptWordBlock],
    char_offset: usize,
) -> Vec<PromptWordBlock> {
    if char_offset == 0 {
        return blocks.to_vec();
    }

    blocks
        .iter()
        .cloned()
        .map(|mut block| {
            block.leading_spaces.start_char += char_offset;
            block.word.start_char += char_offset;
            block.trailing_spaces.start_char += char_offset;
            block
        })
        .collect()
}

fn tokenize_detailed_segments(text: &str) -> Vec<DetailedWrapSegment> {
    if text.is_empty() {
        return Vec::new();
    }

    let mut segments = Vec::new();
    let mut current = String::new();
    let mut current_width = 0;
    let mut current_chars = 0;
    let mut current_start_char = 0;
    let mut current_is_space = false;
    let mut has_current = false;
    let mut chars_consumed = 0;

    let flush_current = |segments: &mut Vec<DetailedWrapSegment>,
                         current: &mut String,
                         current_width: &mut usize,
                         current_chars: &mut usize,
                         current_start_char: usize,
                         current_is_space: bool,
                         has_current: &mut bool| {
        if !*has_current {
            return;
        }

        segments.push(DetailedWrapSegment {
            text: std::mem::take(current),
            width: *current_width,
            start_char: current_start_char,
            char_count: *current_chars,
            is_space: current_is_space,
        });
        *current_width = 0;
        *current_chars = 0;
        *has_current = false;
    };

    for cluster in UnicodeSegmentation::graphemes(text, true) {
        let cluster_width = measure_width(cluster);
        let cluster_chars = cluster.chars().count();
        let cluster_is_space = is_space_cluster(cluster);

        if !has_current {
            current_start_char = chars_consumed;
            current_is_space = cluster_is_space;
            has_current = true;
        } else if current_is_space != cluster_is_space {
            flush_current(
                &mut segments,
                &mut current,
                &mut current_width,
                &mut current_chars,
                current_start_char,
                current_is_space,
                &mut has_current,
            );
            current_start_char = chars_consumed;
            current_is_space = cluster_is_space;
            has_current = true;
        }

        current.push_str(cluster);
        current_width += cluster_width;
        current_chars += cluster_chars;
        chars_consumed += cluster_chars;
    }

    flush_current(
        &mut segments,
        &mut current,
        &mut current_width,
        &mut current_chars,
        current_start_char,
        current_is_space,
        &mut has_current,
    );

    segments
}

fn hard_wrap_prompt_visible_text(
    text: &str,
    first_width: usize,
    continuation_width: usize,
    raw_start_char: usize,
    visible_start_char: usize,
    logical_line: usize,
) -> Vec<PromptVisualLine> {
    if text.is_empty() {
        return Vec::new();
    }

    let mut lines = Vec::with_capacity((measure_width(text) / first_width.max(1)).max(1));
    let mut current_limit = first_width.max(1);
    let continuation_width = continuation_width.max(1);
    let mut current = PromptLineBuilder {
        start_char: raw_start_char,
        visible_start_char,
        end_char: visible_start_char,
        column_offsets: vec![0],
        ..PromptLineBuilder::default()
    };

    let flush_current = |lines: &mut Vec<PromptVisualLine>,
                         current: &mut PromptLineBuilder,
                         current_limit: &mut usize| {
        if !current.has_content {
            return;
        }

        let next_start = current.end_char;
        lines.push(PromptVisualLine {
            text: std::mem::take(&mut current.text),
            logical_line,
            start_char: current.start_char,
            visible_start_char: current.visible_start_char,
            end_char: current.end_char,
            column_offsets: std::mem::take(&mut current.column_offsets),
        });
        *current = PromptLineBuilder {
            start_char: next_start,
            visible_start_char: next_start,
            end_char: next_start,
            column_offsets: vec![0],
            ..PromptLineBuilder::default()
        };
        *current_limit = continuation_width;
    };

    for cluster in UnicodeSegmentation::graphemes(text, true) {
        let cluster_width = measure_width(cluster);
        let cluster_chars = cluster.chars().count();

        if current.width + cluster_width > current_limit && current.has_content {
            flush_current(&mut lines, &mut current, &mut current_limit);
        }

        current.text.push_str(cluster);
        current.width += cluster_width;
        current.end_char += cluster_chars;
        current.column_offsets = append_column_offset_run(
            std::mem::take(&mut current.column_offsets),
            cluster_chars,
            cluster_width,
        );
        current.has_content = true;
    }

    flush_current(&mut lines, &mut current, &mut current_limit);
    lines
}

fn wrap_prompt_literal_line_with_tabs(
    line: &str,
    width: usize,
    line_prefix_width: usize,
    logical_line: usize,
) -> Vec<PromptVisualLine> {
    if line.is_empty() || width == 0 {
        return vec![PromptVisualLine {
            text: line.to_string(),
            logical_line,
            start_char: 0,
            visible_start_char: 0,
            end_char: line.chars().count(),
            column_offsets: build_column_offsets(line),
        }];
    }

    let mut lines = Vec::with_capacity(line.chars().count().max(1));
    let mut current = LiteralPromptLineBuilder::default();
    let current_limit = width.max(1);
    let mut current_width_limit = current_limit;

    let flush_current = |lines: &mut Vec<PromptVisualLine>,
                         current: &mut LiteralPromptLineBuilder| {
        if !current.has_content {
            return;
        }

        lines.push(PromptVisualLine {
            text: std::mem::take(&mut current.text),
            logical_line,
            start_char: current.start_char,
            visible_start_char: current.visible_start_char,
            end_char: current.end_char,
            column_offsets: std::mem::take(&mut current.column_offsets),
        });
        *current = LiteralPromptLineBuilder::default();
    };

    let mut char_offset = 0;
    for cluster in UnicodeSegmentation::graphemes(line, true) {
        let cluster_chars = cluster.chars().count();
        let cluster_start_char = char_offset;
        char_offset += cluster_chars;

        let mut rendered_cluster = String::new();
        loop {
            if current.width >= current_width_limit && current.has_content {
                flush_current(&mut lines, &mut current);
            }
            if rendered_cluster.is_empty() {
                rendered_cluster =
                    render_cluster_for_display(cluster, line_prefix_width + current.width).0;
            }

            let available_width = current_width_limit.saturating_sub(current.width);
            if cluster != "\t"
                && current.has_content
                && measure_width(&rendered_cluster) > available_width
            {
                flush_current(&mut lines, &mut current);
                rendered_cluster.clear();
                continue;
            }

            let (mut fitted, mut overflow) =
                split_text_to_width(&rendered_cluster, available_width);
            if fitted.is_empty() {
                fitted = rendered_cluster.clone();
                overflow.clear();
            }

            current.append_fragment(&fitted, cluster_start_char, cluster_chars);
            if overflow.is_empty() {
                break;
            }

            rendered_cluster = overflow;
            flush_current(&mut lines, &mut current);
            current_width_limit = current_limit;
        }
    }

    if current.has_content {
        flush_current(&mut lines, &mut current);
    }
    if lines.is_empty() {
        return vec![PromptVisualLine {
            logical_line,
            column_offsets: vec![0],
            ..PromptVisualLine::default()
        }];
    }

    lines
}

impl LiteralPromptLineBuilder {
    fn append_fragment(&mut self, rendered: &str, start_char: usize, char_count: usize) {
        if !self.has_content {
            self.start_char = start_char;
            self.visible_start_char = start_char;
            self.end_char = start_char;
            self.column_offsets = vec![0];
        }

        self.text.push_str(rendered);
        self.width += measure_width(rendered);
        self.end_char = start_char + char_count;
        self.column_offsets = append_column_offset_run(
            std::mem::take(&mut self.column_offsets),
            char_count,
            measure_width(rendered),
        );
        self.has_content = true;
    }
}

fn build_column_offsets(text: &str) -> Vec<usize> {
    #[cfg(test)]
    COLUMN_OFFSET_REBUILD_CALL_COUNT.with(|count| count.set(count.get() + 1));

    let mut offsets = vec![0];
    for cluster in UnicodeSegmentation::graphemes(text, true) {
        offsets =
            append_column_offset_run(offsets, cluster.chars().count(), measure_width(cluster));
    }
    offsets
}

#[cfg(test)]
fn reset_column_offset_rebuild_call_count() {
    COLUMN_OFFSET_REBUILD_CALL_COUNT.with(|count| count.set(0));
}

#[cfg(test)]
fn column_offset_rebuild_call_count() -> usize {
    COLUMN_OFFSET_REBUILD_CALL_COUNT.with(std::cell::Cell::get)
}

fn append_column_offset_run(
    mut offsets: Vec<usize>,
    char_count: usize,
    width: usize,
) -> Vec<usize> {
    if offsets.is_empty() {
        offsets.push(0);
    }

    let current_width = *offsets.last().unwrap_or(&0);
    for _ in 1..char_count {
        offsets.push(current_width);
    }
    offsets.push(current_width + width);
    offsets
}

fn should_hard_wrap_prompt_line(line: &str) -> bool {
    leading_space_count(line) >= 4
}

#[cfg(test)]
mod tests {
    use unicode_segmentation::UnicodeSegmentation;

    use super::{
        column_offset_rebuild_call_count, reset_column_offset_rebuild_call_count,
        wrap_prompt_visual_lines,
    };
    use crate::frontend::tui::transcript::wrap::measure_width;

    #[test]
    fn wrap_prompt_visual_lines_preserves_basic_invariants_across_seed_cases() {
        let cases = [
            ("hello world", 10, 2),
            (" abc def", 5, 0),
            ("a\tb", 8, 2),
            ("中文和 emoji 👨‍👩‍👧", 6, 0),
        ];

        for (value, width, line_prefix_width) in cases {
            assert_prompt_wrap_invariants(value, width, line_prefix_width);
        }
    }

    #[test]
    fn wrap_prompt_visual_lines_preserves_invariants_across_generated_cases() {
        for (value, width, line_prefix_width) in generated_prompt_cases() {
            assert_prompt_wrap_invariants(&value, width, line_prefix_width);
        }
    }

    #[test]
    #[ignore = "performance smoke test"]
    fn wrap_prompt_visual_lines_perf_smoke() {
        use std::hint::black_box;

        let prose = "the composer should preserve wrapped words and cursor anchors across resize "
            .repeat(8);
        let literal =
            "\tfunc benchmark() error {\n\t\treturn render\tviewport\tanchors\n\t}".to_string();

        for _ in 0..256 {
            black_box(wrap_prompt_visual_lines(&prose, 36, 2));
            black_box(wrap_prompt_visual_lines(&literal, 24, 2));
        }
    }

    #[test]
    fn wrap_prompt_visual_lines_does_not_rebuild_column_offsets_per_word_block() {
        let prose = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu ".repeat(8);

        reset_column_offset_rebuild_call_count();
        let lines = wrap_prompt_visual_lines(&prose, 24, 2);

        assert!(lines.len() > 8);
        assert!(
            column_offset_rebuild_call_count() <= 2,
            "column offsets should be maintained incrementally instead of rebuilding for every appended word block"
        );
    }

    fn assert_prompt_wrap_invariants(value: &str, width: usize, line_prefix_width: usize) {
        let lines = wrap_prompt_visual_lines(value, width, line_prefix_width);
        assert!(!lines.is_empty(), "wrapped lines should not be empty");

        let mut previous_logical_line = None;
        let mut previous_end_char = 0usize;

        for (index, line) in lines.iter().enumerate() {
            if measure_width(&line.text) > width.max(1) {
                assert!(
                    line.text.graphemes(true).count() <= 1,
                    "line {index} width {} exceeded content width {width}: {:?}",
                    measure_width(&line.text),
                    line.text
                );
            }
            assert!(
                line.start_char <= line.visible_start_char
                    && line.visible_start_char <= line.end_char,
                "line {index} has invalid char range: start={} visible={} end={}",
                line.start_char,
                line.visible_start_char,
                line.end_char
            );

            if let Some(previous_logical_line) = previous_logical_line {
                assert!(
                    line.logical_line >= previous_logical_line,
                    "line {index} logical line regressed from {previous_logical_line} to {}",
                    line.logical_line
                );
                if line.logical_line == previous_logical_line {
                    assert!(
                        line.end_char >= previous_end_char,
                        "line {index} end char regressed within logical line: prev end={previous_end_char} end={}",
                        line.end_char
                    );
                }
            }

            previous_logical_line = Some(line.logical_line);
            previous_end_char = line.end_char;
        }
    }

    fn generated_prompt_cases() -> Vec<(String, usize, usize)> {
        let segments = ["a", "b", " ", "  ", "\t", "\n", "中", "文", "👨‍👩‍👧", "emoji"];
        let mut seed = 0x5EED_u64;
        let mut cases = Vec::new();

        for _ in 0..48 {
            let len = next_u32(&mut seed) as usize % 18;
            let mut value = String::new();
            for _ in 0..len {
                let index = next_u32(&mut seed) as usize % segments.len();
                value.push_str(segments[index]);
            }

            let width = (next_u32(&mut seed) as usize % 32) + 1;
            let line_prefix_width = next_u32(&mut seed) as usize % 8;
            cases.push((value, width, line_prefix_width));
        }

        cases
    }

    fn next_u32(seed: &mut u64) -> u32 {
        *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        (*seed >> 32) as u32
    }
}
