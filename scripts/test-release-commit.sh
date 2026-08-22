#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Tests for release-commit.sh against a real git remote in a temp directory.
#
# This exists because the bug it guards against shipped twice: the publish
# step pushed the tag but not the branch for every pre-release, so `dev`
# never advanced and its tags pointed at commits no branch could reach. That
# logic lived as shell inside the workflow, where nothing executed it until a
# real release ran. It lives here instead so it can be run.
#
# shellcheck shell=bash
set -uo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

pass=0; fail=0
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT

check() { # name  expected  actual
  if [[ "$3" == "$2" ]]; then
    pass=$((pass+1)); printf '  ok   %-52s %s\n' "$1" "$3"
  else
    fail=$((fail+1)); printf '  FAIL %-52s %s (wanted %s)\n' "$1" "$3" "$2"
  fi
}

# A throwaway project with a bare remote, so pushes are real pushes.
setup() { # branch -> prints the clone path
  local branch="$1" n="$2"
  local bare="$tmp/remote-$n.git" work="$tmp/work-$n"
  git init -q --bare "$bare"
  git init -q -b "$branch" "$work"
  mkdir -p "$work/src" "$work/scripts"
  printf 'fn main(){}\n' > "$work/src/main.rs"
  printf '[package]\nname = "packrat"\nversion = "0.1.4-2026.aug.22"\nedition = "2021"\n' \
    > "$work/Cargo.toml"
  cp "$repo_root/scripts/stamp-version.sh" "$repo_root/scripts/release-commit.sh" "$work/scripts/"
  (
    cd "$work" || exit 1
    git config user.email t@example.com; git config user.name Tester
    cargo generate-lockfile -q >/dev/null 2>&1
    git add -A; git commit -qm "base"
    git remote add origin "$bare"; git push -q origin "$branch"
  )
  printf '%s' "$work"
}

run() { # workdir args...
  local work="$1"; shift
  (cd "$work" && ./scripts/release-commit.sh "$@" 2>&1)
}

# --- the dev channel: the case that broke -------------------------------
echo "== dev channel: the bump must land on the branch =="
w="$(setup Dev 1)"
run "$w" --version 0.1.5-dev1-2026.aug.22 --tag v0.1.5-dev1-2026.aug.22 \
        --channel dev --branch Dev > /dev/null

check "the tag is reachable from the branch" "yes" \
  "$(cd "$w" && git merge-base --is-ancestor v0.1.5-dev1-2026.aug.22 origin/Dev 2>/dev/null && echo yes || echo ORPHAN)"
check "the remote branch carries the new version" "0.1.5-dev1-2026.aug.22" \
  "$(cd "$w" && git show origin/Dev:Cargo.toml | sed -n 's/^version = "\(.*\)"/\1/p')"
check "the lockfile moved with it" "0.1.5-dev1-2026.aug.22" \
  "$(cd "$w" && git show origin/Dev:Cargo.lock | grep -A2 'name = "packrat"' | sed -n 's/^version = "\(.*\)"/\1/p')"

# --- main's own pre-release channel -------------------------------------
echo
echo "== pre channel on main: same rule =="
w="$(setup main 2)"
run "$w" --version 0.1.5-pre1-2026.aug.22 --tag v0.1.5-pre1-2026.aug.22 \
        --channel pre --branch main > /dev/null
check "the tag is on the branch" "yes" \
  "$(cd "$w" && git merge-base --is-ancestor v0.1.5-pre1-2026.aug.22 origin/main 2>/dev/null && echo yes || echo ORPHAN)"
check "main reports the pre-release" "0.1.5-pre1-2026.aug.22" \
  "$(cd "$w" && git show origin/main:Cargo.toml | sed -n 's/^version = "\(.*\)"/\1/p')"

# --- production ----------------------------------------------------------
echo
echo "== production: unchanged behaviour =="
w="$(setup main 3)"
run "$w" --version 0.1.5-2026.aug.22 --tag v0.1.5-2026.aug.22 --channel "" --branch main > /dev/null
check "the tag is on the branch" "yes" \
  "$(cd "$w" && git merge-base --is-ancestor v0.1.5-2026.aug.22 origin/main 2>/dev/null && echo yes || echo ORPHAN)"
check "main reports the release" "0.1.5-2026.aug.22" \
  "$(cd "$w" && git show origin/main:Cargo.toml | sed -n 's/^version = "\(.*\)"/\1/p')"

# --- a contributor's branch ----------------------------------------------
echo
echo "== a contributor's channel: tag it, do not touch their branch =="
w="$(setup feature 4)"
before="$(cd "$w" && git rev-parse origin/feature)"
run "$w" --version 0.1.5-claude.1-2026.aug.22 --tag v0.1.5-claude.1-2026.aug.22 \
        --channel claude --branch feature > /dev/null
check "their branch is untouched" "$before" "$(cd "$w" && git rev-parse origin/feature)"
check "but the tag still exists on the remote" "v0.1.5-claude.1-2026.aug.22" \
  "$(cd "$w" && git ls-remote --tags origin | sed -n 's|.*refs/tags/\(v0.1.5-claude.*\)$|\1|p' | head -1)"
check "and it is NOT an orphan" "yes" \
  "$(cd "$w" && git merge-base --is-ancestor v0.1.5-claude.1-2026.aug.22 origin/feature 2>/dev/null && echo yes || echo ORPHAN)"

# --- the branch moving mid-build -----------------------------------------
echo
echo "== the branch moving while the build ran =="
w="$(setup Dev 5)"
# --branch matters: the bare repo's HEAD still points at `master`, so a plain
# clone lands on an empty branch and the "concurrent" commit goes nowhere.
other="$tmp/other"; git clone -q --branch Dev "$tmp/remote-5.git" "$other"
(
  cd "$other" || exit 1; git config user.email t@example.com; git config user.name Tester
  echo "// someone else" >> src/main.rs; git add -A; git commit -qm "concurrent work"
  git push -q origin Dev
)
out="$(run "$w" --version 0.1.5-dev2-2026.aug.22 --tag v0.1.5-dev2-2026.aug.22 \
        --channel dev --branch Dev)"
check "it replays rather than forcing" "yes" \
  "$(grep -q 'replaying' <<< "$out" && echo yes || echo no)"
check "the other commit survived" "yes" \
  "$(cd "$w" && git fetch -q origin && git log origin/Dev --oneline | grep -q 'concurrent work' && echo yes || echo LOST)"
check "and the tag is still on the branch" "yes" \
  "$(cd "$w" && git merge-base --is-ancestor v0.1.5-dev2-2026.aug.22 origin/Dev 2>/dev/null && echo yes || echo ORPHAN)"

# --- re-stamping an already-stamped version ------------------------------
echo
echo "== a version already stamped is not an error =="
w="$(setup Dev 6)"
(cd "$w" && ./scripts/stamp-version.sh 0.1.5-dev1-2026.aug.22 >/dev/null 2>&1 \
   && git commit -aqm "pre-stamped")
out="$(run "$w" --version 0.1.5-dev1-2026.aug.22 --tag v0.1.5-dev1-2026.aug.22 \
        --channel dev --branch Dev)"
check "it says so and carries on" "yes" \
  "$(grep -q 'already stamped' <<< "$out" && echo yes || echo no)"
check "the tag still lands on the branch" "yes" \
  "$(cd "$w" && git merge-base --is-ancestor v0.1.5-dev1-2026.aug.22 origin/Dev 2>/dev/null && echo yes || echo ORPHAN)"

echo
echo "passed $pass, failed $fail"
(( fail == 0 ))
