#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Collapses a criterion run into one small JSON file that can be attached to a
# release and compared against the next one.
#
#   cargo bench --bench inventory
#   ./scripts/bench-report.sh --version 0.1.3-2026.aug.21 --out benchmarks.json
#
# Only the median and its absolute deviation are kept. The median is what
# criterion itself reports, and the deviation is what tells a later comparison
# whether a difference is real or just a noisy runner.
#
# shellcheck shell=bash
set -euo pipefail

dir="target/criterion"
out="-"
version=""
commit="${GITHUB_SHA-$(git rev-parse --short HEAD 2>/dev/null || echo unknown)}"

while (($# > 0)); do
  case "$1" in
    --criterion-dir) dir="${2:?--criterion-dir needs a path}"; shift 2 ;;
    --out) out="${2:?--out needs a path}"; shift 2 ;;
    --version) version="${2:?--version needs a value}"; shift 2 ;;
    --commit) commit="${2:?--commit needs a value}"; shift 2 ;;
    *) echo "unknown argument '$1'" >&2; exit 2 ;;
  esac
done

if [[ ! -d "$dir" ]]; then
  echo "no criterion output at '$dir' — run cargo bench first" >&2
  exit 1
fi

if [[ -z "$version" ]]; then
  version="$(grep -m1 '^version = ' Cargo.toml | cut -d'"' -f2)"
fi

# Benchmark names contain spaces, so walk the tree null-delimited.
entries=()
while IFS= read -r -d '' estimates; do
  meta="$(dirname "$estimates")/benchmark.json"
  [[ -f "$meta" ]] || continue
  entries+=("$(jq -n --slurpfile e "$estimates" --slurpfile m "$meta" '
    {
      id: $m[0].full_id,
      median_ns: ($e[0].median.point_estimate),
      mad_ns: ($e[0].median_abs_dev.point_estimate)
    }')")
done < <(find "$dir" -path '*/new/estimates.json' -print0 | sort -z)

if ((${#entries[@]} == 0)); then
  echo "found no benchmark estimates under '$dir'" >&2
  exit 1
fi

report="$(printf '%s\n' "${entries[@]}" | jq -s \
  --arg version "$version" \
  --arg commit "$commit" \
  --arg generated "$(date -u +%Y-%m-%dT%H:%M:%SZ)" '
  {
    schema: 1,
    version: $version,
    commit: $commit,
    generated: $generated,
    benchmarks: (map({key: .id, value: {median_ns: .median_ns, mad_ns: .mad_ns}}) | from_entries)
  }')"

if [[ "$out" == "-" ]]; then
  printf '%s\n' "$report"
else
  printf '%s\n' "$report" > "$out"
  echo "wrote $(jq '.benchmarks | length' <<< "$report") benchmarks to $out" >&2
fi
