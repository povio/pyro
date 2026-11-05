#!/usr/bin/env bash
# Packs the E2E test client (packages/lightning-panda) for a release.
#
#   scripts/pack-test-client.sh OUT_DIR [ARCHIVE_DIR]
#
# Writes OUT_DIR/povio-lightning-panda-<version>.tgz to
# attach to the GitHub release (`bun add <its download URL>`). With ARCHIVE_DIR,
# the directory a release archive is made from (the one holding the
# `lightningpanda` binary), also unpacks it there as `lightning-panda/`,
# where `Lightningpanda.launch()` finds its bundled binary. Both native builds
# must already be staged; see docs/NPM_PUBLISHING.md.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
out="$(mkdir -p "${1:?usage: $0 OUT_DIR [ARCHIVE_DIR]}" && cd "$1" && pwd)"
archive="${2:-}"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)"
pkg="$root/packages/lightning-panda"

cd "$pkg"
bun install --frozen-lockfile
node "$root/scripts/npm/release.mjs" "v$version"
# npm pack runs prepack: builds JS/types, checks binaries, copies notices.
npm pack --silent --pack-destination "$out"
# Lifecycle scripts can write to stdout even with --silent; do not treat their
# build output as part of npm's filename.
tgz="$out/povio-lightning-panda-$version.tgz"
echo "$tgz"

if [ -n "$archive" ]; then
  rm -rf "$archive/lightning-panda"
  mkdir -p "$archive/lightning-panda"
  tar xzf "$tgz" -C "$archive/lightning-panda" --strip-components=1
  echo "$archive/lightning-panda"
fi
