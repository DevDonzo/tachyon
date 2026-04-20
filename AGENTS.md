# AGENTS Instructions

## Project source of truth
- This repository is currently planning-focused around **Tachyon**.
- Treat `PROJECTS.md` as the canonical spec for scope, architecture, and performance targets.

## Build, test, and lint
- No runnable build/test/lint pipeline is defined yet (workspace not scaffolded).
- Once initialized, use workspace Cargo commands unless scripts override:
  - `cargo build --workspace`
  - `cargo test --workspace`
  - `cargo test -p <crate_name> <test_name>` (single test)
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo fmt --all -- --check`

## Intended architecture
- Multi-crate Rust workspace:
  - `tachyon-core`: shared types/errors/contracts
  - `tachyon-ingest`: mmap, chunking, newline/timestamp indexing
  - `tachyon-search`: query compile + chunk-parallel search + streaming hits
  - `tachyon-render`: virtualized viewport + GPU draw prep
  - `tachyon-trace`: OTLP/JSON span parsing + time-window indexes
  - `tachyon-app`: UI shell and orchestration
  - `tachyon-bench`: reproducible performance benchmarks

## Repository-specific conventions
- Keep design **performance-first** and aligned with the metrics in `PROJECTS.md`.
- Prefer **zero-copy, byte-offset-oriented** logic; avoid full-file line materialization.
- Keep the **UI thread non-blocking**; indexing/search run in workers with cancelation.
- Prioritize **visible-region-first** updates for viewport and search highlighting.
- Treat **benchmarks/profiling evidence** as part of feature completion for hot paths.
