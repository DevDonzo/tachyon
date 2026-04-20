# Tachyon: The Zed-Tier Rust Project (Selected Direction)

This repository is now focused on **one project only**: **Tachyon**.

Tachyon is a desktop log and trace explorer built in Rust that can open massive files (100GB+) with instant navigation, real-time filtering, and a high-refresh-rate GPU UI. The goal is to build something that feels impossibly fast and clearly demonstrates the engineering values Zed cares about: performance, responsiveness, and strong systems design.

---

## 1) What Tachyon is

Tachyon is a **local-first observability workstation** for developers and SREs who need to inspect huge logs and traces without waiting.

It should feel like this:
1. Open a gigantic log file and scroll immediately.
2. Jump to any position without stutter.
3. Type a filter and see results update in real time.
4. Switch from raw logs to trace timeline view to understand latency across services.

The key promise is simple: **size should not change user experience**.

---

## 2) Product goals

### Core goals (must have)
1. Open and interact with files up to **100GB+**.
2. Maintain smooth UX at high frame rates (target **120 FPS**, acceptable **60 FPS** baseline on mid hardware).
3. Support instant, incremental search/filtering for plain text and regex.
4. Virtualize rendering so memory stays bounded regardless of file size.
5. Provide basic distributed trace timeline visualization from OTLP-like data.

### Performance goals (explicit targets)
1. Initial file open: first visible content in **< 500ms** for warm cache, **< 2s** cold.
2. Line index throughput: at least **3-5 GB/s** on modern laptop CPU.
3. Search throughput: at least **1+ GB/s** for simple patterns, higher with SIMD-friendly paths.
4. Scroll/frame latency: no visible hitching during continuous scroll.
5. Keystroke-to-highlight latency while filtering: **< 50ms** median.

### Non-goals (for v1)
1. Full ELK/Splunk replacement.
2. Cluster-wide ingestion pipeline.
3. Cloud backend or multi-user collaboration.
4. Perfect parser support for every log format.

---

## 3) Why this is the right project

Tachyon is the strongest "impress Zed" project because it combines:
1. **Systems engineering** (memory mapping, indexing, concurrency, CPU cache behavior).
2. **Interactive UI engineering** (virtualized rendering, smooth scrolling, low-latency updates).
3. **Performance culture** (benchmarks, profiling, measurable constraints).

If this is executed well, the repo itself proves strong technical taste.

---

## 4) User stories that define success

1. **SRE on incident call:** Opens a 70GB production log and jumps to a suspicious time window instantly.
2. **Backend engineer:** Filters by request ID and regex patterns while continuing to scroll without lockups.
3. **Platform engineer:** Loads trace spans and quickly sees which service dominates end-to-end latency.
4. **Developer on laptop battery:** Uses the tool without fans exploding due to unnecessary copying/rendering.

---

## 5) High-level architecture

Tachyon should be implemented as a multi-crate Rust workspace:

1. `tachyon-core`
   - Shared domain types (`FileId`, `ByteRange`, `LineNumber`, `SearchQuery`, `Span`, etc.)
   - Error types and metrics interfaces.
2. `tachyon-ingest`
   - File opening, memory mapping, chunk scheduling.
   - Newline indexing and optional timestamp indexing.
3. `tachyon-search`
   - Query compilation and parallel chunk scanning.
   - Incremental result streaming.
4. `tachyon-render`
   - Viewport model, line shaping/layout cache, GPU draw data generation.
5. `tachyon-trace`
   - OTLP/JSON trace parsing and time-window indexing.
6. `tachyon-app`
   - UI app shell (WGPU or GPUI), command palette, panes, state orchestration.
7. `tachyon-bench`
   - Criterion benchmarks and reproducible perf harness.

### Data flow
1. Open file -> memory map -> chunk metadata.
2. Background indexers build newline/timestamp indexes.
3. UI requests viewport lines by line range or byte range.
4. Search engine scans chunks in parallel and streams matches.
5. Renderer consumes viewport + highlights and draws only visible rows.
6. Trace mode loads spans and maps current time window to visible tracks.

---

## 6) Core technical design decisions

### A) File access model
1. Use `memmap2` for zero-copy file-backed memory slices.
2. Work with byte slices and offsets, not `String` ownership.
3. Decode UTF-8 lazily; handle invalid UTF-8 with lossy fallback markers.
4. Avoid copying line content unless needed for display cache.

### B) Indexing strategy
1. Build a **newline index** in parallel:
   - Divide file into fixed-size chunks (e.g., 8-64MB).
   - Each worker finds `\n` offsets in its chunk.
   - Merge into one monotonic index.
2. Optional secondary indexes:
   - Timestamp coarse index (for fast time jumps).
   - Block-level bloom filters for quick prefiltering.

### C) Search strategy
1. Query types:
   - Substring (case-sensitive / insensitive).
   - Regex (compiled once, reused).
2. Chunk-parallel execution with work stealing (`rayon`).
3. Stream partial match sets to UI progressively.
4. Prioritize visible region first, then full-file completion.

### D) Viewport virtualization
1. Keep only visible rows + overscan in memory (example: visible 80 rows, overscan 200-500).
2. Scrollbar maps to logical line range, not loaded rows.
3. Jump operations convert target line -> byte offset via newline index.
4. Never materialize whole file into line `Vec<String>`.

### E) Rendering architecture
1. Use GPU-based text rendering path.
2. Keep frame pipeline deterministic:
   - Input update.
   - View model update.
   - Upload changed glyph/instance buffers.
   - Draw.
3. Cache glyph atlases and only re-upload dirty regions.
4. Highlight overlays for search matches should be batched draw calls.

### F) Trace view
1. Parse spans into normalized model:
   - `trace_id`, `span_id`, `parent_span_id`, `service`, `name`, `start_ns`, `end_ns`.
2. Build per-track sorted intervals.
3. Time-window query returns only visible spans.
4. Render as timeline/Gantt bars with zoom + pan.

### G) Concurrency model
1. UI thread is never blocked by indexing/search.
2. Use dedicated worker pools for:
   - Index build.
   - Query execution.
   - Optional parsing/transforms.
3. Communication via channels with explicit backpressure.
4. Cancel stale tasks when user changes query rapidly.

---

## 7) Suggested stack

1. Core: Rust stable, workspace + crates.
2. File access: `memmap2`.
3. Parallelism: `rayon`, `crossbeam-channel`.
4. Search: `regex-automata` (and evaluate `hyperscan` optionally).
5. UI/Rendering: `wgpu` (+ text stack such as `glyphon`) or GPUI if preferred.
6. CLI/flags for benchmarks/dev mode: `clap`.
7. Profiling + telemetry: `tracing`, `tracing-subscriber`.
8. Benchmarks: `criterion`.
9. Testing helpers: `tempfile`, `proptest` (for parser/index invariants).

---

## 8) Repository scaffold (target)

```text
/Cargo.toml
/crates
  /tachyon-core
  /tachyon-ingest
  /tachyon-search
  /tachyon-render
  /tachyon-trace
  /tachyon-app
  /tachyon-bench
/assets
  /samples
/scripts
  gen_synthetic_logs.sh
  perf_smoke.sh
```

---

## 9) Detailed implementation roadmap

## Phase 0 - Bootstrap and quality gates
1. Create workspace + crate boundaries.
2. Add lint and formatting setup (`clippy`, `rustfmt`).
3. Add baseline benchmark harness and synthetic fixture generator.
4. Add CI that runs format, lint, tests, and one perf smoke benchmark.

**Exit criteria:** project builds cleanly; perf harness runnable locally.

## Phase 1 - Massive file open + line index
1. Implement file open + `memmap2`.
2. Implement chunked newline scanning in parallel.
3. Provide APIs:
   - `line_to_byte(line_no) -> byte_offset`
   - `byte_to_line(byte_offset) -> line_no`
4. Add tests on random synthetic files and edge chunks.

**Exit criteria:** can seek to arbitrary line quickly in huge files.

## Phase 2 - Virtualized text viewport
1. Build viewport model (top line, visible rows, overscan).
2. Fetch only required ranges from mmap/index.
3. Implement smooth wheel + keyboard scrolling.
4. Add jump-to-line command.

**Exit criteria:** stable scrolling with bounded memory footprint.

## Phase 3 - Search engine v1
1. Add substring search and regex search.
2. Execute chunk scans in parallel and stream hits.
3. Highlight hits in visible viewport first.
4. Add cancellation token for stale queries.

**Exit criteria:** responsive live search on large files.

## Phase 4 - GPU text rendering optimization
1. Integrate glyph cache/atlas strategy.
2. Reduce per-frame allocations and CPU->GPU transfers.
3. Batch highlights and text draws.
4. Measure frame consistency under active search.

**Exit criteria:** smooth interaction with no major frame drops.

## Phase 5 - Trace mode foundation
1. Implement OTLP-like span parser (JSON first, protobuf optional later).
2. Build interval index by time.
3. Add timeline pane with zoom/pan.
4. Cross-link selected span to related raw log lines if possible.

**Exit criteria:** usable trace timeline with fast window queries.

## Phase 6 - Product polish and credibility
1. Command palette: open file, jump, filter presets.
2. Saved sessions (recent files + query state).
3. Clear error handling for huge/bad files.
4. Bench report generation for README.

**Exit criteria:** compelling demo-ready application.

---

## 10) Benchmark and profiling plan

Track these metrics in `tachyon-bench`:
1. Newline index build throughput (GB/s).
2. Search throughput by pattern type (substring/regex).
3. Time-to-first-render after open.
4. Scroll stability under load (frame time histogram).
5. Memory overhead at 1GB, 10GB, 100GB synthetic datasets.

Profiling tools:
1. `cargo flamegraph` for CPU hotspots.
2. `tracing` spans around indexing/search/render stages.
3. Optional platform GPU profiling where available.

---

## 11) Definition of done (v1)

Tachyon v1 is done when all are true:
1. Opens and navigates very large files with no freeze.
2. Live search remains responsive and cancelable.
3. UI uses virtualization and GPU rendering effectively.
4. Basic trace timeline works with realistic sample data.
5. README includes hard numbers, methodology, and flamegraph screenshots.
6. Demo script is reproducible on another machine.

---

## 12) Demo script for interviews / portfolio

1. Open a 50-100GB synthetic log.
2. Scroll end-to-end and jump to random lines instantly.
3. Run regex filter while scrolling.
4. Show trace view and zoom into latency spike.
5. Open benchmark dashboard and explain measured bottlenecks + fixes.

If this sequence is smooth and quantified, the project reads as elite-level.

---

## 13) Risks and mitigation

1. **Regex performance surprises**
   - Mitigate with query classification and fallback strategies.
2. **GPU text complexity**
   - Start with functional renderer, then optimize in stages.
3. **Index build stalls on slow disks**
   - Stream partial usability while background indexing continues.
4. **Memory blowups from accidental copies**
   - Add instrumentation and audit allocations in hot paths.

---

## 14) Copy/paste brief for `/init`

Use this as your starter brief:

> Build **Tachyon**, a Rust desktop app for exploring huge logs/traces with near-instant interaction.  
> Requirements: memory-mapped file access, parallel newline indexing, virtualized viewport rendering, streaming search (substring + regex), GPU-accelerated text UI, and basic trace timeline view.  
> Prioritize measurable performance: GB/s indexing/search throughput, low frame-time variance, and responsive query cancellation.  
> Create a Rust workspace with clear crate boundaries (`core`, `ingest`, `search`, `render`, `trace`, `app`, `bench`) and ship benchmarks + profiling evidence in README.

---

This file is now the canonical project direction: **build Tachyon**.
