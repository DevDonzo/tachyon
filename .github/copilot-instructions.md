# Copilot Instructions for this Repository

## Current project state
- This repository is currently planning-focused and centered on **Tachyon**.
- The canonical project definition is `PROJECTS.md`; treat it as the source of truth for scope, architecture, and performance targets.

## Build, test, and lint commands
- There are **no runnable build/test/lint commands currently defined** in the repository yet (no Rust workspace has been scaffolded yet).
- After the workspace is initialized, use workspace-level Cargo commands unless project scripts override them:
  - Build: `cargo build --workspace`
  - Test (full): `cargo test --workspace`
  - Test (single): `cargo test -p <crate_name> <test_name>`
  - Lint: `cargo clippy --workspace --all-targets -- -D warnings`
  - Format check: `cargo fmt --all -- --check`

## High-level architecture (from `PROJECTS.md`)
- Tachyon is intended as a multi-crate Rust workspace with clear boundaries:
  - `tachyon-core`: shared types/errors/metrics contracts.
  - `tachyon-ingest`: file open, memory mapping, chunk scheduling, newline/timestamp indexing.
  - `tachyon-search`: query compilation, chunk-parallel scan, streaming results.
  - `tachyon-render`: viewport model, layout/glyph caches, GPU draw data.
  - `tachyon-trace`: OTLP/JSON span parsing and time-window indexing.
  - `tachyon-app`: desktop shell, panes, commands, orchestration.
  - `tachyon-bench`: repeatable performance benchmarks.
- Target data flow: open file -> mmap + chunk metadata -> background indexes -> viewport reads/search stream -> GPU render of visible rows only -> optional trace timeline window queries.

## Key repository conventions
- **Performance-first constraints are part of the spec**: changes should preserve measurable goals (throughput, frame stability, interaction latency).
- **Zero-copy and bounded-memory behavior are default expectations**:
  - operate on byte slices/offsets;
  - avoid whole-file materialization (`Vec<String>`-style loading is explicitly against intended design).
- **UI responsiveness is non-negotiable**:
  - UI thread must not block on indexing/search;
  - long operations run in workers with cancellation for stale queries.
- **Visible-region-first behavior**:
  - prioritize work for currently visible viewport before full-file completion when possible (especially for search/highlighting).
- **Benchmark evidence is required deliverable quality**:
  - benchmark/profiling artifacts are expected to accompany major performance-sensitive changes.
