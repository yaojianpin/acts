#!/usr/bin/env bash
#
# Render the README's coverage badge from the summary `cargo llvm-cov report
# --json --summary-only` writes.
#
# The badge is a file this repository renders and stores, not an image fetched
# from a coverage service: no account, no upload token, and nothing in the
# README that stops resolving when a third party's free tier changes. The
# number in it is the one `.github/workflows/coverage.yml` measured on the
# commit it ran on, and CI publishes the file to a branch (`badges`) that
# `raw.githubusercontent.com` serves — so the badge is a commit, traceable to
# the run that produced it.
#
# Usage: coverage-badge.sh <summary.json> <out.svg>
#
# `<summary.json>` is the LLVM export JSON (whose totals live at
# `data[0].totals.<metric>.percent`), not the human-readable table.

set -euo pipefail

# A locale with a comma decimal separator (de_DE and friends) would put
# `78,4%` in the badge, and `%.1f` would round on the wrong side of it.
export LC_ALL=C

summary="${1:?usage: coverage-badge.sh <summary.json> <out.svg>}"
svg="${2:?usage: coverage-badge.sh <summary.json> <out.svg>}"

# Line coverage, not regions/functions/branches: it is the metric a reader of a
# badge assumes, and the one the summary in the run's own step prints.
percent="$(jq -r '.data[0].totals.lines.percent // empty' "$summary")"
if [ -z "$percent" ]; then
  echo "error: $summary carries no line coverage total" >&2
  exit 1
fi

# One decimal, and the same `printf` the run's summary table uses, so the two
# numbers printed for one metric are never a tenth apart. (Not jq arithmetic:
# `round / 10` renders `95` in one jq and `95.0` in another, and which one is
# on the badge would depend on the runner's jq.)
shown="$(printf '%.1f' "$percent")"
value="${shown}%"

# The same bands shields.io draws, so a reader's expectations hold: bright green
# at 90 and up, then green / yellow-green / yellow / orange, red below 50.
#
# The band is chosen from the *displayed* number, not the raw one: 89.96% is
# drawn as `90.0%`, and a reader who checks the number against the colour would
# find a green `90.0%` wrong.
color="$(
  awk -v p="$shown" 'BEGIN {
    if (p >= 90)      print "#4c1";
    else if (p >= 80) print "#97ca00";
    else if (p >= 70) print "#a4a61d";
    else if (p >= 60) print "#dfb317";
    else if (p >= 50) print "#fe7d37";
    else              print "#e05d44";
  }'
)"

# One fixed geometry (140x20: a 70px label, a 70px value) rather than widths
# measured from the text: the value is at most `100%` and the label is a
# constant, so both boxes have room, and a fixed width is what keeps the badge
# from jumping around in the README between runs. The two `<text>` pairs per
# string are the embossed look shields badges have (a shadow copy a pixel lower).
cat > "$svg" <<SVG
<svg xmlns="http://www.w3.org/2000/svg" width="140" height="20" role="img" aria-label="coverage: ${value}">
  <title>coverage: ${value}</title>
  <defs>
    <clipPath id="round"><rect width="140" height="20" rx="3" fill="#fff"/></clipPath>
    <linearGradient id="shade" x2="0" y2="100%">
      <stop offset="0" stop-color="#bbb" stop-opacity=".1"/>
      <stop offset="1" stop-opacity=".1"/>
    </linearGradient>
  </defs>
  <g clip-path="url(#round)">
    <rect width="70" height="20" fill="#555"/>
    <rect x="70" width="70" height="20" fill="${color}"/>
    <rect width="140" height="20" fill="url(#shade)"/>
  </g>
  <g fill="#fff" text-anchor="middle" font-family="Verdana,Geneva,DejaVu Sans,sans-serif" font-size="11">
    <text x="35" y="15" fill="#010101" fill-opacity=".3">coverage</text>
    <text x="35" y="14">coverage</text>
    <text x="105" y="15" fill="#010101" fill-opacity=".3">${value}</text>
    <text x="105" y="14">${value}</text>
  </g>
</svg>
SVG

echo "coverage: ${value} (${color}) -> ${svg}"
