use std::hint::black_box;

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use lumos::frontend::tui::{StyleMode, benchmark, theme::default_palette};

fn transcript_benches(c: &mut Criterion) {
    let palette = default_palette();
    let markdown = benchmark::markdown_document_fixture();
    let prose = benchmark::prompt_prose_fixture();
    let literal = benchmark::prompt_tabbed_literal_fixture();
    let mut group = c.benchmark_group("frontend_tui/transcript");

    group.throughput(Throughput::Bytes(markdown.len() as u64));
    group.bench_function("render_markdown", |b| {
        b.iter(|| {
            black_box(benchmark::render_markdown_plain_text(
                &markdown, 72, palette,
            ))
        });
    });

    group.throughput(Throughput::Bytes(prose.len() as u64));
    group.bench_function("wrap_prompt_visual_lines/prose", |b| {
        b.iter(|| black_box(benchmark::wrap_prompt_visual_lines_summary(&prose, 36, 2)));
    });

    group.throughput(Throughput::Bytes(literal.len() as u64));
    group.bench_function("wrap_prompt_visual_lines/literal_tabs", |b| {
        b.iter(|| black_box(benchmark::wrap_prompt_visual_lines_summary(&literal, 24, 2)));
    });

    for item_count in [64_usize, 512_usize, 2048_usize] {
        group.throughput(Throughput::Elements(item_count as u64));
        group.bench_with_input(
            BenchmarkId::new("list_render/cold", item_count),
            &item_count,
            |b, &item_count| {
                b.iter_batched(
                    || benchmark::TranscriptBench::new(item_count, 72, palette),
                    |mut bench| black_box(bench.render()),
                    BatchSize::SmallInput,
                );
            },
        );
        group.bench_with_input(
            BenchmarkId::new("list_render/cache_hit", item_count),
            &item_count,
            |b, &item_count| {
                let mut bench = benchmark::TranscriptBench::new(item_count, 72, palette);
                black_box(bench.render());
                b.iter(|| black_box(bench.render()));
            },
        );
        group.bench_with_input(
            BenchmarkId::new("list_render/append_fast_path", item_count),
            &item_count,
            |b, &item_count| {
                b.iter_batched(
                    || {
                        let mut bench = benchmark::TranscriptBench::new(item_count, 72, palette);
                        black_box(bench.render());
                        bench
                    },
                    |mut bench| black_box(bench.append_benchmark_item_and_render()),
                    BatchSize::SmallInput,
                );
            },
        );
    }
}

fn composer_benches(c: &mut Criterion) {
    let palette = default_palette();
    let draft = benchmark::composer_draft_fixture();
    let mut group = c.benchmark_group("frontend_tui/composer");

    group.throughput(Throughput::Bytes(draft.len() as u64));
    group.bench_function("render_document_with_input", |b| {
        b.iter(|| {
            black_box(benchmark::render_composer_document_with_input(
                &draft,
                64,
                StyleMode::Ms,
                palette,
            ))
        });
    });
}

fn document_benches(c: &mut Criterion) {
    let mut group = c.benchmark_group("frontend_tui/document");

    for item_count in [24_usize, 512_usize, 2048_usize] {
        group.throughput(Throughput::Elements(item_count as u64));
        group.bench_with_input(
            BenchmarkId::new("composition/build_layout", item_count),
            &item_count,
            |b, &item_count| {
                let mut bench = benchmark::DocumentBench::new(item_count, 80, 18);
                b.iter(|| black_box(bench.rebuild_layout()));
            },
        );
        group.bench_with_input(
            BenchmarkId::new("composition/build_offset_viewport", item_count),
            &item_count,
            |b, &item_count| {
                let mut bench = benchmark::DocumentBench::new(item_count, 80, 18);
                bench.prepare_offset_viewport_state();
                b.iter(|| black_box(bench.build_offset_viewport()));
            },
        );
        group.bench_with_input(
            BenchmarkId::new("composition/build_bottom_follow_viewport", item_count),
            &item_count,
            |b, &item_count| {
                let mut bench = benchmark::DocumentBench::new(item_count, 80, 18);
                bench.prepare_bottom_follow_viewport_state();
                b.iter(|| black_box(bench.build_bottom_follow_viewport()));
            },
        );
        group.bench_with_input(
            BenchmarkId::new("layout_after_transcript_append", item_count),
            &item_count,
            |b, &item_count| {
                b.iter_batched(
                    || benchmark::DocumentBench::new(item_count, 80, 18),
                    |mut bench| black_box(bench.rebuild_layout_after_transcript_append()),
                    BatchSize::SmallInput,
                );
            },
        );
        group.bench_with_input(
            BenchmarkId::new("layout_after_composer_edit", item_count),
            &item_count,
            |b, &item_count| {
                b.iter_batched(
                    || benchmark::DocumentBench::new(item_count, 80, 18),
                    |mut bench| black_box(bench.rebuild_layout_after_composer_edit()),
                    BatchSize::SmallInput,
                );
            },
        );
    }
}

fn model_render_benches(c: &mut Criterion) {
    let mut group = c.benchmark_group("frontend_tui/model");

    for (width, height, item_count) in [(80_u16, 24_u16, 2048_usize), (120_u16, 40_u16, 2048_usize)]
    {
        group.throughput(Throughput::Elements(item_count as u64));
        group.bench_with_input(
            BenchmarkId::new(
                "render_frame",
                format!("{width}x{height}_{item_count}_items"),
            ),
            &(width, height, item_count),
            |b, &(width, height, item_count)| {
                b.iter_batched(
                    || benchmark::ModelRenderBench::new(item_count, width, height),
                    |mut bench| black_box(bench.render_frame()),
                    BatchSize::PerIteration,
                );
            },
        );
    }
}

criterion_group!(
    name = benches;
    config = Criterion::default().sample_size(20);
    targets = transcript_benches, composer_benches, document_benches, model_render_benches
);
criterion_main!(benches);
