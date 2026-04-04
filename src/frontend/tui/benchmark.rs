use std::fmt::Write as _;

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
    layout: super::document::DocumentLayout,
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
            if index % 3 == 0 {
                model.transcript_mut().append_message_with_style_mode(
                    Sender::User,
                    benchmark_user_message(index),
                    StyleMode::Cx,
                );
            } else {
                model.transcript_mut().append_message_with_style_mode(
                    Sender::Assistant,
                    benchmark_assistant_markdown(index),
                    StyleMode::Cx,
                );
            }
        }

        model.set_window(width, height);
        model.set_palette(default_palette(), true);
        model
            .composer_mut()
            .replace_text_and_move_to_end(benchmark_composer_draft_for_document());
        model.sync_composer_height();
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

    /// `build_offset_viewport` 使用手动滚动 offset 构造 viewport 并返回稳定摘要。
    pub fn build_offset_viewport(&mut self) -> DocumentViewportSummary {
        self.model.follow_bottom = false;
        self.model.manual_document_scroll = true;
        self.model.document_viewport_cache = Default::default();
        self.model.document_viewport_y = self.model.clamp_document_viewport_offset(
            self.layout.line_count().saturating_sub(12),
            self.layout.line_count(),
        );

        summarize_document_viewport(&self.model.build_document_viewport(&self.layout))
    }

    /// `build_bottom_follow_viewport` 使用 bottom-follow 语义构造 viewport 并返回稳定摘要。
    pub fn build_bottom_follow_viewport(&mut self) -> DocumentViewportSummary {
        self.model.follow_bottom = true;
        self.model.manual_document_scroll = false;
        self.model.document_viewport_y = 0;
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
        plain_text_len: plain_lines_len(&layout.plain_lines),
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
        plain_text_len: plain_lines_len(&viewport.plain_lines),
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
