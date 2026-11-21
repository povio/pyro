# Polygon and polyline live point lists

SVGPolygonElement and SVGPolylineElement expose separate stable points and animatedPoints lists. A detached item can be adopted intact. Supplying a point already attached to a list inserts a copy. Removed items become independently writable. A retained point follows its item across insert/remove operations. When script changes the points attribute, surviving items keep their identity and acquire the new coordinate at their index; truncated items retain their prior coordinates. Point list writes update the ordinary attribute and therefore path length and bounding boxes. Animated views mirror base coordinates with read-only setters. The engine's SMIL subsystem currently animates CSS properties and does not sample SMIL animations of the non-CSS points attribute.

| Matched ordinary SVG URLs | Named passes | Fully passing pages |
| --- | ---: | ---: |
| LP 01be7f2bc | 13,009 | 312 |
| LP 36d7b4c18 | 13,092 | 316 |
| Local Chrome stable 158.0.8127.37 | 14,799 | 672 |

All 787 selected URLs were observed on each browser. The candidate gained 83 named passes and lost 0. Among the gains, 68 also pass on signed Chrome 158, and 15 do not. LP had 58 timed-out pages and 9 harness errors. Chrome had 1 timed-out pages. Named rows retain their original text and occurrence number; missing results are null. The page and named tables give control, candidate, then Chrome.

The polygon/polyline test geometry and list mutation probe also ran directly on the signed reference. Chrome 158 returns distinct wrappers for repeated accesses to one item. Several probe fields therefore diverge from the SVG2 list algorithms intentionally implemented here, including the identity of getItem and append/insert/replace results. The raw comparisons retain both results. The algorithm for initialize removes/detaches existing entries before checking the input; when the supplied item was in this same list, it can be reattached intact. [SVG2 list interfaces](https://svgwg.org/svg2-draft/types.html#ListInterfaces)

The DOM stores weak observers for elements whose lists are live. A point wrapper holds its attached list and the list holds the element writer; a slot only weakly references a point wrapper. There is no strong reference from that DOM registry back to the list. A native regression retains references across an unobserved shrink/regrow, checks mutation after detachment and checks that the last attachment releases its writer. Other SVG number, length and transform list interfaces remain separate.

The original stable reference comprises matching complete shards of the exact-158 corpus. reference-identity.json and provenance.json identify the signed binary, checksum, original receipts, runner, pinned WPT manifest and both LP binaries. WPT used the same manifest, four jobs and no native retry. The CSS hit-test supplement uses two jobs.

```sh
bun bench/wpt/run-no-native-retry.mjs --kind testharness --wpt "$WPT" \
  --server wptserve --testdriver --http2 --generated-globals "$MANIFEST" \
  --manifest-inventory --measure-reviewed-skips --dirs non-css --jobs 4 \
  --tests selection.json --bin "lp=$BIN" --out "$OUT"
node probe.cjs normal-lp "$BIN"
CHROME_BIN="$CHROME" node probe.cjs normal-chrome chrome
```

The native ownership/synchronization regression passed. All 31 supplemental CSS hit-test pages preserved 98 named statuses.
