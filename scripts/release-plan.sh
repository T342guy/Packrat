#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Decides what a pipeline run should publish, if anything.
#
# Prints shell-ish KEY=VALUE lines, which the workflow appends to $GITHUB_OUTPUT:
#
#   publish=true|false     whether anything is tagged and released at all
#   prerelease=true|false  whether that release is marked as a pre-release
#   channel=<id>           the pre-release channel, empty for production
#   bump=patch|minor|major|none
#
# The rules:
#
#   main, pushed        pre-release on the `pre` channel
#   main, dispatched    production release, with the bump you choose
#   dev, pushed         pre-release on the `dev` channel
#   any other branch    pre-release on the pull request author's channel, but
#                       only while an open pull request exists for it
#   a fork's PR         nothing published — untrusted code never gets a tag
#
# Production is deliberately manual-only: it is the one decision that needs a
# human to say whether it is a patch, a minor or a major.
#
# shellcheck shell=bash
set -euo pipefail

event=""; branch=""; actor=""; pr_author=""; release_type=""; bump=""

while (($# > 0)); do
  case "$1" in
    --event)        event="${2-}"; shift 2 ;;
    --branch)       branch="${2-}"; shift 2 ;;
    --actor)        actor="${2-}"; shift 2 ;;
    --pr-author)    pr_author="${2-}"; shift 2 ;;
    --release-type) release_type="${2-}"; shift 2 ;;
    --bump)         bump="${2-}"; shift 2 ;;
    *) echo "unknown argument '$1'" >&2; exit 2 ;;
  esac
done

[[ -n "$event"  ]] || { echo "--event is required" >&2; exit 2; }
[[ -n "$branch" ]] || { echo "--branch is required" >&2; exit 2; }

# A login has to survive as a semver pre-release identifier: lowercase, and
# only alphanumerics and hyphens. `dependabot[bot]` becomes `dependabot-bot`.
sanitise() {
  local raw="${1,,}"
  raw="${raw//[^a-z0-9-]/-}"
  # Collapse and trim the hyphens that substitution leaves behind.
  while [[ "$raw" == *--* ]]; do raw="${raw//--/-}"; done
  raw="${raw#-}"; raw="${raw%-}"
  # An all-digit identifier is numeric to semver, where a leading zero is
  # forbidden outright. A letter in front sidesteps the whole question.
  if [[ "$raw" =~ ^[0-9]+$ ]]; then raw="u${raw}"; fi
  printf '%s' "$raw"
}

# Branch names are compared case-insensitively: the branch may be `dev` or
# `Dev`, and neither spelling should silently fall through to "no release".
lower_branch="${branch,,}"

channel_for_branch() {
  case "$lower_branch" in
    main|master) printf 'pre' ;;
    dev|develop|development) printf 'dev' ;;
    *) sanitise "${pr_author:-$actor}" ;;
  esac
}

emit() {
  printf 'publish=%s\nprerelease=%s\nchannel=%s\nbump=%s\n' "$1" "$2" "$3" "$4"
}

case "$event" in
  workflow_dispatch)
    [[ -n "$bump" ]] || bump="patch"
    if [[ "$release_type" == "production" ]]; then
      # Production only ever comes off the release branch. Cutting one from a
      # feature branch would tag code that main has never seen.
      if [[ "$lower_branch" != "main" && "$lower_branch" != "master" ]]; then
        echo "a production release must be cut from main, not '$branch'" >&2
        exit 3
      fi
      emit true false "" "$bump"
    else
      channel="$(channel_for_branch)"
      if [[ -z "$channel" ]]; then
        echo "cannot work out a pre-release channel for '$branch'" >&2
        exit 3
      fi
      emit true true "$channel" "$bump"
    fi
    ;;

  push)
    # Pushes never cut production; they only ever advance a pre-release
    # channel. The bump is assumed to be a patch, because nothing here knows
    # what the next production release will be called.
    case "$lower_branch" in
      main|master|dev|develop|development)
        emit true true "$(channel_for_branch)" patch
        ;;
      *)
        # "A pull request branch" is exactly that: a branch with an open pull
        # request. Without one there is no author to name the channel after,
        # and a scratch branch should not be minting public releases.
        if [[ -n "$pr_author" ]]; then
          channel="$(sanitise "$pr_author")"
          if [[ -n "$channel" ]]; then
            emit true true "$channel" patch
          else
            emit false false "" none
          fi
        else
          emit false false "" none
        fi
        ;;
    esac
    ;;

  pull_request)
    # Only fork pull requests reach here; same-repo ones are covered by their
    # branch's own push run. Fork code is untrusted, so it is built and
    # checked but never tagged, released or pushed to a registry.
    emit false false "" none
    ;;

  *)
    echo "unknown event '$event'" >&2
    exit 2
    ;;
esac
