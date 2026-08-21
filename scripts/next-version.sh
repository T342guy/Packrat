#!/usr/bin/env bash
# Prints the next release version, given the version in Cargo.toml and a bump.
#
#   ./scripts/next-version.sh patch   ->  0.1.1-2026.aug.21
#
# The date suffix is a semver pre-release, so a build is identifiable at a
# glance. Note the day is not zero-padded: semver forbids leading zeros in
# numeric pre-release identifiers and cargo rejects them outright.
#
# shellcheck shell=bash
set -euo pipefail

bump="${1:-patch}"
manifest="${2:-Cargo.toml}"

current="$(grep -m1 '^version = ' "$manifest" | cut -d'"' -f2)"
[[ -n "$current" ]] || { echo "no version found in $manifest" >&2; exit 1; }

# Drop any existing date suffix before bumping.
base="${current%%-*}"
IFS=. read -r major minor patch <<< "$base"

case "$bump" in
  major) major=$((major + 1)); minor=0; patch=0 ;;
  minor) minor=$((minor + 1)); patch=0 ;;
  patch) patch=$((patch + 1)) ;;
  none)  ;;
  *) echo "unknown bump '$bump' (want major, minor, patch or none)" >&2; exit 2 ;;
esac

year="$(date -u +%Y)"
month="$(date -u +%b | tr '[:upper:]' '[:lower:]')"
day="$(date -u +%-d)"

printf '%s.%s.%s-%s.%s.%s\n' "$major" "$minor" "$patch" "$year" "$month" "$day"
