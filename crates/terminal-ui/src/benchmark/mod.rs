//! TUI 渲染与滚动性能基准入口。

use std::cell::RefCell;
use std::fmt::Write as _;
use std::io;
use std::mem::{size_of, size_of_val};
use std::rc::Rc;

use ratatui::{
    backend::{Backend, ClearType, WindowSize},
    buffer::Cell,
    layout::{Position, Size},
};

#[cfg(feature = "bench-support")]
use super::message::AssistantProjectionOutcome;
use super::{
    Model, ModelOptions, Sender, StartupBannerOptions, StyleMode,
    composer::Composer,
    document::ViewportState,
    styled_text::lines_to_plain_text,
    theme::{TerminalPalette, default_palette},
    transcript::{
        RenderResult, Transcript, TranscriptItem, render_markdown_lines, wrap_prompt_visual_lines,
    },
};
use crate::runner::terminal_surface::TerminalSurface;

mod tail;
mod terminal_surface;

#[cfg(feature = "bench-support")]
pub use tail::{StreamActivityTailBench, TailLayoutSummary};
#[cfg(feature = "bench-support")]
pub use terminal_surface::{
    TerminalCommandSummary, TerminalFlushBench, TerminalFlushSummary, TerminalGridBench,
    TerminalGridScenario,
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
    pub output_bytes: usize,
    pub flushes: usize,
}

/// `SmoothScrollDrainSummary` 收敛 smooth-scroll drain benchmark 的稳定输出特征。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SmoothScrollDrainSummary {
    /// burst 收敛消耗的 drain 帧数。
    pub drain_steps: usize,
    /// 整个收敛过程实际滚动的行数。
    pub scrolled_lines: usize,
    /// 收敛后的 document viewport offset。
    pub final_offset: usize,
}

/// `DocumentStressScenario` 标记当前 stress summary 对应的测量场景。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentStressScenario {
    ColdResume,
    ColdResumeRestoreAnchor,
    WidthChange { from_width: u16, to_width: u16 },
    WidthChangeRestoreAnchor { from_width: u16, to_width: u16 },
}

/// `DocumentMemorySummary` 粗略估算 benchmark fixture 常驻结构的体积拆分。
/// Phase C 之后，`RenderResult` 本身只保留 index，但 transcript warmed item block cache
/// 仍会在 steady-state 中常驻，因此这里需要单独计入其 retained block 开销。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DocumentMemorySummary {
    pub raw_text_bytes: usize,
    pub estimated_item_bytes: usize,
    pub estimated_render_ui_bytes: usize,
    /// 这里统计的是 plain-line 相关常驻元数据。
    /// 在当前实现下，它主要对应每行长度表，而不是整份字符串副本。
    pub estimated_plain_line_bytes: usize,
    pub estimated_anchor_bytes: usize,
    pub estimated_index_bytes: usize,
    pub estimated_total_bytes: usize,
}

/// `DocumentStressSummary` 收敛超大 transcript 下 document pipeline 的测量结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentStressSummary {
    pub scenario: DocumentStressScenario,
    pub item_count: usize,
    pub width: u16,
    pub height: u16,
    pub transcript_line_count: usize,
    pub document_line_count: usize,
    pub viewport_line_count: usize,
    pub frame_non_empty_cells: usize,
    pub transcript_render_time: std::time::Duration,
    pub estimate_time: std::time::Duration,
    pub assistant_estimate_time: std::time::Duration,
    pub user_estimate_time: std::time::Duration,
    pub startup_banner_estimate_time: std::time::Duration,
    pub other_non_assistant_estimate_time: std::time::Duration,
    pub assistant_estimate_items: usize,
    pub user_estimate_items: usize,
    pub startup_banner_estimate_items: usize,
    pub other_non_assistant_estimate_items: usize,
    pub non_assistant_estimate_items: usize,
    pub assistant_resize_reuse_items: usize,
    pub user_resize_reuse_items: usize,
    pub visible_exact_time: std::time::Duration,
    pub first_visible_time: std::time::Duration,
    pub full_settle_time: std::time::Duration,
    pub document_layout_time: std::time::Duration,
    pub document_viewport_time: std::time::Duration,
    pub frame_render_time: std::time::Duration,
    pub frame_output_bytes: usize,
    pub frame_flushes: usize,
    pub rss_before_kib: Option<usize>,
    pub rss_after_transcript_kib: Option<usize>,
    pub rss_after_layout_kib: Option<usize>,
    pub rss_after_viewport_kib: Option<usize>,
    pub rss_after_frame_kib: Option<usize>,
    pub memory: DocumentMemorySummary,
}

/// `PhaseABaselineSummary` 汇总 item-level virtualization Phase A 需要冻结的基线。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhaseABaselineSummary {
    pub item_count: usize,
    pub width: u16,
    pub height: u16,
    pub cold_resume: DocumentStressSummary,
    pub width_change: DocumentStressSummary,
    pub manual_scroll_viewport: DocumentViewportSummary,
    pub bottom_follow_viewport: DocumentViewportSummary,
    pub frame: FrameRenderSummary,
}

/// `TranscriptBench` 封装 transcript 渲染 benchmark 所需的状态。
#[derive(Debug, Clone)]
pub struct TranscriptBench {
    transcript: Transcript,
    next_index: usize,
}

/// `AssistantProjectionBench` 测量长Markdown在有界page store上的顺序物化。
#[cfg(feature = "bench-support")]
#[derive(Debug)]
pub struct AssistantProjectionBench {
    projection: super::message::AssistantMessageRenderProjection,
    next_line: usize,
}

#[cfg(feature = "bench-support")]
impl AssistantProjectionBench {
    pub fn long_list(item_count: usize, width: u16, palette: TerminalPalette) -> Self {
        let markdown = format!(
            "# Projected list\n\n{}",
            "- projected item with enough text to exercise wrapping\n".repeat(item_count)
        );
        let message = super::message::MessageItem::new_with_style_mode_and_source(
            Sender::Assistant,
            markdown,
            StyleMode::Cx,
            None,
            None,
        );
        let AssistantProjectionOutcome::Projected(projection) =
            message.render_assistant_projection(width, palette)
        else {
            panic!("long-list benchmark fixture must remain projectable");
        };
        Self {
            projection: *projection,
            next_line: 0,
        }
    }

    pub fn materialize_next_page(&mut self) -> usize {
        if self.next_line >= self.projection.line_count() {
            self.next_line = 0;
        }
        let line = self.next_line;
        self.next_line = self.next_line.saturating_add(64);
        self.projection
            .line_at(line)
            .map_or(0, |line| line.spans.len())
    }
}

/// `DocumentBench` 封装 unified document benchmark 所需的状态。
#[derive(Debug, Clone)]
pub struct DocumentBench {
    model: Model,
    layout: Rc<super::document::DocumentLayout>,
    layout_context: crate::frame_time::FrameRenderContext,
    next_index: usize,
}

/// `ModelRenderBench` 封装整帧渲染 benchmark 所需的状态。
#[derive(Debug)]
pub struct ModelRenderBench {
    model: Model,
    frame_surface: FrameSurfaceHarness,
}

#[derive(Debug)]
struct FrameSurfaceHarness {
    surface: TerminalSurface<FrameSurfaceBackend>,
    output: FrameSurfaceOutput,
}

#[derive(Debug, Clone, Default)]
struct FrameSurfaceOutput(Rc<RefCell<FrameSurfaceOutputState>>);

#[derive(Debug, Default)]
struct FrameSurfaceOutputState {
    bytes: usize,
    flushes: usize,
}

#[derive(Debug)]
struct FrameSurfaceBackend {
    output: FrameSurfaceOutput,
    size: Size,
    cursor: Position,
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

/// `large_rust_code_block_fixture` 构造可按代码行数扩展的 fenced Rust markdown。
pub fn large_rust_code_block_fixture(line_count: usize) -> String {
    let mut markdown = String::from("```rust\nfn benchmark_values() -> usize {\n");
    for index in 0..line_count {
        let _ = writeln!(
            markdown,
            "    let value_{index}: usize = {index}; // syntax benchmark line {index}"
        );
    }
    markdown.push_str("    0\n}\n```\n");
    markdown
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

/// `large_composer_draft_fixture` 构造达到目标字节数的多语言长草稿。
pub fn large_composer_draft_fixture(min_bytes: usize) -> String {
    let mut draft = String::with_capacity(min_bytes);
    let mut line_index = 0usize;
    while draft.len() < min_bytes {
        let _ = writeln!(
            draft,
            "line {line_index:04}\tcomposer viewport keeps 中文宽字 and emoji 👨‍👩‍👧 aligned while the draft wraps across the terminal"
        );
        line_index += 1;
    }
    draft
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
    summarize_text_lines(&render_markdown_lines(markdown, width, palette, None))
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
    composer.reset_text_and_move_to_end(value);

    let document = composer.document_snapshot(palette);
    let range = document.range(0, document.line_count());
    let (cursor_x, cursor_y) = composer.cursor_visual_position();
    ComposerRenderSummary {
        line_count: range.lines.len(),
        plain_text_len: plain_lines_len(&range.plain_lines),
        anchor_count: document.line_count(),
        selectable_count: range.selectable_ranges.len(),
        cursor_x,
        cursor_y,
    }
}

/// `measure_document_pipeline_stress` 测量大规模 transcript 下的 document 冷路径。
pub fn measure_document_pipeline_stress(
    item_count: usize,
    width: u16,
    height: u16,
) -> DocumentStressSummary {
    let model = new_cold_stress_document_model(item_count, width, height);
    measure_document_pipeline_stress_with_model(
        model,
        DocumentStressScenario::ColdResume,
        width.max(1),
        height,
    )
}

/// `measure_document_pipeline_stress_with_restore_anchor` 测量冷恢复时以中段锚点恢复的首帧路径。
pub fn measure_document_pipeline_stress_with_restore_anchor(
    item_count: usize,
    width: u16,
    height: u16,
) -> DocumentStressSummary {
    let restore_viewport_state = mid_history_restore_viewport_state(item_count, width, height);
    let mut model = new_cold_stress_document_model(item_count, width, height);
    model.document_runtime.follow_bottom = false;
    model.document_runtime.manual_scroll = true;
    model.document_runtime.viewport_y = restore_viewport_state.resolved_offset();
    model.document_runtime.viewport_state = restore_viewport_state;

    measure_document_pipeline_stress_with_model(
        model,
        DocumentStressScenario::ColdResumeRestoreAnchor,
        width.max(1),
        height,
    )
}

/// `measure_width_change_document_pipeline_stress` 测量宽度变化后的 rerender 冷路径。
pub fn measure_width_change_document_pipeline_stress(
    item_count: usize,
    from_width: u16,
    to_width: u16,
    height: u16,
) -> DocumentStressSummary {
    let mut model = new_warm_stress_document_model(item_count, from_width, height);
    apply_stress_window_resize_without_render(&mut model, to_width, height);

    measure_document_pipeline_stress_with_model(
        model,
        DocumentStressScenario::WidthChange {
            from_width: from_width.max(1),
            to_width: to_width.max(1),
        },
        to_width.max(1),
        height,
    )
}

/// `measure_width_change_document_pipeline_stress_with_restore_anchor` 测量中段锚点下的 resize 恢复路径。
pub fn measure_width_change_document_pipeline_stress_with_restore_anchor(
    item_count: usize,
    from_width: u16,
    to_width: u16,
    height: u16,
) -> DocumentStressSummary {
    let mut model = new_warm_stress_document_model(item_count, from_width, height);
    let restore_viewport_state = mid_history_restore_viewport_state(item_count, from_width, height);
    model.document_runtime.follow_bottom = false;
    model.document_runtime.manual_scroll = true;
    model.document_runtime.viewport_y = restore_viewport_state.resolved_offset();
    model.document_runtime.viewport_state = restore_viewport_state;
    apply_stress_window_resize_without_render(&mut model, to_width, height);

    measure_document_pipeline_stress_with_model(
        model,
        DocumentStressScenario::WidthChangeRestoreAnchor {
            from_width: from_width.max(1),
            to_width: to_width.max(1),
        },
        to_width.max(1),
        height,
    )
}

fn measure_document_pipeline_stress_with_model(
    mut model: Model,
    scenario: DocumentStressScenario,
    width: u16,
    height: u16,
) -> DocumentStressSummary {
    let items = model.transcript.items_snapshot();
    let rss_before_kib = process_rss_kib();

    let generation_started_at = std::time::Instant::now();
    let sync_profile = model.sync_transcript_render_profile();
    let transcript_render_time = sync_profile
        .estimate_time
        .saturating_add(sync_profile.visible_exact_time);
    let rss_after_transcript_kib = process_rss_kib();
    let memory = estimate_document_memory_summary(
        items.as_slice(),
        &model.transcript_render,
        model.transcript.retained_block_memory_summary(),
    );

    model.sync_command_panel_navigation();
    model.sync_composer_height();

    let document_layout_started_at = std::time::Instant::now();
    let context = crate::frame_time::FrameRenderContext::capture();
    let layout = model.build_document_layout(context);
    let document_layout_time = document_layout_started_at.elapsed();
    let rss_after_layout_kib = process_rss_kib();

    let document_viewport_started_at = std::time::Instant::now();
    let viewport = model.build_document_viewport(&layout, context);
    let document_viewport_time = document_viewport_started_at.elapsed();
    let rss_after_viewport_kib = process_rss_kib();

    let frame_render_started_at = std::time::Instant::now();
    let frame_summary = render_model_frame(&mut model, width, height);
    let frame_render_time = frame_render_started_at.elapsed();
    let rss_after_frame_kib = process_rss_kib();
    let first_visible_time = generation_started_at.elapsed();

    let full_settle_time = generation_started_at.elapsed();

    DocumentStressSummary {
        scenario,
        item_count: items.len(),
        width,
        height,
        transcript_line_count: model.transcript_render.line_count,
        document_line_count: layout.line_count(),
        viewport_line_count: viewport.lines.len(),
        frame_non_empty_cells: frame_summary.non_empty_cells,
        transcript_render_time,
        estimate_time: sync_profile.estimate_time,
        assistant_estimate_time: sync_profile.estimate_breakdown.assistant_estimate_time,
        user_estimate_time: sync_profile.estimate_breakdown.user_estimate_time,
        startup_banner_estimate_time: sync_profile.estimate_breakdown.startup_banner_estimate_time,
        other_non_assistant_estimate_time: sync_profile
            .estimate_breakdown
            .other_non_assistant_estimate_time,
        assistant_estimate_items: sync_profile.estimate_breakdown.assistant_item_count,
        user_estimate_items: sync_profile.estimate_breakdown.user_item_count,
        startup_banner_estimate_items: sync_profile.estimate_breakdown.startup_banner_item_count,
        other_non_assistant_estimate_items: sync_profile
            .estimate_breakdown
            .other_non_assistant_item_count,
        non_assistant_estimate_items: sync_profile.estimate_breakdown.non_assistant_item_count,
        assistant_resize_reuse_items: sync_profile.estimate_breakdown.assistant_resize_reuse_count,
        user_resize_reuse_items: sync_profile.estimate_breakdown.user_resize_reuse_count,
        visible_exact_time: sync_profile.visible_exact_time,
        first_visible_time,
        full_settle_time,
        document_layout_time,
        document_viewport_time,
        frame_render_time,
        frame_output_bytes: frame_summary.output_bytes,
        frame_flushes: frame_summary.flushes,
        rss_before_kib,
        rss_after_transcript_kib,
        rss_after_layout_kib,
        rss_after_viewport_kib,
        rss_after_frame_kib,
        memory,
    }
}

/// `format_document_stress_summary` 输出便于人工比较的 stress 摘要。
pub fn format_document_stress_summary(summary: &DocumentStressSummary) -> String {
    format!(
        "scenario={scenario} items={items} size={width}x{height} transcript_lines={transcript_lines} document_lines={document_lines} viewport_lines={viewport_lines} frame_cells={frame_cells} frame_output={{bytes:{frame_output_bytes}, flushes:{frame_flushes}}} timings_ms={{metrics:{render:.3}, estimate:{estimate:.3}, visible_exact:{visible_exact:.3}, first_visible:{first_visible:.3}, full_settle:{full_settle:.3}, layout:{layout:.3}, viewport:{viewport:.3}, frame:{frame:.3}}} estimate_breakdown_ms={{assistant:{assistant_estimate_ms:.3}, user:{user_estimate_ms:.3}, startup_banner:{startup_banner_estimate_ms:.3}, other_non_assistant:{other_non_assistant_estimate_ms:.3}}} estimate_items={{assistant:{assistant_items}, user:{user_items}, startup_banner:{startup_banner_items}, other_non_assistant:{other_non_assistant_items}, non_assistant:{non_assistant_items}, assistant_resize_reuse:{assistant_resize_reuse}, user_resize_reuse:{user_resize_reuse}}} rss_kib={{before:{rss_before:?}, after_metrics:{rss_render:?}, after_layout:{rss_layout:?}, after_viewport:{rss_viewport:?}, after_frame:{rss_frame:?}}} memory_bytes={{raw_text:{raw_text}, items:{item_bytes}, render_ui:{render_ui}, plain_lines:{plain_lines}, anchors:{anchors}, indexes:{indexes}, estimated_total:{estimated_total}}}",
        scenario = format_document_stress_scenario(summary.scenario),
        items = summary.item_count,
        width = summary.width,
        height = summary.height,
        transcript_lines = summary.transcript_line_count,
        document_lines = summary.document_line_count,
        viewport_lines = summary.viewport_line_count,
        frame_cells = summary.frame_non_empty_cells,
        frame_output_bytes = summary.frame_output_bytes,
        frame_flushes = summary.frame_flushes,
        render = summary.transcript_render_time.as_secs_f64() * 1000.0,
        estimate = summary.estimate_time.as_secs_f64() * 1000.0,
        visible_exact = summary.visible_exact_time.as_secs_f64() * 1000.0,
        first_visible = summary.first_visible_time.as_secs_f64() * 1000.0,
        full_settle = summary.full_settle_time.as_secs_f64() * 1000.0,
        layout = summary.document_layout_time.as_secs_f64() * 1000.0,
        viewport = summary.document_viewport_time.as_secs_f64() * 1000.0,
        frame = summary.frame_render_time.as_secs_f64() * 1000.0,
        assistant_estimate_ms = summary.assistant_estimate_time.as_secs_f64() * 1000.0,
        user_estimate_ms = summary.user_estimate_time.as_secs_f64() * 1000.0,
        startup_banner_estimate_ms = summary.startup_banner_estimate_time.as_secs_f64() * 1000.0,
        other_non_assistant_estimate_ms =
            summary.other_non_assistant_estimate_time.as_secs_f64() * 1000.0,
        assistant_items = summary.assistant_estimate_items,
        user_items = summary.user_estimate_items,
        startup_banner_items = summary.startup_banner_estimate_items,
        other_non_assistant_items = summary.other_non_assistant_estimate_items,
        non_assistant_items = summary.non_assistant_estimate_items,
        assistant_resize_reuse = summary.assistant_resize_reuse_items,
        user_resize_reuse = summary.user_resize_reuse_items,
        rss_before = summary.rss_before_kib,
        rss_render = summary.rss_after_transcript_kib,
        rss_layout = summary.rss_after_layout_kib,
        rss_viewport = summary.rss_after_viewport_kib,
        rss_frame = summary.rss_after_frame_kib,
        raw_text = summary.memory.raw_text_bytes,
        item_bytes = summary.memory.estimated_item_bytes,
        render_ui = summary.memory.estimated_render_ui_bytes,
        plain_lines = summary.memory.estimated_plain_line_bytes,
        anchors = summary.memory.estimated_anchor_bytes,
        indexes = summary.memory.estimated_index_bytes,
        estimated_total = summary.memory.estimated_total_bytes,
    )
}

/// `measure_phase_a_baseline` 汇总 Phase A 需要冻结的性能与行为基线。
pub fn measure_phase_a_baseline(
    item_count: usize,
    width: u16,
    height: u16,
) -> PhaseABaselineSummary {
    let width = width.max(1);
    let height = height.max(1);
    let width_change_target = width.saturating_sub(24).max(1);
    let cold_resume = measure_document_pipeline_stress(item_count, width, height);
    let width_change = measure_width_change_document_pipeline_stress(
        item_count,
        width,
        width_change_target,
        height,
    );

    let mut document_bench = DocumentBench::new(item_count, width, height);
    document_bench.prepare_offset_viewport_state();
    let manual_scroll_viewport = document_bench.build_offset_viewport();
    document_bench.prepare_bottom_follow_viewport_state();
    let bottom_follow_viewport = document_bench.build_bottom_follow_viewport();

    let mut frame_bench = ModelRenderBench::new(item_count, width, height);
    let frame = frame_bench.render_frame();

    PhaseABaselineSummary {
        item_count,
        width,
        height,
        cold_resume,
        width_change,
        manual_scroll_viewport,
        bottom_follow_viewport,
        frame,
    }
}

/// `format_phase_a_baseline_summary` 输出便于写入工作记录的 Phase A 摘要。
pub fn format_phase_a_baseline_summary(summary: &PhaseABaselineSummary) -> String {
    format!(
        "phase_a items={items} size={width}x{height} cold_resume=[{cold_resume}] width_change=[{width_change}] manual_scroll={{lines:{manual_lines}, plain_text_len:{manual_plain_text_len}, resolved_offset:{manual_offset}}} bottom_follow={{lines:{bottom_lines}, plain_text_len:{bottom_plain_text_len}, resolved_offset:{bottom_offset}}} frame={{cells:{frame_cells}, size:{frame_width}x{frame_height}, output_bytes:{frame_output_bytes}, flushes:{frame_flushes}}}",
        items = summary.item_count,
        width = summary.width,
        height = summary.height,
        cold_resume = format_document_stress_summary(&summary.cold_resume),
        width_change = format_document_stress_summary(&summary.width_change),
        manual_lines = summary.manual_scroll_viewport.line_count,
        manual_plain_text_len = summary.manual_scroll_viewport.plain_text_len,
        manual_offset = summary.manual_scroll_viewport.resolved_offset,
        bottom_lines = summary.bottom_follow_viewport.line_count,
        bottom_plain_text_len = summary.bottom_follow_viewport.plain_text_len,
        bottom_offset = summary.bottom_follow_viewport.resolved_offset,
        frame_cells = summary.frame.non_empty_cells,
        frame_width = summary.frame.width,
        frame_height = summary.frame.height,
        frame_output_bytes = summary.frame.output_bytes,
        frame_flushes = summary.frame.flushes,
    )
}

impl TranscriptBench {
    /// `new` 创建一个与 Go transcript benchmark 场景对齐的 transcript bench。
    pub fn new(item_count: usize, width: u16, palette: TerminalPalette) -> Self {
        let mut transcript = Transcript::new(palette, None);
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
        summarize_transcript_render(
            &self
                .transcript
                .render(crate::frame_time::FrameRenderContext::capture()),
        )
    }

    /// `append_benchmark_item_and_render` 追加一项并测量 append fast path。
    pub fn append_benchmark_item_and_render(&mut self) -> TranscriptRenderSummary {
        append_transcript_benchmark_item(&mut self.transcript, self.next_index);
        self.next_index += 1;
        summarize_transcript_render(
            &self
                .transcript
                .render(crate::frame_time::FrameRenderContext::capture()),
        )
    }
}

impl DocumentBench {
    /// `new` 创建一个与 Go unified document benchmark 场景对齐的 document bench。
    pub fn new(item_count: usize, width: u16, height: u16) -> Self {
        let mut model = new_stress_document_model(item_count, width, height);
        model.sync_transcript_render();

        let layout_context = crate::frame_time::FrameRenderContext::capture();
        let layout = model.build_document_layout(layout_context);
        Self {
            model,
            layout,
            layout_context,
            next_index: item_count,
        }
    }

    /// `rebuild_layout` 强制重建 unified document layout 并返回稳定摘要。
    pub fn rebuild_layout(&mut self) -> DocumentLayoutSummary {
        self.model.document_runtime.transcript_cache = Default::default();
        self.model.document_runtime.layout_cache = Default::default();
        let layout_context = crate::frame_time::FrameRenderContext::capture();
        let layout = self.model.build_document_layout(layout_context);
        let summary = summarize_document_layout(&layout);
        self.layout = layout;
        self.layout_context = layout_context;
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
        self.model.document_runtime.viewport_cache = Default::default();

        summarize_document_viewport(
            &self
                .model
                .build_document_viewport(&self.layout, self.layout_context),
        )
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
        self.model.document_runtime.viewport_cache = Default::default();

        summarize_document_viewport(
            &self
                .model
                .build_document_viewport(&self.layout, self.layout_context),
        )
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

        let layout_context = crate::frame_time::FrameRenderContext::capture();
        let layout = self.model.build_document_layout(layout_context);
        let summary = summarize_document_layout(&layout);
        self.layout = layout;
        self.layout_context = layout_context;
        summary
    }

    /// `rebuild_layout_after_composer_edit` 模拟草稿编辑后重建 layout 并返回稳定摘要。
    pub fn rebuild_layout_after_composer_edit(&mut self) -> DocumentLayoutSummary {
        self.model.composer_mut().insert_text("x");
        self.model.sync_composer_height();

        let layout_context = crate::frame_time::FrameRenderContext::capture();
        let layout = self.model.build_document_layout(layout_context);
        let summary = summarize_document_layout(&layout);
        self.layout = layout;
        self.layout_context = layout_context;
        summary
    }
}

impl FrameSurfaceHarness {
    fn new(width: u16, height: u16) -> Self {
        let output = FrameSurfaceOutput::default();
        let backend = FrameSurfaceBackend {
            output: output.clone(),
            size: Size::new(width, height),
            cursor: Position::ORIGIN,
        };
        let surface =
            TerminalSurface::new(backend).expect("benchmark terminal surface should initialize");

        Self { surface, output }
    }

    fn render_model(&mut self, model: &mut Model) -> FrameRenderSummary {
        self.output.reset();
        self.surface
            .draw(|area, buffer| model.render_to_buffer(area, buffer))
            .expect("benchmark terminal surface draw should succeed");

        let buffer = self.surface.last_frame_buffer();
        let non_empty_cells = (0..buffer.area.height)
            .flat_map(|row| (0..buffer.area.width).map(move |column| (column, row)))
            .filter(|&(column, row)| buffer[(column, row)].symbol() != " ")
            .count();
        let (output_bytes, flushes) = self.output.snapshot();

        FrameRenderSummary {
            non_empty_cells,
            width: buffer.area.width,
            height: buffer.area.height,
            output_bytes,
            flushes,
        }
    }
}

impl FrameSurfaceOutput {
    fn reset(&self) {
        *self.0.borrow_mut() = FrameSurfaceOutputState::default();
    }

    fn snapshot(&self) -> (usize, usize) {
        let state = self.0.borrow();
        (state.bytes, state.flushes)
    }
}

impl io::Write for FrameSurfaceBackend {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.output.0.borrow_mut().bytes += bytes.len();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Backend for FrameSurfaceBackend {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, _content: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        Ok(())
    }

    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
        Ok(self.cursor)
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> Result<(), Self::Error> {
        self.cursor = position.into();
        Ok(())
    }

    fn clear(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn clear_region(&mut self, _clear_type: ClearType) -> Result<(), Self::Error> {
        Ok(())
    }

    fn size(&self) -> Result<Size, Self::Error> {
        Ok(self.size)
    }

    fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
        Ok(WindowSize {
            columns_rows: self.size,
            pixels: Size::new(0, 0),
        })
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        self.output.0.borrow_mut().flushes += 1;
        Ok(())
    }
}

impl ModelRenderBench {
    /// `new` 创建一个整帧 render benchmark 场景。
    pub fn new(item_count: usize, width: u16, height: u16) -> Self {
        let model = DocumentBench::new(item_count, width, height).model;
        let frame_surface = FrameSurfaceHarness::new(width, height);

        Self {
            model,
            frame_surface,
        }
    }

    /// `render_frame` 通过生产 `TerminalSurface` 运行一帧完整渲染。
    pub fn render_frame(&mut self) -> FrameRenderSummary {
        self.frame_surface.render_model(&mut self.model)
    }
}

/// `SmoothScrollDrainBench` 测量滚轮 burst 后平滑滚动 drain 至收敛的完整路径：
/// 每个 drain 步都会走生产 `scroll_document_by`（含 layout 重建与方向性 exactize）。
#[derive(Debug)]
pub struct SmoothScrollDrainBench {
    model: Model,
    /// 人工推进的单调时间线：注入的滚轮/帧时刻全部由它派生，
    /// 迭代间持续前进，保证加速度窗口判定确定且互不干扰。
    timeline: std::time::Instant,
}

impl SmoothScrollDrainBench {
    /// `new` 预置长 transcript 并停在贴底位置，等待滚轮 burst。
    pub fn new(item_count: usize, width: u16, height: u16) -> Self {
        let mut model = new_warm_stress_document_model(item_count, width, height);
        model.sync_transcript_render();
        model.sync_document_viewport_to_bottom();
        assert!(
            model.smooth_scroll_enabled(),
            "smooth scroll drain benchmark requires the default animated scroll path"
        );

        Self {
            model,
            timeline: std::time::Instant::now(),
        }
    }

    /// `drain_wheel_burst` 注入一段连续快速向上滚轮 burst（触发加速度爬升），
    /// 然后按 8ms 帧间隔推进 drain 至收敛，返回稳定摘要。
    pub fn drain_wheel_burst(&mut self) -> SmoothScrollDrainSummary {
        // 迭代间隔拉开 1s：远大于加速度窗口（120ms），每次测量从基线倍率起步，
        // 迭代之间完全同构、结果可复现。
        self.timeline += std::time::Duration::from_secs(1);
        // 回到贴底，保证每次迭代拥有相同的起点与可滚动空间。
        self.model.sync_document_viewport_to_bottom();
        let start_offset = self.model.document_runtime.viewport_y;

        for event_index in 0..6u64 {
            let event_at = self.timeline + std::time::Duration::from_millis(50 * event_index);
            self.model
                .document_mouse_wheel_at(-Model::document_mouse_wheel_delta(), event_at);
        }

        let mut frame_at = self.timeline + std::time::Duration::from_millis(300);
        let mut drain_steps = 0usize;
        while self.model.document_runtime.smooth_scroll.is_settling() {
            frame_at += std::time::Duration::from_millis(8);
            self.model.advance_smooth_scroll_at(frame_at);
            drain_steps += 1;
        }

        let final_offset = self.model.document_runtime.viewport_y;
        SmoothScrollDrainSummary {
            drain_steps,
            scrolled_lines: start_offset.saturating_sub(final_offset),
            final_offset,
        }
    }
}

fn render_model_frame(model: &mut Model, width: u16, height: u16) -> FrameRenderSummary {
    let mut frame_surface = FrameSurfaceHarness::new(width, height);
    frame_surface.render_model(model)
}

fn new_stress_document_model(item_count: usize, width: u16, height: u16) -> Model {
    new_warm_stress_document_model(item_count, width, height)
}

fn new_warm_stress_document_model(item_count: usize, width: u16, height: u16) -> Model {
    let mut model = Model::new_with_options(
        StartupBannerOptions {
            app_name: Some("hunea".to_string()),
            version: Some("dev".to_string()),
            model_name: None,
            work_dir: Some("/tmp/hunea".to_string()),
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
        .reset_text_and_move_to_end(benchmark_composer_draft_for_document());
    model.sync_composer_height();
    model
}

fn new_cold_stress_document_model(item_count: usize, width: u16, height: u16) -> Model {
    let mut model = Model::new_with_options(
        StartupBannerOptions {
            app_name: Some("hunea".to_string()),
            version: Some("dev".to_string()),
            model_name: None,
            work_dir: Some("/tmp/hunea".to_string()),
            width: 0,
        },
        ModelOptions {
            style_mode: StyleMode::Cx,
            ..ModelOptions::default()
        },
    );
    model.transcript_mut().clear();
    model.set_window(width, height);
    model.set_palette(default_palette(), true);

    for index in 0..item_count {
        append_transcript_benchmark_item(model.transcript_mut(), index);
    }

    model
        .composer_mut()
        .reset_text_and_move_to_end(benchmark_composer_draft_for_document());
    model.sync_composer_height();
    model
}

fn apply_stress_window_resize_without_render(model: &mut Model, width: u16, height: u16) {
    let width = width.max(1);
    model.width = width;
    model.height = height;
    model.has_window = true;
    model.transcript.set_width(width);
    model.composer.set_width(width);
    model.document_runtime.transcript_cache = Default::default();
    model.document_runtime.layout_cache = Default::default();
    model.document_runtime.viewport_cache = Default::default();
}

fn mid_history_restore_viewport_state(item_count: usize, width: u16, height: u16) -> ViewportState {
    let mut model = new_warm_stress_document_model(item_count, width, height);
    let exact_index = model.transcript.item_metrics_index();
    let layout = model.document_layout_for_transcript_index(
        exact_index,
        crate::frame_time::FrameRenderContext::capture(),
    );
    let target_item_index = item_count / 2;
    let target_line = (0..layout.line_count())
        .find(|&line_index| {
            layout
                .line_anchor_at(line_index, crate::frame_time::FrameRenderContext::capture())
                .is_some_and(|anchor| {
                    anchor.region == super::document::DocumentAnchorRegion::Transcript
                        && anchor.transcript.item_index == target_item_index
                })
        })
        .unwrap_or(0);
    let composer_offset = model.current_composer_viewport_offset(&layout, target_line);
    model.apply_document_viewport_position(&layout, target_line, composer_offset, false, true);
    model.current_document_viewport_state()
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
        plain_text_len: render.plain_text_len(),
        anchor_count: render.anchor_count(),
        selectable_count: render.selectable_ranges.len(),
        append_start_line: render.append_start_line,
    }
}

fn summarize_document_layout(layout: &super::document::DocumentLayout) -> DocumentLayoutSummary {
    DocumentLayoutSummary {
        line_count: layout.line_count(),
        plain_text_len: layout.plain_text_len(crate::frame_time::FrameRenderContext::capture()),
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

fn format_document_stress_scenario(scenario: DocumentStressScenario) -> String {
    match scenario {
        DocumentStressScenario::ColdResume => "cold_resume".to_string(),
        DocumentStressScenario::ColdResumeRestoreAnchor => "cold_resume_restore_anchor".to_string(),
        DocumentStressScenario::WidthChange {
            from_width,
            to_width,
        } => format!("width_change({from_width}->{to_width})"),
        DocumentStressScenario::WidthChangeRestoreAnchor {
            from_width,
            to_width,
        } => format!("width_change_restore_anchor({from_width}->{to_width})"),
    }
}

fn estimate_document_memory_summary(
    items: &[Rc<TranscriptItem>],
    render: &RenderResult,
    retained_blocks: super::transcript::RetainedBlockMemorySummary,
) -> DocumentMemorySummary {
    let raw_text_bytes = items.iter().map(|item| item.source_text_byte_len()).sum();
    let estimated_item_bytes = size_of::<Vec<Rc<TranscriptItem>>>()
        + size_of_val(items)
        + items.len() * size_of::<TranscriptItem>()
        + raw_text_bytes;
    let estimated_render_ui_bytes = retained_blocks.estimated_render_ui_bytes;
    let estimated_plain_line_bytes = retained_blocks.estimated_plain_line_bytes;
    let estimated_anchor_bytes = retained_blocks.estimated_anchor_bytes;

    let estimated_index_bytes = size_of::<RenderResult>()
        + size_of_val(render.items.as_slice())
        + size_of_val(render.index.metrics.as_slice())
        + size_of_val(render.index.visible_items.as_slice())
        + size_of_val(render.index.visible_positions.as_slice())
        + size_of_val(render.index.content_prefix_sums.as_slice())
        + size_of_val(render.selectable_ranges.as_slice())
        + retained_blocks.estimated_cache_slot_bytes;

    DocumentMemorySummary {
        raw_text_bytes,
        estimated_item_bytes,
        estimated_render_ui_bytes,
        estimated_plain_line_bytes,
        estimated_anchor_bytes,
        estimated_index_bytes,
        estimated_total_bytes: estimated_item_bytes
            + estimated_render_ui_bytes
            + estimated_plain_line_bytes
            + estimated_anchor_bytes
            + estimated_index_bytes,
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
mod tests;
#[cfg(test)]
mod visual_baseline;
