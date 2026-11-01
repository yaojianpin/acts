#!/usr/bin/env bash
#
# Render the README's `examples/prof` badges for one store from its report.
#
# Usage: prof-badge.sh <report.json> <out-dir> <memory|sled>
#
# `<report.json>` is the `prof` schema (`status`, `avg_us`, `p99_us`,
# `completions_per_second`, ...). Both stores are published separately, so a
# reader can compare the in-memory engine path with the durable sled path
# without one hiding the other.
set -euo pipefail

# A locale with a comma decimal separator would render `1,24 ms`.
export LC_ALL=C

report="${1:?usage: prof-badge.sh <report.json> <out-dir> <memory|sled>}"
out="${2:?usage: prof-badge.sh <report.json> <out-dir> <memory|sled>}"
store="${3:?usage: prof-badge.sh <report.json> <out-dir> <memory|sled>}"
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

render_badge "prof ${store}" "$status" "$status_color" 170 "$out/prof-${store}.svg"
render_badge "avg ${store}" "$avg_value" "#007ec6" 160 "$out/prof-${store}-avg.svg"
render_badge "p99 ${store}" "$p99_value" "#007ec6" 160 "$out/prof-${store}-p99.svg"
render_badge "rate ${store}" "$rate_value" "#007ec6" 160 "$out/prof-${store}-throughput.svg"

echo "prof ${store}: ${status} | avg ${avg_value} | p99 ${p99_value} | ${rate_value} -> ${out}"
