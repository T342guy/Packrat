#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Stamps the version, commits it, tags it, and pushes — deciding per channel
# whether the commit belongs on the branch.
#
#   release-commit.sh --version 0.1.5-dev1-2026.aug.22 --tag v0.1.5-dev1-... \
#                     --channel dev --branch Dev
#
# Where the commit goes:
#
#   production, `pre`, `dev`   commit onto the branch, then tag it
#   a contributor's channel    tag the commit as it is, create nothing
#
# The split is about whose branch it is. main and dev are ours and their
# release cadence is the pipeline's business, so the version they report
# should match the release just cut from them. A pull request branch belongs
# to whoever opened it: pushing a commit into it mid-review would rebase work
# under someone's feet, so those releases tag the commit that was built and
# leave the branch alone. Either way nothing ends up on a commit that no
# branch can reach.
#
# Artifacts do not depend on any of this — every build job stamps the version
# into its own checkout — so a channel that skips the commit still ships
# binaries reporting the right version.
#
# shellcheck shell=bash
set -euo pipefail

version=""; tag=""; channel=""; branch=""; remote="origin"; push=1

while (($# > 0)); do
  case "$1" in
    --version) version="${2:?--version needs a value}"; shift 2 ;;
    --tag)     tag="${2:?--tag needs a value}"; shift 2 ;;
    --channel) channel="${2-}"; shift 2 ;;
    --branch)  branch="${2:?--branch needs a value}"; shift 2 ;;
    --remote)  remote="${2:?--remote needs a value}"; shift 2 ;;
    --no-push) push=0; shift ;;
    *) echo "unknown argument '$1'" >&2; exit 2 ;;
  esac
done

[[ -n "$version" ]] || { echo "--version is required" >&2; exit 2; }
[[ -n "$tag"     ]] || { echo "--tag is required" >&2; exit 2; }
[[ -n "$branch"  ]] || { echo "--branch is required" >&2; exit 2; }

# Channels whose branch the pipeline is allowed to commit to. Empty means a
# production release, which always lands on its branch.
commits_to_branch=0
case "$channel" in
  ""|pre|dev) commits_to_branch=1 ;;
  *)          commits_to_branch=0 ;;
esac

git config user.name  >/dev/null 2>&1 || git config user.name "github-actions[bot]"
git config user.email >/dev/null 2>&1 ||
  git config user.email "41898282+github-actions[bot]@users.noreply.github.com"

if ((commits_to_branch)); then
  scripts/stamp-version.sh "$version"
  git add Cargo.toml Cargo.lock
  # Re-running a version that is already stamped should not fail the release.
  if git diff --cached --quiet; then
    echo "version $version was already stamped; tagging without a new commit"
  else
    git commit -q -m "Release $version [skip ci]"
  fi
else
  echo "channel '${channel}' is someone else's branch — tagging the built commit as is"
fi

git tag -a "$tag" -m "Packrat $version"

if ((!push)); then
  if ((commits_to_branch)); then
    echo "would push: branch $branch and tag $tag"
  else
    echo "would push: tag $tag only"
  fi
  exit 0
fi

if ((!commits_to_branch)); then
  git push "$remote" "$tag"
  exit 0
fi

# Push the branch and the tag together so a reader never finds one without the
# other. If the branch moved while the build ran, replay the stamp on top of
# where it got to rather than forcing anything.
if ! git push "$remote" "HEAD:refs/heads/${branch}" "$tag" 2>/dev/null; then
  echo "branch moved while this build ran; replaying the version bump on the new tip" >&2
  git fetch "$remote" "$branch"
  git tag -d "$tag"
  git rebase "${remote}/${branch}"
  git tag -a "$tag" -m "Packrat $version"
  git push "$remote" "HEAD:refs/heads/${branch}" "$tag"
fi
