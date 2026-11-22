# Live SVG number lists

SVGTextPositioningElement.rotate, SVGFEColorMatrixElement.values, SVGFEConvolveMatrixElement.kernelMatrix and SVGComponentTransferFunctionElement.tableValues now expose typed SVGAnimatedNumberList objects. This covers text, tspan, feColorMatrix, feConvolveMatrix and feFuncR/G/B/A. Existing point and new number lists share item attachment, mutation, indexing and per-realm wrapper handling. The animated view reflects the base value until an animation applies. Sampling active SVG number-list animations remains unimplemented.

The matched selector was frozen before product edits. It covers 51 ordinary window URLs in SVG IDL, list reflection, item ownership, existing point and length lists, and animation guards. The previous published census identified 67 target names across these pages. Both engines had completed the selected URLs in that census. This focused result does not estimate full non-CSS parity. Workers, reftests, unselected pages and previously withheld inputs have no new measurements here.

The 51 matched Chrome observations were extracted from the original signed macOS ARM64 Google Chrome stable 158.0.8127.37 publication ee24e07f6 across 24 four-job shards. The page map, source blob IDs, named statuses, identities and receipt hashes are included. Restart and watchdog totals are unavailable on this reference projection. LP baseline and feature builds used the same pinned WPT, manifest, CI JSC archive, release-fast-derived wptiter build profile and four-job runner. Each LP page used a fresh context.

## Direct behaviors

The ordinary direct probe creates each exposing element through createElementNS. It captures the original reflected wrappers, rewrites and removes items, inserts an owned item into a second list, writes a namespaced attribute, exercises read-only animVal, and reports actual exception names, brands and object identity. check-probe.py validates expected behavior for each class. Positive animation sampling is covered separately by the WPT failures.

The SVG2 list synchronization algorithm preserves surviving wrapper identities on attribute rewrite, detaches removed items, and updates retained values by index. Inserting an attached item copies it; inserting a detached item reuses it. Reserialization sets an empty attribute string after clearing or removing the last number. Chrome stable's observed differences include unstable item wrappers and removing the empty attribute. The retained direct rows document the actual results of both engines.

SVG2 text rotate uses the CSS grammar [ <number>+ ]#. CSS tokenization accepts adjacent unambiguous number tokens, comments and form feeds. Filter Effects requires separators between numbers; the filter attributes reject adjacent signed or decimal tokens. A trailing comma is invalid. Direct Chrome observations are included even where Chrome is more permissive. [SVG2 types](https://svgwg.org/svg2-draft/types.html#TermReserialize) · [SVG2 rotate](https://svgwg.org/svg2-draft/text.html#TextElementRotateAttribute) · [CSS component whitespace](https://www.w3.org/TR/css-values-4/#component-whitespace) · [Filter Effects](https://www.w3.org/TR/filter-effects-1/#element-attrdef-fecolormatrix-values)

## Commands

Run these with the sibling caches referenced in run.py and import-reference.py. Build each exact source tree from provenance.json using the unchanged matching CI JSC archive, Cargo profile wptiter inheriting release-fast, LTO disabled, 16 codegen units, incremental compilation, feature render, and one Cargo job. Place the two executables under base-pyro and pyro.

    python3 import-reference.py
    python3 run.py base
    node probe.cjs base ./base-pyro
    node probe.cjs chrome158
    python3 run.py candidate
    node probe.cjs candidate ./pyro
    python3 check-probe.py
    python3 report.py
    cargo test --locked --offline --profile wptiter -p pyro-dom --lib svg_point_list::tests

named.jsonl.gz retains URL, original name, occurrence, then base/candidate/Chrome status. pages.json records page outcome, harness status and strict pass for each engine in that order. A strict pass requires a completed page, OK harness and all observed named tests passing with at least one subtest. Null denotes an unobserved result. summary.json includes full receipt digests. The precursor directory retains measurements for 65d7a64bc before the rotate parser correction.

## Measured result

| Engine | Named passes / 2,162 | Strict pages / 51 |
|---|---:|---:|
| Original LP 97864f1fa | 1,440 | 16 |
| Feature LP f5d9413db | 1,520 | 20 |
| Signed Chrome stable 158.0.8127.37 | 2,058 | 45 |

All three ledgers reported every selected page and name. All 80 gains were previous failures, with zero named or strict page losses. Of those 80, 65 also passed Chrome and 15 failed Chrome. The resulting named pass sets have 573 Chrome-only and 35 LP-only names, a net difference of 538. The feature recovered 65 of the 67 ranked Chrome-only names. Two valid active SMIL animations on text.rotate remain in that original target set. All 51 pages completed. Each LP run had zero restarts and watchdog kills. The three focused native tests passed.

Independent source review identified empty-attribute serialization and verified the follow-up. Direct Chrome comparison and a second grammar review identified CSS-token handling for text rotation. c61976c83 implemented that change and f5d9413db used the constructor provided by the pinned cssparser. The empty-serialization and parser probes retain raw outcomes for Chrome, the precursor and final feature.

Independent runtime QA repeated the final feature on all 51 URLs and independently exercised the eight direct DOM classes. Its full named ledger, page outcomes and direct probe values match the implementation run exactly. Its distinct candidate receipt digest is preserved under qa/.

The 15 new passes that Chrome fails are all SVGNumberList identity or synchronization assertions: two on the getItem identity page, three across attribute changes, four for mutator return identity and six for live synchronization under grow, shrink and same-count rewrites. They follow SVG2's list item identity and synchronization rules. Every name and status is preserved. [SVG list item identity](https://svgwg.org/svg2-draft/types.html#ListItemIdentity) · [Synchronizing reflected lists](https://svgwg.org/svg2-draft/types.html#TermSynchronizeList)
Independent source review cleared final tip f5d9413db, including the exact parser constructor follow-up.
