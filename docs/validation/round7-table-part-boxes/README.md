# CSS table part geometry — focused A/B/B/A

Baseline `0736e531e4c91a1387dce3100e877bd32e42a400`; implementation `12b3480caed58428906444a561d24ec50489b716`. The previous implementation looked for HTML tag names when assigning geometry to rows and row groups, and did not project column or column-group boxes. Use the effective table role and existing grid tracks; do not manufacture tracks for replaced controls. A collapsed row group removes its space even when its row has explicit `visibility:visible`, both on initial layout and update. Basis: [CSS Tables 3 working draft §3.11](https://www.w3.org/TR/2025/WD-css-tables-3-20251216/#bounding-box-assignment) and [CSS 2.2 §17.5.5](https://www.w3.org/TR/CSS22/tables.html#dynamic-effects).

| Focused eligible CSS Tables cohort | Parent | Candidate |
| --- | ---: | ---: |
| Complete pages, harness OK | 138 | 138 |
| All assertions passing on page | 94 | 97 |
| Passing named assertions of 822 | 638 | 660 |

Both repeats agree on every name and status. Of 22 recovered names, the historical Chrome reference passes 21: eight each on `bounding-box-computation-{1,2}`, four on `bounding-box-computation-3`, and one dynamic row-group-collapse assertion. The first two bounding-box pages and the collapse page became fully passing. Chrome fails the additional recovered consecutive-column/border-spacing assertion on `html5-table-formatting-3`; do not count it as Chrome-confirmed. No baseline pass was lost. `bounding-box-computation-3` still fails two empty-group offset checks. The existing horizontal track geometry guard remains; no change to zero-column sizing or height redistribution is claimed. No withheld/fault cases or full-suite run.

## Reproduction and controls

Use runner tree `7efc9828957ae18dbd8c8c7df59cba3c59524633` (`bench/wpt`), WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, normal Studio build environment, separate target, `CARGO_BUILD_JOBS=3 pyro-build --features pyro/render` at each code revision. Run for base, final, final-repeat, base-repeat with the corresponding binary:

```sh
D=docs/validation/round7-table-part-boxes
bun bench/wpt/run.mjs --kind testharness --dirs css --server wptserve --testdriver --jobs 2 --tests "$D/urls.json" --wpt "$WPT_DIR" --bin "$label=$binary" --out "$label.json"
python3 "$D/verify.py"
```

The verifier checks manifest, harness, named repeats, totals and deltas, and prints the exact changed names and Chrome results. The manifest also retains replaced-control geometry, cell dimensions, and unaffected table layout cases. `runs.log` includes ports and vendor patch reconstruction checks. Binary SHA-256: parent `2c1c84927ee045b38177e98a4ad26aca705d456096dd8935159d3ed8a5794a8f`, candidate `52abc117f7e9f191a6beeb95f452a7fa535a27105ab45cd936dc7c95ae7fe277`.

`chrome.json.gz` contains only the selected URLs from `7c4f42242:docs/validation/studio-integrated-4126821de/refs/outcomes-chrome-main.json.gz` (source SHA-256 `7858861e3a061f3d352b47336de17ba0dc06a89f81ceb7b48e480ce94e85a4e0`). This is the matched-WPT signed stable Chrome 158.0.8127.37 reference, not a new Chrome execution. `git merge-tree --write-tree` of the implementation and round7 `d1cd467fd659e12f7b0bbecd6413e7e66418056d` was clean; this is textual compatibility only. No integration branch was updated.
