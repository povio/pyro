# Composed selection ranges

Pyro implements `Selection.getComposedRanges({shadowRoots})`. It returns static snapshots, orders cross-tree endpoints and rescopes endpoints outside the supplied shadow roots. New snapshots track removal of selected nodes or their shadow hosts. Previously returned snapshots retain their original endpoints.

Across 11 selection and focus pages, passes increased from 153/198 to 189/198 (+36, zero losses). Fully passing pages increased from two to seven. All 12 removal cases now pass, six each for open and closed shadow roots. Independent QA reproduced the gain and passed eight supplemental controls. Thirteen ordinary DOM Range guard pages retained all 5,634 named outcomes. Three exact native tests each executed and passed.

Eight checks in the selection and focus cohort still pass in the archived Chrome 158 comparison and fail in Pyro. They cover live Range updates across tree boundaries, slot-related selection behavior, collapsed selection reporting and Range.isPointInRange. Four pre-existing focus checks pass only in Pyro.

| Pyro run | Source | Binary SHA-256 |
|---|---|---|
| Base | 3a5719c37f343853bcb258d342406735367595cb | 07dd0d229c3c24f704f8170fe8fcb36ca66d9d782ddb55f93903ce4bae6fb800 |
| Candidate | d20cdcc4e234ecaee21cf60f0325b9596b728e51 | 663f057002859a28d49196d9a6b07cbaf23304c75172d778d1942c135dbcb5a2 |

Rows contain URL, subtest, occurrence, base status, candidate status and archived Chrome 158.0.8127.37 census status at WPT revision `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`. All 36 gained names passed in that census. Both Pyro builds ran the same static-server checks locally. The Chrome column uses historical WPT-server receipts from the [round six census](https://github.com/povio/pyro/tree/9a195fb10/evidence/round6-noncss-census-8e7144b7e).

```sh
gzip -dc named.jsonl.gz | jq -sc 'map(.[0]) | unique' > urls.json
bun bench/wpt/run.mjs --wpt "$WPT" --dirs dom/ranges,selection --tests urls.json \
  --jobs 1 --bin "$LABEL=$BIN" --out "$LABEL.json"
for guard in composed_selection_uses_iterable_roots_and_static_snapshots static_range_keeps_its_points_as_given range_boundaries_follow_mutations; do
  cargo test -p pyro-webapi --test integration "dom_js::$guard" -- --exact
done
```
