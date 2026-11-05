#!/bin/sh
# Fetches the WPT revision pinned in bench/wpt/wpt-revision.txt into DIR: one commit, no history, and only the paths
# bench/wpt/wpt-paths.txt lists (the test directories plus the support files they load). About 85 MB, ~15 s.
# Usage: bench/wpt/fetch-wpt.sh DIR        (WPT_REVISION=<sha> overrides the pin)
set -eu
here="$(cd "$(dirname "$0")" && pwd)"
dir="$1"
rev="${WPT_REVISION:-$(tr -d '[:space:]' < "$here/wpt-revision.txt")}"
# Sparse-checkout patterns, anchored at the WPT root; comments and blank lines dropped (a bare "/" would match all).
patterns() { grep -v -e '^#' -e '^[[:space:]]*$' "$here/wpt-paths.txt" | sed 's|^|/|'; }
if [ -d "$dir/.git" ] && [ "$(git -C "$dir" rev-parse HEAD 2>/dev/null)" = "$rev" ]; then
  patterns | git -C "$dir" sparse-checkout set --no-cone --stdin
  echo "WPT $rev already in $dir"
  exit 0
fi
rm -rf "$dir"
git init -q "$dir"
git -C "$dir" remote add origin https://github.com/web-platform-tests/wpt.git
patterns | git -C "$dir" sparse-checkout set --no-cone --stdin
# Blobless: the checkout then downloads only the files the sparse patterns select.
git -C "$dir" fetch -q --depth 1 --filter=blob:none origin "$rev"
git -C "$dir" -c advice.detachedHead=false checkout -q FETCH_HEAD
echo "WPT $rev in $dir ($(du -sh "$dir" | cut -f1))"
