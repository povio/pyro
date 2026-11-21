#!/bin/sh
set -eu
p=$(CDPATH= cd -- "$(dirname "$0")" && pwd)
: "${RUNNER_ROOT:?}" "${WPT_CHECKOUT:?}" "${WPT_MANIFEST:?}" "${LP_BASE:?}" "${LP_EXPERIMENT:?}" "${LP_FEATURE_OFF:?}" "${CHROME_LAUNCHER:?}"
test "$(git -C "$WPT_CHECKOUT" rev-parse HEAD)" = a421f2fea5eb7f91e0db399e44b0fb0779ee1603
mkdir -p "$p/replayed"
cd "$RUNNER_ROOT"
for who in chrome base feature off; do
 case "$who" in chrome) bin=$CHROME_LAUNCHER;; base) bin=$LP_BASE;; feature) bin=$LP_EXPERIMENT;; off) bin=$LP_FEATURE_OFF;; esac
 for corpus in urls guard; do
  if [ "$corpus" = guard ] && { [ "$who" = chrome ] || [ "$who" = off ]; }; then continue; fi
  bun bench/wpt/run-no-native-retry.mjs --kind testharness --wpt "$WPT_CHECKOUT" --server wptserve --testdriver --http2 --generated-globals "$WPT_MANIFEST" --manifest-inventory --measure-reviewed-skips --dirs WebCryptoAPI --jobs 4 --tests "$p/$corpus.json" --bin "$who=$bin" --out "$p/replayed/$corpus-$who.json"
 done
done
ln -snf "$WPT_CHECKOUT/resources" "$p/probes/resources"
ln -snf "$WPT_CHECKOUT/fonts" "$p/probes/fonts"
ln -snf "$WPT_CHECKOUT/WebCryptoAPI/util/helpers.js" "$p/probes/cases/upstream-helpers.js"
ln -snf "$WPT_CHECKOUT/WebCryptoAPI/modern-algos/encap_decap/ml_kem_vectors.js" "$p/probes/cases/upstream-ml-kem-vectors.js"
for who in base feature; do
 case $who in base)bin=$LP_BASE;; feature)bin=$LP_EXPERIMENT;; esac
 bun bench/wpt/run-no-native-retry.mjs --kind testharness --wpt "$p/probes" --dirs cases --jobs 1 --bin "$who=$bin" --out "$p/replayed/probes-$who.json"
done
