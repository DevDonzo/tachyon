use criterion::{Criterion, black_box, criterion_group, criterion_main};
use tachyon_bench::synthetic_log_bytes;
use tachyon_core::LineNumber;
use tachyon_ingest::NewlineIndex;
use tachyon_render::{HighlightKind, HighlightSpan, RenderPipelineState, Viewport};

fn bench_frame_planning(c: &mut Criterion) {
    let bytes = synthetic_log_bytes(200_000, 64);
    let index = NewlineIndex::from_bytes_parallel(&bytes, 8 * 1024 * 1024);
    let total_lines = index.total_lines();

    let mut viewport = Viewport::new(80, 200);
    let mut planner = RenderPipelineState::new(256);
    let highlights = vec![
        HighlightSpan::new(LineNumber(10), 4, 12, HighlightKind::Search).unwrap(),
        HighlightSpan::new(LineNumber(10), 13, 20, HighlightKind::Search).unwrap(),
        HighlightSpan::new(LineNumber(12), 2, 5, HighlightKind::Diagnostic).unwrap(),
    ];

    c.bench_function("render_frame_plan", |bench| {
        bench.iter(|| {
            viewport.scroll_lines(black_box(1), total_lines);
            let visible = viewport.visible_line_range(total_lines);
            let line_text = (visible.start.0..visible.end.0)
                .map(|line| {
                    let range = index.line_byte_range(LineNumber(line)).unwrap();
                    String::from_utf8_lossy(&bytes[range.start.0 as usize..range.end.0 as usize])
                        .into_owned()
                })
                .collect::<Vec<_>>();

            let plan = planner.plan_frame(
                visible,
                line_text.iter().map(String::as_str),
                black_box(&highlights),
            );
            black_box(plan.upload_line_ranges.len() + plan.highlight_batches.len())
        });
    });
}

criterion_group!(benches, bench_frame_planning);
criterion_main!(benches);
