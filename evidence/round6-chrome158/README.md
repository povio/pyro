# Chrome 158.0.8127.37 WPT reference — priority receipt

This priority snapshot records 4,041 distinct observations out of 31,450 selections on this Mac. All 39,989 upstream testharness URL variants are retained. The designated shared non-CSS selection is 30,359 of 31,827; the local CSS guard is 1,091 of 8,162. Remaining CSS execution belongs to the separate CSS lane. All count differences refer to local measured results on pinned WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603.

| Scope | Observed URLs | Fully passing pages | Named results | Named passes | Awaiting in this snapshot | Explicitly withheld |
|---|---:|---:|---:|---:|---:|
| non-CSS | 2,950 | 2,408 | 205,553 | 193,011 | 27,409 | 1,468 |
| CSS | 1,091 | 966 | 36,966 | 35,121 | 0 | 7,071 |

## Reference identity

Official signed Google Chrome stable 158.0.8127.37, macOS 27.2.1 arm64 (Apple M1 Max). The distribution was fetched from the Google stable update channel; codesign --verify --deep --strict succeeded, TeamIdentifier EQHXZ8M8AV. Separate CDP connections to the live browser reported Chrome/158.0.8127.37, V8 15.8.62.31 and protocol 1.3.

The private runner uses the unmodified official playwright-core 1.65.1 driver and the exact no-native-replay WPT runner recorded in reference-identity.json. The manifest-first harness uses one fresh browser context per page, WPT stable profile settings, 800×600 at scale 1, native wptserve, H2 and testdriver, four jobs per shard, at most two concurrent reference shards. Each browser launch gets a new isolated temporary profile. A 15s normal or 45s long deadline is recorded as observed, with actual reported names; no score is imputed for absent names. The same-origin popup and CPU/GPU/NPU WebNN feature checks were predeclared and included as regular WPT observations.

## Selection and withholding

The previous reviewed non-CSS cohort contained 30,513 URLs. Native event review removed 158 historical-risk URLs from that cohort. Four separately proven synchronous WebAssembly controls were eligible for this shared plan: module/exports, module/imports, module/toString and table/length in Window. Grouped Wasm fault inputs were not replayed. The rest of Wasm remains excluded where identified in the per-URL ledger. Public Chrome CRASH, local native crash/hang markers, and unassigned browser restarts in the prior LP Wasm and Chrome html-035 receipts are explicitly represented. Where every prior restart was explained by a specific guarded hang, only the recorded implicated URL is withheld. An earlier Playwright assertion or dialog race is recorded separately as infrastructure. Technical omissions such as JS shell, extensions, unavailable testdriver modes and direct HTTP/1 transport remain in the denominator.

## Combined-topic comparison

LP fadcbb56e data is reused verbatim from its completed same-pin evidence; Chrome 158.0.8127.37 is freshly measured here. Matching uses variant URL, corrected local server-port normalization, exact name and one-based occurrence. Overlapping guard memberships remain separate. Prior Chrome 157 results keep their historical identity.

| Guard | Matched URLs | Missing Chrome pages | LP / Chrome named passes | LP-only / Chrome-only | LP / Chrome fully passing |
|---|---:|---:|---:|---:|---:|
| loss | 158 | 1 | 60,231 / 60,721 | 1,236 / 1,726 | 42 / 79 |
| main | 2,405 | 2 | 179,494 / 188,128 | 3,669 / 12,303 | 1,066 / 1,896 |
| ext | 38 | 0 | 330 / 300 | 44 / 14 | 29 / 18 |
| extra | 414 | 0 | 4,693 / 3,542 | 1,217 / 66 | 397 / 409 |
| css | 1,050 | 0 | 33,321 / 34,502 | 767 / 1,948 | 819 / 931 |
| workers | 41 | 0 | 577 / 619 | 0 / 42 | 27 / 35 |

For the 292 fixed prior loss occurrences the observed Chrome158 dispositions are {'Pass': 190, 'Fail': 91, 'missing-name': 7, 'unobserved-page': 4}. The 157 fixed referrer-policy occurrences are {'Pass': 157}. Unobserved URL and missing-on-observed-page are distinct.

## Latest LP comparison

The independently run LP cbdeb11931f74c3de4417a25fb9103f1df84d54e uses identical selections, manifest, viewport, server and driver. Only common complete shard receipts contribute to these paired totals. The cbdeb page and name sets are provided separately from the earlier fadc observations.

| Scope | Paired URLs | LP / Chrome named passes | LP-only / Chrome-only | LP / Chrome fully passing |
|---|---:|---:|---:|---:|
| non-css | 2,950 | 185,374 / 193,011 | 4,951 / 12,588 | 1,535 / 2,408 |
| css | 1,091 | 33,897 / 35,121 | 767 / 1,991 | 845 / 966 |

## Cross-machine CSS compatibility

The Studio main and 41-URL WPT-certificate supplement also used official stable 158.0.8127.37 on macOS 27.2.1 and pinned WPT. Studio recorded 8,123/8,162 CSS URLs, 6,985 fully passing and 183,582 named passes. Thirty-nine crash/security fixtures remain inventoried without execution. The 1,091 locally observed CSS URLs all have Studio records. The 7,032 Studio-only CSS URLs retain their Studio results and their local coverage reason. No URL or named results are pooled on an overlap.

On those 1,091 shared URLs the two machines each recorded 35,121 passes. The intersection contains 35,117 named passes; 4 are exclusive to Studio and 4 to this Mac. Studio fully passed 967 of the overlapping URLs and this Mac fully passed 966. All 41 supplemental URLs matched on named and strict-page pass sets.

Studio's original 8,082-URL run used headless CDP in disposable profiles and 16 disjoint shards, with extensions/background component extensions disabled in four shards following a driver attachment failure. Its separate service-worker supplement used the WPT leaf SPKI. The original recorded configuration does not establish that Studio used this Mac's popup preference, per-feature WebNN setting, native GPU choice, exact Playwright version, concurrency or complete normalized-port mapping. There were no experimental web-platform flags. The paired difference is observational and does not assign a cause to the profile differences. [Studio main reference](https://github.com/povio/pyro/tree/fd629a078f697139cf140adbb2a315e90037bb25/docs/validation/studio-css-e4ca26) · [Studio supplemental reference](https://github.com/povio/pyro/tree/8e84143126a3335af41906350060477c69751616/docs/validation/studio-css-e4ca26/color-gap-controls.md)

cross-machine-inventory.jsonl.gz enumerates all 39,989 variants with separate local and Studio slots. studio-css-pages.jsonl.gz and studio-css-names.jsonl.gz preserve actual Studio harness status, raw named result, one-based occurrence and the parallel MacBook observation when present. Studio file hashes, source commits and exact missing rows are in studio-compatibility.json.

## Reproduction and interpretation

Run python3 verify.py in this evidence directory to verify checksums, archived page records, occurrence identities, name/pass totals and paired comparisons without a browser. official-pages.jsonl.gz preserves all URLs as source, blob, environment, scope, guard membership, status, omission reasons, shard and page summary. reference-names.jsonl.gz stores URL, canonical subtest name, one-based occurrence, raw status and original name. A fully passing page requires a complete runner status, OK harness, at least one subtest and every named outcome passing.

The compressed receipts preserve the structured input plan, all raw statuses/names/messages and the rest of the runner metadata. Their original installation path is replaced by a portable artifact label. Original raw SHA-256s, launch counts, exact runner hash and the producing server configurations are retained. Names containing encoded WPT ports are mapped only through that originating server. No original Chrome 157 receipt was changed. The stable acceptance binary version matches the requested cross-machine reference. OS, GPU backend, timeouts, visibility and driver/harness setup may differ from other machines; an identical version alone does not guarantee identical observations.
