# Pyro 0.0.50-rc.5 compatibility progress

Release candidate **0.0.50-rc.5** tags source commit `516dc3159`. The measurements below identify the executable and coverage for each run. [Tagged source](https://github.com/povio/pyro/tree/v0.0.50-rc.5) · [Build and publication workflow](https://github.com/povio/pyro/actions/runs/44998826328)

## RC.5 standalone macOS arm64 executable

The ordinary macOS arm64 executable from the RC.5 release workflow loads the system JavaScriptCore framework and reports `fork: false`. The npm macOS arm64 binary uses a patched static JavaScriptCore and supports fork mode. The measurements in this section cover the standalone executable.

| Verified shard set | Eligible URLs measured | Named assertions passing / observed | Fully passing pages |
| --- | ---: | ---: | ---: |
| 160 even shards | 15,072 | 825,072 / 874,074 | 7,664 |

The even-shard run used macOS 27.2.1. On the same URLs, archived signed Chrome 158.0.8127.37 results recorded 856,367 of 879,997 named assertions passing and 12,174 fully passing pages. The full non-CSS plan contains 320 shards and 30,179 eligible URLs. [Executable identity, runtime and even-shard receipts](https://github.com/povio/pyro/tree/9a39f6638cee1d721bb028607c9975f13ef8ebed/evidence/rc5-census-516dc3159-even)

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
