#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

./scripts/gen_synthetic_logs.sh assets/samples/perf-smoke.log 200000

cargo run -q -p tachyon-app -- assets/samples/perf-smoke.log --chunk-size $((8 * 1024 * 1024)) >/dev/null
cargo bench -q -p tachyon-bench --bench newline_index -- --sample-size 10
