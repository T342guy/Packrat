#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Compares two bench-report.sh files and writes a markdown verdict.
#
#   ./scripts/bench-compare.sh previous.json current.json
#
# A shared runner is noisy, so a difference only counts when it clears both a
# flat 5% floor and the two runs' own measured spread. Everything else is
# reported as unchanged rather than dressed up as a win.
#
# shellcheck shell=bash
set -euo pipefail

floor=5.0
title="Benchmarks"

while (($# > 2)); do
  case "$1" in
    --floor) floor="${2:?--floor needs a percentage}"; shift 2 ;;
    --title) title="${2:?--title needs a value}"; shift 2 ;;
    *) break ;;
  esac
done

previous="${1-}"
current="${2:?usage: bench-compare.sh [--floor PCT] [--title T] <previous.json> <current.json>}"

[[ -f "$current" ]] || { echo "no current report at '$current'" >&2; exit 1; }

# No baseline to compare against: still show the numbers, just without a verdict.
if [[ -z "$previous" || ! -f "$previous" ]]; then
  jq -r --arg title "$title" '
    "### \($title)", "",
    "No previous release to compare against — these are the baseline numbers.", "",
    "| case | median |", "| --- | --- |",
    (.benchmarks | to_entries | sort_by(.key)[]
      | "| \(.key) | \(.value.median_ns | (. / 1000 * 100 | round / 100)) µs |")
  ' "$current"
  exit 0
fi

jq -rn --slurpfile prev "$previous" --slurpfile cur "$current" \
       --argjson floor "$floor" --arg title "$title" '
  def us: . / 1000 * 100 | round / 100;
  # Two decimal places. Everything reaching it is already a percentage.
  def pct: . * 100 | round / 100;
  def sign: if . > 0 then "+" else "" end;

  ($prev[0].benchmarks) as $p
  | ($cur[0].benchmarks) as $c
  | [ $c | to_entries[]
      | select($p[.key] != null)
      | .key as $id
      | .value.median_ns as $now
      | $p[$id].median_ns as $was
      | (($now - $was) / $was * 100) as $delta
      # The runs own spread, as a percentage, sets a second floor: a change
      # smaller than the measurement wobble is not a change.
      | (($p[$id].mad_ns / $was + .value.mad_ns / $now) * 100) as $noise
      | ([$floor, $noise] | max) as $threshold
      | {
          id: $id, was: $was, now: $now, delta: $delta,
          verdict: (if $delta <= -$threshold then "faster"
                    elif $delta >= $threshold then "slower"
                    else "same" end)
        }
    ] as $rows

  | ($rows | map(select(.verdict == "faster")) | length) as $faster
  | ($rows | map(select(.verdict == "slower")) | length) as $slower
  | ($rows | length) as $total
  | ($c | to_entries | map(select($p[.key] == null) | .key)) as $added
  | ($p | to_entries | map(select($c[.key] == null) | .key)) as $dropped

  # Geometric mean of the ratios: the honest way to average a set of speedups.
  # Scaled to a percentage here so it shares units with the per-case deltas.
  | (if $total == 0 then 0
     else ((($rows | map((.now / .was) | log) | add) / $total | exp) - 1) * 100
     end) as $overall

  | (if $total == 0 then ["⚪", "no shared benchmarks", "Nothing in common with the previous report."]
     elif $faster == 0 and $slower == 0 then ["⚪", "no change", "Every case landed inside the noise band."]
     elif $slower == 0 then ["🟢", "better", "\($faster) case\(if $faster == 1 then "" else "s" end) got faster and nothing regressed."]
     elif $faster == 0 then ["🔴", "worse", "\($slower) case\(if $slower == 1 then "" else "s" end) regressed and nothing improved."]
     elif $overall < 0 then ["🟡", "mixed, net better", "\($faster) faster against \($slower) slower."]
     elif $overall > 0 then ["🟠", "mixed, net worse", "\($slower) slower against \($faster) faster."]
     else ["🟡", "mixed", "\($faster) faster against \($slower) slower."] end) as $call

  | (if ($rows | map(select(.verdict != "same")) | length) > 0
     then $rows | map(select(.verdict != "same")) | sort_by(.delta) | reverse
     else [] end) as $moved

  | def row: "| \(.id) | \(.was | us) µs | \(.now | us) µs | \(if .verdict == "faster" then "🟢 " elif .verdict == "slower" then "🔴 " else "" end)\(.delta | pct | sign)\(.delta | pct)%\(if .verdict == "same" then " (noise)" else "" end) |";
    def header: "| case | \($prev[0].version) | \($cur[0].version) | change |", "| --- | ---: | ---: | :--- |";

    "### \($title)",
    "",
    "\($call[0]) **\($call[1] | ascii_upcase)** against `\($prev[0].version)` — \($call[2]) Overall \((($overall | pct) | sign))\($overall | pct)% across \($total) shared case\(if $total == 1 then "" else "s" end).",
    "",
    ( if ($moved | length) > 0 then
        ("**What moved**", "", header, ($moved[] | row), "")
      else empty end ),
    "<details><summary>All \($total) cases</summary>",
    "",
    header,
    ( $rows | sort_by(.delta) | reverse | .[] | row ),
    "",
    "</details>",
    ( if ($added | length) > 0 then
        "", "New in this release, so not compared: " + ($added | sort | map("`\(.)`") | join(", ")) + "."
      else empty end ),
    ( if ($dropped | length) > 0 then
        "", "Gone since the last release: " + ($dropped | sort | map("`\(.)`") | join(", ")) + "."
      else empty end )
'
