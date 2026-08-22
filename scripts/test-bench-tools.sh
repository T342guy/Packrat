#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Tests for bench-report.sh and bench-compare.sh against synthetic criterion
# output, so the verdict machinery is checked without running a real bench.
#
# shellcheck shell=bash
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

pass=0; fail=0
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT

expect() { # name  needle  <<< haystack
  local name="$1" needle="$2" hay; hay="$(cat)"
  if grep -qF -- "$needle" <<< "$hay"; then
    pass=$((pass+1)); printf '  ok   %s\n' "$name"
  else
    fail=$((fail+1)); printf '  FAIL %s\n     wanted: %s\n     got:\n%s\n' \
      "$name" "$needle" "$(sed 's/^/       /' <<< "$hay" | head -6)"
  fi
}

# --- bench-report.sh reads a criterion tree ------------------------------
fake_criterion() { # dir  name  median  mad
  local d="$1/$2/new"; mkdir -p "$d"
  printf '{"median":{"point_estimate":%s},"median_abs_dev":{"point_estimate":%s}}\n' \
    "$3" "$4" > "$d/estimates.json"
  printf '{"full_id":"%s"}\n' "$2" > "$d/benchmark.json"
}

crit="$tmp/criterion"
fake_criterion "$crit" "inventory/fast case" 1000 10
fake_criterion "$crit" "inventory/slow case" 2000000 50000

echo "== bench-report.sh =="
report="$(./scripts/bench-report.sh --criterion-dir "$crit" --version 1.2.3 --commit abc)"
expect "keeps the version"      '"version": "1.2.3"'  <<< "$report"
expect "keeps the commit"       '"commit": "abc"'     <<< "$report"
expect "finds both benchmarks"  '2'                   <<< "$(jq '.benchmarks|length' <<< "$report")"
expect "records the median"     '1000'                <<< "$(jq '.benchmarks["inventory/fast case"].median_ns' <<< "$report")"
expect "records the deviation"  '50000'               <<< "$(jq '.benchmarks["inventory/slow case"].mad_ns' <<< "$report")"

./scripts/bench-report.sh --criterion-dir "$tmp/nothing-here" >/dev/null 2>&1
if (($? != 0)); then
  pass=$((pass+1)); printf '  ok   fails on a missing criterion tree\n'
else
  fail=$((fail+1)); printf '  FAIL missing criterion tree should fail\n'
fi

# --- bench-compare.sh verdicts -------------------------------------------
# Tight deviations, so the 5%% floor rather than the noise band decides.
mk() { # file  version  ns...
  local file="$1" version="$2"; shift 2
  local i=0 body=""
  for ns in "$@"; do
    body+="$(printf '"case%d":{"median_ns":%s,"mad_ns":%s}' "$i" "$ns" "$(bc -l <<< "$ns * 0.002")")"
    i=$((i+1))
    (($# > i)) && body+=","
  done
  printf '{"schema":1,"version":"%s","commit":"x","generated":"now","benchmarks":{%s}}\n' \
    "$version" "$body" > "$file"
}

echo
echo "== bench-compare.sh verdicts =="
mk "$tmp/base.json" "1.0.0" 1000 2000 4000

mk "$tmp/half.json" "1.1.0" 500 1000 2000
out="$(./scripts/bench-compare.sh "$tmp/base.json" "$tmp/half.json")"
expect "halving every case reads BETTER"  '**BETTER**'  <<< "$out"
expect "and reports -50% overall"         'Overall -50%' <<< "$out"

mk "$tmp/double.json" "1.1.0" 2000 4000 8000
out="$(./scripts/bench-compare.sh "$tmp/base.json" "$tmp/double.json")"
expect "doubling every case reads WORSE"  '**WORSE**'    <<< "$out"
expect "and reports +100% overall"        'Overall +100%' <<< "$out"

mk "$tmp/same.json" "1.1.0" 1010 1980 4020
out="$(./scripts/bench-compare.sh "$tmp/base.json" "$tmp/same.json")"
expect "sub-1% drift is NO CHANGE"        '**NO CHANGE**' <<< "$out"
expect "and is labelled as noise"         '(noise)'       <<< "$out"
expect "with no 'what moved' section"     'no-what-moved' \
  <<< "$(grep -q 'What moved' <<< "$out" && echo 'what-moved-present' || echo 'no-what-moved')"

mk "$tmp/mixed.json" "1.1.0" 500 2000 8000
out="$(./scripts/bench-compare.sh "$tmp/base.json" "$tmp/mixed.json")"
expect "one up one down reads MIXED"      '**MIXED'       <<< "$out"
expect "and lists what moved"             'What moved'    <<< "$out"

echo
echo "== bench-compare.sh edge cases =="
out="$(./scripts/bench-compare.sh "" "$tmp/base.json")"
expect "no baseline still prints numbers" 'No previous release' <<< "$out"

out="$(./scripts/bench-compare.sh "$tmp/absent.json" "$tmp/base.json")"
expect "an unreadable baseline degrades"  'No previous release' <<< "$out"

mk "$tmp/other.json" "0.9.0" 7
jq '.benchmarks = {"unrelated": .benchmarks.case0}' "$tmp/other.json" > "$tmp/other2.json"
out="$(./scripts/bench-compare.sh "$tmp/other2.json" "$tmp/base.json")"
expect "disjoint sets are not a verdict"  'NO SHARED BENCHMARKS' <<< "$out"
expect "new cases are called out"         'New in this release'  <<< "$out"
expect "dropped cases are called out"     'Gone since the last'  <<< "$out"

# A change smaller than the measurement spread is noise even past the floor.
mk "$tmp/wobbly-base.json" "1.0.0" 1000
mk "$tmp/wobbly-now.json"  "1.1.0" 1080
jq '.benchmarks.case0.mad_ns = 200' "$tmp/wobbly-base.json" > "$tmp/wb.json"
jq '.benchmarks.case0.mad_ns = 200' "$tmp/wobbly-now.json"  > "$tmp/wn.json"
out="$(./scripts/bench-compare.sh "$tmp/wb.json" "$tmp/wn.json")"
expect "a wobbly runner does not fake a regression" '**NO CHANGE**' <<< "$out"

echo
echo "passed $pass, failed $fail"
(( fail == 0 ))
