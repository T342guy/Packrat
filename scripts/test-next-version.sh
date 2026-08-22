#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Tests for next-version.sh, driven through the NEXT_VERSION_TAGS seam so no
# real tags are needed. The load-bearing case is "pre-releases must not
# advance production numbering".
#
# shellcheck shell=bash
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

pass=0; fail=0
today="$(date -u +%Y).$(date -u +%b | tr '[:upper:]' '[:lower:]').$(date -u +%-d)"

check() { # name  expected  tags  args...
  local name="$1" expect="$2" tags="$3"; shift 3
  local got
  got="$(NEXT_VERSION_TAGS="$tags" ./scripts/next-version.sh "$@" 2>&1)" || got="ERROR($?): $got"
  if [[ "$got" == "$expect" ]]; then
    pass=$((pass+1)); printf '  ok   %-46s -> %s\n' "$name" "$got"
  else
    fail=$((fail+1)); printf '  FAIL %-46s -> %s (wanted %s)\n' "$name" "$got" "$expect"
  fi
}

echo "== no tags at all: the manifest supplies the base =="
check "patch from empty history"      "0.1.1-$today"      ""  patch
check "prerelease from empty history" "0.1.1-pre1-$today" ""  patch --prerelease

echo
echo "== a plain production history =="
prod=$'v0.1.0-2026.aug.19\nv0.1.1-2026.aug.20\nv0.1.2-2026.aug.20'
check "patch"  "0.1.3-$today" "$prod" patch
check "minor"  "0.2.0-$today" "$prod" minor
check "major"  "1.0.0-$today" "$prod" major
check "none"   "0.1.2-$today" "$prod" none

echo
echo "== THE CONSTRAINT: pre-releases must not advance production =="
mixed="$prod"$'\nv0.1.3-pre1-2026.aug.21\nv0.1.3-pre2-2026.aug.21'
check "prod patch still lands on 0.1.3"     "0.1.3-$today"      "$mixed" patch
check "next prerelease continues the run"   "0.1.3-pre3-$today" "$mixed" patch --prerelease
deep="$mixed"$'\nv0.1.3-pre3-2026.aug.21\nv0.1.3-pre4-2026.aug.21\nv0.1.3-pre5-2026.aug.21'
check "5 pre-releases deep, prod is 0.1.3"  "0.1.3-$today"      "$deep"  patch
check "6th pre-release"                     "0.1.3-pre6-$today" "$deep"  patch --prerelease
check "minor ignores 0.1.3 pre-releases"    "0.2.0-$today"      "$deep"  minor
check "minor prerelease starts a fresh run" "0.2.0-pre1-$today" "$deep"  minor --prerelease

echo
echo "== once the production release ships, the run resets =="
shipped="$deep"$'\nv0.1.3-2026.aug.21'
check "base advances to 0.1.4"              "0.1.4-$today"      "$shipped" patch
check "pre-run for 0.1.4 starts at pre1"    "0.1.4-pre1-$today" "$shipped" patch --prerelease

echo
echo "== ordering and parsing edge cases =="
check "sorts numerically, not lexically" "0.1.11-$today" \
  $'v0.1.9-2026.aug.19\nv0.1.10-2026.aug.20' patch
check "double-digit pre counter" "0.1.4-pre11-$today" \
  $'v0.1.3-2026.aug.20\nv0.1.4-pre9-2026.aug.21\nv0.1.4-pre10-2026.aug.21' patch --prerelease
check "malformed tags ignored" "0.2.0-$today" \
  $'v0.1.9-2026.aug.19\nnightly\nv2-broken\nv0.1.999' minor
check "bad argument rejected" "ERROR(2): unknown argument '--nope'" "$prod" patch --nope

echo
echo "== every emitted version is valid semver for cargo =="
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/src"; echo 'fn main(){}' > "$tmp/src/main.rs"
for v in "0.1.3-$today" "0.1.3-pre1-$today" "0.1.4-pre11-$today"; do
  printf '[package]\nname = "vercheck"\nversion = "%s"\nedition = "2021"\n' "$v" > "$tmp/Cargo.toml"
  if (cd "$tmp" && cargo metadata --no-deps --format-version 1 >/dev/null 2>&1); then
    pass=$((pass+1)); printf '  ok   cargo accepts %s\n' "$v"
  else
    fail=$((fail+1)); printf '  FAIL cargo rejects %s\n' "$v"
  fi
done

echo
echo "passed $pass, failed $fail"
(( fail == 0 ))
