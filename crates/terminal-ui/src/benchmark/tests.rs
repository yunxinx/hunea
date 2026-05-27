use super::*;

#[test]
fn cold_resume_stress_fixture_keeps_transcript_render_cold_before_measurement() {
    let model = new_cold_stress_document_model(24, 80, 18);

    assert_eq!(model.width, 80);
    assert_eq!(model.height, 18);
    assert!(model.has_window);
    assert!(model.has_palette);
    assert_eq!(
        model.transcript_render.line_count, 0,
        "cold-resume fixture should leave transcript render cold until the measured render step"
    );
}

#[test]
fn document_pipeline_stress_summary_reports_consistent_counts_for_small_fixture() {
    let summary = measure_document_pipeline_stress(24, 80, 18);

    assert_eq!(summary.scenario, DocumentStressScenario::ColdResume);
    assert_eq!(summary.item_count, 24);
    assert_eq!(summary.width, 80);
    assert_eq!(summary.height, 18);
    assert!(
        summary.transcript_line_count > summary.item_count,
        "benchmark transcript should materialize substantially more than one visual line per item"
    );
    assert!(summary.document_line_count >= summary.transcript_line_count);
    assert!(summary.viewport_line_count > 0);
    assert!(summary.frame_non_empty_cells > 0);
    assert!(summary.first_visible_time >= summary.estimate_time);
    assert!(summary.first_visible_time >= summary.visible_exact_time);
    assert!(summary.full_settle_time >= summary.first_visible_time);
    assert!(summary.memory.raw_text_bytes > 0);
    assert!(summary.memory.estimated_item_bytes >= summary.memory.raw_text_bytes);
    assert_eq!(
        summary.memory.estimated_render_ui_bytes, 0,
        "Phase E sync_transcript_render should stop after metrics rebuild instead of retaining warmed render UI blocks"
    );
    assert_eq!(
        summary.memory.estimated_plain_line_bytes, 0,
        "metrics-only rebuild should not retain per-line plain-text block metadata before viewport materialization"
    );
    assert_eq!(
        summary.memory.estimated_anchor_bytes, 0,
        "metrics-only rebuild should not retain anchor metadata before viewport materialization"
    );
    assert!(summary.memory.estimated_index_bytes > 0);
    let formatted = format_document_stress_summary(&summary);
    assert!(formatted.contains("scenario=cold_resume"));
    assert!(formatted.contains("items=24"));
    assert!(formatted.contains("timings_ms={metrics:"));
    assert!(formatted.contains("estimate:"));
    assert!(!formatted.contains("assistant_estimate:"));
    assert!(!formatted.contains("non_assistant_estimate:"));
    assert!(formatted.contains("estimate_breakdown_ms={assistant:"));
    assert!(formatted.contains("estimate_items={assistant:"));
    assert!(formatted.contains("user_resize_reuse:"));
    assert!(formatted.contains("visible_exact:"));
    assert!(formatted.contains("first_visible:"));
    assert!(formatted.contains("full_settle:"));
    assert!(formatted.contains("after_metrics"));
    assert!(formatted.contains("memory_bytes={"));
}

#[test]
fn width_change_document_pipeline_stress_reports_resize_direction_and_memory_breakdown() {
    let summary = measure_width_change_document_pipeline_stress(24, 80, 56, 18);

    assert_eq!(
        summary.scenario,
        DocumentStressScenario::WidthChange {
            from_width: 80,
            to_width: 56,
        }
    );
    assert_eq!(summary.item_count, 24);
    assert_eq!(summary.width, 56);
    assert_eq!(summary.height, 18);
    assert!(
        summary.transcript_line_count > summary.item_count,
        "resize rerender should still materialize multiple visual lines per item"
    );
    assert!(summary.document_line_count >= summary.transcript_line_count);
    assert!(summary.viewport_line_count > 0);
    assert!(summary.frame_non_empty_cells > 0);
    assert!(summary.first_visible_time >= summary.estimate_time);
    assert!(summary.first_visible_time >= summary.visible_exact_time);
    assert!(summary.full_settle_time >= summary.first_visible_time);
    assert!(summary.memory.raw_text_bytes > 0);
    assert!(summary.memory.estimated_total_bytes >= summary.memory.raw_text_bytes);
    assert_eq!(summary.memory.estimated_render_ui_bytes, 0);
    assert_eq!(summary.memory.estimated_plain_line_bytes, 0);
    assert_eq!(summary.memory.estimated_anchor_bytes, 0);
    let formatted = format_document_stress_summary(&summary);
    assert!(formatted.contains("scenario=width_change(80->56)"));
    assert!(formatted.contains("timings_ms={metrics:"));
    assert!(!formatted.contains("assistant_estimate:"));
    assert!(!formatted.contains("non_assistant_estimate:"));
    assert!(formatted.contains("estimate_breakdown_ms={assistant:"));
    assert!(formatted.contains("estimate_items={assistant:"));
    assert!(formatted.contains("assistant_resize_reuse:"));
    assert!(formatted.contains("user_resize_reuse:"));
    assert!(formatted.contains("first_visible:"));
    assert!(formatted.contains("full_settle:"));
    assert!(formatted.contains("after_metrics"));
    assert!(formatted.contains("memory_bytes={"));
}

#[test]
fn cold_resume_restore_anchor_stress_reports_restore_scenario() {
    let summary = measure_document_pipeline_stress_with_restore_anchor(24, 80, 18);

    assert_eq!(
        summary.scenario,
        DocumentStressScenario::ColdResumeRestoreAnchor
    );
    assert!(summary.transcript_line_count > 0);
    assert!(summary.first_visible_time >= summary.visible_exact_time);
    assert!(summary.full_settle_time >= summary.first_visible_time);

    let formatted = format_document_stress_summary(&summary);
    assert!(formatted.contains("scenario=cold_resume_restore_anchor"));
    assert!(formatted.contains("first_visible:"));
    assert!(formatted.contains("full_settle:"));
}

#[test]
fn width_change_restore_anchor_stress_reports_restore_scenario() {
    let summary = measure_width_change_document_pipeline_stress_with_restore_anchor(24, 80, 56, 18);

    assert_eq!(
        summary.scenario,
        DocumentStressScenario::WidthChangeRestoreAnchor {
            from_width: 80,
            to_width: 56,
        }
    );
    assert!(summary.transcript_line_count > 0);
    assert!(summary.first_visible_time >= summary.visible_exact_time);
    assert!(summary.full_settle_time >= summary.first_visible_time);

    let formatted = format_document_stress_summary(&summary);
    assert!(formatted.contains("scenario=width_change_restore_anchor(80->56)"));
    assert!(formatted.contains("first_visible:"));
    assert!(formatted.contains("full_settle:"));
}

#[test]
fn phase_a_baseline_summary_reports_core_sections() {
    let summary = measure_phase_a_baseline(24, 80, 18);
    eprintln!("{}", format_phase_a_baseline_summary(&summary));

    assert_eq!(summary.item_count, 24);
    assert_eq!(summary.width, 80);
    assert_eq!(summary.height, 18);
    assert_eq!(
        summary.cold_resume.scenario,
        DocumentStressScenario::ColdResume
    );
    assert_eq!(
        summary.width_change.scenario,
        DocumentStressScenario::WidthChange {
            from_width: 80,
            to_width: 56,
        }
    );
    assert!(
        summary.manual_scroll_viewport.resolved_offset > 0,
        "manual scroll viewport should keep a non-zero offset for the baseline fixture"
    );
    assert!(summary.bottom_follow_viewport.line_count > 0);
    assert_eq!(summary.frame.width, 80);
    assert_eq!(summary.frame.height, 18);
    assert!(summary.frame.non_empty_cells > 0);

    let formatted = format_phase_a_baseline_summary(&summary);
    assert!(formatted.contains("phase_a"));
    assert!(formatted.contains("cold_resume"));
    assert!(formatted.contains("width_change(80->56)"));
    assert!(formatted.contains("manual_scroll"));
    assert!(formatted.contains("bottom_follow"));
    assert!(formatted.contains("frame={"));
}

#[test]
fn transcript_benchmark_render_summary_scales_with_item_count() {
    let mut bench = TranscriptBench::new(24, 80, default_palette());
    assert_eq!(bench.transcript.len(), 24);

    let render = bench.transcript.render();
    let summary = summarize_transcript_render(&render);

    assert!(
        summary.line_count > 24,
        "benchmark transcript should render substantially more than one visual line per item, got {summary:?}, dirty_from={}, item_summaries={}, plain_lines={:?}",
        bench.transcript.dirty_from_for_test(),
        render.items.len(),
        render.all_plain_lines()
    );
}

#[test]
#[ignore = "targeted long user-message scroll profile"]
fn long_user_message_scroll_profile() {
    let width = 80;
    let height = 24;
    let message_count = 15;
    let scroll_steps = 180;
    let mut terminal = Terminal::new(TestBackend::new(width, height))
        .expect("long message scroll profile backend should initialize");
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
    let long_user_message = long_scroll_user_message();
    for _ in 0..message_count {
        model.transcript_mut().append_message_with_style_mode(
            Sender::User,
            long_user_message.clone(),
            StyleMode::Cx,
        );
    }
    model.composer_mut().replace_text_and_move_to_end("");
    model.sync_composer_height();

    let sync_started_at = std::time::Instant::now();
    let sync_profile = model.sync_transcript_render_profile();
    let sync_time = sync_started_at.elapsed();
    model.sync_command_panel_navigation();
    model.sync_composer_height();

    let initial_layout = model.build_document_layout();
    let document_lines = initial_layout.line_count();
    let transcript_lines = initial_layout.transcript_line_count;
    model.apply_document_viewport_position(&initial_layout, 0, 0, false, true);
    drop(initial_layout);

    let mut scroll_times = Vec::with_capacity(scroll_steps);
    let mut viewport_times = Vec::with_capacity(scroll_steps);
    let mut frame_times = Vec::with_capacity(scroll_steps);
    let mut tick_times = Vec::with_capacity(scroll_steps);
    for _ in 0..scroll_steps {
        let tick_started_at = std::time::Instant::now();

        let scroll_started_at = std::time::Instant::now();
        model.scroll_document_by(Model::document_mouse_wheel_delta());
        scroll_times.push(scroll_started_at.elapsed());

        let viewport_started_at = std::time::Instant::now();
        let layout = model.build_document_layout();
        let viewport = model.build_document_viewport(&layout);
        assert_eq!(viewport.lines.len(), usize::from(height));
        viewport_times.push(viewport_started_at.elapsed());

        let frame_started_at = std::time::Instant::now();
        terminal
            .draw(|frame| model.render(frame))
            .expect("long message scroll profile frame render should succeed");
        frame_times.push(frame_started_at.elapsed());
        tick_times.push(tick_started_at.elapsed());
    }

    eprintln!(
        "long_user_scroll messages={message_count} message_bytes={} size={width}x{height} document_lines={document_lines} transcript_lines={transcript_lines} sync_ms={:.3} estimate_ms={:.3} visible_exact_ms={:.3} scroll_ms={} viewport_ms={} frame_ms={} tick_ms={}",
        long_user_message.len(),
        duration_ms(sync_time),
        duration_ms(sync_profile.estimate_time),
        duration_ms(sync_profile.visible_exact_time),
        format_duration_distribution(&scroll_times),
        format_duration_distribution(&viewport_times),
        format_duration_distribution(&frame_times),
        format_duration_distribution(&tick_times),
    );

    assert_release_hot_path_budget(
        "long_user_message_scroll_profile",
        &viewport_times,
        &frame_times,
        &tick_times,
    );
}

fn long_scroll_user_message() -> String {
    "这是一条用于滚动性能测试的超长用户消息 mixed English content with symbols and wrapping pressure. "
            .repeat(72)
}

fn format_duration_distribution(values: &[std::time::Duration]) -> String {
    let distribution = duration_distribution(values);
    format!(
        "{{mean:{:.3},p50:{:.3},p95:{:.3},max:{:.3}}}",
        distribution.mean_ms, distribution.p50_ms, distribution.p95_ms, distribution.max_ms,
    )
}

fn percentile_ms(sorted: &[std::time::Duration], percentile: usize) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let index = (sorted.len().saturating_sub(1) * percentile) / 100;
    duration_ms(sorted[index])
}

fn duration_ms(duration: std::time::Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

#[derive(Debug, Clone, Copy)]
struct DurationDistribution {
    mean_ms: f64,
    p50_ms: f64,
    p95_ms: f64,
    max_ms: f64,
}

fn duration_distribution(values: &[std::time::Duration]) -> DurationDistribution {
    let mut sorted = values.to_vec();
    sorted.sort();
    let total: std::time::Duration = sorted.iter().copied().sum();
    let mean_ms = if sorted.is_empty() {
        0.0
    } else {
        duration_ms(total) / sorted.len() as f64
    };
    DurationDistribution {
        mean_ms,
        p50_ms: percentile_ms(&sorted, 50),
        p95_ms: percentile_ms(&sorted, 95),
        max_ms: sorted
            .last()
            .map(|duration| duration_ms(*duration))
            .unwrap_or_default(),
    }
}

fn assert_release_hot_path_budget(
    label: &str,
    viewport_times: &[std::time::Duration],
    frame_times: &[std::time::Duration],
    tick_times: &[std::time::Duration],
) {
    if cfg!(debug_assertions) {
        return;
    }

    let viewport = duration_distribution(viewport_times);
    let frame = duration_distribution(frame_times);
    let tick = duration_distribution(tick_times);
    let slow_ticks_over_8ms = tick_times
        .iter()
        .filter(|duration| duration.as_millis() >= 8)
        .count();
    let slow_ticks_over_16ms = tick_times
        .iter()
        .filter(|duration| duration.as_millis() >= 16)
        .count();

    assert!(
        tick.p95_ms < 2.0,
        "{label} tick p95 should stay under target 2ms, got {tick:?}"
    );
    assert!(
        tick.max_ms < 8.0,
        "{label} tick max should stay under 120fps frame budget, got {tick:?}"
    );
    assert_eq!(
        slow_ticks_over_8ms, 0,
        "{label} should not produce ticks over 8ms, tick={tick:?}"
    );
    assert_eq!(
        slow_ticks_over_16ms, 0,
        "{label} should not produce ticks over 16ms, tick={tick:?}"
    );
    assert!(
        frame.p95_ms < 1.0,
        "{label} frame p95 should stay under 1ms, got {frame:?}"
    );
    assert!(
        viewport.p95_ms < 2.0,
        "{label} viewport p95 should stay under 2ms, got {viewport:?}"
    );
}

fn assert_release_large_history_240hz_budget(
    label: &str,
    viewport_times: &[std::time::Duration],
    frame_times: &[std::time::Duration],
    tick_times: &[std::time::Duration],
) {
    if cfg!(debug_assertions) {
        return;
    }

    const FRAME_240HZ_MS: f64 = 1000.0 / 240.0;

    let viewport = duration_distribution(viewport_times);
    let frame = duration_distribution(frame_times);
    let tick = duration_distribution(tick_times);
    let slow_ticks_over_8ms = tick_times
        .iter()
        .filter(|duration| duration.as_millis() >= 8)
        .count();
    let slow_ticks_over_16ms = tick_times
        .iter()
        .filter(|duration| duration.as_millis() >= 16)
        .count();

    assert!(
        tick.p95_ms < FRAME_240HZ_MS,
        "{label} tick p95 should stay under 240Hz frame budget, got {tick:?}"
    );
    assert!(
        tick.max_ms < 8.0,
        "{label} tick max should stay under 120fps frame budget, got {tick:?}"
    );
    assert_eq!(
        slow_ticks_over_8ms, 0,
        "{label} should not produce ticks over 8ms, tick={tick:?}"
    );
    assert_eq!(
        slow_ticks_over_16ms, 0,
        "{label} should not produce ticks over 16ms, tick={tick:?}"
    );
    assert!(
        frame.p95_ms < 1.0,
        "{label} frame p95 should stay under 1ms, got {frame:?}"
    );
    assert!(
        viewport.p95_ms < FRAME_240HZ_MS,
        "{label} viewport p95 should stay under 240Hz frame budget, got {viewport:?}"
    );
}

#[derive(Debug, Default)]
struct HotPathTimings {
    scroll_times: Vec<std::time::Duration>,
    viewport_times: Vec<std::time::Duration>,
    frame_times: Vec<std::time::Duration>,
    tick_times: Vec<std::time::Duration>,
}

fn new_hot_path_profile_model(width: u16, height: u16) -> Model {
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
    model
}

fn prepare_hot_path_profile_model(model: &mut Model, _width: u16, _height: u16) {
    model
        .composer_mut()
        .replace_text_and_move_to_end("ready for high-frequency scroll profile");
    model.sync_composer_height();
}

fn warm_hot_path_frame(model: &mut Model, terminal: &mut Terminal<TestBackend>, height: u16) {
    let layout = model.build_document_layout();
    let viewport = model.build_document_viewport(&layout);
    assert_eq!(viewport.lines.len(), usize::from(height));
    terminal
        .draw(|frame| model.render(frame))
        .expect("hot path warm frame should render");
}

fn measure_scroll_hot_path(
    model: &mut Model,
    terminal: &mut Terminal<TestBackend>,
    height: u16,
    steps: usize,
) -> HotPathTimings {
    let mut timings = HotPathTimings {
        scroll_times: Vec::with_capacity(steps),
        viewport_times: Vec::with_capacity(steps),
        frame_times: Vec::with_capacity(steps),
        tick_times: Vec::with_capacity(steps),
    };

    for _ in 0..steps {
        let tick_started_at = std::time::Instant::now();

        let scroll_started_at = std::time::Instant::now();
        model.scroll_document_by(Model::document_mouse_wheel_delta());
        timings.scroll_times.push(scroll_started_at.elapsed());

        let viewport_started_at = std::time::Instant::now();
        let layout = model.build_document_layout();
        let viewport = model.build_document_viewport(&layout);
        assert_eq!(viewport.lines.len(), usize::from(height));
        timings.viewport_times.push(viewport_started_at.elapsed());

        let frame_started_at = std::time::Instant::now();
        terminal
            .draw(|frame| model.render(frame))
            .expect("hot path frame render should succeed");
        timings.frame_times.push(frame_started_at.elapsed());

        timings.tick_times.push(tick_started_at.elapsed());
    }

    timings
}

fn measure_selection_drag_hot_path(
    model: &mut Model,
    terminal: &mut Terminal<TestBackend>,
    height: u16,
    steps: usize,
) -> HotPathTimings {
    let mut timings = HotPathTimings {
        scroll_times: Vec::with_capacity(steps),
        viewport_times: Vec::with_capacity(steps),
        frame_times: Vec::with_capacity(steps),
        tick_times: Vec::with_capacity(steps),
    };

    for _ in 0..steps {
        let tick_started_at = std::time::Instant::now();

        let scroll_started_at = std::time::Instant::now();
        model.scroll_document_by(Model::document_mouse_wheel_delta());
        timings.scroll_times.push(scroll_started_at.elapsed());

        let viewport_started_at = std::time::Instant::now();
        let layout = model.build_document_layout();
        if let Some(point) = model.selection_point_for_mouse_with_layout(12, height - 1, &layout) {
            model.update_selection_focus(point);
        }
        let viewport = model.build_document_viewport(&layout);
        assert_eq!(viewport.lines.len(), usize::from(height));
        timings.viewport_times.push(viewport_started_at.elapsed());

        let frame_started_at = std::time::Instant::now();
        terminal
            .draw(|frame| model.render(frame))
            .expect("selection drag hot path frame render should succeed");
        timings.frame_times.push(frame_started_at.elapsed());

        timings.tick_times.push(tick_started_at.elapsed());
    }

    timings
}

fn print_hot_path_profile(label: &str, metadata: String, timings: &HotPathTimings) {
    let slow_ticks_over_8ms = timings
        .tick_times
        .iter()
        .filter(|duration| duration.as_millis() >= 8)
        .count();
    let slow_ticks_over_16ms = timings
        .tick_times
        .iter()
        .filter(|duration| duration.as_millis() >= 16)
        .count();
    eprintln!(
        "{label} {metadata} scroll_ms={} viewport_ms={} frame_ms={} tick_ms={} slow_ticks={{over_8ms:{slow_ticks_over_8ms},over_16ms:{slow_ticks_over_16ms}}}",
        format_duration_distribution(&timings.scroll_times),
        format_duration_distribution(&timings.viewport_times),
        format_duration_distribution(&timings.frame_times),
        format_duration_distribution(&timings.tick_times),
    );
}

fn huge_user_message_for_hot_path() -> String {
    "这是一条单条 100k 级别用户消息，用于验证超长 item 在高频滚动下不会退化到全文路径。mixed English text with emoji 👨‍👩‍👧 and wrapping pressure. "
            .repeat(900)
}

fn huge_assistant_markdown_for_hot_path() -> String {
    let mut content = String::from("# Huge Assistant Markdown\n\n");
    for index in 0..220 {
        let _ = writeln!(
            content,
            "## Section {index}\n\n- viewport rendering should stay local\n- markdown keeps lists and prose active\n\n```rust\nfn section_{index}() -> &'static str {{\n    \"{}\"\n}}\n```\n\n{}",
            "large markdown code content ".repeat(10),
            "Follow-up prose wraps through the document viewport without forcing full-frame work. "
                .repeat(4),
        );
    }
    content
}

#[test]
#[ignore = "targeted single 100k user-message scroll profile"]
fn single_100k_user_message_scroll_profile() {
    let width = 100;
    let height = 30;
    let scroll_steps = 600;
    let mut terminal = Terminal::new(TestBackend::new(width, height))
        .expect("single 100k user-message profile backend should initialize");
    let mut model = new_hot_path_profile_model(width, height);
    model.transcript_mut().append_message_with_style_mode(
        Sender::User,
        "short prelude",
        StyleMode::Cx,
    );
    let long_user_message = huge_user_message_for_hot_path();
    model.transcript_mut().append_message_with_style_mode(
        Sender::User,
        long_user_message.clone(),
        StyleMode::Cx,
    );
    model.transcript_mut().append_message_with_style_mode(
        Sender::Assistant,
        "short tail",
        StyleMode::Cx,
    );
    prepare_hot_path_profile_model(&mut model, width, height);

    let sync_profile = model.sync_transcript_render_profile();
    model.sync_command_panel_navigation();
    model.sync_composer_height();
    let layout = model.build_document_layout();
    let document_lines = layout.line_count();
    let transcript_lines = layout.transcript_line_count;
    model.apply_document_viewport_position(&layout, 0, 0, false, true);
    drop(layout);
    warm_hot_path_frame(&mut model, &mut terminal, height);

    let timings = measure_scroll_hot_path(&mut model, &mut terminal, height, scroll_steps);
    print_hot_path_profile(
        "single_100k_user_scroll",
        format!(
            "message_bytes={} document_lines={document_lines} transcript_lines={transcript_lines} estimate_ms={:.3} visible_exact_ms={:.3}",
            long_user_message.len(),
            duration_ms(sync_profile.estimate_time),
            duration_ms(sync_profile.visible_exact_time),
        ),
        &timings,
    );
    assert_release_hot_path_budget(
        "single_100k_user_message_scroll_profile",
        &timings.viewport_times,
        &timings.frame_times,
        &timings.tick_times,
    );
}

#[test]
#[ignore = "targeted huge assistant-markdown scroll profile"]
fn huge_assistant_markdown_scroll_profile() {
    let width = 100;
    let height = 30;
    let scroll_steps = 600;
    let mut terminal = Terminal::new(TestBackend::new(width, height))
        .expect("huge assistant markdown profile backend should initialize");
    let mut model = new_hot_path_profile_model(width, height);
    model.transcript_mut().append_message_with_style_mode(
        Sender::User,
        "short prelude",
        StyleMode::Cx,
    );
    let markdown = huge_assistant_markdown_for_hot_path();
    model.transcript_mut().append_message_with_style_mode(
        Sender::Assistant,
        markdown.clone(),
        StyleMode::Cx,
    );
    model.transcript_mut().append_message_with_style_mode(
        Sender::User,
        "short tail",
        StyleMode::Cx,
    );
    prepare_hot_path_profile_model(&mut model, width, height);

    let sync_profile = model.sync_transcript_render_profile();
    model.sync_command_panel_navigation();
    model.sync_composer_height();
    let layout = model.build_document_layout();
    let document_lines = layout.line_count();
    let transcript_lines = layout.transcript_line_count;
    model.apply_document_viewport_position(&layout, 0, 0, false, true);
    drop(layout);
    warm_hot_path_frame(&mut model, &mut terminal, height);

    let timings = measure_scroll_hot_path(&mut model, &mut terminal, height, scroll_steps);
    print_hot_path_profile(
        "huge_assistant_markdown_scroll",
        format!(
            "message_bytes={} document_lines={document_lines} transcript_lines={transcript_lines} estimate_ms={:.3} visible_exact_ms={:.3}",
            markdown.len(),
            duration_ms(sync_profile.estimate_time),
            duration_ms(sync_profile.visible_exact_time),
        ),
        &timings,
    );
    assert_release_hot_path_budget(
        "huge_assistant_markdown_scroll_profile",
        &timings.viewport_times,
        &timings.frame_times,
        &timings.tick_times,
    );
}

#[test]
#[ignore = "targeted selection-drag mixed long-history profile"]
fn selection_drag_mixed_long_history_profile() {
    let width = 100;
    let height = 30;
    let item_count = 3_000;
    let drag_steps = 600;
    let mut terminal = Terminal::new(TestBackend::new(width, height))
        .expect("selection drag profile backend should initialize");
    let mut model = new_hot_path_profile_model(width, height);
    let mut raw_text_bytes = 0usize;
    let mut long_item_count = 0usize;
    for index in 0..item_count {
        let (sender, content, is_long) = mixed_scroll_profile_message(index);
        raw_text_bytes += content.len();
        long_item_count += usize::from(is_long);
        model
            .transcript_mut()
            .append_message_with_style_mode(sender, content, StyleMode::Cx);
    }
    prepare_hot_path_profile_model(&mut model, width, height);
    let sync_profile = model.sync_transcript_render_profile();
    model.sync_command_panel_navigation();
    model.sync_composer_height();
    let layout = model.build_document_layout();
    let document_lines = layout.line_count();
    let transcript_lines = layout.transcript_line_count;
    model.apply_document_viewport_position(&layout, 0, 0, false, true);
    if let Some(point) = model.selection_point_for_mouse_with_layout(4, 0, &layout) {
        model.start_selection(point);
    }
    drop(layout);
    warm_hot_path_frame(&mut model, &mut terminal, height);

    let timings = measure_selection_drag_hot_path(&mut model, &mut terminal, height, drag_steps);
    print_hot_path_profile(
        "selection_drag_mixed_long_scroll",
        format!(
            "items={item_count} long_items={long_item_count} raw_bytes={raw_text_bytes} document_lines={document_lines} transcript_lines={transcript_lines} estimate_ms={:.3} visible_exact_ms={:.3}",
            duration_ms(sync_profile.estimate_time),
            duration_ms(sync_profile.visible_exact_time),
        ),
        &timings,
    );
    assert_release_hot_path_budget(
        "selection_drag_mixed_long_history_profile",
        &timings.viewport_times,
        &timings.frame_times,
        &timings.tick_times,
    );
}

#[test]
#[ignore = "targeted mixed long-history high-frequency scroll profile"]
fn mixed_long_history_high_frequency_scroll_profile() {
    let width = 100;
    let height = 30;
    let item_count = 3_000;
    let scroll_steps = 900;
    let mut terminal = Terminal::new(TestBackend::new(width, height))
        .expect("mixed long-history scroll profile backend should initialize");
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

    let mut raw_text_bytes = 0usize;
    let mut long_item_count = 0usize;
    for index in 0..item_count {
        let (sender, content, is_long) = mixed_scroll_profile_message(index);
        raw_text_bytes += content.len();
        long_item_count += usize::from(is_long);
        model
            .transcript_mut()
            .append_message_with_style_mode(sender, content, StyleMode::Cx);
    }
    model
        .composer_mut()
        .replace_text_and_move_to_end("ready for high-frequency scroll profile");
    model.sync_composer_height();

    let sync_started_at = std::time::Instant::now();
    let sync_profile = model.sync_transcript_render_profile();
    let sync_time = sync_started_at.elapsed();
    model.sync_command_panel_navigation();
    model.sync_composer_height();

    let initial_layout = model.build_document_layout();
    let document_lines = initial_layout.line_count();
    let transcript_lines = initial_layout.transcript_line_count;
    model.apply_document_viewport_position(&initial_layout, 0, 0, false, true);
    drop(initial_layout);

    let warm_layout = model.build_document_layout();
    let warm_viewport = model.build_document_viewport(&warm_layout);
    assert_eq!(warm_viewport.lines.len(), usize::from(height));
    terminal
        .draw(|frame| model.render(frame))
        .expect("mixed long-history warm frame should render");

    let mut scroll_times = Vec::with_capacity(scroll_steps);
    let mut viewport_times = Vec::with_capacity(scroll_steps);
    let mut frame_times = Vec::with_capacity(scroll_steps);
    let mut tick_times = Vec::with_capacity(scroll_steps);
    for _ in 0..scroll_steps {
        let tick_started_at = std::time::Instant::now();

        let scroll_started_at = std::time::Instant::now();
        model.scroll_document_by(Model::document_mouse_wheel_delta());
        scroll_times.push(scroll_started_at.elapsed());

        let viewport_started_at = std::time::Instant::now();
        let layout = model.build_document_layout();
        let viewport = model.build_document_viewport(&layout);
        assert_eq!(viewport.lines.len(), usize::from(height));
        viewport_times.push(viewport_started_at.elapsed());

        let frame_started_at = std::time::Instant::now();
        terminal
            .draw(|frame| model.render(frame))
            .expect("mixed long-history frame render should succeed");
        frame_times.push(frame_started_at.elapsed());

        tick_times.push(tick_started_at.elapsed());
    }

    let slow_ticks_over_8ms = tick_times
        .iter()
        .filter(|duration| duration.as_millis() >= 8)
        .count();
    let slow_ticks_over_16ms = tick_times
        .iter()
        .filter(|duration| duration.as_millis() >= 16)
        .count();

    eprintln!(
        "mixed_long_scroll items={item_count} long_items={long_item_count} raw_bytes={raw_text_bytes} size={width}x{height} document_lines={document_lines} transcript_lines={transcript_lines} sync_ms={:.3} estimate_ms={:.3} visible_exact_ms={:.3} scroll_ms={} viewport_ms={} frame_ms={} tick_ms={} slow_ticks={{over_8ms:{slow_ticks_over_8ms},over_16ms:{slow_ticks_over_16ms}}}",
        duration_ms(sync_time),
        duration_ms(sync_profile.estimate_time),
        duration_ms(sync_profile.visible_exact_time),
        format_duration_distribution(&scroll_times),
        format_duration_distribution(&viewport_times),
        format_duration_distribution(&frame_times),
        format_duration_distribution(&tick_times),
    );

    assert_release_hot_path_budget(
        "mixed_long_history_high_frequency_scroll_profile",
        &viewport_times,
        &frame_times,
        &tick_times,
    );
}

fn mixed_scroll_profile_message(index: usize) -> (Sender, String, bool) {
    if index.is_multiple_of(37) {
        return (Sender::User, mixed_long_user_message(index), true);
    }
    if index.is_multiple_of(11) {
        return (Sender::Assistant, mixed_markdown_code_message(index), false);
    }
    if index.is_multiple_of(3) {
        return (
            Sender::User,
            format!(
                "short user message {index}: {}",
                "中文 mixed English prompt keeps normal rows in the transcript. ".repeat(2)
            ),
            false,
        );
    }

    (
        Sender::Assistant,
        format!(
            "## Assistant {index}\n\n- short markdown row\n- viewport should remain cheap\n\n```text\n{}\n```\n",
            "small code block ".repeat(4),
        ),
        false,
    )
}

fn mixed_long_user_message(index: usize) -> String {
    let target_repeats = match index % 3 {
        0 => 64,
        1 => 128,
        _ => 212,
    };
    format!(
            "long user message {index}: {}",
            "这是一段高频滚动基准使用的超长消息 mixed English text, symbols, emoji 👨‍👩‍👧, and wrapping pressure. "
                .repeat(target_repeats)
        )
}

fn mixed_markdown_code_message(index: usize) -> String {
    format!(
        "## Tool output {index}\n\nThe assistant response mixes prose, lists, and code blocks.\n\n- keep markdown parsing active\n- preserve viewport-only rendering\n\n```rust\nfn profile_{index}() -> &'static str {{\n    \"{}\"\n}}\n```\n\n{}",
        "markdown code content ".repeat(8),
        "Follow-up prose keeps wrapping across several terminal columns. ".repeat(3),
    )
}

#[test]
#[ignore = "targeted 100000-item high-frequency scroll profile"]
fn document_pipeline_scroll_profile_for_100000_items() {
    let width = 80;
    let height = 24;
    let item_count = 100_000;
    let scroll_steps = 900;
    let mut terminal = Terminal::new(TestBackend::new(width, height))
        .expect("100000-item scroll profile backend should initialize");
    let mut model = new_hot_path_profile_model(width, height);
    let raw_text_bytes = append_standard_benchmark_history(&mut model, item_count);
    prepare_hot_path_profile_model(&mut model, width, height);

    let sync_started_at = std::time::Instant::now();
    let sync_profile = model.sync_transcript_render_profile();
    let sync_time = sync_started_at.elapsed();
    model.sync_command_panel_navigation();
    model.sync_composer_height();

    let layout = model.build_document_layout();
    let document_lines = layout.line_count();
    let transcript_lines = layout.transcript_line_count;
    model.apply_document_viewport_position(&layout, 0, 0, false, true);
    drop(layout);
    warm_hot_path_frame(&mut model, &mut terminal, height);

    let timings = measure_scroll_hot_path(&mut model, &mut terminal, height, scroll_steps);
    print_hot_path_profile(
        "document_100000_scroll",
        format!(
            "items={item_count} raw_bytes={raw_text_bytes} size={width}x{height} document_lines={document_lines} transcript_lines={transcript_lines} sync_ms={:.3} estimate_ms={:.3} visible_exact_ms={:.3}",
            duration_ms(sync_time),
            duration_ms(sync_profile.estimate_time),
            duration_ms(sync_profile.visible_exact_time),
        ),
        &timings,
    );
    assert_release_large_history_240hz_budget(
        "document_pipeline_scroll_profile_for_100000_items",
        &timings.viewport_times,
        &timings.frame_times,
        &timings.tick_times,
    );
}

fn append_standard_benchmark_history(model: &mut Model, item_count: usize) -> usize {
    let mut raw_text_bytes = 0usize;
    for index in 0..item_count {
        let (sender, content) = standard_benchmark_message(index);
        raw_text_bytes += content.len();
        model
            .transcript_mut()
            .append_message_with_style_mode(sender, content, StyleMode::Cx);
    }
    raw_text_bytes
}

fn standard_benchmark_message(index: usize) -> (Sender, String) {
    if index.is_multiple_of(3) {
        return (Sender::User, benchmark_user_message(index));
    }

    (Sender::Assistant, benchmark_assistant_markdown(index))
}

#[test]
fn standard_benchmark_fast_estimates_match_exact_line_counts() {
    let width = 80;
    let palette = default_palette();
    let mut mismatches = Vec::new();

    for index in 0..30 {
        let (sender, content) = standard_benchmark_message(index);
        let mut transcript = Transcript::new(palette);
        transcript.set_gap(0);
        transcript.set_width(width);
        transcript.append_message_with_style_mode(sender, content, StyleMode::Cx);

        let estimated = transcript.progressive_item_metrics_index();
        let estimated_metrics = estimated.metrics[0];
        let exact = transcript.item_metrics_index();
        let exact_metrics = exact.metrics[0];

        if estimated_metrics.content_line_count != exact_metrics.content_line_count
            || estimated_metrics.content_char_len != exact_metrics.content_char_len
        {
            mismatches.push((
                index,
                sender,
                (
                    estimated_metrics.content_line_count,
                    estimated_metrics.content_char_len,
                ),
                (
                    exact_metrics.content_line_count,
                    exact_metrics.content_char_len,
                ),
            ));
        }
    }

    assert!(
        mismatches.is_empty(),
        "standard benchmark messages should not force suffix line-position rewrites during hot scroll: {mismatches:?}"
    );
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

#[test]
#[ignore = "stress profile for width-change rerender scales"]
fn document_pipeline_width_change_profiles_up_to_one_million_items() {
    for item_count in [10_000_usize, 100_000_usize, 1_000_000_usize] {
        for &(from_width, to_width) in &[(80_u16, 120_u16), (120_u16, 80_u16), (80_u16, 56_u16)] {
            let summary =
                measure_width_change_document_pipeline_stress(item_count, from_width, to_width, 24);
            eprintln!("{}", format_document_stress_summary(&summary));

            assert_eq!(summary.item_count, item_count);
            assert_eq!(
                summary.scenario,
                DocumentStressScenario::WidthChange {
                    from_width,
                    to_width,
                }
            );
            assert!(summary.transcript_line_count > 0);
            assert!(summary.document_line_count >= summary.transcript_line_count);
            assert!(summary.viewport_line_count > 0);
            assert!(summary.frame_non_empty_cells > 0);
        }
    }
}

#[test]
#[ignore = "targeted 100000-item cold-resume profile"]
fn document_pipeline_stress_profile_for_100000_items() {
    let summary = measure_document_pipeline_stress(100_000, 80, 24);
    eprintln!("{}", format_document_stress_summary(&summary));

    assert_eq!(summary.item_count, 100_000);
    assert_eq!(summary.scenario, DocumentStressScenario::ColdResume);
    assert!(summary.transcript_line_count > 0);
}

#[test]
#[ignore = "targeted 100000-item width-change profile"]
fn document_pipeline_width_change_profile_for_100000_items() {
    for &(from_width, to_width) in &[(80_u16, 120_u16), (120_u16, 80_u16), (80_u16, 56_u16)] {
        let summary =
            measure_width_change_document_pipeline_stress(100_000, from_width, to_width, 24);
        eprintln!("{}", format_document_stress_summary(&summary));

        assert_eq!(summary.item_count, 100_000);
        assert_eq!(
            summary.scenario,
            DocumentStressScenario::WidthChange {
                from_width,
                to_width,
            }
        );
        assert!(summary.transcript_line_count > 0);
    }
}
