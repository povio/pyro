# IndexedDB discovery

`window.indexedDB` is a per-realm `IDBFactory` wrapper. `databases()` returns a
Promise with a fresh snapshot of the context-and-origin database catalog. The
catalog belongs to the existing context-owned local `StorageNamespace`, so the
browser's current sharing across documents/worlds and isolation across contexts
also applies here. No new browser setup API is required. Opaque origins reject
with `SecurityError`; a captured factory rejects after document disposal and
holds only a weak document reference.

This is discovery support, **not an IndexedDB database implementation**. This
build cannot create or import databases, so production catalogs remain empty.
`open`, `deleteDatabase` and `cmp` fail explicitly with `NotSupportedError`.
Object stores, transactions, key comparison, persistence and nonempty database
restoration are not implemented. Adding those operations must update this same
catalog; discovery must never hide existing databases.

The first unchanged TOS profile smoke (TOS f9fde4aa0d, browser a7dfe98) failed
before page assertions because Playwright 1.59.1 storage-state restoration calls
`indexedDB.databases()` even when only localStorage entries are supplied. The
new binding permits that empty-database path. Nonempty IndexedDB state still
fails explicitly through `open`; it is not reported as restored or silently
discarded. The original failing smoke remains separate evidence; this primitive
alone does not establish TOS compatibility.

Focused tests cover fresh snapshots, context/origin partitioning, the empty
storage-state/localStorage restore sequence, explicit nonempty-restore failure
without clearing existing localStorage, opaque origins and document disposal.
