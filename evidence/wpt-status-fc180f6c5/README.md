# Why 6,507 testharness URLs have no dashboard row at fc180f6c5

The exporter joins the frozen WPT manifest to the keys of the gate and whole-suite baselines. The root’s 40,045 URLs comprise 39,989 official URLs and 56 additional baseline keys. Of the 33,538 dashboard observations, 33,482 match the manifest. The remaining 6,507 official URLs have no baseline row at this dashboard commit. An error or timeout with a row counts as measured.

| Disjoint census disposition among the 6,507 | URLs | In first nightly shard 0 | Worker environments |
|---|---:|---:|---:|
| Non-CSS, observed on c3372ad21 | 5,648 | 1,484 | 4,094 |
| CSS, Studio main Pyro receipt | 448 | 448 | 0 |
| CSS, Studio supplemental Pyro receipt | 41 | 0 | 37 |
| Non-CSS, historically withheld at c3372ad21 | 328 | 77 | 158 |
| Non-CSS, newly withheld at c3372ad21 | 39 | 9 | 29 |
| CSS, withheld by Studio | 3 | 3 | 0 |
| **Total** | **6,507** | **2,021** | **4,318** |

**Baseline history.** All 2,021 missing URLs in the 35,562-page first nightly appear in its shard 0 and also in the saved dashboard at 0402c4e52. None appear in the first nightly’s other three shards. That first run recorded 1,964 complete, 34 timeouts, 21 errors and two hangs among them. The next baseline-producing run lost shard 0 after port exhaustion and a failed browser restart. Its imported full baseline contained the other three shards. Gate baselines can supply overlapping pages. This explains why the entire historical shard size of 8,887 cannot be assigned to the present gap. The exporter reads baseline keys for the snapshot and does not accumulate prior dashboard rows. [Runner history](https://github.com/povio/pyro/blob/fc180f6c5/bench/wpt/README.md) · [Exporter](https://github.com/povio/pyro/blob/fc180f6c5/bench/wpt/status-export.mjs)

**Discovery and runnable backlog.** Static discovery at fc180f6c5 with the full pinned checkout, all directories, wptserve and testdriver selects 1,990 missing URLs. All had first-nightly results. Of these, 1,907 have an ordinary census observation (1,459 non-CSS and 448 CSS). The other 83 are withheld in the cited census (71 historical, nine new, three CSS). The other 31 of the first nightly’s 2,021 have explicit current per-file skip reasons. Among the 4,517 outside current discovery, 4,318 are worker URLs, 78 JS shell, six extension, and 115 window including three window-module. The dashboard runner reads window wrappers from .any.js and .window.js. It does not enumerate the manifest’s worker execution environments. Source-level skip reasons can also apply to a worker’s source and are retained separately. Among the 115 window URLs, 31 carry explicit skips, three module variants have a no-window reason, and 81 lack a file-level reason. Four of those 81 are the Studio UTF-16 supplement. A missing file-level reason is left open.

**Historical measurements and unknowns.** The primary CSS/non-CSS receipts cover 6,137 missing URLs. Of the 370 withheld in those lanes, 205 have an older LP census or focused receipt. Another 44 have an earlier nightly observation. Thus **6,386 have a Pyro observation in at least one inspected archive**. The remaining **121** have no Pyro observation in these joins. The inventory assigns 78 to JS shell, 31 to crash/security fixtures, six to the extension harness, three to manifest proxy auto-configuration and three to a previous review that placed them outside ordinary execution. Other archives may exist. An earlier measurement gives neither present-head coverage nor authority to replay withheld cases. Current non-CSS measured rows include 5,310 complete, 208 errors and 130 timeouts.

## Per-URL evidence

urls.jsonl.gz contains one JSON record per missing exact URL, retaining query variants. Fields include source, environment, current static discovery membership, optional source-file skip reason, census disposition, observed page status, older LP receipt memberships, nightly shard/status and the earlier dashboard status. Null indicates absent evidence in that column. The shard and worker columns in the table are overlapping dimensions of its first count. Only saved observations and source discovery were read.

| Evidence | Immutable repository object and path |
|---|---|
| Dashboard and official universe | c0d789d1bc2eefab9121979512b00b53dad5d65e, runs/fc180f6c52cc7b6c6578c33f5bee7ee2bdb595f6.json.gz, runs/0402c4e52534d7b27bc4a83bf74ee64414b058d4.json.gz, manifest/a421f2fea5eb7f91e0db399e44b0fb0779ee1603.json.gz |
| Runner selection | fc180f6c5 bench/wpt/lib.mjs discover and allDirs, WPT checkout a421f2fea5eb7f91e0db399e44b0fb0779ee1603 |
| Nightly | Actions run 44215242259, wpt-full-testharness-0.json.gz SHA-256 4633dd381bc84e34a536a7e048d95459bd5123c686602719e0f136daab13e767 |
| Current non-CSS | f2f9256dbb9eed19c991cca4934a34a6634e03c4, evidence/round7-census-c3372ad21-combined/comparison/{coverage,pages}.jsonl.gz |
| CSS LP | 8e84143126a3335af41906350060477c69751616, docs/validation/studio-css-e4ca26/{outcomes-base,discovery-supplement,coverage}.json.gz, controls field in supplement |
| CSS source and environment | 3bcc56d4d10420ffa143df23e717380bc3d1a592, evidence/round6-chrome158/studio-css-pages.jsonl.gz |
| Older LP joins | 57a9450abdaa792dafae09f6e84d4236fa72a2ea, noncss-census-evidence.tar.gz pages.jsonl.gz columns lpBase, lpCombined2 and priorLpWasm. fdf757052183b8df60dae474f478d20de16aa436, round6-noncss-902fe811f/pages.jsonl.gz current and prior LP. 01f49f43da24a20ecb20d99c967fc5de54272120, round7-noncss-census-1f1be0324/{pages,archive-pages}.jsonl.gz current LP. |
