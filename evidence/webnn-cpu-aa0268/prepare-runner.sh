#!/bin/sh
set -eu
E=$(CDPATH= cd -- "$(dirname "$0")" && pwd)
: "${SOURCE_REPO:?}" "${RUNNER_ROOT:?}" "${CHROME_APP:?}"
test ! -e "$RUNNER_ROOT/bench" || { echo 'Use an empty RUNNER_ROOT' >&2; exit 1; }
mkdir -p "$RUNNER_ROOT"
git -C "$SOURCE_REPO" archive 81758fc77afe80f22ac0af3b0cecbf89c706074f bench/wpt bench/package.json | tar -x -C "$RUNNER_ROOT"
cd "$RUNNER_ROOT"
cp bench/wpt/run.mjs bench/wpt/run-no-native-retry.mjs
patch -p1 < "$E/source/runner.patch"
test "$(shasum -a256 bench/wpt/run-no-native-retry.mjs | cut -d' ' -f1)" = b82c52329f881a5193e21ceb22634517c4cb776bd39bb080692c24eb21ac594a
mkdir -p census
cp "$E/source/launch-158" census/launch-158
ln -s "$CHROME_APP" 'Google Chrome.app'
