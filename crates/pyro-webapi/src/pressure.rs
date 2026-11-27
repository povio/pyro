//! Compute Pressure (https://w3c.github.io/compute-pressure/): `PressureObserver` and `PressureRecord` on windows and
//! dedicated and shared workers, made in JS (pressure.js) on a realm's first use and secure-context only
//! (`crate::secure_context`, the worker install).
//!
//! The data comes from a [`PressureHub`], one per browser context and shared with the context's worker threads
//! (`ContextServices::pressure`). A source is either
//!  - virtual: `Emulation.setPressureSourceOverrideEnabled` makes the context's `cpu` source a virtual one
//!    (WebDriver "create virtual pressure source"), whose reading `Emulation.setPressureStateOverride` /
//!    `setPressureDataOverride` set; the real source is then not consulted; or
//!  - real: the host's CPU utilisation, read from the OS (`host_statistics` on macOS, `/proc/stat` on Linux), at most
//!    once a second, mapped to the four states with Chrome's kind of thresholds and hysteresis. Where the platform has
//!    no such reading the source is unsupported and `observe()` rejects with NotSupportedError.
//!
//! The observers poll the hub from their realm's timers (pressure.js): the hub holds no references into a realm.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use pyro_js::{ClassDef, JsClass, JsResult, Object, Scope, Value};

use crate::lazy_module::LazyModule;
use crate::window::WindowHost;

static MODULE: LazyModule = LazyModule {
    key: "lpPressure",
    source: include_str!("pressure.js"),
    url: "pyro://internal/pressure.js",
    natives: Some(natives),
};

/// `PressureObserver` and `PressureRecord` on a window global.
pub(crate) fn define<W: JsClass>(def: &mut ClassDef<W>)
where
    W::Data: WindowHost,
{
    // Absent from the window of a frame that was removed (WPT compute_pressure_detached_iframe).
    macro_rules! interface {
        ($name:literal) => {
            def.global_value_accessor(
                $name,
                |s, w| {
                    let page = WindowHost::page(w);
                    if !page.is_fully_active() {
                        return Ok(s.undefined());
                    }
                    MODULE.global(s, page, $name)
                },
                |s, w, value| crate::lazy_module::replace(s, WindowHost::page(w), $name, value),
            );
        };
    }
    interface!("PressureObserver");
    interface!("PressureRecord");
}

/// The same on a dedicated or shared worker's global (a secure one: the caller checks).
pub(crate) fn install_worker(s: &Scope<'_>) -> JsResult<()> {
    struct PressureObserver;
    impl pyro_js::LazyGlobalInterface for PressureObserver {
        const NAME: &'static str = "PressureObserver";
        fn value<'s>(s: &Scope<'s>) -> JsResult<Value<'s>> {
            MODULE.module(s, &crate::target::page_of(s)?)?.get_atom(Self::NAME)
        }
    }
    struct PressureRecord;
    impl pyro_js::LazyGlobalInterface for PressureRecord {
        const NAME: &'static str = "PressureRecord";
        fn value<'s>(s: &Scope<'s>) -> JsResult<Value<'s>> {
            MODULE.module(s, &crate::target::page_of(s)?)?.get_atom(Self::NAME)
        }
    }
    s.define_lazy_global_interface::<PressureObserver>()?;
    s.define_lazy_global_interface::<PressureRecord>()
}

/// Permissions Policy "define an inherited policy for feature in container at origin" for `compute-pressure`, whose
/// default allowlist is `'self'`: a child document may observe pressure when its parent document may and its
/// container's `allow` attribute lets its origin (without a `compute-pressure` declaration: when it is at its parent's
/// origin). Origins are ASCII serializations; `None` is opaque and matches only `*`. (The document's own
/// `Permissions-Policy` header is not read.)
pub fn allowed_in_frame(
    parent_allowed: bool,
    allow: Option<&str>,
    parent_origin: Option<&str>,
    declared_origin: Option<&str>,
    child_origin: Option<&str>,
) -> bool {
    parent_allowed
        && crate::fullscreen::container_allows("compute-pressure", allow, parent_origin, declared_origin, child_origin)
            .unwrap_or(parent_origin.is_some() && parent_origin == child_origin)
}

fn hub_of(s: &Scope<'_>) -> JsResult<PressureHub> {
    Ok(crate::target::page_of(s)?.context_services().pressure().clone())
}

fn natives<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    let native = s.object();
    // active(): the realm's document is fully active; allowed(): its permissions policy allows `compute-pressure`.
    native.set_atom("active", s.function("active", |s, _, _| Ok(s.boolean(crate::target::page_of(s)?.is_fully_active())))?.value())?;
    native.set_atom("allowed", s.function("allowed", |s, _, _| Ok(s.boolean(crate::target::page_of(s)?.compute_pressure_allowed())))?.value())?;
    // supported(source): whether the source has a reading to give (a virtual one that is available, or the host's).
    native.set_atom("supported", s.function("supported", |s, _, args| Ok(s.boolean(hub_of(s)?.supported(&args.get(0).to_string()?))))?.value())?;
    // sample(source): the state now, or undefined while there is no reading yet.
    native.set_atom(
        "sample",
        s.function("sample", |s, _, args| {
            Ok(match hub_of(s)?.sample(&args.get(0).to_string()?) {
                Some(state) => s.string(state.as_str()),
                None => s.undefined(),
            })
        })?
        .value(),
    )?;
    // pollMillis(source): how often an observer should look at the source; minIntervalMillis(source): the least time
    // between two records of one observer (a real source is sampled at 1 Hz).
    native.set_atom("pollMillis", s.function("pollMillis", |s, _, args| Ok(s.number(hub_of(s)?.poll_millis(&args.get(0).to_string()?) as f64)))?.value())?;
    native.set_atom(
        "minIntervalMillis",
        s.function("minIntervalMillis", |s, _, args| Ok(s.number(hub_of(s)?.min_interval_millis(&args.get(0).to_string()?) as f64)))?.value(),
    )?;
    Ok(native)
}

/// The `PressureState` values, from the lowest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PressureState {
    Nominal,
    Fair,
    Serious,
    Critical,
}

impl PressureState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Nominal => "nominal",
            Self::Fair => "fair",
            Self::Serious => "serious",
            Self::Critical => "critical",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "nominal" => Self::Nominal,
            "fair" => Self::Fair,
            "serious" => Self::Serious,
            "critical" => Self::Critical,
            _ => return None,
        })
    }
}

/// The context's virtual `cpu` source.
struct VirtualSource {
    /// `{available}` of the creation's metadata: false makes `observe()` reject with NotSupportedError.
    available: bool,
    state: Option<PressureState>,
    /// `setPressureDataOverride`'s `ownContributionEstimate` (0 to 1), kept with the reading.
    own_contribution_estimate: Option<f64>,
}

#[derive(Default)]
struct Inner {
    cpu_virtual: Option<VirtualSource>,
    probe: CpuProbe,
}

/// The pressure sources of a browser context; shared by its pages and workers.
#[derive(Clone, Default)]
pub struct PressureHub(Arc<Mutex<Inner>>);

/// How often an observer looks at a virtual source (an update is seen within this).
const VIRTUAL_POLL_MS: u64 = 25;
/// How often it looks at the host's, and the shortest time between its records: the probe's own rate.
const REAL_SAMPLE_MS: u64 = 1000;

impl PressureHub {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `Emulation.setPressureSourceOverrideEnabled`: `enabled` makes `source` virtual (with no reading, `available`
    /// as the metadata says), else the real source again.
    pub fn set_override(&self, source: &str, enabled: bool, available: bool) -> Result<(), String> {
        if source != "cpu" {
            return Err(format!("Unknown pressure source '{source}'"));
        }
        self.lock().cpu_virtual = enabled.then_some(VirtualSource { available, state: None, own_contribution_estimate: None });
        Ok(())
    }

    /// `Emulation.setPressureStateOverride` / `setPressureDataOverride`: the virtual source's new reading.
    pub fn set_reading(&self, source: &str, state: PressureState, own_contribution_estimate: Option<f64>) -> Result<(), String> {
        if source != "cpu" {
            return Err(format!("Unknown pressure source '{source}'"));
        }
        let mut inner = self.lock();
        let Some(virtual_source) = inner.cpu_virtual.as_mut() else {
            return Err(format!("The pressure source '{source}' is not overridden"));
        };
        virtual_source.state = Some(state);
        virtual_source.own_contribution_estimate = own_contribution_estimate;
        Ok(())
    }

    /// The virtual source's last `ownContributionEstimate`, if the source is virtual and one was given.
    pub fn own_contribution_estimate(&self, source: &str) -> Option<f64> {
        if source != "cpu" {
            return None;
        }
        self.lock().cpu_virtual.as_ref().and_then(|v| v.own_contribution_estimate)
    }

    /// Whether `source` can be observed.
    pub fn supported(&self, source: &str) -> bool {
        if source != "cpu" {
            return false;
        }
        let mut guard = self.lock();
        let inner = &mut *guard;
        match &inner.cpu_virtual {
            Some(v) => v.available,
            None => inner.probe.supported(Instant::now()),
        }
    }

    /// The source's state: a virtual source's last reading, else the host's utilisation mapped to a state; `None`
    /// while there is no reading (a virtual source never updated, a host probe still on its first interval).
    pub fn sample(&self, source: &str) -> Option<PressureState> {
        if source != "cpu" {
            return None;
        }
        let mut guard = self.lock();
        let inner = &mut *guard;
        match &inner.cpu_virtual {
            Some(v) => v.state.filter(|_| v.available),
            None => inner.probe.sample(Instant::now()),
        }
    }

    pub fn poll_millis(&self, source: &str) -> u64 {
        if source == "cpu" && self.lock().cpu_virtual.is_none() {
            REAL_SAMPLE_MS / 2
        } else {
            VIRTUAL_POLL_MS
        }
    }

    pub fn min_interval_millis(&self, source: &str) -> u64 {
        if source == "cpu" && self.lock().cpu_virtual.is_none() {
            REAL_SAMPLE_MS
        } else {
            0
        }
    }
}

/// Utilisation (0 to 1) at which each state starts: fair, serious, critical.
const THRESHOLDS: [f64; 3] = [0.6, 0.8, 0.9];
/// A state is left downward only once the utilisation is this far below the threshold that entered it, so a load
/// hovering at a threshold does not flap.
const HYSTERESIS: f64 = 0.05;

/// The state of a utilisation, given the state it was in (hysteresis on the way down).
fn classify(utilisation: f64, previous: Option<PressureState>) -> PressureState {
    let raw = THRESHOLDS.iter().filter(|&&t| utilisation >= t).count();
    let level = match previous {
        Some(p) if (p as usize) > raw && utilisation >= THRESHOLDS[p as usize - 1] - HYSTERESIS => p as usize,
        _ => raw,
    };
    [PressureState::Nominal, PressureState::Fair, PressureState::Serious, PressureState::Critical][level]
}

/// The host's CPU utilisation between two reads of its tick counters.
#[derive(Default)]
struct CpuProbe {
    /// Whether the platform has a reading (decided on the first look).
    available: Option<bool>,
    /// The counters at the last sample, as (busy, total).
    last: Option<(u64, u64)>,
    at: Option<Instant>,
    state: Option<PressureState>,
}

impl CpuProbe {
    fn supported(&mut self, now: Instant) -> bool {
        if let Some(available) = self.available {
            return available;
        }
        let reading = cpu_ticks();
        self.available = Some(reading.is_some());
        // The first interval starts now.
        self.last = reading;
        self.at = Some(now);
        reading.is_some()
    }

    fn sample(&mut self, now: Instant) -> Option<PressureState> {
        if !self.supported(now) {
            return None;
        }
        if self.at.is_some_and(|at| now.duration_since(at) < Duration::from_millis(REAL_SAMPLE_MS)) {
            return self.state;
        }
        let current = cpu_ticks()?;
        if let Some((busy, total)) = self.last {
            let (busy, total) = (current.0.wrapping_sub(busy), current.1.wrapping_sub(total));
            if total > 0 && current.1 >= self.last.map_or(0, |l| l.1) {
                self.state = Some(classify(busy as f64 / total as f64, self.state));
            }
        }
        self.last = Some(current);
        self.at = Some(now);
        self.state
    }
}

/// (busy ticks, all ticks) of every CPU since boot, or `None` where the platform gives none.
#[cfg(target_os = "macos")]
fn cpu_ticks() -> Option<(u64, u64)> {
    // <mach/host_info.h>: HOST_CPU_LOAD_INFO is flavor 3, a `host_cpu_load_info` of four natural_t tick counts
    // (user, system, idle, nice) read with `host_statistics`; both functions are libSystem's.
    const HOST_CPU_LOAD_INFO: i32 = 3;
    const HOST_CPU_LOAD_INFO_COUNT: u32 = 4;
    unsafe extern "C" {
        fn mach_host_self() -> u32;
        fn host_statistics(host: u32, flavor: i32, info: *mut i32, count: *mut u32) -> i32;
    }
    let mut info = [0i32; 4];
    let mut count = HOST_CPU_LOAD_INFO_COUNT;
    // SAFETY: `info` has the `count` integers the flavor writes, and both pointers outlive the call.
    let status = unsafe { host_statistics(mach_host_self(), HOST_CPU_LOAD_INFO, info.as_mut_ptr(), &mut count) };
    if status != 0 || count != HOST_CPU_LOAD_INFO_COUNT {
        return None;
    }
    let [user, system, idle, nice] = info.map(|t| t as u32 as u64);
    Some((user + system + nice, user + system + idle + nice))
}

#[cfg(target_os = "linux")]
fn cpu_ticks() -> Option<(u64, u64)> {
    parse_proc_stat(&std::fs::read_to_string("/proc/stat").ok()?)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn cpu_ticks() -> Option<(u64, u64)> {
    None
}

/// The aggregate `cpu` line of `/proc/stat`: user nice system idle iowait irq softirq steal. Idle time is `idle` and
/// `iowait`; guest time is already in user/nice.
#[cfg(any(target_os = "linux", test))]
fn parse_proc_stat(text: &str) -> Option<(u64, u64)> {
    let line = text.lines().find(|l| l.split_ascii_whitespace().next() == Some("cpu"))?;
    let fields: Vec<u64> = line.split_ascii_whitespace().skip(1).take(8).map(|f| f.parse().ok()).collect::<Option<_>>()?;
    if fields.len() < 4 {
        return None;
    }
    let total: u64 = fields.iter().sum();
    let idle = fields[3] + fields.get(4).copied().unwrap_or(0);
    Some((total - idle, total))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_follow_thresholds_with_hysteresis() {
        assert_eq!(classify(0.1, None), PressureState::Nominal);
        assert_eq!(classify(0.65, None), PressureState::Fair);
        assert_eq!(classify(0.85, None), PressureState::Serious);
        assert_eq!(classify(0.95, None), PressureState::Critical);
        // Just under the threshold that entered the state: kept; well under: left.
        assert_eq!(classify(0.88, Some(PressureState::Critical)), PressureState::Critical);
        assert_eq!(classify(0.84, Some(PressureState::Critical)), PressureState::Serious);
        assert_eq!(classify(0.58, Some(PressureState::Fair)), PressureState::Fair);
        assert_eq!(classify(0.5, Some(PressureState::Fair)), PressureState::Nominal);
        // Rising is immediate.
        assert_eq!(classify(0.61, Some(PressureState::Nominal)), PressureState::Fair);
    }

    #[test]
    fn proc_stat_cpu_line() {
        let text = "cpu  100 0 50 800 50 0 0 0 0 0\ncpu0 1 2 3 4 5 6 7 8 9 10\n";
        assert_eq!(parse_proc_stat(text), Some((150, 1000)));
        assert_eq!(parse_proc_stat("intr 1 2 3"), None);
    }

    #[test]
    fn virtual_source_replaces_the_real_one() {
        let hub = PressureHub::default();
        assert!(hub.set_reading("cpu", PressureState::Fair, None).is_err());
        hub.set_override("cpu", true, true).unwrap();
        assert!(hub.supported("cpu"));
        assert_eq!(hub.sample("cpu"), None);
        hub.set_reading("cpu", PressureState::Critical, Some(0.5)).unwrap();
        assert_eq!(hub.sample("cpu"), Some(PressureState::Critical));
        assert_eq!(hub.own_contribution_estimate("cpu"), Some(0.5));
        hub.set_override("cpu", false, true).unwrap();
        assert_eq!(hub.own_contribution_estimate("cpu"), None);
        hub.set_override("cpu", true, false).unwrap();
        assert!(!hub.supported("cpu"));
        assert!(hub.set_override("gpu", true, true).is_err());
    }

    #[test]
    fn the_real_source_is_sampled_at_most_once_a_second() {
        let mut probe = CpuProbe::default();
        let start = Instant::now();
        if !probe.supported(start) {
            return;
        }
        assert_eq!(probe.sample(start), None);
        assert_eq!(probe.sample(start + Duration::from_millis(500)), None);
        // After a second the probe reads the counters again and has a state.
        let _ = probe.sample(start + Duration::from_millis(1500));
        assert_eq!(probe.at, Some(start + Duration::from_millis(1500)));
    }
}
