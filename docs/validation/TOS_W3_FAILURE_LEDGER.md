# W3 failure reconciliation at merged main eabee486

This reconciles existing evidence; it does not rerun TOS or report a current-main
pass rate. All TOS results below retain their tested commit, binary and host.
The source audit is against merged `eabee486`. No test, authentication, assertion
or routing manifest was changed. The source is TOS `f9fde4aa0d`, manifest SHA-256
`542d479b709249d072e3b57c1324c894c4378fc5373892ae64c0687486f2ae19`.

## W3-19 was completed, not uncovered

The older [W3-09–18 report](TOS_COVERAGE_W3_09_18.md) ended before W3-19 ran.
Recovered original reports confirm that all 32 selected `positions-overview`
cases ran. W3-01–19 therefore selected **275 distinct manifest-common cases in
39 files**, with no overlap between those selections. This is selection coverage,
not 275 passing cases or coverage of all 742 common cases.

| Retained run | Browser commit | Passed / failed | Interpretation |
| --- | --- | --- | --- |
| `w3l19-1018-085922` | `81ad383` | 0 / 32 | All fail fixture setup on unsupported `Network.setExtraHTTPHeaders` |
| `w3l19u-1018-090100` | upstream `2ad38773` | 29 / 3 | Two INTTRA cases and air-import customs status fail |
| `w3l19h-1018-093618` | `fd520e4` | 30 / 2 | Native page headers + CDP mapping; only two INTTRA cases fail |
| `w3l19hr-1018-093835` | same `fd520e4` | 30 / 2 | Same two failures on repeat |
| `w3l19m-1018-094717` | integrated `c32f29b` | 30 / 2 | Same two failures with response-delivery dependencies merged |

PR102 merged the header mapping; both repair commits are ancestors of the audit
main. The two remaining cases expect “Select INTTRA Service Type”; the app says
“Select Carrier Service Type”, as established in
[the source triage](TOS_SHARED_FAILURE_TRIAGE_W3.md#w3-19-inttra-service-type-tos-ui-text-drift).
The original 32 setup failures must not be carried forward as 32 open protocol
failures. Upstream's additional customs-status failure was not attributed.

[Recovered results](../../evidence/tos-coverage-w3/w3-19/results.json) retain full
browser SHAs, binary hashes, failed case identities and source artifact hashes.
[The recovery script](../../evidence/tos-coverage-w3/w3-19/recover.py) checks the
19 selections against the exact manifest, rejects duplicates, verifies all 32
case identities and reported counts, excludes diagnostic proxy runs, and checks
post-cleanup records. No run database remained. The header-repair runs retained
only the explicitly authorized template; the first baseline/upstream runs had
none. This does not assert the template is reusable on a later UTC date.

## Reconciled W3 failure inventory

This counts distinct cases with an unresolved or stale-assertion disposition
in the retained evidence, not failures measured on one current binary.

| Cases | Disposition | Evidence / owner |
| ---: | --- | --- |
| W3-02: 1 | Repaired, unchanged selection 15/15 twice on PR82 `3f9b73d` | [Navigation-history validation](TOS_COVERAGE_W3_01_02.md#pr-82-verification-on-the-w3-host-docker-postgresql-17) |
| W3-08: 1 | Repaired, unchanged selection 13/13 twice on combined `78d6740` | [Escape/arrow validation](TOS_COVERAGE_W3_09_18.md#w3-08-on-the-combined-escape-and-arrowuparrowdown-candidate) |
| W3-19: 32 setup failures | Header blocker repaired; 30 pass and 2 remain text drift | Exact runs above; those 2 are counted below |
| W3-03: 7 | Confirmed calendar request mismatch: test waits Container, app requests Booking | [Retained Chromium traces and source](TOS_COVERAGE_W3_03_04.md#follow-up-triage-added-after-merge); not W2-29's separate timing case |
| W3-03: 2; W3-07: 1 | Canvas-locator/DOM-renderer drift | [W3-03 triage](TOS_COVERAGE_W3_03_04.md#follow-up-triage-added-after-merge), [W3-07 source triage](TOS_SHARED_FAILURE_TRIAGE_W3.md#w3-07-business-partner-canvas-table-tos-renderer-drift) |
| W3-17: 13 | Confirmed analytics date/seed drift | [Database/source/Chromium evidence](TOS_SHARED_FAILURE_TRIAGE_W3.md#w3-17-analytics-month-buckets-tos-data-drift) |
| W3-19: 2 | Confirmed INTTRA text drift | Source triage and exact repaired runs above |
| W3-10: 2 | Fails on rewrite, upstream and Chromium; cause unresolved | [Three-engine triage](TOS_SHARED_FAILURE_TRIAGE_W3.md#w3-10-two-cases-fail-on-every-engine) |
| W3-18: 1 | Backend deadlock during API fixture setup; cause unresolved | [Backend evidence](TOS_SHARED_FAILURE_TRIAGE_W3.md#w3-18-backend-deadlock-during-api-fixture-setup) |
| W3-12: 2 | Reduced geometry mismatch; full-page contribution still unresolved | [PR114 evidence](TOS_SHARED_FAILURE_TRIAGE_W3.md#w3-12-two-cases-fail-only-in-the-lightpanda-lanes); native repair owned by W2 |

The last seven rows account for **30** cases: 25 with established TOS
request/renderer/data/text drift, two with unknown cross-engine causes, one
backend-setup failure and two route geometry candidates. Shared failure alone
does not exclude a browser defect. None is presently established as an unfixed
CDP method defect. Do not silently change tests or mark these cases passed.

## Separate W2 targeted evidence and next protocol work

The following are W2-reported targeted results, not new W3 executions:

| Case | Exact candidate | Result and scope |
| --- | --- | --- |
| DocuBox CSV preview | `ca24d212868836b00bf7762e583e8b1dec0b8053` | Passed alongside an XLSX failure; XML excluded |
| DocuBox XLSX preview | `123ea7f8c89f53787c0fed73509b458bf769bdf3` | 1/1 passed after PR119 + runtime PR120; original case unchanged |
| DocuBox DOCX preview | `de6bc8819b5d418881a2d04ab9dcaf173a911224` | 1/1 passed with XML + MessageEvent.source + FileReader; original Blob input and package, no overrides |

The implementation dependencies are merged by `eabee486`; these precise
candidate results are not relabelled as executions on that main. W2 retained
artifacts at `/private/tmp/lp-tos-preview-frames-ca24d212-r1`,
`/private/tmp/lp-tos-preview-xlsx-123ea7f`, and its DOCX handoff. W2 reported
unchanged fixture/auth/assertion/manifest hashes and zero run databases. There
is no whole-DocuBox pass claim, new upload disposition, or timing comparison.
W2-26 HBL placement remains native geometry work. W2-29 calendar timing remains
W1-owned, paused behind performance work; it is separate from W3-03 above.

The next proven protocol limitation is the deliberately broad phase1 key/text
refusal whenever a live child frame exists, plus child `DOM.focus` refusal.
The separate preparation branch `fix/cdp-frame-focus` at `5ad0893` retains four
compiled red contracts and a passing Chromium147 HTTP reference probe in three
sandbox modes. It changes no production guard and is not merge-ready. Correct
routing requires W2 native element/window focus transitions and W1's pending
owned `Browser::focused_frame` snapshot. Root activeElement or last Runtime
context cannot substitute for that state. Child geometry/mouse translation
remains explicitly unsupported. No unchanged TOS failure is attributed to this
focus limitation yet, and no duplicate full workload was run for this audit.
