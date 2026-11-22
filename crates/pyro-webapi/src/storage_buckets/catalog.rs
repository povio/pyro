//! Named storage buckets for one browser context. A lease addresses a
//! particular generation on its storage shelf, independently of its name.
//! Backend operations use the shelf ledger while holding their own locks;
//! the ledger never calls back into those backends.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use rustc_hash::FxHashMap;

use crate::{cache_storage, indexeddb, opfs};

pub(crate) const DEFAULT_QUOTA: u64 = 10 * 1024 * 1024 * 1024;
const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn now_ms() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs_f64() * 1000.0
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct StorageKey {
    pub(crate) origin: String,
    pub(crate) top_level_site: Option<String>,
    pub(crate) cross_site_ancestor: bool,
}

#[derive(Default)]
pub(crate) struct Options {
    pub(crate) quota: Option<u64>,
    pub(crate) expires: Option<f64>,
    pub(crate) persisted: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Error {
    InvalidName,
    InvalidQuota,
    InvalidExpiration,
    Removed,
    QuotaExceeded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Endpoint {
    IndexedDb,
    Caches,
    FileSystem,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Estimate {
    pub(crate) quota: u64,
    pub(crate) usage: u64,
    pub(crate) indexed_db: u64,
    pub(crate) caches: u64,
    pub(crate) file_system: u64,
}

impl Estimate {
    fn endpoint_mut(&mut self, endpoint: Endpoint) -> &mut u64 {
        match endpoint {
            Endpoint::IndexedDb => &mut self.indexed_db,
            Endpoint::Caches => &mut self.caches,
            Endpoint::FileSystem => &mut self.file_system,
        }
    }
}

struct Account {
    estimate: Estimate,
    reserved: u64,
    expires: Option<f64>,
}

impl Account {
    fn expired(&self, now: f64) -> bool {
        self.expires.is_some_and(|expires| expires < now)
    }
}

#[derive(Default)]
struct Ledger {
    next_generation: u64,
    accounts: FxHashMap<u64, Account>,
    /// Committed bytes plus unused reservations across all named buckets.
    claimed: u64,
}

impl Ledger {
    fn remove(&mut self, generation: u64) {
        if let Some(account) = self.accounts.remove(&generation) {
            self.claimed -= account.estimate.usage + account.reserved;
        }
    }

    fn live(&mut self, generation: u64, now: f64) -> Result<&mut Account, Error> {
        if self.accounts.get(&generation).is_some_and(|account| account.expired(now)) {
            self.remove(generation);
        }
        self.accounts.get_mut(&generation).ok_or(Error::Removed)
    }

    fn expire_others(&mut self, now: f64) {
        let claimed = &mut self.claimed;
        self.accounts.retain(|_, account| {
            if account.expired(now) {
                *claimed -= account.estimate.usage + account.reserved;
                false
            } else {
                true
            }
        });
    }

    /// Apply a quota transition atomically, including transfer of previously
    /// reserved bytes to committed endpoint usage. Unsatisfied transitions
    /// leave both old usage and reservations intact.
    fn adjust(
        &mut self,
        generation: u64,
        endpoint: Endpoint,
        removed: u64,
        added: u64,
        unreserve: u64,
        reserve: u64,
    ) -> Result<(), Error> {
        let now = now_ms();
        let account = self.live(generation, now)?;
        let mut estimate = account.estimate;
        let used = estimate.endpoint_mut(endpoint);
        *used = used.checked_sub(removed).and_then(|v| v.checked_add(added)).ok_or(Error::QuotaExceeded)?;
        estimate.usage =
            estimate.usage.checked_sub(removed).and_then(|v| v.checked_add(added)).ok_or(Error::QuotaExceeded)?;
        let reserved =
            account.reserved.checked_sub(unreserve).and_then(|v| v.checked_add(reserve)).ok_or(Error::QuotaExceeded)?;
        let new_claim =
            estimate.usage.checked_add(reserved).filter(|v| *v <= estimate.quota).ok_or(Error::QuotaExceeded)?;
        let old_claim = account.estimate.usage + account.reserved;
        if self.claimed - old_claim > DEFAULT_QUOTA - new_claim {
            self.expire_others(now);
            if self.claimed - old_claim > DEFAULT_QUOTA - new_claim {
                return Err(Error::QuotaExceeded);
            }
        }
        self.claimed = self.claimed - old_claim + new_claim;
        let account = self.accounts.get_mut(&generation).expect("live account");
        account.estimate = estimate;
        account.reserved = reserved;
        Ok(())
    }
}

#[derive(Clone)]
pub(crate) struct Lease {
    ledger: Arc<Mutex<Ledger>>,
    generation: u64,
}

impl Lease {
    pub(crate) fn check(&self) -> Result<(), Error> {
        lock(&self.ledger).live(self.generation, now_ms()).map(|_| ())
    }

    pub(crate) fn charge(&self, endpoint: Endpoint, removed: u64, added: u64) -> Result<(), Error> {
        lock(&self.ledger).adjust(self.generation, endpoint, removed, added, 0, 0)
    }

    pub(crate) fn reserve(&self, endpoint: Endpoint, bytes: u64) -> Result<Reservation, Error> {
        lock(&self.ledger).adjust(self.generation, endpoint, 0, 0, 0, bytes)?;
        Ok(Reservation { lease: self.clone(), endpoint, bytes })
    }
}

/// Private staged bytes are not visible in estimate(), but count against
/// both bucket and shelf quotas. Aborting a write drops its reservation.
pub(crate) struct Reservation {
    lease: Lease,
    endpoint: Endpoint,
    bytes: u64,
}

impl Reservation {
    pub(crate) fn bytes(&self) -> u64 {
        self.bytes
    }

    pub(crate) fn resize(&mut self, bytes: u64) -> Result<(), Error> {
        lock(&self.lease.ledger).adjust(self.lease.generation, self.endpoint, 0, 0, self.bytes, bytes)?;
        self.bytes = bytes;
        Ok(())
    }

    pub(crate) fn commit(mut self, removed: u64, added: u64) -> Result<(), Error> {
        if added.saturating_sub(removed) > self.bytes {
            return Err(Error::QuotaExceeded);
        }
        lock(&self.lease.ledger).adjust(self.lease.generation, self.endpoint, removed, added, self.bytes, 0)?;
        self.bytes = 0;
        Ok(())
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if self.bytes > 0 {
            // Removal/expiry has already released this generation's claims.
            let _ = lock(&self.lease.ledger).adjust(self.lease.generation, self.endpoint, 0, 0, self.bytes, 0);
        }
    }
}

pub(crate) struct Bucket {
    pub(crate) name: String,
    pub(crate) origin: String,
    pub(crate) lease: Lease,
    pub(crate) indexed_db: indexeddb::Namespace,
    pub(crate) caches: cache_storage::Namespace,
    pub(crate) opfs: opfs::Namespace,
}

impl Bucket {
    pub(crate) fn estimate(&self) -> Result<Estimate, Error> {
        Ok(lock(&self.lease.ledger).live(self.lease.generation, now_ms())?.estimate)
    }

    pub(crate) fn persisted(&self) -> Result<bool, Error> {
        self.lease.check()?;
        Ok(false)
    }

    pub(crate) fn expires(&self) -> Result<Option<f64>, Error> {
        Ok(lock(&self.lease.ledger).live(self.lease.generation, now_ms())?.expires)
    }

    pub(crate) fn set_expires(&self, expires: f64) -> Result<(), Error> {
        if !expires.is_finite() {
            return Err(Error::InvalidExpiration);
        }
        lock(&self.lease.ledger).live(self.lease.generation, now_ms())?.expires = Some(expires);
        Ok(())
    }

    fn retire_backends(&self) {
        self.caches.retire();
        self.indexed_db.retire();
        self.opfs.retire();
    }
}

#[derive(Default)]
struct Shelf {
    buckets: BTreeMap<String, Arc<Bucket>>,
    ledger: Arc<Mutex<Ledger>>,
}

fn validate_name(name: &str) -> Result<(), Error> {
    let valid = !name.is_empty()
        && name.len() <= 64
        && !name.starts_with(['-', '_'])
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_'));
    if valid {
        Ok(())
    } else {
        Err(Error::InvalidName)
    }
}

#[derive(Clone, Default)]
pub(crate) struct Catalog(Arc<Mutex<FxHashMap<StorageKey, Shelf>>>);

impl Catalog {
    pub(crate) fn open(&self, key: &StorageKey, name: &str, options: Options) -> Result<Arc<Bucket>, Error> {
        validate_name(name)?;
        let now = now_ms();
        if options.expires.is_some_and(|v| !v.is_finite() || v < now) {
            return Err(Error::InvalidExpiration);
        }
        if options.quota.is_some_and(|v| v == 0 || v > MAX_SAFE_INTEGER) {
            return Err(Error::InvalidQuota);
        }
        // As for the default StorageManager, this ephemeral context cannot
        // grant a request for persistent storage.
        let _requested_persistence = options.persisted;
        let mut shelves = lock(&self.0);
        let shelf = shelves.entry(key.clone()).or_default();
        let mut ledger = lock(&shelf.ledger);
        if let Some(bucket) = shelf.buckets.get(name) {
            if let Ok(account) = ledger.live(bucket.lease.generation, now) {
                if let Some(expires) = options.expires {
                    account.expires = Some(expires);
                }
                return Ok(bucket.clone());
            }
        }
        let generation = ledger.next_generation;
        ledger.next_generation = generation.checked_add(1).expect("storage bucket generations exhausted");
        let quota = options.quota.unwrap_or(DEFAULT_QUOTA).min(DEFAULT_QUOTA);
        ledger.accounts.insert(
            generation,
            Account { estimate: Estimate { quota, ..Estimate::default() }, reserved: 0, expires: options.expires },
        );
        drop(ledger);
        let lease = Lease { ledger: shelf.ledger.clone(), generation };
        let bucket = Arc::new(Bucket {
            name: name.into(),
            origin: key.origin.clone(),
            indexed_db: indexeddb::Namespace::for_bucket(lease.clone()),
            caches: cache_storage::Namespace::for_bucket(lease.clone()),
            opfs: opfs::Namespace::for_bucket(lease.clone()),
            lease,
        });
        let retired = shelf.buckets.insert(name.into(), bucket.clone());
        drop(shelves);
        if let Some(previous) = retired {
            previous.retire_backends();
        }
        Ok(bucket)
    }

    pub(crate) fn delete(&self, key: &StorageKey, name: &str) -> Result<(), Error> {
        validate_name(name)?;
        let mut shelves = lock(&self.0);
        let Some(shelf) = shelves.get_mut(key) else { return Ok(()) };
        let Some(bucket) = shelf.buckets.remove(name) else { return Ok(()) };
        lock(&shelf.ledger).remove(bucket.lease.generation);
        if shelf.buckets.is_empty() {
            shelves.remove(key);
        }
        drop(shelves);
        bucket.retire_backends();
        Ok(())
    }

    /// Take a single coherent snapshot. Reclaimed namespaces are retired
    /// outside both locks so a backend may finish an operation already holding
    /// its own lock without ever inverting the ledger/backend lock order.
    fn snapshot<R>(&self, key: &StorageKey, read: impl FnOnce(&Shelf, &Ledger) -> R) -> Option<R> {
        let mut shelves = lock(&self.0);
        let shelf = shelves.get_mut(key)?;
        let mut ledger = lock(&shelf.ledger);
        ledger.expire_others(now_ms());
        let mut retired = Vec::new();
        shelf.buckets.retain(|_, bucket| {
            let keep = ledger.accounts.contains_key(&bucket.lease.generation);
            if !keep {
                retired.push(bucket.clone());
            }
            keep
        });
        let result = read(shelf, &ledger);
        drop(ledger);
        if shelf.buckets.is_empty() {
            shelves.remove(key);
        }
        drop(shelves);
        for bucket in retired {
            bucket.retire_backends();
        }
        Some(result)
    }

    pub(crate) fn keys(&self, key: &StorageKey) -> Vec<String> {
        self.snapshot(key, |shelf, _| shelf.buckets.keys().cloned().collect()).unwrap_or_default()
    }

    pub(crate) fn usage(&self, key: &StorageKey) -> Estimate {
        let mut total = Estimate { quota: DEFAULT_QUOTA, ..Estimate::default() };
        let _ = self.snapshot(key, |_, ledger| {
            for account in ledger.accounts.values() {
                let estimate = &account.estimate;
                total.usage += estimate.usage;
                total.caches += estimate.caches;
                total.indexed_db += estimate.indexed_db;
                total.file_system += estimate.file_system;
            }
        });
        total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> StorageKey {
        StorageKey {
            origin: "https://example.test".to_owned(),
            top_level_site: Some("https://example.test".to_owned()),
            cross_site_ancestor: false,
        }
    }

    #[test]
    fn names_reopen_and_recreation_are_generation_scoped() {
        let catalog = Catalog::default();
        let key = key();
        let initial = Options { quota: Some(100), expires: Some(now_ms() + 86400000.0), persisted: true };
        let original = catalog.open(&key, "z", initial).unwrap();
        let second = catalog.open(&key, "z", Options { quota: Some(3), ..Default::default() }).unwrap();
        assert!(Arc::ptr_eq(&original, &second));
        assert_eq!(second.estimate().unwrap().quota, 100);
        assert!(!second.persisted().unwrap());
        assert!(second.expires().unwrap().is_some());

        let name = "a".repeat(64);
        catalog.open(&key, &name, Default::default()).unwrap();
        assert_eq!(catalog.open(&key, &"a".repeat(65), Default::default()).err(), Some(Error::InvalidName));
        assert_eq!(catalog.delete(&key, "-bad"), Err(Error::InvalidName));
        assert_eq!(catalog.keys(&key), [name, "z".to_owned()]);
        original.lease.charge(Endpoint::Caches, 0, 75).unwrap();
        let pending = original.lease.reserve(Endpoint::FileSystem, 20).unwrap();
        catalog.delete(&key, "z").unwrap();
        assert_eq!(pending.commit(0, 20), Err(Error::Removed));
        let remade = catalog.open(&key, "z", Default::default()).unwrap();
        assert!(!Arc::ptr_eq(&remade, &original));
        assert_eq!(second.estimate().err(), Some(Error::Removed));
        assert_eq!(remade.estimate().unwrap().usage, 0);
        assert_eq!(remade.estimate().unwrap().quota, DEFAULT_QUOTA);

        remade.set_expires(1.0).unwrap();
        assert_eq!(remade.lease.check(), Err(Error::Removed));
        assert!(!catalog.keys(&key).iter().any(|name| name == "z"));
        assert_eq!(
            catalog.open(&key, "old", Options { expires: Some(1.0), ..Default::default() }).err(),
            Some(Error::InvalidExpiration)
        );
        assert_eq!(
            catalog.open(&key, "zero", Options { quota: Some(0), ..Default::default() }).err(),
            Some(Error::InvalidQuota)
        );
    }

    #[test]
    fn quota_reservations_cover_endpoints_and_remain_invisible_until_commit() {
        let catalog = Catalog::default();
        let key = key();
        let bucket = catalog.open(&key, "account", Options { quota: Some(10), ..Default::default() }).unwrap();
        let mut pending = bucket.lease.reserve(Endpoint::IndexedDb, 6).unwrap();
        bucket.lease.charge(Endpoint::FileSystem, 0, 4).unwrap();
        assert_eq!(bucket.lease.charge(Endpoint::Caches, 0, 1), Err(Error::QuotaExceeded));
        assert_eq!(pending.resize(7), Err(Error::QuotaExceeded));
        assert_eq!(pending.bytes(), 6);
        assert_eq!(catalog.usage(&key).usage, 4);
        pending.resize(5).unwrap();
        bucket.lease.charge(Endpoint::Caches, 0, 1).unwrap();
        pending.commit(0, 5).unwrap();
        assert_eq!(
            bucket.estimate().unwrap(),
            Estimate { quota: 10, usage: 10, indexed_db: 5, file_system: 4, caches: 1 }
        );
        bucket.lease.reserve(Endpoint::IndexedDb, 0).unwrap().commit(5, 2).unwrap();
        let aborted = bucket.lease.reserve(Endpoint::FileSystem, 3).unwrap();
        assert_eq!(bucket.lease.charge(Endpoint::Caches, 0, 1), Err(Error::QuotaExceeded));
        drop(aborted);
        bucket.lease.charge(Endpoint::Caches, 0, 3).unwrap();
        assert_eq!(catalog.usage(&key).usage, 10);
    }

    #[test]
    fn separate_contexts_origins_and_top_level_sites_cannot_share_buckets() {
        let catalog = Catalog::default();
        let key = key();
        let mut others = [key.clone(), key.clone(), key.clone()];
        others[0].origin = "https://other.test".to_owned();
        others[1].top_level_site = Some("https://other.test".to_owned());
        others[2].cross_site_ancestor = true;
        let bucket = catalog.open(&key, "shared", Default::default()).unwrap();
        let worker = catalog.clone();
        let worker_key = key.clone();
        std::thread::spawn(move || {
            let shared = worker.open(&worker_key, "shared", Default::default()).unwrap();
            shared.lease.charge(Endpoint::Caches, 0, 42).unwrap();
        })
        .join()
        .unwrap();
        assert_eq!(bucket.estimate().unwrap().usage, 42);
        for other in others {
            assert!(catalog.keys(&other).is_empty());
            assert_eq!(catalog.open(&other, "shared", Default::default()).unwrap().estimate().unwrap().usage, 0);
        }
        assert!(Catalog::default().keys(&key).is_empty());
    }

    #[test]
    fn workers_open_one_generation_and_compete_for_its_remaining_quota() {
        let catalog = Catalog::default();
        let barrier = Arc::new(std::sync::Barrier::new(8));
        let workers: Vec<_> = (0..8)
            .map(|index| {
                let catalog = catalog.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    let bucket =
                        catalog.open(&key(), "race", Options { quota: Some(4), ..Default::default() }).unwrap();
                    let endpoint = if index % 2 == 0 { Endpoint::Caches } else { Endpoint::IndexedDb };
                    let result = bucket.lease.charge(endpoint, 0, 1);
                    (bucket, result)
                })
            })
            .collect();
        let bucket = catalog.open(&key(), "race", Options { quota: Some(4), ..Default::default() }).unwrap();
        let mut successes = 0;
        for worker in workers {
            let (other, result) = worker.join().unwrap();
            assert!(Arc::ptr_eq(&other, &bucket));
            match result {
                Ok(()) => successes += 1,
                Err(error) => assert_eq!(error, Error::QuotaExceeded),
            }
        }
        assert_eq!(successes, 4);
        let estimate = bucket.estimate().unwrap();
        assert_eq!(estimate.usage, 4);
        assert_eq!(estimate.indexed_db + estimate.caches, 4);
    }

    #[test]
    fn one_shelf_quota_limits_concurrent_bucket_reservations() {
        let catalog = Catalog::default();
        let key = key();
        let a = catalog.open(&key, "a", Default::default()).unwrap();
        let b = catalog.open(&key, "b", Default::default()).unwrap();
        let reservation = a.lease.reserve(Endpoint::IndexedDb, DEFAULT_QUOTA).unwrap();
        assert_eq!(b.lease.charge(Endpoint::Caches, 0, 1), Err(Error::QuotaExceeded));
        a.set_expires(0.0).unwrap();
        b.lease.charge(Endpoint::FileSystem, 0, 1).unwrap();
        assert_eq!(reservation.commit(0, 1), Err(Error::Removed));
        assert_eq!(catalog.usage(&key).usage, 1);
        assert_eq!(catalog.keys(&key), ["b".to_owned()]);
    }
}
