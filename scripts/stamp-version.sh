#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Writes a version into Cargo.toml and keeps Cargo.lock in step, so a
# --locked build still works afterwards.
#
#   ./scripts/stamp-version.sh 0.1.1-2026.aug.21
#
# Idempotent: stamping a version that is already there does nothing, which is
# what happens on a real release, where the tag already carries the bump.
#
# shellcheck shell=bash
set -euo pipefail

version="${1:?usage: stamp-version.sh <version> [manifest]}"
manifest="${2:-Cargo.toml}"

current="$(grep -m1 '^version = ' "$manifest" | cut -d'"' -f2)"
if [[ "$current" == "$version" ]]; then
  echo "already at $version"
  exit 0
fi

# GNU sed; these run on Linux runners.
sed -i "0,/^version = .*/s//version = \"$version\"/" "$manifest"

# Rewrites the packrat entry in Cargo.lock.
cargo metadata --format-version 1 > /dev/null

locked="$(grep -A1 'name = "packrat"' Cargo.lock | grep '^version' | cut -d'"' -f2)"
if [[ "$locked" != "$version" ]]; then
  echo "Cargo.lock still says $locked, expected $version" >&2
  exit 1
fi

echo "stamped $version"
