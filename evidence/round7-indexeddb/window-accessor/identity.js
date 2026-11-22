async function probe() {
  const open = indexedDB.open('initial-worker-idb-identity', 1);
  const results = { factory: Object.getPrototypeOf(indexedDB) === IDBFactory.prototype,
    request: Object.getPrototypeOf(open) === IDBOpenDBRequest.prototype,
    range: Object.getPrototypeOf(IDBKeyRange.only(1)) === IDBKeyRange.prototype };
  return new Promise((resolve, reject) => {
    open.onerror = () => reject(Error(open.error.name));
    open.onupgradeneeded = () => open.result.createObjectStore('objects');
    open.onsuccess = () => {
      const db = open.result, tx = db.transaction('objects'), store = tx.objectStore('objects');
      Object.assign(results, { database: Object.getPrototypeOf(db) === IDBDatabase.prototype,
        transaction: Object.getPrototypeOf(tx) === IDBTransaction.prototype,
        store: Object.getPrototypeOf(store) === IDBObjectStore.prototype,
        ordinaryRequest: Object.getPrototypeOf(store.get(1)) === IDBRequest.prototype });
      db.close();
      resolve(results);
    };
  });
}