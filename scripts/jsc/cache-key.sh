#!/bin/sh
# Prints the directory name of the patched JavaScriptCore build for this checkout, as scripts/jsc/build.sh names it
# under ~/Library/Caches/lightningpanda/jsc: the WebKit revision (12 characters) and a hash of the patches.
# CI caches the build as patched-jsc-static-macos-arm64-<this> (.github/workflows/jsc.yml); wpt.yml looks it up by the same
# name. Keep it in step with build.sh's `out`.
set -eu
root="$(cd "$(dirname "$0")/../.." && pwd)"
inputs="$root/crates/lp-jsc-sys/webkit"
rev="$(tr -d '[:space:]' < "$inputs/REVISION")"
# The patches in name order (numbered, so the same in every locale).
patch_hash="$(cat "$inputs"/patches/*.patch | shasum -a 256 | cut -c1-12)"
echo "$(printf '%s' "$rev" | cut -c1-12)-$patch_hash"
