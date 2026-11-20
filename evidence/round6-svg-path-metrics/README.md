# SVG geometry path metrics

The SVG DOM now computes path length and points at a distance in element-local user units. The shared path builder uses Stylo's SVG2 grammar, preserves valid segments before the first parse error, and handles subpaths and curved segments. Geometry attributes, authored CSS geometry, viewport-relative and common absolute lengths are resolved when measuring. Points are mutable, detached SVGPoint objects. Shape-specific probes cover asymmetric rounded rectangles, tall ellipses, scoped CSS path geometry, degenerate shapes and a CSS-sized viewport. A 50%-wide SVG in a 200px HTML containing block has a 100px content box and a 50-user-unit half-width line in both engines. The JSC archive, WPT revision, selection and binaries are identified in inputs.json.

| Same ordinary SVG URLs | Named passes | Fully passing pages |
| --- | ---: | ---: |
| LP b26c9619e | 12,721 | 288 |
| LP dce5b8236 | 12,922 | 311 |
| Chrome stable 157.0.8103.80 | 14,791 | 669 |

Across all 787 selected official URLs the candidate gained 201 named passes and lost 0. Both LP runs restarted zero times. LP recorded 58 timeouts and 9 page errors; Chrome recorded 1 timeouts and 0 page errors. The Chromium comparison reuses the corrected stable-profile receipts from the original [full census](https://github.com/povio/pyro/tree/57a9450abdaa792dafae09f6e84d4236fa72a2ea/evidence/round6-noncss-census) on the same WPT revision. The 31-page CSS hit-test guard recorded 98 identical named statuses. Only previously completed ordinary URLs were used for that supplemental guard. An intermediate build lost one ellipse hit-test assertion because the hit tester only supplied XML attributes; preserving its existing radius treatment repaired that regression. The existing SVG Web API native suite passed all 32 cases.

The named table stores URL, original name, one-based duplicate occurrence and the statuses of control, candidate and stable Chrome, in that order. The page table stores URL and each browser's outer status, harness status and fully passing flag. Null means a missing observation. Bézier arc-length approximations give small numerical differences from Chrome for curved elementary shapes. Remaining SVG conformance gaps include point and transform list reflection, text character geometry and animation.

Use the original reviewed manifest runner with a complete checkout of the pinned WPT revision. Set WPT and MANIFEST to their paths, BIN to the LP executable, and OUT to the report path.

```sh
bun bench/wpt/run.mjs --wpt "$WPT" --kind testharness --manifest-inventory \
  --measure-reviewed-skips --generated-globals "$MANIFEST" --dirs non-css \
  --server wptserve --testdriver --http2 --jobs 4 --tests selection.json \
  --bin "lp=$BIN" --out "$OUT"
```

Use the same runner with --dirs css, --jobs 2 and hit-guard.json for the supplemental controls. The numeric probe takes a report label and either the executable path or the argument chrome. It uses playwright-core with the local CDP adapter. This topic adds a direct Cargo dependency edge to svgtypes 0.16.1, which was already locked and shipped. Both builds used the original upstream WPT expectations and the pinned JSC backend.
