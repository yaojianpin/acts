#!/usr/bin/env bash
#
# Shared SVG badge renderer for the README's measured badges.
#
# The badges are files this repository renders and stores (`coverage.svg`,
# `stress*.svg`), not images fetched from a badge service: no
# account, no upload token, and nothing that stops resolving when a third
# party's free tier changes. The workflows publish them to a branch and
# `raw.githubusercontent.com` serves them, so a badge is a commit traceable to
# the run that produced it.
#
# Sourced by `coverage`-style scripts (stress-badge.sh); it is
# not executable on its own.
#
# render_badge <label> <value> <color> <width> <file>
#
# One fixed geometry per badge, split evenly between label and value: labels
# are constants and values are short (`82.4 wf/s`, `1.24 ms`), so a fixed width
# keeps the badges from jumping around in the README between runs. The two
# <text> pairs per string are the embossed look shields badges have (a shadow
# copy a pixel lower).
render_badge() {
  local label="$1" value="$2" color="$3" width="$4" file="$5"
  local label_w=$((width / 2))
  local value_w=$((width - label_w))
  local label_x=$((label_w / 2))
  local value_x=$((label_w + value_w / 2))
  cat > "$file" <<SVG
<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="20" role="img" aria-label="${label}: ${value}">
  <title>${label}: ${value}</title>
  <defs>
    <clipPath id="round"><rect width="${width}" height="20" rx="3" fill="#fff"/></clipPath>
    <linearGradient id="shade" x2="0" y2="100%">
      <stop offset="0" stop-color="#bbb" stop-opacity=".1"/>
      <stop offset="1" stop-opacity=".1"/>
    </linearGradient>
  </defs>
  <g clip-path="url(#round)">
    <rect width="${label_w}" height="20" fill="#555"/>
    <rect x="${label_w}" width="${value_w}" height="20" fill="${color}"/>
    <rect width="${width}" height="20" fill="url(#shade)"/>
  </g>
  <g fill="#fff" text-anchor="middle" font-family="Verdana,Geneva,DejaVu Sans,sans-serif" font-size="11">
    <text x="${label_x}" y="15" fill="#010101" fill-opacity=".3">${label}</text>
    <text x="${label_x}" y="14">${label}</text>
    <text x="${value_x}" y="15" fill="#010101" fill-opacity=".3">${value}</text>
    <text x="${value_x}" y="14">${value}</text>
  </g>
</svg>
SVG
}
