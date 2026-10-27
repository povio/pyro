#!/usr/bin/env bash
# Packs the E2E test client (packages/lightningpanda-test) for a release.
#
#   scripts/pack-test-client.sh OUT_DIR [ARCHIVE_DIR]
#
# Writes OUT_DIR/lightningpanda-test-<version>.tgz, versioned from Cargo.toml, to
# attach to the GitHub release (`bun add <its download URL>`). With ARCHIVE_DIR,
# the directory a release archive is made from (the one holding the
# `lightningpanda` binary), also unpacks it there as `lightningpanda-test/`,
# where `Lightningpanda.launch()` finds the binary beside it.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
out="$(mkdir -p "${1:?usage: $0 OUT_DIR [ARCHIVE_DIR]}" && cd "$1" && pwd)"
archive="${2:-}"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)"
pkg="$root/packages/lightningpanda-test"

cd "$pkg"
bun install --frozen-lockfile
npm version --no-git-tag-version --allow-same-version "$version" >/dev/null
rm -rf dist
bun run build
tgz="$out/$(npm pack --silent --pack-destination "$out")"
echo "$tgz"

if [ -n "$archive" ]; then
  rm -rf "$archive/lightningpanda-test"
  mkdir -p "$archive/lightningpanda-test"
  tar xzf "$tgz" -C "$archive/lightningpanda-test" --strip-components=1
  echo "$archive/lightningpanda-test"
fi
