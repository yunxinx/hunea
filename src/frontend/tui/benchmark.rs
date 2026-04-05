use std::fmt::Write as _;
use std::rc::Rc;

use ratatui::{Terminal, backend::TestBackend};

use super::{
    HeroOptions, Model, ModelOptions, Sender, StyleMode,
    composer::Composer,
    styled_text::lines_to_plain_text,
    theme::{TerminalPalette, default_palette},
    transcript::{Transcript, render_markdown_lines, wrap_prompt_visual_lines},
};

/// `TextRenderSummary` 收敛一类文本渲染 benchmark 的稳定输出特征。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextRenderSummary {
    pub line_count: usize,
    pub plain_text_len: usize,
    pub span_count: usize,
}

/// `PromptWrapSummary` 收敛 prompt wrap benchmark 的稳定输出特征。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptWrapSummary {
    pub line_count: usize,
    pub text_len: usize,
    pub last_end_char: usize,
}

/// `TranscriptRenderSummary` 收敛 transcript render benchmark 的稳定输出特征。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranscriptRenderSummary {
    pub line_count: usize,
    pub plain_text_len: usize,
    pub anchor_count: usize,
    pub selectable_count: usize,
    pub append_start_line: isize,
}

/// `ComposerRenderSummary` 收敛 composer render benchmark 的稳定输出特征。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComposerRenderSummary {
    pub line_count: usize,
    pub plain_text_len: usize,
    pub anchor_count: usize,
    pub selectable_count: usize,
    pub cursor_x: u16,
    pub cursor_y: usize,
}

/// `DocumentLayoutSummary` 收敛 unified document layout benchmark 的稳定输出特征。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocumentLayoutSummary {
    pub line_count: usize,
    pub plain_text_len: usize,
    pub transcript_line_count: usize,
    pub composer_line_count: usize,
    pub cursor_x: u16,
    pub cursor_y: usize,
}

/// `DocumentViewportSummary` 收敛 unified document viewport benchmark 的稳定输出特征。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocumentViewportSummary {
    pub line_count: usize,
    pub plain_text_len: usize,
    pub resolved_offset: usize,
}

/// `FrameRenderSummary` 收敛整帧渲染 benchmark 的稳定输出特征。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameRenderSummary {
    pub non_empty_cells: usize,
    pub width: u16,
    pub height: u16,
}

/// `DocumentStressSummary` 收敛超大 transcript 下 document pipeline 的冷路径测量结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentStressSummary {
    pub item_count: usize,
    pub width: u16,
    pub height: u16,
    pub transcript_line_count: usize,
    pub document_line_count: usize,
    pub viewport_line_count: usize,
    pub frame_non_empty_cells: usize,
    pub transcript_render_time: std::time::Duration,
    pub document_layout_time: std::time::Duration,
    pub document_viewport_time: std::time::Duration,
    pub frame_render_time: std::time::Duration,
    pub rss_before_kib: Option<usize>,
    pub rss_after_transcript_kib: Option<usize>,
    pub rss_after_layout_kib: Option<usize>,
    pub rss_after_viewport_kib: Option<usize>,
    pub rss_after_frame_kib: Option<usize>,
}

/// `TranscriptBench` 封装 transcript 渲染 benchmark 所需的状态。
#[derive(Debug, Clone)]
pub struct TranscriptBench {
    transcript: Transcript,
    next_index: usize,
}

/// `DocumentBench` 封装 unified document benchmark 所需的状态。
#[derive(Debug, Clone)]
pub struct DocumentBench {
    model: Model,
    layout: Rc<super::document::DocumentLayout>,
    next_index: usize,
}

/// `ModelRenderBench` 封装整帧渲染 benchmark 所需的状态。
#[derive(Debug)]
pub struct ModelRenderBench {
    model: Model,
    terminal: Terminal<TestBackend>,
}

/// `markdown_document_fixture` 返回与 Go benchmark 对齐的 assistant markdown 文本。
pub fn markdown_document_fixture() -> String {
    let mut sections = Vec::with_capacity(6);

    for index in 0..6 {
        sections.push(format!(
            "## Section {index}\n\n- summarize the latest transcript cache behavior\n- explain why viewport anchors stay stable across resize\n- keep the markdown renderer width-aware\n\n```rust\nfn section_{index}() -> Result<(), &'static str> {{\n    Err(\"{}\")\n}}\n```\n",
            "benchmark content ".repeat(6)
        ));
    }

    sections.join("\n")
}

/// `prompt_prose_fixture` 返回与 Go benchmark 对齐的 prose prompt 文本。
pub fn prompt_prose_fixture() -> String {
    "the composer should preserve wrapped words and cursor anchors across resize ".repeat(8)
}

/// `prompt_tabbed_literal_fixture` 返回与 Go benchmark 对齐的 literal-tabs prompt 文本。
pub fn prompt_tabbed_literal_fixture() -> String {
    [
        "\tfunc benchmark() error {",
        "\t\treturn render\tviewport\tanchors",
        "\t}",
    ]
    .join("\n")
}

/// `composer_draft_fixture` 返回与 Go benchmark 对齐的 composer draft 文本。
pub fn composer_draft_fixture() -> String {
    [
        "draft heading for transcript and composer benchmark".to_string(),
        String::new(),
        "soft wrap should stay stable under repeated rendering ".repeat(3),
        "    indented literal line with spaces".to_string(),
        "\tindented literal line with tabs".to_string(),
        "中文内容需要继续参与真实宽度计算。".to_string(),
        "emoji cluster 👨‍👩‍👧 should keep cursor mapping correct".to_string(),
        "line eight keeps the input tall enough to exercise viewport math".to_string(),
        "line nine keeps the document renderer allocating multiple visual rows".to_string(),
        "line ten keeps the cursor near the bottom of the draft".to_string(),
        "benchmark final line with emoji 👨‍👩‍👧 and trailing text".to_string(),
    ]
    .join("\n")
}

/// `rendered_block_fixture` 返回 transcript 列表 benchmark 使用的稳定块文本。
pub fn rendered_block_fixture(index: usize) -> String {
    format!(
        "item {index:02}\n{}\n{}",
        "alpha beta gamma ".repeat(3),
        "delta epsilon zeta ".repeat(2),
    )
}

/// `render_markdown_plain_text` 运行 markdown 渲染并返回稳定摘要。
pub fn render_markdown_plain_text(
    markdown: &str,
    width: usize,
    palette: TerminalPalette,
) -> TextRenderSummary {
    summarize_text_lines(&render_markdown_lines(markdown, width, palette))
}

/// `wrap_prompt_visual_lines_summary` 运行 prompt wrap 并返回稳定摘要。
pub fn wrap_prompt_visual_lines_summary(
    value: &str,
    width: usize,
    line_prefix_width: usize,
) -> PromptWrapSummary {
    let lines = wrap_prompt_visual_lines(value, width, line_prefix_width);

    PromptWrapSummary {
        line_count: lines.len(),
        text_len: lines.iter().map(|line| line.text.len()).sum(),
        last_end_char: lines.last().map(|line| line.end_char).unwrap_or(0),
    }
}

/// `render_composer_document_with_input` 运行 composer document render 并返回稳定摘要。
pub fn render_composer_document_with_input(
    value: &str,
    width: u16,
    style_mode: StyleMode,
    palette: TerminalPalette,
) -> ComposerRenderSummary {
    let mut composer = Composer::new(style_mode);
    composer.set_width(width);
    composer.replace_text_and_move_to_end(value);

    let document = composer.render_document(palette);
    ComposerRenderSummary {
        line_count: document.lines.len(),
        plain_text_len: plain_lines_len(&document.plain_lines),
        anchor_count: document.anchors.len(),
        selectable_count: document.selectable_ranges.len(),
        cursor_x: document.cursor_x,
        cursor_y: document.cursor_y,
    }
}

/// `measure_document_pipeline_stress` 测量大规模 transcript 下的 document 冷路径。
pub fn measure_document_pipeline_stress(
    item_count: usize,
    width: u16,
    height: u16,
) -> DocumentStressSummary {
    let mut model = new_stress_document_model(item_count, width, height);
    let mut terminal = Terminal::new(TestBackend::new(width, height))
        .expect("stress benchmark backend should initialize");

    let rss_before_kib = process_rss_kib();

    let transcript_render_started_at = std::time::Instant::now();
    model.sync_transcript_render();
    let transcript_render_time = transcript_render_started_at.elapsed();
    let rss_after_transcript_kib = process_rss_kib();

    let document_layout_started_at = std::time::Instant::now();
    let layout = model.build_document_layout();
    let document_layout_time = document_layout_started_at.elapsed();
    let rss_after_layout_kib = process_rss_kib();

    let document_viewport_started_at = std::time::Instant::now();
    let viewport = model.build_document_viewport(&layout);
    let document_viewport_time = document_viewport_started_at.elapsed();
    let rss_after_viewport_kib = process_rss_kib();

    let frame_render_started_at = std::time::Instant::now();
    terminal
        .draw(|frame| model.render(frame))
        .expect("stress benchmark frame render should succeed");
    let frame_render_time = frame_render_started_at.elapsed();
    let rss_after_frame_kib = process_rss_kib();

    let buffer = terminal.backend().buffer();
    let frame_non_empty_cells = (0..buffer.area.height)
        .flat_map(|row| (0..buffer.area.width).map(move |column| (column, row)))
        .filter(|&(column, row)| buffer[(column, row)].symbol() != " ")
        .count();

    DocumentStressSummary {
        item_count,
        width,
        height,
        transcript_line_count: model.transcript_render.line_count,
        document_line_count: layout.line_count(),
        viewport_line_count: viewport.lines.len(),
        frame_non_empty_cells,
        transcript_render_time,
        document_layout_time,
        document_viewport_time,
        frame_render_time,
        rss_before_kib,
        rss_after_transcript_kib,
        rss_after_layout_kib,
        rss_after_viewport_kib,
        rss_after_frame_kib,
    }
}

/// `format_document_stress_summary` 输出便于人工比较的 stress 摘要。
pub fn format_document_stress_summary(summary: &DocumentStressSummary) -> String {
    format!(
        "items={items} size={width}x{height} transcript_lines={transcript_lines} document_lines={document_lines} viewport_lines={viewport_lines} frame_cells={frame_cells} timings_ms={{render:{render:.3}, layout:{layout:.3}, viewport:{viewport:.3}, frame:{frame:.3}}} rss_kib={{before:{rss_before:?}, after_render:{rss_render:?}, after_layout:{rss_layout:?}, after_viewport:{rss_viewport:?}, after_frame:{rss_frame:?}}}",
        items = summary.item_count,
        width = summary.width,
        height = summary.height,
        transcript_lines = summary.transcript_line_count,
        document_lines = summary.document_line_count,
        viewport_lines = summary.viewport_line_count,
        frame_cells = summary.frame_non_empty_cells,
        render = summary.transcript_render_time.as_secs_f64() * 1000.0,
        layout = summary.document_layout_time.as_secs_f64() * 1000.0,
        viewport = summary.document_viewport_time.as_secs_f64() * 1000.0,
        frame = summary.frame_render_time.as_secs_f64() * 1000.0,
        rss_before = summary.rss_before_kib,
        rss_render = summary.rss_after_transcript_kib,
        rss_layout = summary.rss_after_layout_kib,
        rss_viewport = summary.rss_after_viewport_kib,
        rss_frame = summary.rss_after_frame_kib,
    )
}

impl TranscriptBench {
    /// `new` 创建一个与 Go transcript benchmark 场景对齐的 transcript bench。
    pub fn new(item_count: usize, width: u16, palette: TerminalPalette) -> Self {
        let mut transcript = Transcript::new(palette);
        transcript.set_gap(1);
        transcript.set_width(width.max(1));

        for index in 0..item_count {
            append_transcript_benchmark_item(&mut transcript, index);
        }

        Self {
            transcript,
            next_index: item_count,
        }
    }

    /// `render` 渲染当前 transcript 并返回稳定摘要。
    pub fn render(&mut self) -> TranscriptRenderSummary {
        summarize_transcript_render(&self.transcript.render())
    }

    /// `append_benchmark_item_and_render` 追加一项并测量 append fast path。
    pub fn append_benchmark_item_and_render(&mut self) -> TranscriptRenderSummary {
        append_transcript_benchmark_item(&mut self.transcript, self.next_index);
        self.next_index += 1;
        summarize_transcript_render(&self.transcript.render())
    }
}

impl DocumentBench {
    /// `new` 创建一个与 Go unified document benchmark 场景对齐的 document bench。
    pub fn new(item_count: usize, width: u16, height: u16) -> Self {
        let mut model = new_stress_document_model(item_count, width, height);
        model.sync_transcript_render();

        let layout = model.build_document_layout();
        Self {
            model,
            layout,
            next_index: item_count,
        }
    }

    /// `rebuild_layout` 强制重建 unified document layout 并返回稳定摘要。
    pub fn rebuild_layout(&mut self) -> DocumentLayoutSummary {
        self.model.document_transcript_cache = Default::default();
        self.model.document_layout_cache = Default::default();
        let layout = self.model.build_document_layout();
        let summary = summarize_document_layout(&layout);
        self.layout = layout;
        summary
    }

    /// `prepare_offset_viewport_state` 把模型切到固定的手动滚动 viewport 状态。
    pub fn prepare_offset_viewport_state(&mut self) {
        let document_offset = self.model.clamp_document_viewport_offset(
            self.layout.line_count().saturating_sub(12),
            self.layout.line_count(),
        );
        let composer_offset = self
            .model
            .current_composer_viewport_offset(&self.layout, document_offset);
        self.model.apply_document_viewport_position(
            &self.layout,
            document_offset,
            composer_offset,
            false,
            true,
        );
    }

    /// `build_offset_viewport` 使用手动滚动 offset 构造 viewport 并返回稳定摘要。
    pub fn build_offset_viewport(&mut self) -> DocumentViewportSummary {
        self.model.document_viewport_cache = Default::default();

        summarize_document_viewport(&self.model.build_document_viewport(&self.layout))
    }

    /// `prepare_bottom_follow_viewport_state` 把模型切到底部跟随状态。
    pub fn prepare_bottom_follow_viewport_state(&mut self) {
        let (document_offset, composer_offset) =
            self.model.bottom_follow_viewport_offsets(&self.layout);
        self.model.apply_document_viewport_position(
            &self.layout,
            document_offset,
            composer_offset,
            true,
            false,
        );
    }

    /// `build_bottom_follow_viewport` 使用 bottom-follow 语义构造 viewport 并返回稳定摘要。
    pub fn build_bottom_follow_viewport(&mut self) -> DocumentViewportSummary {
        self.model.document_viewport_cache = Default::default();

        summarize_document_viewport(&self.model.build_document_viewport(&self.layout))
    }

    /// `rebuild_layout_after_transcript_append` 追加 transcript 后重建 layout 并返回稳定摘要。
    pub fn rebuild_layout_after_transcript_append(&mut self) -> DocumentLayoutSummary {
        self.model.transcript_mut().append_message_with_style_mode(
            Sender::Assistant,
            benchmark_assistant_markdown(self.next_index),
            StyleMode::Cx,
        );
        self.next_index += 1;
        self.model.sync_transcript_render();

        let layout = self.model.build_document_layout();
        let summary = summarize_document_layout(&layout);
        self.layout = layout;
        summary
    }

    /// `rebuild_layout_after_composer_edit` 模拟草稿编辑后重建 layout 并返回稳定摘要。
    pub fn rebuild_layout_after_composer_edit(&mut self) -> DocumentLayoutSummary {
        self.model.composer_mut().insert_text("x");
        self.model.sync_composer_height();

        let layout = self.model.build_document_layout();
        let summary = summarize_document_layout(&layout);
        self.layout = layout;
        summary
    }
}

impl ModelRenderBench {
    /// `new` 创建一个整帧 render benchmark 场景。
    pub fn new(item_count: usize, width: u16, height: u16) -> Self {
        let model = DocumentBench::new(item_count, width, height).model;
        let terminal = Terminal::new(TestBackend::new(width, height))
            .expect("benchmark backend should initialize");

        Self { model, terminal }
    }

    /// `render_frame` 运行一帧 Ratatui 渲染并返回稳定摘要。
    pub fn render_frame(&mut self) -> FrameRenderSummary {
        self.terminal
            .draw(|frame| self.model.render(frame))
            .expect("benchmark frame render should succeed");

        let buffer = self.terminal.backend().buffer();
        let non_empty_cells = (0..buffer.area.height)
            .flat_map(|row| (0..buffer.area.width).map(move |column| (column, row)))
            .filter(|&(column, row)| buffer[(column, row)].symbol() != " ")
            .count();

        FrameRenderSummary {
            non_empty_cells,
            width: buffer.area.width,
            height: buffer.area.height,
        }
    }
}

fn new_stress_document_model(item_count: usize, width: u16, height: u16) -> Model {
    let mut model = Model::new_with_options(
        HeroOptions {
            app_name: Some("lumos".to_string()),
            version: Some("dev".to_string()),
            work_dir: Some("/tmp/lumos".to_string()),
            width: 0,
        },
        ModelOptions {
            style_mode: StyleMode::Cx,
            ..ModelOptions::default()
        },
    );
    model.transcript_mut().clear();

    for index in 0..item_count {
        append_transcript_benchmark_item(model.transcript_mut(), index);
    }

    model.set_window(width, height);
    model.set_palette(default_palette(), true);
    model
        .composer_mut()
        .replace_text_and_move_to_end(benchmark_composer_draft_for_document());
    model.sync_composer_height();
    model
}

fn process_rss_kib() -> Option<usize> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|line| {
        let value = line.strip_prefix("VmRSS:")?;
        value
            .split_whitespace()
            .next()
            .and_then(|number| number.parse::<usize>().ok())
    })
}

fn append_transcript_benchmark_item(transcript: &mut Transcript, index: usize) {
    if index.is_multiple_of(3) {
        transcript.append_message_with_style_mode(
            Sender::User,
            benchmark_user_message(index),
            StyleMode::Cx,
        );
    } else {
        transcript.append_message_with_style_mode(
            Sender::Assistant,
            benchmark_assistant_markdown(index),
            StyleMode::Cx,
        );
    }
}

fn summarize_text_lines(lines: &[ratatui::text::Line<'_>]) -> TextRenderSummary {
    TextRenderSummary {
        line_count: lines.len(),
        plain_text_len: lines_to_plain_text(lines).len(),
        span_count: lines.iter().map(|line| line.spans.len()).sum(),
    }
}

fn summarize_transcript_render(
    render: &super::transcript::RenderResult,
) -> TranscriptRenderSummary {
    TranscriptRenderSummary {
        line_count: render.line_count,
        plain_text_len: plain_lines_len(&render.plain_lines),
        anchor_count: render.line_anchors.len(),
        selectable_count: render.selectable_ranges.len(),
        append_start_line: render.append_start_line,
    }
}

fn summarize_document_layout(layout: &super::document::DocumentLayout) -> DocumentLayoutSummary {
    DocumentLayoutSummary {
        line_count: layout.line_count(),
        plain_text_len: layout.plain_text_len(),
        transcript_line_count: layout.transcript_line_count,
        composer_line_count: layout.composer_line_count,
        cursor_x: layout.cursor_x,
        cursor_y: layout.cursor_y,
    }
}

fn summarize_document_viewport(
    viewport: &super::document::DocumentViewport,
) -> DocumentViewportSummary {
    DocumentViewportSummary {
        line_count: viewport.lines.len(),
        plain_text_len: viewport.plain_text_len,
        resolved_offset: viewport.resolved_offset,
    }
}

fn plain_lines_len(lines: &[String]) -> usize {
    if lines.is_empty() {
        return 0;
    }

    lines.iter().map(String::len).sum::<usize>() + lines.len().saturating_sub(1)
}

fn benchmark_user_message(index: usize) -> String {
    format!(
        "user message {index:02}: {}",
        "keep scrollback anchored while the composer draft keeps growing ".repeat(2),
    )
}

fn benchmark_assistant_markdown(index: usize) -> String {
    let mut content = String::new();
    let _ = writeln!(content, "## Assistant {index:02}");
    let _ = writeln!(content);
    let _ = writeln!(content, "- summarize viewport recovery");
    let _ = writeln!(content, "- explain transcript cache reuse");
    let _ = writeln!(content, "- keep document layout stable");
    let _ = writeln!(content);
    let _ = writeln!(content, "```rust");
    let _ = writeln!(content, "fn assistant_{index}() -> &'static str {{");
    let _ = writeln!(content, "    \"{}\"", "benchmark content ".repeat(6));
    let _ = writeln!(content, "}}");
    let _ = writeln!(content, "```");
    content
}

fn benchmark_composer_draft_for_document() -> String {
    [
        "draft heading for unified document flow benchmark".to_string(),
        "the current draft should span enough rows to exercise composer anchors ".repeat(2),
        "\tindented literal line with tabs and emoji 👨‍👩‍👧".to_string(),
        "中文输入需要继续参与统一文档流的宽度计算。".to_string(),
        "bottom follow should keep the last draft line visible".to_string(),
        "cursor placement should stay near the bottom of the document".to_string(),
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_pipeline_stress_summary_reports_consistent_counts_for_small_fixture() {
        let summary = measure_document_pipeline_stress(24, 80, 18);

        assert_eq!(summary.item_count, 24);
        assert_eq!(summary.width, 80);
        assert_eq!(summary.height, 18);
        assert!(summary.transcript_line_count > 0);
        assert!(summary.document_line_count >= summary.transcript_line_count);
        assert!(summary.viewport_line_count > 0);
        assert!(summary.frame_non_empty_cells > 0);
        assert!(format_document_stress_summary(&summary).contains("items=24"));
    }

    #[test]
    #[ignore = "stress profile for large transcript scales"]
    fn document_pipeline_stress_profiles_up_to_one_million_items() {
        for item_count in [10_000_usize, 100_000_usize, 1_000_000_usize] {
            let summary = measure_document_pipeline_stress(item_count, 80, 24);
            eprintln!("{}", format_document_stress_summary(&summary));

            assert_eq!(summary.item_count, item_count);
            assert!(summary.transcript_line_count > 0);
            assert!(summary.document_line_count >= summary.transcript_line_count);
            assert!(summary.viewport_line_count > 0);
            assert!(summary.frame_non_empty_cells > 0);
        }
    }
}
