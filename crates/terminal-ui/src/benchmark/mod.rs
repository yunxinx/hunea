//! TUI 渲染与滚动性能基准入口。

use std::fmt::Write as _;
use std::mem::{size_of, size_of_val};
use std::rc::Rc;

use ratatui::{Terminal, backend::TestBackend};

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
    let mut terminal = Terminal::new(TestBackend::new(width, height))
        .expect("stress benchmark backend should initialize");

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
    let first_visible_time = generation_started_at.elapsed();

    let full_settle_time = generation_started_at.elapsed();

    let buffer = terminal.backend().buffer();
    let frame_non_empty_cells = (0..buffer.area.height)
        .flat_map(|row| (0..buffer.area.width).map(move |column| (column, row)))
        .filter(|&(column, row)| buffer[(column, row)].symbol() != " ")
        .count();

    DocumentStressSummary {
        scenario,
        item_count: items.len(),
        width,
        height,
        transcript_line_count: model.transcript_render.line_count,
        document_line_count: layout.line_count(),
        viewport_line_count: viewport.lines.len(),
        frame_non_empty_cells,
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
        "scenario={scenario} items={items} size={width}x{height} transcript_lines={transcript_lines} document_lines={document_lines} viewport_lines={viewport_lines} frame_cells={frame_cells} timings_ms={{metrics:{render:.3}, estimate:{estimate:.3}, visible_exact:{visible_exact:.3}, first_visible:{first_visible:.3}, full_settle:{full_settle:.3}, layout:{layout:.3}, viewport:{viewport:.3}, frame:{frame:.3}}} estimate_breakdown_ms={{assistant:{assistant_estimate_ms:.3}, user:{user_estimate_ms:.3}, startup_banner:{startup_banner_estimate_ms:.3}, other_non_assistant:{other_non_assistant_estimate_ms:.3}}} estimate_items={{assistant:{assistant_items}, user:{user_items}, startup_banner:{startup_banner_items}, other_non_assistant:{other_non_assistant_items}, non_assistant:{non_assistant_items}, assistant_resize_reuse:{assistant_resize_reuse}, user_resize_reuse:{user_resize_reuse}}} rss_kib={{before:{rss_before:?}, after_metrics:{rss_render:?}, after_layout:{rss_layout:?}, after_viewport:{rss_viewport:?}, after_frame:{rss_frame:?}}} memory_bytes={{raw_text:{raw_text}, items:{item_bytes}, render_ui:{render_ui}, plain_lines:{plain_lines}, anchors:{anchors}, indexes:{indexes}, estimated_total:{estimated_total}}}",
        scenario = format_document_stress_scenario(summary.scenario),
        items = summary.item_count,
        width = summary.width,
        height = summary.height,
        transcript_lines = summary.transcript_line_count,
        document_lines = summary.document_line_count,
        viewport_lines = summary.viewport_line_count,
        frame_cells = summary.frame_non_empty_cells,
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
        "phase_a items={items} size={width}x{height} cold_resume=[{cold_resume}] width_change=[{width_change}] manual_scroll={{lines:{manual_lines}, plain_text_len:{manual_plain_text_len}, resolved_offset:{manual_offset}}} bottom_follow={{lines:{bottom_lines}, plain_text_len:{bottom_plain_text_len}, resolved_offset:{bottom_offset}}} frame={{cells:{frame_cells}, size:{frame_width}x{frame_height}}}",
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
        self.model.document_runtime.transcript_cache = Default::default();
        self.model.document_runtime.layout_cache = Default::default();
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
        self.model.document_runtime.viewport_cache = Default::default();

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
        self.model.document_runtime.viewport_cache = Default::default();

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
    new_warm_stress_document_model(item_count, width, height)
}

fn new_warm_stress_document_model(item_count: usize, width: u16, height: u16) -> Model {
    let mut model = Model::new_with_options(
        StartupBannerOptions {
            app_name: Some("hunea".to_string()),
            version: Some("dev".to_string()),
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
        .replace_text_and_move_to_end(benchmark_composer_draft_for_document());
    model.sync_composer_height();
    model
}

fn new_cold_stress_document_model(item_count: usize, width: u16, height: u16) -> Model {
    let mut model = Model::new_with_options(
        StartupBannerOptions {
            app_name: Some("hunea".to_string()),
            version: Some("dev".to_string()),
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
        .replace_text_and_move_to_end(benchmark_composer_draft_for_document());
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
    let layout = model.document_layout_for_transcript_index(exact_index);
    let target_item_index = item_count / 2;
    let target_line = (0..layout.line_count())
        .find(|&line_index| {
            layout.line_anchor_at(line_index).is_some_and(|anchor| {
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
