# Live SVG string list reflection

The SVGGraphicsElement implementation now exposes requiredExtensions and systemLanguage as distinct stable SVGStringList objects. Both support indexed access and assignment, iteration, insertion, replacement, initialization, removal and clearing. The DOM arena retains string items, including separator characters inside values assigned through the list. An external write to the reflected attribute reparses even identical serialized text. Updates to namespaced attributes of the same local name leave the null-namespace list alone. Slot reclamation releases both lists.

| Matched ordinary SVG corpus | Named passes | Fully passing pages |
| --- | ---: | ---: |
| LP cbdeb1193 | 12,922 | 311 |
| LP 01be7f2bc | 13,009 | 312 |
| Google Chrome stable 158.0.8127.37 | 14,799 | 672 |

On these 787 URL variants the candidate gained 87 named passes and lost 0. The full page-status breakdown, original names and one-based occurrence indices are preserved. Null means an unreported observation. All selected historical LP and Chrome shard receipts had zero native restarts and watchdog kills. Both fresh LP controls retained the same WPT revision, manifest, CI JSC and four-job configuration. The corrected reference uses the exact signed macOS Chrome 158.0.8127.37 binary. reference-identity.json and provenance.json identify the corresponding binary, framework, DMG and per-shard raw receipts.

The normal probe covers tokens with commas and spaces, positional mutations, mutation through bracket assignment, missing and explicit empty attributes, external attribute writes, a namespaced shadow attribute, clone state, normal iteration, required-argument exceptions, conversion order, and unsigned long coercion outside the signed 64-bit range. probe-differences.json compares the full output to the separate live Chrome run. A native regression exercises separator retention followed by a same-string rewrite.

SVGAnimationElement, including its separate SVGTests mixin, remains an unimplemented interface. Object-valued number, point, length and transform list APIs retain separate gaps. The current Web API DOMString conversion uses Rust UTF-8 and replaces isolated UTF-16 surrogate code units, including string-list arguments.

The guard used the prior exact-158 runner with native process retries disabled. Set WPT, MANIFEST, BIN and OUT to the pinned checkout, manifest, browser and report destination.

```sh
bun bench/wpt/run-no-native-retry.mjs --kind testharness --wpt "$WPT" \
  --server wptserve --testdriver --http2 --generated-globals "$MANIFEST" \
  --manifest-inventory --measure-reviewed-skips --dirs non-css --jobs 4 \
  --tests selection.json --bin "lp=$BIN" --out "$OUT"
node probe.cjs normal-lp "$BIN"
CHROME_BIN="$CHROME" node probe.cjs normal-chrome chrome
```

The new DOM regression and all 32 existing native SVG integration tests passed. The 72 signed-Chrome probe fields matched the candidate.
