#!/bin/sh
# Set WPT_CHECKOUT, WPT_MANIFEST, RUNNER_ROOT, CHROME_LAUNCHER (acceptance
# profile), LP_BASE and LP_EXPERIMENT. The runner needs Playwright Core.
set -eu
E=$(CDPATH= cd -- "$(dirname "$0")" && pwd)
: "${WPT_CHECKOUT:?}" "${WPT_MANIFEST:?}" "${RUNNER_ROOT:?}" "${CHROME_LAUNCHER:?}" "${LP_BASE:?}" "${LP_EXPERIMENT:?}"
test "$(git -C "$WPT_CHECKOUT" rev-parse HEAD)" = a421f2fea5eb7f91e0db399e44b0fb0779ee1603
mkdir -p "$E/replayed"; cd "$RUNNER_ROOT"
for label in chrome base experiment; do
 case "$label" in chrome) bin="$CHROME_LAUNCHER";; base) bin="$LP_BASE";; experiment) bin="$LP_EXPERIMENT";; esac
 bun bench/wpt/run-no-native-retry.mjs --kind testharness --wpt "$WPT_CHECKOUT" --server wptserve --testdriver --http2 --generated-globals "$WPT_MANIFEST" --manifest-inventory --measure-reviewed-skips --dirs webnn --jobs 4 --tests "$E/urls.json" --bin "$label=$bin" --out "$E/replayed/$label.json"
done
ln -snf "$WPT_CHECKOUT/resources" "$E/probes/resources"
ln -snf "$WPT_CHECKOUT/fonts" "$E/probes/fonts"
ln -snf "$WPT_CHECKOUT/webnn/resources/utils.js" "$E/probes/cases/original-utils.js"
bun "$E/source/extract-vectors.cjs"
for label in base experiment; do
 case "$label" in base) bin="$LP_BASE";; experiment) bin="$LP_EXPERIMENT";; esac
 bun bench/wpt/run-no-native-retry.mjs --wpt "$E/probes" --dirs cases --jobs 1 --bin "$label=$bin" --out "$E/replayed/probes-$label.json"
done
