use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use tachyon_bench::synthetic_log_bytes;
use tachyon_ingest::{DEFAULT_CHUNK_SIZE, NewlineIndex};

fn bench_newline_index(c: &mut Criterion) {
    let bytes = synthetic_log_bytes(500_000, 48);

    let mut group = c.benchmark_group("newline_index");
    group.throughput(Throughput::Bytes(bytes.len() as u64));
    group.bench_function("parallel_chunk_scan", |bench| {
        bench.iter(|| {
            let index = NewlineIndex::from_bytes_parallel(black_box(&bytes), DEFAULT_CHUNK_SIZE);
            black_box(index.newline_count())
        });
    });
    group.finish();
}

criterion_group!(benches, bench_newline_index);
criterion_main!(benches);
