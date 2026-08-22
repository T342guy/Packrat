#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Tests for next-version.sh and release-plan.sh, driven through the
# NEXT_VERSION_TAGS seam so no real tags are needed.
#
# The load-bearing case is "pre-releases must not advance production
# numbering", on every channel at once.
#
# shellcheck shell=bash
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

pass=0; fail=0
today="$(date -u +%Y).$(date -u +%b | tr '[:upper:]' '[:lower:]').$(date -u +%-d)"

tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT

# A few cases exercise the "nothing tagged yet" fallback, where the base comes
# from the manifest. They get their own manifest to read, never the repo's:
# asserting against the real Cargo.toml would mean the expected values silently
# tracked whatever the last release stamped there, and the suite would break on
# its own release commits. Nothing in here may depend on the version Packrat
# happens to be at today.
fixture() { # version -> path to a manifest carrying it
  local dir="$tmp/manifest-$1"
  mkdir -p "$dir"
  printf '[package]\nname = "packrat"\nversion = "%s"\n' "$1" > "$dir/Cargo.toml"
  printf '%s' "$dir/Cargo.toml"
}

check() { # name  expected  tags  args...
  local name="$1" expect="$2" tags="$3"; shift 3
  local got
  got="$(NEXT_VERSION_TAGS="$tags" ./scripts/next-version.sh "$@" 2>&1)" || got="ERROR($?): $got"
  if [[ "$got" == "$expect" ]]; then
    pass=$((pass+1)); printf '  ok   %-48s -> %s\n' "$name" "$got"
  else
    fail=$((fail+1)); printf '  FAIL %-48s -> %s (wanted %s)\n' "$name" "$got" "$expect"
  fi
}

plan() { # name  expected-substring  args...
  local name="$1" expect="$2"; shift 2
  local got
  got="$(./scripts/release-plan.sh "$@" 2>&1 | tr '\n' ' ')" || got="ERROR: $got"
  if [[ "$got" == *"$expect"* ]]; then
    pass=$((pass+1)); printf '  ok   %-48s -> %s\n' "$name" "$expect"
  else
    fail=$((fail+1)); printf '  FAIL %-48s -> %s (wanted %s)\n' "$name" "$got" "$expect"
  fi
}

echo "== no tags at all: the manifest supplies the base =="
plain="$(fixture 3.7.2)"
check "patch from empty history"   "3.7.3-$today"        "" patch --manifest "$plain"
check "minor from empty history"   "3.8.0-$today"        "" minor --manifest "$plain"
check "pre channel from empty"     "3.7.3-pre1-$today"   "" patch --channel pre --manifest "$plain"
check "dev channel from empty"     "3.7.3-dev1-$today"   "" patch --channel dev --manifest "$plain"

# After any release the manifest carries a full version string, so the
# fallback has to strip the date and any channel back off before bumping.
check "a dated manifest is read back to its number" "0.1.4-$today" \
  "" patch --manifest "$(fixture 0.1.3-2026.aug.21)"
check "a pre-release manifest too"                  "0.1.4-$today" \
  "" patch --manifest "$(fixture 0.1.3-pre7-2026.aug.21)"
check "a contributor-channel manifest too"          "0.1.4-$today" \
  "" patch --manifest "$(fixture 0.1.3-claude.2-2026.aug.21)"

# The guard on all of the above: the fixture must win over the repo's own
# Cargo.toml, or these assertions would drift with every release.
check "the repo's own version is never consulted" "9.9.10-$today" \
  "" patch --manifest "$(fixture 9.9.9)"

echo
echo "== a plain production history =="
prod=$'v0.1.0-2026.aug.19\nv0.1.1-2026.aug.20\nv0.1.2-2026.aug.20'
check "patch"  "0.1.3-$today" "$prod" patch
check "minor"  "0.2.0-$today" "$prod" minor
check "major"  "1.0.0-$today" "$prod" major
check "none"   "0.1.2-$today" "$prod" none

echo
echo "== the three channel formats =="
check "pre joins straight on"       "0.1.3-pre1-$today"       "$prod" patch --channel pre
check "dev joins straight on"       "0.1.3-dev1-$today"       "$prod" patch --channel dev
check "a username takes a dot"      "0.1.3-claude.1-$today"   "$prod" patch --channel claude
check "a username ending in a digit" "0.1.3-user1.1-$today"   "$prod" patch --channel user1
check "a hyphenated username"       "0.1.3-some-user.1-$today" "$prod" patch --channel some-user

echo
echo "== THE CONSTRAINT: no channel advances production =="
mixed="$prod"
for t in v0.1.3-pre1-2026.aug.21 v0.1.3-pre2-2026.aug.21 \
         v0.1.3-dev1-2026.aug.21 v0.1.3-dev2-2026.aug.21 v0.1.3-dev3-2026.aug.21 \
         v0.1.3-claude.1-2026.aug.21 v0.1.3-t342guy.1-2026.aug.21; do
  mixed="$mixed"$'\n'"$t"
done
check "7 pre-releases deep, production is 0.1.3" "0.1.3-$today" "$mixed" patch
check "a minor bump is unaffected too"           "0.2.0-$today" "$mixed" minor

echo
echo "== counters are per channel, not shared =="
check "pre continues from 2"          "0.1.3-pre3-$today"      "$mixed" patch --channel pre
check "dev continues from 3"          "0.1.3-dev4-$today"      "$mixed" patch --channel dev
check "claude continues from 1"       "0.1.3-claude.2-$today"  "$mixed" patch --channel claude
check "t342guy continues from 1"      "0.1.3-t342guy.2-$today" "$mixed" patch --channel t342guy
check "an unseen channel starts at 1" "0.1.3-newbie.1-$today"  "$mixed" patch --channel newbie
check "a channel for another base starts at 1" "0.2.0-pre1-$today" "$mixed" minor --channel pre

echo
echo "== channels do not read each other's tags =="
# `dev` must not match `v0.1.3-dev...` when asked for the `d` channel, and a
# username must not match a differently-separated tag.
check "pre does not count dev tags"  "0.1.3-pre3-$today" "$mixed" patch --channel pre
check "claude ignores t342guy tags"  "0.1.3-claude.2-$today" "$mixed" patch --channel claude
check "a prefix is not a match"      "0.1.3-pr.1-$today" "$mixed" patch --channel pr

echo
echo "== once production ships, every channel resets =="
shipped="$mixed"$'\nv0.1.3-2026.aug.21'
check "base advances to 0.1.4"        "0.1.4-$today"       "$shipped" patch
check "pre restarts at 1"             "0.1.4-pre1-$today"  "$shipped" patch --channel pre
check "dev restarts at 1"             "0.1.4-dev1-$today"  "$shipped" patch --channel dev
check "claude restarts at 1"          "0.1.4-claude.1-$today" "$shipped" patch --channel claude

echo
echo "== ordering and parsing edge cases =="
check "sorts numerically, not lexically" "0.1.11-$today" \
  $'v0.1.9-2026.aug.19\nv0.1.10-2026.aug.20' patch
check "double-digit counters" "0.1.4-dev11-$today" \
  $'v0.1.3-2026.aug.20\nv0.1.4-dev9-2026.aug.21\nv0.1.4-dev10-2026.aug.21' patch --channel dev
check "malformed tags ignored" "0.2.0-$today" \
  $'v0.1.9-2026.aug.19\nnightly\nv2-broken\nv0.1.999' minor
check "a pre-release tag is never read as production" "5.0.1-$today" \
  $'v0.1.0-pre4-2026.aug.19\nv0.1.0-dev9-2026.aug.19' patch --manifest "$(fixture 5.0.0)"
check "bad argument rejected" "ERROR(2): unknown argument '--nope'" "$prod" patch --nope
check "an uppercase channel is rejected" \
  "ERROR(2): channel 'Claude' is not a usable version identifier" "$prod" patch --channel Claude
check "a channel with a slash is rejected" \
  "ERROR(2): channel 'a/b' is not a usable version identifier" "$prod" patch --channel a/b

echo
echo "== release-plan.sh: which branch publishes what =="
plan "main pushed cuts a pre-release"  "publish=true prerelease=true channel=pre bump=patch" \
  --event push --branch main --actor t342guy
plan "master counts as main"           "channel=pre" --event push --branch master --actor t342guy
plan "dev pushed cuts a dev release"   "publish=true prerelease=true channel=dev bump=patch" \
  --event push --branch dev --actor t342guy
plan "Dev is matched case-insensitively" "channel=dev" --event push --branch Dev --actor t342guy
plan "a PR branch uses the PR author"  "publish=true prerelease=true channel=claude bump=patch" \
  --event push --branch feature/x --actor t342guy --pr-author claude
plan "a branch with no PR publishes nothing" "publish=false" \
  --event push --branch feature/x --actor t342guy
plan "a fork PR publishes nothing"     "publish=false" \
  --event pull_request --branch feature/x --actor outsider --pr-author outsider

echo
echo "== release-plan.sh: manual triggers =="
plan "dispatch on main cuts production" "publish=true prerelease=false channel= bump=minor" \
  --event workflow_dispatch --branch main --release-type production --bump minor
plan "a major production bump"          "bump=major" \
  --event workflow_dispatch --branch main --release-type production --bump major
plan "dispatch defaults to a patch"     "bump=patch" \
  --event workflow_dispatch --branch main --release-type production
plan "dispatch on main, pre-release"    "publish=true prerelease=true channel=pre" \
  --event workflow_dispatch --branch main --release-type prerelease
plan "dispatch on dev"                  "channel=dev" \
  --event workflow_dispatch --branch Dev --release-type prerelease
plan "dispatch on a feature branch uses the actor" "channel=t342guy" \
  --event workflow_dispatch --branch feature/x --actor T342guy --release-type prerelease
plan "production off main is refused"   "must be cut from main" \
  --event workflow_dispatch --branch dev --release-type production

echo
echo "== release-plan.sh: login sanitising =="
plan "uppercase is folded down"    "channel=t342guy" \
  --event push --branch f --actor x --pr-author T342guy
plan "a bot login is made safe"    "channel=dependabot-bot" \
  --event push --branch f --actor x --pr-author 'dependabot[bot]'
plan "an all-digit login gains a letter" "channel=u12345" \
  --event push --branch f --actor x --pr-author 12345
plan "an unusable login publishes nothing" "publish=false" \
  --event push --branch f --actor x --pr-author '---'

echo
echo "== every emitted version is valid semver for cargo =="
mkdir -p "$tmp/src"; echo 'fn main(){}' > "$tmp/src/main.rs"
for v in "0.1.3-$today" "0.1.3-pre1-$today" "0.1.3-dev11-$today" \
         "0.1.3-claude.1-$today" "0.1.3-some-user.9-$today" "0.1.3-u12345.1-$today"; do
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
