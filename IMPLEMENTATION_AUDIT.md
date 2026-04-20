# Tachyon Implementation Audit Report

## Scope
This audit focuses on implementation quality, architecture fitness, and package/tool choices for the current Tachyon codebase against the stated goals (large-file handling, responsive interaction, bounded memory, visible-first search, and trace timeline usability).

## Executive verdict
**Verdict: Foundation-ready, not production-ready.**

The current implementation is strong enough to continue building on, but there are a few high-impact hotspots where the design will not scale cleanly to sustained 100GB-class workloads and highly interactive GUI usage without further optimization.

---

## What is already good

1. **Correct structural direction**
   - Workspace split and crate boundaries are coherent.
   - Ingest/search/render/trace concerns are separated in a maintainable way.

2. **Large-file ingest baseline is correct**
   - `memmap2`-based zero-copy read path is appropriate.
   - Parallel newline scan (`memchr`) is a practical throughput-oriented choice.

3. **Search design has a solid core**
   - Visible-first + background stage split is the right user-facing behavior.
   - Case-sensitive substring fast path using `memchr::memmem` is a good optimization direction.

4. **GUI stack choice is viable**
   - `eframe/egui` is a reasonable choice for fast iteration and cross-platform desktop delivery.
   - Background worker model for open/search/trace loading is directionally correct for responsiveness.

5. **Benchmark culture is present**
   - Dedicated benches and `perf_smoke.sh` are in place.
   - This is a strong foundation for performance discipline.

---

## Key findings (why this is not yet production-ready)

### High impact

1. **Per-frame search-hit processing in GUI is too expensive**
   - In `crates/tachyon-app/src/gui.rs` (`show_logs`), the code clones all hits per frame and re-filters per visible line (`self.search_hits.clone()` and per-line `hits.iter().filter(...)`).
   - This creates heavy CPU/allocation pressure when hit count is large, and risks frame hitching.
   - **Impact:** direct risk to smooth scrolling and typing latency under realistic high-match scenarios.

2. **Backpressure is missing in search streaming path**
   - `crates/tachyon-search/src/lib.rs` uses `crossbeam_channel::unbounded()` for background batch delivery.
   - Under high-match queries, producers can outpace consumers and spike memory.
   - **Impact:** memory blowups and unstable behavior on larger workloads.

3. **Case-insensitive substring path is allocation-heavy**
   - `find_substring_offsets` lowercases per-line buffers and then scans naively.
   - This is materially slower and allocation-heavy compared to the case-sensitive fast path.
   - **Impact:** avoidable performance cliff for a common search mode.

### Medium impact

4. **Visible-first prioritization can become stale while scrolling**
   - Search captures visible range at start; rapid viewport changes during active search are not reprioritized.
   - **Impact:** weaker interactive feel during continuous navigation.

5. **Trace query output clones strings per result**
   - `crates/tachyon-trace/src/lib.rs` returns owned `TimelineSpan` values with cloned string fields.
   - **Impact:** extra allocation overhead in repeated timeline queries.

6. **Benchmark/query mismatch in search bench**
   - Synthetic logs use `req=...` while regex bench checks `request_id=...`.
   - This can measure mostly miss-scan behavior for that case.
   - **Impact:** misleading benchmark conclusions if interpreted as representative hit workloads.

---

## Package/tool assessment and swap recommendations

## Keep (good choices now)

- **`memmap2`**: correct for zero-copy file-backed access.
- **`memchr`**: excellent for byte-pattern primitives and substring fast path.
- **`rayon`**: good default for CPU-parallel chunk scanning.
- **`eframe/egui`**: acceptable UI stack for current stage; can ship real apps.
- **`criterion`**: correct benchmark tooling baseline.

## Improve usage (before swapping)

1. **`crossbeam-channel`**
   - Keep crate, but switch from `unbounded` to **`bounded(cap)`** in hot paths.
   - Add explicit batch sizing + producer throttling to enforce memory ceilings.

2. **`regex`**
   - Keep for general regex coverage, but avoid recompiling frequently and monitor regex-heavy workloads distinctly.

## Potential upgrades where valuable

1. **`regex-automata` (targeted use)**
   - Consider for advanced/hot regex paths needing tighter control (multi-pattern, cache reuse, lower-level control).
   - Use only where profiling proves `regex` bottlenecks.

2. **Case-insensitive search strategy**
   - Introduce ASCII fast path with reduced allocation (or pre-normalized chunk strategy) for case-insensitive substring.
   - Current per-line lowercase allocation should be replaced.

3. **Data structures for hit rendering**
   - Move from flat `Vec<SearchHit>` frame filtering to line-indexed representation:
     - `BTreeMap<LineNumber, Vec<SearchHit>>` or
     - sorted vector + binary-range lookups.
   - This is higher impact than swapping UI libraries.

---

## Prioritized remediation plan (auditor recommendation)

1. **Fix GUI hit rendering complexity** (remove clone + O(visible_lines * total_hits) scan per frame).
2. **Add bounded channel/backpressure for search batches** and enforce hard memory ceilings.
3. **Optimize case-insensitive substring path** to avoid per-line heap allocations.
4. **Add viewport-aware reprioritization during active search** (cancel/restart or dynamic scheduling).
5. **Align search benchmark fixtures and patterns**; include hit-heavy, miss-heavy, and case-insensitive scenarios.
6. **Introduce performance acceptance gates** in CI/perf-smoke for frame time and keystroke-to-highlight targets.
7. **Optimize trace query allocations** (borrowed/indexed views where possible).

---

## Final audit decision

This implementation is **good and worth continuing**. It is not fundamentally flawed, and the package stack is mostly appropriate.

However, it is **not yet the best attainable implementation** for the project’s top-end performance goals. Most remaining risk is in **algorithmic/dataflow details**, not in the high-level crate choices. Fixing the highlighted hotspots should yield larger gains than a wholesale framework rewrite at this stage.
