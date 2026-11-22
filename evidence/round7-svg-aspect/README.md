# SVG preserveAspectRatio reflection

Across 38 frozen ordinary SVG testharness URLs, LP gained 102 named passes with zero losses. All 102 gained names also pass signed Chrome stable 158.0.8127.37. The baseline had 1,576 named passes and 29 fully passing pages; the candidate had 1,678 and 31. Chrome had 2,247 and 32. All URLs completed with OK harness status on each browser and the LP named inventory was unchanged. The 99 IDL gains include the reflected interfaces, constants and element brands. One name each improved on SameObject-identity, the direct SVGAnimatedPreserveAspectRatio behavior page, and the no-effect non-additive animation case. Across this selection 581 names still pass only Chrome and 12 only LP. [Source](https://github.com/povio/pyro/commit/52592f139eeebdf1d1163cf9bca47be546994662)

The original paired SVG IDL census recorded 664 Chrome-only names, 78 naming preserveAspectRatio. LP now reflects the value on svg, image, symbol, view, marker, pattern and feImage. Each element's animated wrapper and its two distinct component objects keep their observable identity. Base writes serialize through normal DOM attribute mutation; a content-attribute write or removal is observed immediately. AnimVal is read-only and reflects the base value when active animation is absent. This patch implements no SMIL sampling, and the no-effect WPT gain does not verify active animation support. viewBox reflection and other remaining SVG IDL families are outside the batch.

## Standards and measured browser differences

SVG 2 specifies that invalid base values throw TypeError; writes to a read-only value throw NoModificationAllowedError after Web IDL argument conversion. SVG 2 also specifies that meetOrSlice reflects an explicitly present meet-or-slice keyword. For the attribute `none slice`, the candidate therefore reports (none, slice) while mapping the axes independently. Signed Chrome instead reports (none, meet) when parsing the attribute and throws NotSupportedError for out-of-range base enum assignments. Both implementations preserve the attribute on rejected assignments and return the same visible geometry. [SVGPreserveAspectRatio algorithms](https://svgwg.org/svg2-draft/coords.html#InterfaceSVGPreserveAspectRatio)

The seven-receiver probe checks the element brands, live base/animated values, identity, parsing, attribute writes/removal, setter behavior and read-only rejection. In a 300×100 viewport, a 10×10 rectangle under a 100×100 viewBox becomes 30×10 with align=none. A supplemental fixture checks 13 assertions, including both setters' invalid values, unchanged attributes, read-only invalid writes, propagation of custom conversion exceptions before read-only rejection, and explicit `none slice` geometry. LP passes 13/13. Chrome passes 9/13; the four differing assertions are the two base/animated mode reads and the two invalid-base exception names. The native geometry unit verifies the full affine matrix [3, 0, 0, 1, -30, -20] for viewBox `10 20 100 100`. Both focused native tests passed.

## Provenance and reproduction

The exact baseline is bdd2f16099bd1580b3252595dff1e4aeb6b53e2b. The candidate is 52592f139eeebdf1d1163cf9bca47be546994662. Both use the CI JSC archive 0e0dd7038a7a-6f4649e395e0 (arm64), separately verified sources, and one-job wptiter builds. WPT is a421f2fea5eb7f91e0db399e44b0fb0779ee1603; manifest SHA-256 is 34a67512a84d43630f7adbecebd4378df5f410446591015cc6a20a3034e02819. All three engines ran at 800×600 DPR 1, one job, fresh contexts, wptserve, testdriver and HTTP/2 on macOS 27.2.1 arm64. No restarts, watchdog kills or missing URLs were recorded. The selection was frozen against the official ordinary inventory with existing holds excluded. The signed Chrome profile and executable hashes are in chrome-identity.json.

Independent source inspection identified the two SVG 2 divergences before compilation. The initial Chrome-matching draft 297e377af was never built. Its standards correction, 48d949ea, did not compile because the binding builder takes noncapturing callbacks; it produced no credited results. The registration fix 0d62d2e79 and subsequent test-only matrix guard form the measured source. Independent runtime QA reran the candidate WPT and fixture measurements and confirmed the 102 gains, all seven receiver identities, 13 normative assertions and nonsquare geometry. Raw implementation-owner measurements and independent receipt hashes are recorded separately.

```sh
bun bench/wpt/run-no-native-retry.mjs --wpt "$WPT" --kind testharness --dirs non-css --jobs 1 \
  --server wptserve --testdriver --http2 --generated-globals "$MANIFEST" --manifest-inventory \
  --measure-reviewed-skips --tests urls.json --bin "$LABEL=$BIN" --out "$OUT"
```

named.jsonl.gz records URL, original name, duplicate occurrence and the control, candidate and Chrome statuses. pages.json records the corresponding page outcome, harness status and fully-passing flag. Null means unobserved. Native test names and results are in native-results.txt. direct-probes.tar.gz contains the runnable ordinary JS controls and per-receiver raw results.

Final independent registration review confirmed both field-to-accessor mappings, identity handling and conversion/read-only/enum error ordering. The final commit adds only the geometry regression test.
