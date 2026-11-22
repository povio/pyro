# Matched cumulative non-CSS census export

The exporter joins completed current receipts to the immutable prior LP and signed Chrome ledgers at 01f49f43da24a20ecb20d99c967fc5de54272120. The prior LP was 1f1be032436bfeee9f48cdb5acbc0f71d21d5af9. Chrome was signed stable 158.0.8127.37 on macOS arm64. These reference measurements retain the original 4-job provenance. The current run uses c3372ad21df84bb4bb7386ae4f114fa185d10f30, tree c4d45b20c3ea5429596bd4e7b7f2ca33c32065db.

Build the read-only reference index once from the frozen evidence.

    python3 reference_index.py "$PRIOR/evidence" "$WORK/exporter/cache/reference.sqlite"

For a partial, immutable snapshot of available local shards, use a new destination each time the set of completed keys changes.

    python3 aggregate.py --plans "$WORK/plans-v2" --reference-db "$WORK/exporter/cache/reference.sqlite" \
      --run "$WORK" --owner local --cache "$WORK/exporter/cache/shards" --out "$WORK/checkpoint-local-N"

For the full local assignment add --require-complete. For the combined census use --owner all --require-complete and a second --run pointing to the Studio receipt bundle. --keys accepts a JSON array of canonical shard keys. The launched v2 plan assigns 160 shards per host. It covers 30,179 URLs: 15,072 local even-indexed and 15,107 Studio odd-indexed, with 179 new exclusions and 1,469 historical holds. The v1 five-exclusion plan was superseded before measurements; its 30,353-URL denominator is not used for this census. Each host directory provides machine-provenance.json, raw/completed.jsonl, raw/KEY.json, raw/KEY.log and verified-ports/KEY.json. This exporter takes any fully validated completion order. It never starts a browser or requests an incomplete shard be rerun.

Each invocation verifies the pinned selector files, source/environment/variant, digest of each claimed raw receipt, authenticated port evidence, runner settings and manifest identity. Partial ledger reads acquire a shared file lock. Per-shard immutable comparison caches and final snapshots are constructed in private temporary directories, atomically renamed and verified on reuse. The frozen comparison inputs are indexed once using SQLite. A checksum over a cache key includes the scripts, source identity and immutable receipt, selector, log and port digests.

* named.jsonl.gz rows contain URL, canonical subtest name, one-based occurrence, current, Chrome and previous LP. Cells are [status, originalReportedName] or null when unobserved. The frozen canonicalizer rewrites only recognized encoded WPT URL authority ports.
* pages.jsonl.gz rows contain URL, current, Chrome, previous LP, three source receipt keys and the current error. Each page cell is [lifecycle, harnessStatus, strictPass, passingNames, observedNames]. A strict pass requires completion, an OK harness and a nonempty all-passing subtest set. No WPT expectations influence scoring.
* gains.jsonl.gz and losses.jsonl.gz contain directional changes versus previous LP, with page/harness state, current subtest diagnostic if reported, and source key. summary.json separates matching names, newly observed and missing names, strict pages, area and environment.
* issues.jsonl.gz includes all current non-OK or incomplete pages, including partial named results. Machine provenance preserves individual native restart/watchdog amounts and receipt identities.
* coverage.jsonl.gz includes every non-CSS inventory URL as [URL,coverageState,currentScheduledShard,reason,environment,source,blob,guard,historicalWithheldReasons]. States distinguish measured, pending requested work, other machines/scopes, previous holds and the newly withheld variants. A pending or withheld URL does not enter a pass or failure total.
* SHA256SUMS covers the entire snapshot. The signed/reference provenance maps identify the exact producing receipts. Gzip members are deterministic per key and concatenated in canonical plan order.

The historical demonstration re-reads only preflight-000 from the published 1f1be0324 raw artifacts. It resolves six URLs and 26 observed names, with 22 passes and one strict page, reproducing all prior statuses (+0/-0). Its summary carries historicalDemonstration=true and fullCensusComplete=false. Historical demo artifacts are separate from all fresh results.

native-events.json retains explicit new native events and unattributed exits separately; a recorded new event affects coverage and is never an instruction to reproduce it. The collection plan and historical withholding reasons are preserved alongside the export. Use archive evidence from the pinned prior commit and its original-official-inventory.jsonl.gz to regenerate the reference index; --inventory accepts the decomposed archive path on a machine with a different cache layout.

If the pinned runner writes a complete receipt after its global stall watchdog fires, generated “runner stalled” error placeholders retain that lifecycle result and missing named observations in the comparison. They receive an explicit runnerStalledPlaceholder coverage state and per-shard URL list instead of being labeled a navigated page.

If the coordinator approves a revised subtractive plan, use --plan-sha with its exact tasks.json SHA-256. The exporter checks every effective URL list against the 320 original shards and their original hashes, rejects additions and order changes, and computes the effective count at run time. Historical rows remain frozen. historical292.json and previous106.json retain each original URL, canonical name, occurrence, prior metadata, current observation and current page disposition; an absent receipt stays pending or out-of-scope. historical-tracking.json identifies the immutable historical sources and counts each disposition. Use --prior to give the original census root containing tasks.json, selections/ and evidence/, and --loss-audit to provide the separately published prior technical classification file when using a portable archive.

A packed run may deliver raw/KEY.json.gz instead of raw/KEY.json. The exporter checks the decompressed bytes against the original independent receipt digest and records the gzip transport checksum separately. Raw and losslessly packed representations produce identical named and page streams.

checkpoints.py follows the published local receipt ledger by filesystem completion events. It freezes the first 40, 80 and 120 committed local keys into separate checkpoint directories and all 160 into final-local, with matching immutable key, raw-digest and summary sidecars in checkpoints/. It only reads run artifacts and never controls browser capacity. Each exported partial scope states its measured URLs; remainingLocalUrls in the sidecar covers the rest of the even-indexed assignment.
