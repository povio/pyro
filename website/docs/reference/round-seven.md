# Pyro 0.0.50-rc.6 compatibility progress

Release candidate **0.0.50-rc.6** tags source commit `9fad5953f`. RC.5 stopped before npm publication when its package exceeded the 185 MiB budget. RC.6 raises that budget to 190 MiB and updates the release documentation. Both tags contain the same browser source. [RC.5 workflow failure](https://github.com/povio/pyro/actions/runs/44998826328) · [Changes between tags](https://github.com/povio/pyro/compare/v0.0.50-rc.5...v0.0.50-rc.6) · [RC.6 tagged source](https://github.com/povio/pyro/tree/v0.0.50-rc.6) · [RC.6 publication workflow](https://github.com/povio/pyro/actions/runs/45008736564)

Standalone RC.6 binaries and SHA-256 checksums are available for macOS and Linux on GitHub. npm registry publication is pending. [Download RC.6 standalone binaries and checksums](https://github.com/povio/pyro/releases/tag/v0.0.50-rc.6)

## RC.5 standalone macOS arm64 executable

Two Apple silicon Macs ran the same standalone RC.5 workflow executable from `516dc3159`, using the same system JavaScriptCore on macOS 27.2.1. Together they covered all **30,179 eligible non-CSS URLs** across 320 disjoint shards. The local Mac ran the even shards (15,072 URLs) and the Mac Studio ran the odd shards (15,107 URLs). The RC.6 npm macOS arm64 package uses a patched static JavaScriptCore with fork support. Its conformance is outside this measurement.

| Measured executable | Named assertions passing / observed | Fully passing pages out of 30,179 |
| --- | ---: | ---: |
| Pyro RC.5 standalone | 1,691,766 / 1,785,720 | 15,277 |
| Signed Chrome 158.0.8127.37 (archived reference) | 1,754,948 / 1,802,021 | 24,385 |

Across matched named occurrences, **75,805 passed only in Chrome** and **12,623 passed only in Pyro**. The pinned inventory also contains 1,469 historical holds and 179 additional held variants. [Combined census, runtime identity and shard receipts](https://github.com/povio/pyro/blob/5892e136beb3c09446a3e12953c260ca08938cd9/evidence/rc5-census-516dc3159-combined/README.md)

## Development checkpoint with pinned JavaScriptCore

Pyro fully passed **15,737 of 30,179 eligible pages** at the round seven checkpoint (`c3372ad21`), up from 15,344 in the previous census. All 320 shards ran on macOS. Each engine’s assertion denominator counts the named outcomes observed for that engine.

| Measured engine | Named assertions passing / observed | Fully passing pages |
| --- | ---: | ---: |
| Previous Pyro checkpoint | 1,690,433 / 1,789,453 | 15,344 |
| Pyro at c3372ad21 | 1,696,056 / 1,790,227 | 15,737 |
| Signed Chrome 158.0.8127.37 | 1,754,948 / 1,802,021 | 24,385 |

Compared with the previous Pyro checkpoint, 5,844 named outcomes gained a pass and 221 lost one. On the same inventory, 12,630 passed only in Pyro and 71,522 passed only in Chrome. The manifest records 179 additional omissions and 1,469 historical omissions. [Census, coverage and raw receipts](https://github.com/povio/pyro/tree/f2f9256dbb9eed19c991cca4934a34a6634e03c4/evidence/round7-census-c3372ad21-combined)

## Editing after the census

Pressing Enter inside a paragraph in a list item now splits the item and preserves neighboring content. The ordinary-list change gained 27 passes with zero losses across eight insertParagraph shards. The subsequent description-list change gained another 16 on those same shards, raising the count from 6,989 to **7,005 of 7,195**. Signed Chrome passed 7,021. These figures come from focused runs after the census. [Ordinary-list evidence](https://github.com/povio/pyro/tree/c10471bc788e0d1c79f3132a5652a9d288341ed1/evidence/round7-editing/list-block-split) · [Description-list evidence](https://github.com/povio/pyro/tree/eb81d184aa0d5e5701e9be210b47f7096a0f7d10/evidence/round7-editing/description-split)

All 48 description-list fixtures matched Chrome’s markup and selection endpoints after splitting and typing. Undo grouping still differs from Chrome.

## Main CI snapshot coverage

At `fc180f6c5`, the dashboard had baseline rows for 33,538 of 40,045 testharness URLs. A row can record a pass, failure, error or timeout. The audit traced all **6,507 URLs without a row** to four groups.

| Observation available in the inspected archives | URLs |
| --- | ---: |
| Round seven non-CSS census at `c3372ad21` | 5,648 |
| Studio CSS runs, including the supplement | 489 |
| Earlier census, focused or nightly runs only | 249 |
| No observation found | 121 |

Of those same 6,507 URLs, **4,318** were worker variants and **2,021** appeared on an earlier dashboard before a baseline run lost shard 0. Those counts overlap the table’s categories. [Per-URL audit and baseline history](https://github.com/povio/pyro/tree/166a2be8c4d1e0b9ec4762d2a2b37df194e1cfbd/evidence/wpt-status-fc180f6c5)
