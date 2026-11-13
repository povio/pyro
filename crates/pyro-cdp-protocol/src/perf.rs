//! Opt-in stage timers for CDP handling (`--features perf-trace`).
//!
//! Each stage of a command (socket decode, hop to the browser thread, parse,
//! dispatch, JavaScript, serialization, hop back, socket write) records its
//! duration under a name. `Pyro.perfReset` clears the counters and
//! `Pyro.perfStats` returns them, so `bench/scripts/cdp-perf.mjs --perf-trace`
//! can attribute inner costs to each benchmark scenario.
//!
//! Without the feature every function here is an empty inline stub and the
//! guards are zero-sized, so normal builds carry no timing code.
//!
//! Stages nest (e.g. `cdp.dispatch Runtime.evaluate` contains `js.eval`), so
//! totals are inclusive. Every recorded stage also pays the cost of the
//! timers nested inside it; `Pyro.perfStats` reports that per-record overhead.

#[cfg(feature = "perf-trace")]
mod imp {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    /// Quarter-octave histogram buckets: values below 4 ns are exact, then
    /// four buckets per power of two up to 2^40 ns.
    const BUCKETS: usize = 160;

    #[derive(Clone)]
    pub(super) struct Stat {
        pub(super) count: u64,
        pub(super) total_ns: u64,
        pub(super) max_ns: u64,
        pub(super) buckets: [u64; BUCKETS],
    }

    impl Default for Stat {
        fn default() -> Self {
            Self { count: 0, total_ns: 0, max_ns: 0, buckets: [0; BUCKETS] }
        }
    }

    pub(super) fn bucket(ns: u64) -> usize {
        if ns < 4 {
            return ns as usize;
        }
        let e = 63 - ns.leading_zeros() as usize;
        let m = ((ns >> (e - 2)) & 3) as usize;
        (e * 4 + m).min(BUCKETS - 1)
    }

    /// Lower bound of a bucket in nanoseconds.
    pub(super) fn bucket_floor(i: usize) -> u64 {
        if i < 4 {
            return i as u64;
        }
        // Indexes 4..8 are never produced: values from 4 ns start at index 8.
        let (e, m) = ((i / 4).max(2), (i % 4) as u64);
        (4 + m) << (e - 2)
    }

    impl Stat {
        fn add(&mut self, ns: u64) {
            self.count += 1;
            self.total_ns += ns;
            self.max_ns = self.max_ns.max(ns);
            self.buckets[bucket(ns)] += 1;
        }

        fn merge(&mut self, other: &Stat) {
            self.count += other.count;
            self.total_ns += other.total_ns;
            self.max_ns = self.max_ns.max(other.max_ns);
            for (a, b) in self.buckets.iter_mut().zip(other.buckets.iter()) {
                *a += b;
            }
        }

        /// Approximate quantile: the floor of the bucket holding it.
        pub(super) fn quantile(&self, q: f64) -> u64 {
            let rank = ((self.count as f64) * q).ceil().max(1.0) as u64;
            let mut seen = 0;
            for (i, n) in self.buckets.iter().enumerate() {
                seen += n;
                if seen >= rank {
                    return bucket_floor(i);
                }
            }
            self.max_ns
        }
    }

    type Table = Arc<Mutex<HashMap<String, Stat>>>;

    /// Every thread's table; each thread only locks its own while recording,
    /// so the lock is uncontended except during a snapshot or reset.
    static TABLES: Mutex<Vec<Table>> = Mutex::new(Vec::new());

    thread_local! {
        static LOCAL: Table = {
            let table: Table = Arc::default();
            TABLES.lock().unwrap_or_else(|e| e.into_inner()).push(table.clone());
            table
        };
    }

    pub(super) fn record(name: &str, ns: u64) {
        // Ignore records during thread teardown, when LOCAL is gone.
        let _ = LOCAL.try_with(|table| {
            let mut map = table.lock().unwrap_or_else(|e| e.into_inner());
            match map.get_mut(name) {
                Some(stat) => stat.add(ns),
                None => map.entry(name.to_owned()).or_default().add(ns),
            }
        });
    }

    pub(super) fn reset() {
        for table in TABLES.lock().unwrap_or_else(|e| e.into_inner()).iter() {
            table.lock().unwrap_or_else(|e| e.into_inner()).clear();
        }
    }

    pub(super) fn merged() -> HashMap<String, Stat> {
        let mut all: HashMap<String, Stat> = HashMap::new();
        for table in TABLES.lock().unwrap_or_else(|e| e.into_inner()).iter() {
            for (name, stat) in table.lock().unwrap_or_else(|e| e.into_inner()).iter() {
                all.entry(name.clone()).or_default().merge(stat);
            }
        }
        all
    }

    /// Mean cost of one `Instant::now()` pair and of one timed record, so
    /// readers know the floor below which stage numbers are noise.
    pub(super) fn overhead() -> (f64, f64) {
        const N: u32 = 2000;
        let t = Instant::now();
        for _ in 0..N {
            std::hint::black_box(Instant::now());
        }
        let clock = t.elapsed().as_nanos() as f64 / f64::from(N);
        thread_local! {
            static SCRATCH: RefCell<Stat> = RefCell::new(Stat::default());
        }
        let t = Instant::now();
        for _ in 0..N {
            let s = Instant::now();
            let ns = s.elapsed().as_nanos() as u64;
            // Same lookup and lock path as `record`, into a throwaway stat.
            LOCAL.with(|table| {
                let map = table.lock().unwrap_or_else(|e| e.into_inner());
                std::hint::black_box(map.get("perf.overhead"));
            });
            SCRATCH.with(|scratch| scratch.borrow_mut().add(ns));
        }
        let record = t.elapsed().as_nanos() as f64 / f64::from(N);
        (clock, record)
    }
}

/// Start time of a stage; zero-sized when timing is compiled out.
#[derive(Clone, Copy)]
pub struct Stamp {
    #[cfg(feature = "perf-trace")]
    at: std::time::Instant,
}

#[inline(always)]
pub fn now() -> Stamp {
    Stamp {
        #[cfg(feature = "perf-trace")]
        at: std::time::Instant::now(),
    }
}

/// Records the time since `start` under `name`.
#[inline(always)]
pub fn record_since(name: &str, start: Stamp) {
    #[cfg(feature = "perf-trace")]
    imp::record(name, start.at.elapsed().as_nanos() as u64);
    #[cfg(not(feature = "perf-trace"))]
    let _ = (name, start);
}

/// Records the time until it is dropped.
#[must_use]
pub struct Span {
    #[cfg(feature = "perf-trace")]
    name: std::borrow::Cow<'static, str>,
    #[cfg(feature = "perf-trace")]
    start: std::time::Instant,
}

#[inline(always)]
pub fn span(name: &'static str) -> Span {
    #[cfg(not(feature = "perf-trace"))]
    let _ = name;
    Span {
        #[cfg(feature = "perf-trace")]
        name: std::borrow::Cow::Borrowed(name),
        #[cfg(feature = "perf-trace")]
        start: std::time::Instant::now(),
    }
}

/// A span whose name is built only when timing is compiled in.
#[inline(always)]
pub fn span_with(name: impl FnOnce() -> String) -> Span {
    #[cfg(not(feature = "perf-trace"))]
    let _ = name;
    Span {
        #[cfg(feature = "perf-trace")]
        name: std::borrow::Cow::Owned(name()),
        #[cfg(feature = "perf-trace")]
        start: std::time::Instant::now(),
    }
}

#[cfg(feature = "perf-trace")]
impl Drop for Span {
    fn drop(&mut self) {
        imp::record(&self.name, self.start.elapsed().as_nanos() as u64);
    }
}

/// `Pyro.perfReset`: clears every thread's counters.
#[cfg(feature = "perf-trace")]
pub fn reset() {
    imp::reset();
}

/// `Pyro.perfStats`: all stages, summed over threads, sorted by total time.
#[cfg(feature = "perf-trace")]
pub fn snapshot() -> serde_json::Value {
    let (clock, record) = imp::overhead();
    let mut stages: Vec<_> = imp::merged().into_iter().collect();
    stages.sort_by(|a, b| b.1.total_ns.cmp(&a.1.total_ns));
    let stages: Vec<_> = stages
        .into_iter()
        .map(|(name, s)| {
            serde_json::json!({
                "name": name,
                "count": s.count,
                "totalNs": s.total_ns,
                "meanNs": s.total_ns as f64 / s.count.max(1) as f64,
                "p50Ns": s.quantile(0.5),
                "p99Ns": s.quantile(0.99),
                "maxNs": s.max_ns,
            })
        })
        .collect();
    serde_json::json!({ "clockNs": clock, "recordNs": record, "stages": stages })
}

#[cfg(all(test, feature = "perf-trace"))]
mod tests {
    use super::*;

    #[test]
    fn buckets_are_within_a_quarter_octave() {
        for ns in [0u64, 3, 4, 5, 7, 8, 100, 1000, 123_456, 10_000_000] {
            let floor = imp::bucket_floor(imp::bucket(ns));
            assert!(floor <= ns && ns - floor <= floor / 4, "{ns} -> {floor}");
        }
    }

    #[test]
    fn spans_nest_and_snapshot_sorts_by_total() {
        {
            let _outer = span("test.outer");
            let _inner = span_with(|| "test.inner x".to_owned());
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let snap = snapshot();
        let stages = snap["stages"].as_array().unwrap();
        let names: Vec<_> = stages.iter().map(|s| s["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"test.outer") && names.contains(&"test.inner x"), "{names:?}");
        assert!(stages[0]["totalNs"].as_u64().unwrap() >= 1_000_000);
    }
}
