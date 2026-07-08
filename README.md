<div align="center">
  <img src="assets/logo.svg" width="128" height="128" alt="Tachyon Logo" />
  <h1>Tachyon</h1>
  <p><strong>High-performance desktop log and trace explorer built in Rust.</strong></p>

  [![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
  [![Rust](https://img.shields.io/badge/rust-2024%2B-orange.svg)](https://www.rust-lang.org/)
  [![Release](https://img.shields.io/github/v/release/DevDonzo/tachyon?include_prereleases)](https://github.com/DevDonzo/tachyon/releases)
</div>

---

**Tachyon** is a specialized workstation for developers and SREs who need to navigate, search, and analyze massive log files (100GB+) with zero latency.

It is an independent research project focused on high-performance systems engineering, zero-copy data processing, and GPU-accelerated visualization.

## Download

Tachyon ships as a desktop app with the existing logo as its app/window icon. The same binary also supports CLI smoke-test and automation workflows.

1. Download the `.zip` for your platform from the [Releases](https://github.com/DevDonzo/tachyon/releases) page.
2. Unzip the file.
3. Launch the desktop app:
   - **macOS**: open `Tachyon.app`.
   - **Linux/Windows**: run `tachyon-app` without arguments.
4. CLI mode remains available by passing a log path:
   ```bash
   ./tachyon-app my_massive_log.log --search request_id --print-render-plan
   ```
5. **Security Note**: On macOS, you may need to right-click the app and select "Open" the first time, or go to `System Settings > Privacy & Security` and click "Open Anyway".

| Platform | Download |
|----------|----------|
| **macOS** | [Download for macOS (.zip)](https://github.com/DevDonzo/tachyon/releases/latest) |
| **Windows** | [Download for Windows (.zip)](https://github.com/DevDonzo/tachyon/releases/latest) |
| **Linux** | [Download for Linux (.zip)](https://github.com/DevDonzo/tachyon/releases/latest) |

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
- **Modern Desktop UI:** Native app shell with recent files, saved sessions, command palette, search, jump-to-line, trace timeline, and benchmark report view.

## Architecture

Tachyon is structured as a multi-crate Rust workspace:

| Crate | Responsibility |
|-------|----------------|
| `tachyon-core` | Shared domain types and error handling. |
| `tachyon-ingest` | Memory-mapping, chunked indexing, and file management. |
| `tachyon-search` | Parallel search engine and query compilation. |
| `tachyon-render` | Viewport virtualization and GPU-accelerated drawing. |
| `tachyon-trace` | Distributed trace parsing and timeline indexing. |
| `tachyon-app` | UI orchestration and application shell. |
| `tachyon-bench` | Reproducible performance benchmarks and profiling. |

## Getting Started (Build from Source)

### Prerequisites
- [Rust](https://www.rust-lang.org/tools/install) (2024 edition)

### Building
```bash
git clone https://github.com/DevDonzo/tachyon.git
cd tachyon
cargo build --release
```

### Running
```bash
# Desktop app
cargo run -p tachyon-app

# CLI automation path
cargo run -p tachyon-app -- path/to/log.txt --search request_id
```

### Running Benchmarks
```bash
cargo bench -p tachyon-bench
```

### Performance Report

The reproducible smoke harness covers the core hot paths:

```bash
./scripts/perf_smoke.sh
./scripts/bench_report.sh
```

Latest verified local smoke baseline:

| Benchmark | Observed result | Target |
|---|---:|---:|
| `newline_index/parallel_chunk_scan` | ~33.6-37.5 GiB/s | 3-5 GiB/s |
| `search/substring_rare_match_visible_first` | ~87.1-92.3 GiB/s | 1+ GiB/s |
| `search/regex_visible_first` | ~3.5-3.7 GiB/s | 1+ GiB/s |
| `render_frame_plan` | ~43.5-44.7 us | frame-budget friendly |
| `trace_window_query` | ~660-666 us | sub-frame query latency |

## Roadmap

- [x] **Phase 0:** Project scaffolding and CI/CD setup.
- [x] **Phase 1:** Parallel newline indexing and basic seeking.
- [x] **Phase 2:** Virtualized viewport with smooth scrolling.
- [x] **Phase 3:** High-speed streaming search engine.
- [x] **Phase 4:** Render-frame planning with dirty upload ranges, glyph upload budgeting, and highlight batching.
- [x] **Phase 5:** OTLP/JSON trace indexing with lane assignment and time-window timeline queries.
- [x] **Phase 6:** Desktop app polish: command palette, recent files, saved sessions, branded icon assets, error surfacing, release packaging, and benchmark report generation.

## License

Distributed under the MIT License. See `LICENSE` for more information.
