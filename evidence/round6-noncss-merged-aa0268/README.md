# Validation of the aa0268b47 integration merge

All five guard runs used LP aa0268b479a6b463da16910b4b743cf38901d962 with matched CI JavaScriptCore. Results below count the exact canonical subtest name and one-based occurrence within its page. A fully passing page completed with an OK harness, at least one named subtest and every named outcome passing.

| Guard | Prior LP | URLs | Prior → merged fully passing | Prior → merged named passes | Gained | Lost |
|---|---|---:|---:|---:|---:|---:|
| Non-CSS primary | 52a207a13 | 2,445 | 1,017 → 1,017 | 177,308 → 177,308 | 0 | 0 |
| CSS primary | 52a207a13 | 1,050 | 818 → 819 | 33,313 → 33,321 | 8 | 0 |
| CSS workers and UTF-16 | 52a207a13 | 41 | 27 → 27 | 577 → 577 | 0 | 0 |
| Historical-loss URLs | 35dfb7474 | 159 | 45 → 38 | 60,310 → 60,289 | 50 | 71 |

The primary non-CSS guard combines the disjoint 2,407- and 38-URL selections. The two CSS guards are disjoint. The historical-loss guard overlaps the non-CSS primary guard on 71 URLs. Those URLs were sampled independently in each guard, so the evidence retains both observations and the table does not sum them. Across all five receipts there are 3,695 cohort observations and 3,624 distinct manifest URLs.

The main non-CSS and CSS supplemental named pass sets were identical to their exact 52a counterparts. The CSS primary guard gained eight names. On the historical 159-page sample, referrer-policy accounts for 42 gains and 62 losses; subresource-integrity contributes 6 gains and 6 losses. The output preserves all changed names. No guard reported a browser restart or watchdog kill.

## Chrome references

| Guard | Merged fully passing | Local stable fully passing / observed | Public stable fully passing / observed | Public canary fully passing / observed | Merged-only / local-stable-only named | Merged-only / public-stable-only named |
|---|---:|---:|---:|---:|---:|---:|
| Non-CSS primary | 1,017 | 1,896 / 2,445 | 1,910 / 2,445 | 1,983 / 2,445 | 3,667 / 14,702 | 3,728 / 14,687 |
| CSS primary | 819 | unavailable | 933 / 1,050 | 948 / 1,050 | unavailable | 795 / 1,966 |
| CSS workers and UTF-16 | 27 | unavailable | 35 / 41 | 35 / 41 | unavailable | 0 / 42 |
| Historical-loss URLs | 38 | 78 / 159 | 80 / 159 | 83 / 159 | 1,261 / 1,685 | 1,135 / 1,732 |

| Guard | Merged named passes | Local stable named passes | Public stable named passes | Public canary named passes |
|---|---:|---:|---:|---:|
| Non-CSS primary | 177,308 | 188,343 | 188,267 | 189,572 |
| CSS primary | 33,321 | unavailable | 34,492 | 34,759 |
| CSS workers and UTF-16 | 577 | unavailable | 619 | 619 |
| Historical-loss URLs | 60,289 | 60,713 | 60,886 | 60,896 |

Local stable is macOS arm64 Chrome 157.0.8103.80 using the previously recorded WPT profile at the exact WPT pin. Its non-CSS census contains no CSS guard data. Public Linux stable 157.0.8103.38, run 5478394868006912, used WPT 1eecaa0c30, ten commits before pinned a421f2fea5. The selected URLs and test-source blobs match across those revisions; shared dependencies or platform behavior can differ. Public Linux canary 159.0.8143.0, run 4907012216061952, used the exact pin. Every comparator has its own observed/absent keys in the exported sets.

## Disposition of the 292 historical occurrences

The 292 historical loss keys come from e4ca26f48 versus 4c13e694a, distributed over 159 URLs. The earlier dedicated 159-page comparator was measured on **35dfb7474**. The exact **52a207a13** primary run covered only 71 of those URLs. The table distinguishes an unmeasured URL from a named occurrence absent on a measured URL.

| Observation | Pass | Fail | Not run or other | Name absent on measured page | URL unmeasured |
|---|---:|---:|---:|---:|---:|
| Prior full census 902fe811f | 131 | 149 | 0 | 12 | 0 |
| merged | 148 | 133 | 0 | 11 | 0 |
| prior52a | 12 | 88 | 0 | 0 | 192 |
| prior35df | 154 | 127 | 0 | 11 | 0 |
| localStable | 191 | 91 | 0 | 10 | 0 |
| publicStable | 192 | 90 | 0 | 10 | 0 |
| publicCanary | 193 | 89 | 0 | 10 | 0 |

On these fixed keys, 15 became passing and 21 stopped passing relative to the 35df receipt. The wider 159-page guard also contains other tests, reported separately above. Named changes describe these samples; each URL was run once per guard.

## Coverage and verification

The full preceding non-CSS census of 902fe811f reported 30,513 of 31,827 official URLs and retained reasons for 1,314 unavailable URLs. This merge guard freshly observed 2,533 distinct non-CSS URLs, plus 1,091 CSS URLs. The other 27,980 previously eligible non-CSS URLs were outside this guard. The 1,314 earlier withheld URLs remain separately classified. Historical full-corpus counts are not substituted for fresh observations. [Full preceding census](https://github.com/povio/pyro/tree/fdf757052183b8df60dae474f478d20de16aa436/evidence/round6-noncss-902fe811f)

Run `python3 verify.py` from this directory to recompute the counts and confirm the checksums without launching a browser.

- `pages.jsonl.gz` stores cohort, URL, pinned source, git blob, public-stable source equality, and one record per comparator in the order in provenance.json. A record contains page status, harness status, strict pass, named count, named pass count and local wptserve configuration hash. Null means the URL was not observed by that comparator.
- `names.jsonl.gz` stores cohort, URL, canonical name, occurrence number, and each comparator’s status plus its original name. Null denotes an unreported occurrence. Encoded WPT URL ports are canonicalized only from the producing server configuration. The port maps are included.
- `prior-changes.jsonl.gz` retains every changed occurrence relative to each guard’s designated earlier build. `historical-292.json` carries every historical key and its disposition on all available comparators. `cohorts.json` includes observed/absent/fully passing counts, additions, losses and exclusive pass sets by comparator.
- `noncss-coverage.jsonl.gz` retains all official non-CSS variants, sources, environments, measurement or withholding reasons and overlapping guard memberships. `local-stable-provenance.jsonl.gz` maps matched local stable pages to their producing receipt, log and server config hash.
- `provenance.json` records the binary and input-receipt SHA-256s, WPT/JSC revisions, concurrency, server options and reference versions. Earlier integration guard evidence: [52a replayed topic](https://github.com/povio/pyro/tree/f80be4736/evidence/round6-noncss-integration-a414), [35df platform comparison](https://github.com/povio/pyro/tree/ff87c6239b92966d0c91000ae2e25993e5337380/evidence/round6-noncss-platform-batch).
