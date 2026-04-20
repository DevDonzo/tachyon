use criterion::{Criterion, black_box, criterion_group, criterion_main};
use tachyon_bench::synthetic_trace_spans;
use tachyon_trace::TraceIndex;

fn bench_trace_window_query(c: &mut Criterion) {
    let spans = synthetic_trace_spans(64, 4_000);
    let index = TraceIndex::build(spans).unwrap();

    c.bench_function("trace_window_query", |bench| {
        bench.iter(|| {
            let result = index
                .query_window(black_box(500_000), black_box(1_800_000), black_box(10_000))
                .unwrap();
            black_box(result.len())
        });
    });
}

criterion_group!(benches, bench_trace_window_query);
criterion_main!(benches);
