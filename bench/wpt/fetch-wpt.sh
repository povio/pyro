#!/bin/sh
# Fetches the WPT revision pinned in bench/wpt/wpt-revision.txt into DIR: one commit, no history, and only the paths
# bench/wpt/wpt-paths.txt lists (the test directories plus the support files they load). About 290 MB with its git directory, under a minute.
# Usage: bench/wpt/fetch-wpt.sh [--force] [--all] DIR   (WPT_REVISION=<sha> overrides the pin, WPT_REMOTE=<url> the repo)
# --all checks out the whole revision (every test directory: the full-suite run and coverage.mjs; about 1.5 GB).
# DIR is new or empty, or a checkout this script made (it moves that one to the pinned revision in place; git refuses
# if local changes would be lost). A git checkout it did not make is used as it is when it is at the pinned
# revision, and refused otherwise; --force deletes DIR first.
set -eu
here="$(cd "$(dirname "$0")" && pwd)"
force=
all=
while :; do
  case "${1:-}" in
    --force) force=1; shift ;;
    --all) all=1; shift ;;
    *) break ;;
  esac
done
dir="${1:?usage: fetch-wpt.sh [--force] [--all] DIR}"
rev="${WPT_REVISION:-$(tr -d '[:space:]' < "$here/wpt-revision.txt")}"
marker="$dir/.git/pyro-fetch-wpt"
# Sparse-checkout patterns, anchored at the WPT root; comments and blank lines dropped (a bare "/" would match all).
patterns() { grep -v -e '^#' -e '^[[:space:]]*$' "$here/wpt-paths.txt" | sed 's|^|/|'; }
head_of() { git -C "$dir" rev-parse HEAD 2>/dev/null || true; }
if [ -n "$force" ]; then rm -rf "$dir"; fi
if [ -e "$dir" ] && [ ! -f "$marker" ] && [ -n "$(ls -A "$dir")" ]; then
  if [ -d "$dir/.git" ] && [ "$(head_of)" = "$rev" ]; then
    echo "WPT $rev already in $dir (not made by fetch-wpt.sh: left as it is)"
    exit 0
  fi
  echo "fetch-wpt.sh: $dir exists and was not made by this script; give a new or empty directory, or --force to replace it" >&2
  exit 1
fi
if [ ! -f "$marker" ]; then
  git init -q "$dir"
  git -C "$dir" remote add origin "${WPT_REMOTE:-https://github.com/web-platform-tests/wpt.git}"
  echo "made by bench/wpt/fetch-wpt.sh" > "$marker"
fi
if [ -n "$all" ]; then
  git -C "$dir" sparse-checkout disable
else
  patterns | git -C "$dir" sparse-checkout set --no-cone --stdin
fi
if [ "$(head_of)" = "$rev" ]; then
  echo "WPT $rev already in $dir"
  exit 0
fi
# Blobless: the checkout then downloads only the files the sparse patterns select.
git -C "$dir" fetch -q --depth 1 --filter=blob:none origin "$rev"
git -C "$dir" -c advice.detachedHead=false checkout -q FETCH_HEAD
echo "WPT $rev in $dir ($(du -sh "$dir" | cut -f1))"
