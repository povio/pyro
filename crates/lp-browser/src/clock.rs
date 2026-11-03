//! The time pages see: `performance.now()`, `Date`, and when `setTimeout`s
//! are due.
//!
//! Normally real time. With virtual time on (`LIGHTNINGPANDA_VIRTUAL_TIME=1`)
//! the clock can also jump: when an `LP.run` waits and its page has nothing
//! left but timers (no task, request or navigation), the driver moves the
//! clock to the next timer instead of sleeping until it is due
//! ([`Browser::advance_time`](crate::Browser::advance_time)). A debounce or a
//! delayed save then costs no wall time. The jump is per browser thread, so
//! it is meant for fork mode (one page per process). Time never goes back.
//!
//! `Emulation.setVirtualTimePolicy` freezes the page clock instead: from
//! then on it only moves when virtual time advances
//! ([`Browser::pause_virtual_time`](crate::Browser::pause_virtual_time)).
//!
//! `Date` follows the page clock through a wrapper installed in every realm
//! (`Date.now()`, `new Date()` and `Date()`); its prototype is unchanged.

use std::cell::Cell;
use std::sync::LazyLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The environment variable that turns virtual time on (`1`).
pub const VIRTUAL_TIME_ENV: &str = "LIGHTNINGPANDA_VIRTUAL_TIME";

thread_local! {
    /// How far the page clock is ahead of real time.
    static SKEW: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static MANUAL: Cell<bool> = const { Cell::new(false) };
    /// Virtual time policy: the frozen page clock and its wall time (ms
    /// since the Unix epoch).
    static FROZEN: Cell<Option<(Instant, f64)>> = const { Cell::new(None) };
}

/// Whether the page clock may jump ahead (see the module docs).
pub fn virtual_time() -> bool {
    static ON: LazyLock<bool> = LazyLock::new(|| std::env::var(VIRTUAL_TIME_ENV).is_ok_and(|v| v.trim() == "1"));
    *ON
}

pub(crate) fn page_clock_enabled() -> bool {
    virtual_time() || MANUAL.get()
}

pub(crate) fn enable_manual() {
    MANUAL.set(true);
}

/// The page clock now, as an `Instant` (comparable to timer deadlines kept
/// on this clock).
pub fn now() -> Instant {
    match FROZEN.get() {
        Some((now, _)) => now,
        None => Instant::now() + SKEW.get(),
    }
}

/// The page clock's wall time, in milliseconds since the Unix epoch.
pub fn wall_ms() -> f64 {
    if let Some((_, wall)) = FROZEN.get() {
        return wall;
    }
    let real = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64() * 1000.0);
    real + SKEW.get().as_secs_f64() * 1000.0
}

pub(crate) fn skip(by: Duration) {
    SKEW.set(SKEW.get() + by);
    if let Some((now, wall)) = FROZEN.get() {
        FROZEN.set(Some((now + by, wall + by.as_secs_f64() * 1000.0)));
    }
}

/// Stops the page clock at its current reading (`wall_ms`: its wall time
/// instead of the current one).
pub(crate) fn freeze(wall_ms: Option<f64>) {
    FROZEN.set(Some((now(), wall_ms.unwrap_or_else(self::wall_ms))));
}

pub(crate) fn frozen() -> bool {
    FROZEN.get().is_some()
}

/// Makes `Date` read the page clock: `performance.timeOrigin +
/// performance.now()`, both of which already do.
pub(crate) const DATE_SCRIPT: &str = r#"(() => {
  const RealDate = Date;
  const perf = performance;
  const pnow = perf.now.bind(perf);
  const origin = perf.timeOrigin;
  const now = () => Math.floor(origin + pnow());
  const PageDate = new Proxy(RealDate, {
    construct(target, args, newTarget) {
      return Reflect.construct(target, args.length ? args : [now()], newTarget);
    },
    apply() {
      return new RealDate(now()).toString();
    },
  });
  Object.defineProperty(RealDate, "now", { value: { now() { return now(); } }.now, writable: true, configurable: true });
  Object.defineProperty(RealDate.prototype, "constructor", { value: PageDate, writable: true, configurable: true });
  Object.defineProperty(globalThis, "Date", { value: PageDate, writable: true, configurable: true });
})();
"#;
