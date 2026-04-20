#!/usr/bin/env bash
set -euo pipefail

OUTPUT_PATH="${1:-assets/samples/synthetic.log}"
LINE_COUNT="${2:-1000000}"

mkdir -p "$(dirname "$OUTPUT_PATH")"

awk -v line_count="$LINE_COUNT" '
BEGIN {
  for (i = 1; i <= line_count; i++) {
    printf "2026-01-01T00:%02d:%02dZ service=api level=INFO request_id=%08x msg=\"synthetic line %d\"\n",
           int(i / 60) % 60, i % 60, i, i
  }
}
' > "$OUTPUT_PATH"

echo "generated $LINE_COUNT lines at $OUTPUT_PATH"
