# Named Storage Buckets

The RC.5 standalone census recorded 299 signed-Chrome-only occurrences on 28 eligible bucket URLs and 24 strict-page gaps. The initial development selector included all 28 bucket URLs and 28 neighboring StorageManager URLs. Its SHA-256 is 01127157f14df4f99c83846a30861f6f9a89ebb8e75bbfbb34a126a1cc16b2b7. LP's exact integrated parent is 850e52ba30384ccf3231128f541003a6926db4e2. The parent binary SHA-256 is 2151baad9249bab742c135d34f4478485dfd87382848de49f452a27b38f057a1. Both LP arms use pinned static JSC, wptiter/render and one compiler job. The independently verified comparator projects 51 original signed Google Chrome stable 158.0.8127.37 shards at WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603. The same signed executable ran the ordinary direct probes.

A context's named-bucket catalog is shared with its windows and workers. Its storage key includes the tuple origin, top-level site and cross-site ancestry. Each open name refers to a generation with its own IndexedDB, CacheStorage and OPFS namespaces. Deletion or expiry retires that generation, so reuse of the name starts with empty data. CacheStorage and IDB factories are stable on each bucket and return objects with the realm's existing interfaces. The direct controls cover sharing in each worker kind, isolated origins and browser contexts, same-name recreation, retained or pending operations and ordinary default endpoint compatibility.

The quota ledger tracks logical committed bytes and outstanding reservations across the three store types of a named bucket; it applies a context storage shelf limit across named generations. IDB charges serialized wire values, primary keys and index keys at transaction commit, with rollback on abort; these are logical byte counts rather than filesystem sizes. OPFS charges committed data and reserves staged writer changes; the existing file-system tree safety limit also applies. CacheStorage keeps an unlinked cache's data charged for the lifetime of retained handles while the bucket lives, and retirement eagerly clears the payloads of both named and unlinked caches.

Default CacheStorage and IDB retain their original origin-qualified backing stores. Default navigator.storage.estimate previously counted localStorage and OPFS. The candidate adds named-bucket committed usage and endpoint details for the current partition to that original estimate. Default IDB/CacheStorage byte totals are not included. default-estimate.cjs separately records this preexisting gap relative to Chrome. The named-shelf quota is not a hard cap over all default storage: those paths retain their own limits. The headless context does not grant persistent storage; the persistence query reports false.

The Storage Buckets API is a draft. It permits 64-character valid names and rejects a name exceeding 64; signed Chrome rejects names of length 64 as well. A separate options probe records 63, 64 and 65. The current IDL and signed reference omit the obsolete durability() method; the pinned WPT storage_bucket_object fixture contains 12 failing assertions in Chrome across four globals that use that surface. The catalog, quota and generation probes distinguish currently observed browser behavior from normative/proposal differences.

- https://wicg.github.io/storage-buckets/
- https://storage.spec.whatwg.org/#storage-buckets

## Reproduction

Use the paths in run.py/signed-reference.py and preserve executables and output directories separately. Both WPT arms use four page workers, the original portable runner, and no native retry.

    # Final comparison projects the archived parent and signed reference onto urls.json.
    ./candidate.sh "$FEATURE_COMMIT"
    node default-endpoints.cjs candidate ./pyro
    node catalog.cjs candidate ./pyro
    node backends.cjs candidate ./pyro
    node smoke.cjs
    node cache-lifecycle.cjs candidate ./pyro
    node opfs-lifecycle.cjs candidate ./pyro
    node idb-lifecycle.cjs candidate ./pyro
    node quota.cjs candidate ./pyro
    node options.cjs candidate ./pyro
    node workers.cjs candidate ./pyro
    node partition.cjs candidate ./pyro
    node bucket-webidl.cjs candidate ./pyro
    node default-estimate.cjs candidate ./pyro
    node worker-data.cjs candidate ./pyro
    node idb-close.cjs candidate ./pyro
    node idb-active.cjs candidate ./pyro
    python3 run.py candidate
    python3 report.py
    python3 classify.py
    python3 families.py
    ./native.sh

The raw receipts and complete URL/name/occurrence and strict-page comparisons preserve missing names, harness status and timeouts; these focused totals are distinct from the shipped standalone census.

## Results and excluded coverage

| Selection | LP parent pass / observed | LP candidate pass / observed | Signed Chrome pass / observed | Strict pages parent / candidate / Chrome |
| --- | ---: | ---: | ---: | --- |
| Initial 28 targets at e84de9fc6 | 68/379 | 366/379 | 367/379 | 0 / 23 / 24 |
| Final 27 targets at c438701f1 | 68/377 | 365/377 | 365/377 | 0 / 23 / 23 |
| Same 28 StorageManager guards | 87/103 | 87/103 | 103/103 | 14 / 14 / 28 |

The original 56-page comparison gained 298 observed passes without any losses or missing names. Its raw candidate and complete ledgers are in initial56/. One opaque-origin URL recorded an unresolved popup authorization discrepancy. A subsequent source check makes bucket selection honor the document's already-established opaque-origin policy. The broader popup/navigation policy is unverified. That URL was excluded from both final browser runs, and no fix for its earlier outcome is claimed.

The final 55-page selector has SHA-256 f77a5b120aafd0c4bccf1dc90181fce9aa035752ec2d851a083df6b05b6ac87e. It gained 297 previously observed signed passes with no losses, missing names or new observations. The 27 retained target pages have identical named and strict pass sets to signed Chrome. The 12 shared non-passes exercise obsolete durability-related assertions. Sixteen signed-only names remain in the neighboring guards. Across the 55-page comparison the parent had 51 complete pages and four errors, the candidate had 52 complete pages and three errors, and Chrome completed 55. No restarts, watchdog kills or timeouts occurred.

Nine of the 15 direct control groups matched signed Chrome exactly, including equivalent default stores, cross-worker data roundtrips, per-storage-key isolation, old OPFS writers, unlinked caches, endpoint brands and the confirmed-active IDB transaction. The other six groups document these distinctions:

- Both name-length controls observe the candidate accepting the draft-valid 64-character name. Signed accepts 63 but rejects 64 and 65.
- The two immediate-delete IDB probes permit a transaction and bucket deletion to race. The ordinary candidate transaction completed before retirement, while signed aborted it with UnknownError. LP's close event was delivered after the immediate observation; both the close-event listener and onclose handler ran by the later check. The stale factory returned an asynchronous error; the original signed direct run observed AbortError, a second signed timing probe observed UnknownError, and LP reported AbortError. In the control that kept the transaction genuinely active across deletion, both engines aborted with UnknownError, delivered a close event, rejected a stale database transaction with InvalidStateError and provided an empty replacement database namespace.
- Both engines rejected two large IDB writes atomically, left zero records, enforced the cross-endpoint quota and accepted the retry after deleting the OPFS file. The LP logical ledger accepted an immediate smaller IDB retry after the aborted transaction; Chrome rejected it in this measurement. The result demonstrates LP's reservation refund, not byte-identical physical storage accounting.
- The default-estimate probe confirms the preexisting omission of default CacheStorage and IndexedDB from LP's origin estimate. Signed includes both. The named contribution is independently verified and isolated.

Fifty-three native tests passed on the final candidate: five catalog, two CacheStorage, four IndexedDB and seven OPFS backend tests, plus 27 IndexedDB and eight Storage integration tests. Independent QA reproduced the complete 55-page occurrence/status and lifecycle/strict page ledgers, and all 15 direct output files. The archived parent and signed receipts contain their original 56 URLs; report.py limits reported counts to the final selection. Original WPT files and expectations were preserved.
