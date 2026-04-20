use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use std::sync::atomic::AtomicBool;
use tachyon_bench::synthetic_log_bytes;
use tachyon_core::{LineNumber, SearchQuery};
use tachyon_ingest::{DEFAULT_CHUNK_SIZE, NewlineIndex};
use tachyon_search::{SearchConfig, search_streaming};

fn bench_search(c: &mut Criterion) {
    let bytes = synthetic_log_bytes(500_000, 48);
    let index = NewlineIndex::from_bytes_parallel(&bytes, DEFAULT_CHUNK_SIZE);

    let mut group = c.benchmark_group("search");
    group.throughput(Throughput::Bytes(bytes.len() as u64));

    group.bench_function("substring_rare_match_visible_first", |bench| {
        let query = SearchQuery::substring("req=0007a11f", true).unwrap();
        let config = SearchConfig {
            visible_lines: LineNumber(0)..LineNumber(80),
            chunk_lines: 8_192,
            max_hits: usize::MAX,
            batch_hit_target: 512,
        };
        bench.iter(|| {
            let cancelled = AtomicBool::new(false);
            let emitted = search_streaming(
                black_box(&bytes),
                black_box(&index),
                black_box(&query),
                black_box(&config),
                &cancelled,
                |_| {},
            )
            .unwrap();
            black_box(emitted)
        });
    });

    group.bench_function("regex_visible_first", |bench| {
        let query = SearchQuery::regex(r"request_id=[0-9a-f]{8}").unwrap();
        let config = SearchConfig {
            visible_lines: LineNumber(0)..LineNumber(80),
            chunk_lines: 8_192,
            max_hits: usize::MAX,
            batch_hit_target: 512,
        };
        bench.iter(|| {
            let cancelled = AtomicBool::new(false);
            let emitted = search_streaming(
                black_box(&bytes),
                black_box(&index),
                black_box(&query),
                black_box(&config),
                &cancelled,
                |_| {},
            )
            .unwrap();
            black_box(emitted)
        });
    });

    group.finish();
}

criterion_group!(benches, bench_search);
criterion_main!(benches);
