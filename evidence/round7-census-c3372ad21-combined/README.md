# Cumulative non-CSS census — combined

This release includes 320 of 320 V2 original ordinals, covering 30,179 eligible URLs. Every measured page ran on the local MacBook Pro using the archived browser c3372ad21df84bb4bb7386ae4f114fa185d10f30 and original signed Chrome stable 158.0.8127.37 comparator. Studio released all odd ordinals with zero V1/V2 claims or runs. The plan's studio label denotes its original allocation, while the actual executor field records local execution. Earlier even receipts were preserved unchanged.

Plan commit 850ef1221795720b477d5e481627b3cfe1a5fbe6 pins the 30,179 eligible URLs, 320 ordered source manifest entries, 179 additional omissions and 1,469 historical omissions. The comparison directory records per-URL page status, strict pass, named occurrence statuses and missing/not-run values, directional differences versus prior LP and frozen signed reference, and coverage across the official inventory. Use the immutable readback and receipt hashes to verify this publication without rerunning a browser.

Both raw-part archives and receipt paths are pinned in raw-receipts.json. See supplement/ for complete asymmetric named sets, strict page transitions and all prior-pass loss dispositions.

| Engine | Passing / observed named tests | Strict pages |
|---|---:|---:|
| Current LP | 1,696,056 / 1,790,227 | 15,737 |
| Previous LP | 1,690,433 / 1,789,453 | 15,344 |
| Signed Chrome stable | 1,754,948 / 1,802,021 | 24,385 |

Against previous LP, 5,844 names gained and 221 lost passing status. Against signed Chrome, 12,630 names passed only LP and 71,522 passed only Chrome. Coverage and all non-OK pages are separately recorded.

followup-ranking.json ranks ordinary remaining causes from the saved directional ledgers and static source at integration 4ebaf6917. It separates missing prerequisites, available direct diagnostic evidence and strict-page bounds from projected feature gains. No new WPT results were credited to the ranking.
