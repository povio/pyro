# IndexedDB

`window.indexedDB` (and `self.indexedDB` in dedicated workers) is an in-memory
implementation of the Indexed Database API 3.0, with the `getAll(options)` /
`getAllRecords()` additions Chrome ships. It is meant to make Dexie, idb,
idb-keyval, localForage, Firebase (Auth persistence, Installations, heartbeat)
and Playwright's `storageState({indexedDB: true})` work as in Chrome.

## Storage

- Databases belong to the context's local `StorageNamespace`, per origin, the
  same partition as `localStorage`: shared by every document, world and frame
  of the browser context and by the dedicated workers they start, isolated
  from other contexts and origins, and kept across navigations. Nothing is
  written to disk; a new context starts empty.
- Records are kept natively in IndexedDB key order: idb.js encodes keys into
  ASCII strings whose byte order is key order (number < date < string <
  binary < array, strings by code unit, arrays element-wise). Values are
  stored in the worker wire format after a structured clone, so each read
  returns fresh objects. Blob and File values are not cloneable here and throw
  `DataCloneError` (localForage detects that and falls back to base64).
- `indexedDB.databases()` lists the partition's databases with a version.
- Opaque origins (`data:`, sandboxed documents) throw `SecurityError` from
  `open`/`deleteDatabase` and reject `databases()`; the `IDB*` interfaces still
  exist for feature detection, and `cmp` works (it touches no storage).

## Behaviour

- Requests run in order per transaction, each in its own task; its `success`
  or `error` event is dispatched natively, so microtasks run after every
  listener while the transaction is still active: `await` between requests
  (idb, Dexie) keeps the transaction alive, as in browsers.
- Events propagate request → transaction → connection (capture and bubble).
  An `error` event that is not canceled aborts the transaction with the
  request's error; an exception thrown by a `success`/`error`/`upgradeneeded`
  listener aborts it with `AbortError` (an exception in a listener of another
  event that such a listener dispatches does not).
- Transactions auto-commit once inactive with no pending requests;
  `commit()`, `abort()` and `durability` are supported. Aborting rolls back
  records, key generator values and, for upgrades, the version and the whole
  schema (`objectStoreNames`, store and index names revert on the objects).
- Scheduling: overlapping transactions start in creation order (readonly ones
  run in parallel, readwrite ones wait); across documents and workers a
  conflicting transaction waits until the other one finishes.
- `open` with a higher version fires `versionchange` at every other open
  connection (any document or worker of the context), `blocked` at the
  request if some stay open, then `upgradeneeded`. `deleteDatabase` does the
  same with a null new version. Requests for one database are queued.
- Key paths (identifiers, dotted paths, arrays, `length` of strings and
  arrays), key injection for `autoIncrement` stores, the key generator (up to
  2^53, explicit numeric keys bump it), unique and multiEntry indexes,
  cursors in all four directions with `continue`, `continuePrimaryKey`,
  `advance`, `update` and `delete`, `IDBKeyRange` and `indexedDB.cmp` follow
  the spec, with Chrome's exception names and order.
- A document going away closes its connections and aborts its running
  transactions, which unblocks upgrades waiting in other documents.

## Cost

The API objects and state machine are idb.js, compiled on a realm's first
IndexedDB use (`indexedDB.open`, `cmp`, `deleteDatabase` or reading an `IDB*`
global). Before that a realm has only the `indexedDB` getter and one lazy
accessor per interface name, like the streams interfaces. Event dispatch pays
one emptiness check for the non-node parent table, document disposal one
check of an empty thread-local table.

## Gaps

- An upgrade snapshots the database's stores for rollback, so a version
  change copies the record index (values are shared, not copied).
- No disk persistence and no CDP `IndexedDB` domain or
  `Storage.clearDataForOrigin` for IndexedDB.
- Blob/File values (above).
- A new transaction becomes inactive in the next task rather than right after
  its creating task's microtask checkpoint (so it is also still active in
  later listeners of the same event dispatch).
- Across documents and workers, conflicting transactions start first come
  first served, not strictly in creation order.
