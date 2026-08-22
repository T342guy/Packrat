#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Prints the next release version.
#
#   ./scripts/next-version.sh patch                    ->  0.1.4-2026.aug.22
#   ./scripts/next-version.sh patch --channel pre      ->  0.1.4-pre1-2026.aug.22
#   ./scripts/next-version.sh patch --channel dev      ->  0.1.4-dev1-2026.aug.22
#   ./scripts/next-version.sh patch --channel claude   ->  0.1.4-claude.1-2026.aug.22
#
# The base number always comes from the last *production* tag, never from the
# last tag of any kind. Cutting pre-releases therefore does not advance what
# the next production release will be called: pre-releases of 0.1.4 can run
# from 1 to 99 on every channel at once and production is still 0.1.4, which
# simply drops the channel part.
#
# Counters are per channel, so `pre`, `dev` and each contributor's own run of
# pre-releases advance independently.
#
# `pre` and `dev` are joined straight onto their counter because neither can
# end in a digit. A username can — `user1` and counter 1 would read `user11`,
# which cannot be parsed back — so usernames are separated by a dot. That is
# the only reason the two forms differ.
#
# Days are not zero-padded and channels are lowercased: semver forbids leading
# zeros in numeric pre-release identifiers, and cargo rejects them outright.
#
# NEXT_VERSION_TAGS overrides the tag list, for testing.
#
# shellcheck shell=bash
set -euo pipefail

bump="patch"
channel=""
manifest="Cargo.toml"

while (($# > 0)); do
  case "$1" in
    patch|minor|major|none) bump="$1"; shift ;;
    --channel) channel="${2-}"; shift 2 ;;
    --manifest) manifest="${2:?--manifest needs a path}"; shift 2 ;;
    *) echo "unknown argument '$1'" >&2; exit 2 ;;
  esac
done

# The channel becomes part of a semver pre-release identifier, so it has to be
# a lowercase alphanumeric-or-hyphen word. Anything else is a bug upstream.
if [[ -n "$channel" && ! "$channel" =~ ^[a-z0-9]([a-z0-9-]*[a-z0-9])?$ ]]; then
  echo "channel '$channel' is not a usable version identifier" >&2
  exit 2
fi

tags="${NEXT_VERSION_TAGS-$(git tag -l 'v*' 2>/dev/null || true)}"

date_part='[0-9]{4}\.[a-z]{3}\.[0-9]{1,2}'
# vX.Y.Z-YYYY.mon.D — a production release, and nothing else.
production="^v([0-9]+\.[0-9]+\.[0-9]+)-${date_part}\$"

collect_production() {
  local tag
  while IFS= read -r tag; do
    [[ -n "$tag" ]] || continue
    if [[ "$tag" =~ $production ]]; then
      printf '%s\n' "${BASH_REMATCH[1]}"
    fi
  done <<< "$tags"
}

base="$(collect_production | sort -V | tail -1)"
if [[ -z "$base" ]]; then
  # Nothing released yet: fall back to whatever the manifest says, ignoring
  # any date or channel suffix already on it.
  base="$(grep -m1 '^version = ' "$manifest" | cut -d'"' -f2)"
  base="${base%%-*}"
fi

IFS=. read -r major minor patch <<< "$base"
case "$bump" in
  major) major=$((major + 1)); minor=0; patch=0 ;;
  minor) minor=$((minor + 1)); patch=0 ;;
  patch) patch=$((patch + 1)) ;;
  none)  ;;
esac
next="${major}.${minor}.${patch}"

stamp="$(date -u +%Y).$(date -u +%b | tr '[:upper:]' '[:lower:]').$(date -u +%-d)"

if [[ -z "$channel" ]]; then
  printf '%s-%s\n' "$next" "$stamp"
  exit 0
fi

# `pre` and `dev` are the two fixed channels; everything else is a username.
case "$channel" in
  pre|dev) separator="" ;;
  *)       separator="." ;;
esac

# Continue this channel's run of pre-releases for this exact base.
escaped="${channel//./\\.}"
pattern="^v([0-9]+\.[0-9]+\.[0-9]+)-${escaped}${separator:+\\.}([0-9]+)-${date_part}\$"

highest=0
while IFS= read -r tag; do
  [[ -n "$tag" ]] || continue
  if [[ "$tag" =~ $pattern ]] && [[ "${BASH_REMATCH[1]}" == "$next" ]]; then
    if ((BASH_REMATCH[2] > highest)); then
      highest="${BASH_REMATCH[2]}"
    fi
  fi
done <<< "$tags"

printf '%s-%s%s%s-%s\n' "$next" "$channel" "$separator" "$((highest + 1))" "$stamp"
