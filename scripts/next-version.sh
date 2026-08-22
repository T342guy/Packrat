#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Prints the next release version.
#
#   ./scripts/next-version.sh patch                 ->  0.1.3-2026.aug.21
#   ./scripts/next-version.sh patch --prerelease    ->  0.1.3-pre1-2026.aug.21
#
# The base number is derived from the last *production* tag, never from the
# last tag of any kind. Cutting pre-releases therefore does not advance what
# the next production release will be called: pre-releases of 0.1.3 can run
# from pre1 to pre9 and production is still 0.1.3.
#
# The day is not zero-padded: semver forbids leading zeros in numeric
# pre-release identifiers, and cargo rejects them outright.
#
# NEXT_VERSION_TAGS overrides the tag list, for testing.
#
# shellcheck shell=bash
set -euo pipefail

bump="patch"
prerelease=0
manifest="Cargo.toml"

while (($# > 0)); do
  case "$1" in
    patch|minor|major|none) bump="$1"; shift ;;
    --prerelease|-p) prerelease=1; shift ;;
    --manifest) manifest="${2:?--manifest needs a path}"; shift 2 ;;
    *) echo "unknown argument '$1'" >&2; exit 2 ;;
  esac
done

tags="${NEXT_VERSION_TAGS-$(git tag -l 'v*' 2>/dev/null || true)}"

# vX.Y.Z-YYYY.mon.D — a production release.
production='^v([0-9]+\.[0-9]+\.[0-9]+)-[0-9]{4}\.[a-z]{3}\.[0-9]{1,2}$'
# vX.Y.Z-preN-YYYY.mon.D — a pre-release of X.Y.Z.
prerelease_tag='^v([0-9]+\.[0-9]+\.[0-9]+)-pre([0-9]+)-[0-9]{4}\.[a-z]{3}\.[0-9]{1,2}$'

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
  # any date or pre-release suffix already on it.
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

if ((prerelease)); then
  # Continue the run of pre-releases already cut for this exact base.
  highest=0
  while IFS= read -r tag; do
    [[ -n "$tag" ]] || continue
    if [[ "$tag" =~ $prerelease_tag ]] && [[ "${BASH_REMATCH[1]}" == "$next" ]]; then
      if ((BASH_REMATCH[2] > highest)); then
        highest="${BASH_REMATCH[2]}"
      fi
    fi
  done <<< "$tags"
  printf '%s-pre%s-%s\n' "$next" "$((highest + 1))" "$stamp"
else
  printf '%s-%s\n' "$next" "$stamp"
fi
