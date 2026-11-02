#!/usr/bin/env bash
#
# Render the README's per-store stress badges from the report `examples/stress` writes.
#
# Like coverage-badge.sh, these are files this repository renders and stores,
# not images fetched from a badge service: no account, no upload token, and
# nothing that stops resolving when a third party's free tier changes. CI
# publishes them to the `stress` branch and `raw.githubusercontent.com` serves
# them; the README reads that branch, so a badge is a commit traceable to the
# run that produced it.
#
# Usage: stress-badge.sh <report.json> <out-dir> <store>
#
# `<report.json>` is the `FinalReport` the harness writes (`status`,
# `workflows_per_second`, `max_rss_bytes`, `final_process_rows`, ...).
# `<store>` names the badge files (`stress-<store>.svg`,
# `stress-throughput-<store>.svg`, ...) so memory and sled never overwrite
# each other.

set -euo pipefail

# A locale with a comma decimal separator would render `82,4 wf/s`; the badge
# and the harness's own report must agree on the decimal point.
export LC_ALL=C

report="${1:?usage: stress-badge.sh <report.json> <out-dir> <store>}"
out="${2:?usage: stress-badge.sh <report.json> <out-dir> <store>}"
store="${3:?usage: stress-badge.sh <report.json> <out-dir> <store>}"
mkdir -p "$out"

status="$(jq -r '.status // "FAIL"' "$report")"
throughput="$(jq -r '.workflows_per_second // 0' "$report")"
max_rss="$(jq -r '.max_rss_bytes // 0' "$report")"
rows="$(jq -r '.final_process_rows // 0' "$report")"

case "$status" in
  PASS) status_color="#4c1" ;;
  *) status_color="#e05d44" ;;
esac

throughput_value="$(printf '%.1f wf/s' "$throughput")"
rss_value="$(printf '%.2f GiB' "$(awk -v bytes="$max_rss" 'BEGIN { print bytes / 1073741824 }')")"

# The same bands the cleanup acceptance uses: zero rows is the pass value, a
# small remainder is a warning, and anything larger is a failure the reader
# should look at.
if [ "$rows" -eq 0 ]; then
  rows_color="#4c1"
elif [ "$rows" -le 100 ]; then
  rows_color="#dfb317"
else
  rows_color="#e05d44"
fi

# The SVG geometry is shared with prof-badge.sh (see badge.sh).
# shellcheck source=badge.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/badge.sh"

render_badge "stress ${store}" "$status" "$status_color" 170 "$out/stress-${store}.svg"
render_badge "throughput" "$throughput_value" "#007ec6" 160 "$out/stress-throughput-${store}.svg"
render_badge "peak RSS" "$rss_value" "#007ec6" 150 "$out/stress-rss-${store}.svg"
render_badge "rows left" "$rows" "$rows_color" 140 "$out/stress-rows-${store}.svg"

echo "stress ${store}: ${status} | ${throughput_value} | peak ${rss_value} | ${rows} rows -> ${out}"
