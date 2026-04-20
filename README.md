# Tachyon

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/rust-2024%2B-orange.svg)](https://www.rust-lang.org/)
[![CI](../../actions/workflows/ci.yml/badge.svg)](../../actions/workflows/ci.yml)
[![Release](../../actions/workflows/release.yml/badge.svg)](../../actions/workflows/release.yml)

**Tachyon** is a high-performance desktop log and trace explorer built in Rust. It is designed for developers and SREs who need to navigate, search, and analyze massive log files (100GB+) with zero latency.

## About

Tachyon is an independent research project exploring high-performance systems engineering. It focuses on zero-copy data processing and GPU-accelerated visualization to provide a low-latency environment for inspecting massive datasets.

Tachyon aims to provide a fluid, responsive, and impossibly fast experience by leveraging modern systems engineering, memory-mapped I/O, and GPU-accelerated rendering.

## Performance Goals

Tachyon is built with a performance-first mindset:
- **Instant Interaction:** Open 100GB+ files and start scrolling in < 500ms.
- **High Throughput:** 3-5 GB/s newline indexing and 1+ GB/s search throughput on modern hardware.
- **Fluid UI:** Target 120 FPS rendering for smooth navigation and filtering.
- **Bounded Memory:** Zero-copy file access using `memmap2` to keep memory usage low regardless of file size.

## Key Features

- **Massive File Support:** Seamlessly handle logs and traces that exceed available RAM.
- **Virtualized Viewport:** Only the visible region is rendered, ensuring constant-time interaction regardless of file size.
- **Parallel Search:** Chunk-parallel substring and regex search with progressive result streaming.
- **Trace Visualization:** A high-performance timeline view for distributed traces (OTLP/JSON).
- **Modern UI:** A clean, GPU-accelerated interface built for low-latency feedback.

## Architecture

Tachyon is structured as a multi-crate Rust workspace to ensure modularity and testability:

| Crate | Responsibility |
|-------|----------------|
| `tachyon-core` | Shared domain types and error handling. |
| `tachyon-ingest` | Memory-mapping, chunked indexing, and file management. |
| `tachyon-search` | Parallel search engine and query compilation. |
| `tachyon-render` | Viewport virtualization and GPU-accelerated drawing. |
| `tachyon-trace` | Distributed trace parsing and timeline indexing. |
| `tachyon-app` | UI orchestration and application shell. |
| `tachyon-bench` | Reproducible performance benchmarks and profiling. |

## Getting Started

### Prerequisites
- [Rust](https://www.rust-lang.org/tools/install) (2024 edition)

### Building
```bash
cargo build --release
```

### Running Benchmarks
```bash
cargo bench -p tachyon-bench
```

## CI/CD

GitHub Actions now provides:
- **CI (`.github/workflows/ci.yml`)**: format check, clippy, tests, release-mode workspace build, and perf smoke on main/manual runs.
- **CD (`.github/workflows/release.yml`)**: builds `tachyon-app` binaries for Linux/macOS/Windows and publishes them to GitHub Releases for `v*` tags.

## Roadmap

- [x] **Phase 1 (foundation):** Parallel newline indexing and basic seeking.
- [x] **Phase 2 (foundation):** Virtualized viewport ranges, jump/scroll controls, and bounded window fetches.
- [x] **Phase 3 (foundation):** Streaming search batches with visible-region priority, parallel background chunk scans, and cancellation support.
- [ ] **Phase 4:** GPU-accelerated text rendering optimizations.
- [ ] **Phase 5:** OTLP/JSON trace timeline support.

## License

Distributed under the MIT License. See `LICENSE` for more information.
