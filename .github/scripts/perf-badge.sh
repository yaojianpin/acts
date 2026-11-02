#!/usr/bin/env bash
#
# Render the README's `examples/perf` badges for one store from its report.
#
# Usage: perf-badge.sh <report.json> <out-dir> <memory|sled|rocksdb>
#
# `<report.json>` is the `perf` schema (`status`, `avg_us`, `p99_us`,
# `completions_per_second`, ...). Every store is published separately, so a
# reader can compare the in-memory engine path with the durable sled and
# RocksDB paths without one hiding another.
set -euo pipefail

# A locale with a comma decimal separator would render `1,24 ms`.
export LC_ALL=C

report="${1:?usage: perf-badge.sh <report.json> <out-dir> <memory|sled|rocksdb>}"
out="${2:?usage: perf-badge.sh <report.json> <out-dir> <memory|sled|rocksdb>}"
store="${3:?usage: perf-badge.sh <report.json> <out-dir> <memory|sled|rocksdb>}"
mkdir -p "$out"

# shellcheck source=badge.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/badge.sh"

status="$(jq -r '.status // "FAIL"' "$report")"
avg_us="$(jq -r '.avg_us // 0' "$report")"
p99_us="$(jq -r '.p99_us // 0' "$report")"
rate="$(jq -r '.completions_per_second // 0' "$report")"

case "$status" in
  PASS) status_color="#4c1" ;;
  *) status_color="#e05d44" ;;
esac

avg_value="$(printf '%.2f ms' "$(awk -v us="$avg_us" 'BEGIN { print us / 1000 }')")"
p99_value="$(printf '%.2f ms' "$(awk -v us="$p99_us" 'BEGIN { print us / 1000 }')")"
rate_value="$(printf '%.0f/s' "$rate")"

render_badge "perf ${store}" "$status" "$status_color" 170 "$out/perf-${store}.svg"
render_badge "avg ${store}" "$avg_value" "#007ec6" 160 "$out/perf-${store}-avg.svg"
render_badge "p99 ${store}" "$p99_value" "#007ec6" 160 "$out/perf-${store}-p99.svg"
render_badge "rate ${store}" "$rate_value" "#007ec6" 160 "$out/perf-${store}-throughput.svg"

echo "perf ${store}: ${status} | avg ${avg_value} | p99 ${p99_value} | ${rate_value} -> ${out}"
