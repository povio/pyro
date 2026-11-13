//! Emulation.setVirtualTimePolicy over pyro-browser's page clock and the
//! loop's timers (pyro_browser::clock). The first call freezes the page clock
//! and holds every delayed timer for the browser thread; from then on time
//! only moves when the policy lets it: `advance` (and
//! `pauseIfNetworkFetchesPending` while no fetch is in flight) jumps to the
//! next timer whenever the loop runs out of other work, until the budget, if
//! any, is spent; then the policy turns to `pause` and
//! `Emulation.virtualTimeBudgetExpired` goes to the session that set it.
//! Without a policy nothing here runs.
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};

use super::EngineHost;
use crate::msg::{CdpError, CdpResult};
use crate::target::PageCommand;

pub(crate) struct VirtualTime {
    /// `advance` or `pauseIfNetworkFetchesPending`.
    advancing: bool,
    /// `pauseIfNetworkFetchesPending`: no jump while `page` fetches or
    /// navigates.
    pause_while_fetching: bool,
    /// Virtual time left before the budget expires.
    budget: Option<Duration>,
    page: pyro_browser::PageId,
    session: String,
}

impl VirtualTime {
    /// Whether time may move (`advance`, `pauseIfNetworkFetchesPending`).
    pub(super) fn advancing(&self) -> bool {
        self.advancing
    }
}

/// Monotonic milliseconds of the virtual time origin (`virtualTimeTicksBase`).
fn ticks_ms(at: std::time::Instant) -> f64 {
    static EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let epoch = *EPOCH.get_or_init(|| at - Duration::from_secs(1));
    at.saturating_duration_since(epoch).as_secs_f64() * 1000.0
}

impl EngineHost {
    pub(super) fn set_virtual_time_policy(&mut self, cmd: &PageCommand<'_, '_>) -> CdpResult<Option<Value>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct P {
            policy: String,
            budget: Option<f64>,
            // Accepted; tasks always run before time jumps, so none starve.
            max_virtual_time_task_starvation_count: Option<i64>,
            /// Seconds since the Unix epoch, for `Date` (first call only).
            initial_virtual_time: Option<f64>,
        }
        let p: P = cmd.command.params()?;
        // Chrome treats every other policy string as pause.
        let advancing = matches!(p.policy.as_str(), "advance" | "pauseIfNetworkFetchesPending");
        if !advancing && p.budget.is_some() {
            return Err(CdpError::invalid_params("Can only specify budget for non-Pause policy"));
        }
        if !advancing && p.max_virtual_time_task_starvation_count.is_some() {
            return Err(CdpError::invalid_params("Can only specify starvation count for non-Pause policy"));
        }
        if p.budget.is_some_and(|b| !b.is_finite()) || p.initial_virtual_time.is_some_and(|t| !t.is_finite()) {
            return Err(CdpError::invalid_params("Invalid parameters"));
        }
        let page = self.pages.get(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?.id;
        // The page clock is the browser thread's: one page owns the policy
        // (another may take over once it is closed).
        if let Some(owner) = self.virtual_time.as_ref().map(|v| v.page) {
            if owner != page && self.thread.browser.page(owner).is_some() {
                return Err(CdpError::server(
                    "virtual time is already enabled for another page on this browser thread",
                ));
            }
        }
        let first = self
            .thread
            .browser
            .pause_virtual_time(p.initial_virtual_time.map(|t| t * 1000.0))
            .map_err(super::remote::js_to_cdp)?;
        if first {
            self.virtual_ticks_base = ticks_ms(pyro_browser::clock::now());
        }
        self.virtual_time = Some(VirtualTime {
            advancing,
            pause_while_fetching: p.policy == "pauseIfNetworkFetchesPending",
            budget: p.budget.map(|ms| Duration::from_secs_f64(ms.max(0.0) / 1000.0)),
            page,
            session: cmd.session_id.to_owned(),
        });
        Ok(Some(json!({ "virtualTimeTicksBase": self.virtual_ticks_base })))
    }
}

/// Advances virtual time while the policy lets it and the loop has nothing
/// else to run: runs tasks and due timers, then jumps to the next timer
/// within the budget. Returns true when `ready` asked to stop or after
/// [`MAX_JUMPS`] jumps (an interval would otherwise keep it busy; the
/// next turn goes on).
const MAX_JUMPS: u32 = 1000;

pub(super) fn run(
    thread: &mut pyro_browser::BrowserThread,
    policy: &mut Option<VirtualTime>,
    out: &crate::outbox::Outbox,
    ready: &mut dyn FnMut(&mut pyro_browser::Browser) -> bool,
) -> bool {
    let mut jumps = 0;
    loop {
        if ready(&mut thread.browser) || jumps == MAX_JUMPS {
            return true;
        }
        if thread.event_loop.run_one(&mut thread.browser) {
            continue;
        }
        let Some(vt) = policy.as_mut().filter(|v| v.advancing) else { return false };
        if vt.pause_while_fetching
            && thread.browser.page_work(vt.page).is_some_and(|w| w.requests > 0 || w.navigations > 0)
        {
            return false;
        }
        let step = match (thread.browser.next_virtual_timer(), vt.budget) {
            (Some(next), Some(budget)) if next <= budget => {
                vt.budget = Some(budget - next);
                next
            }
            (Some(next), None) => next,
            (_, Some(budget)) => {
                // Spent: the rest of the budget passes, then time pauses.
                vt.advancing = false;
                vt.budget = None;
                thread.browser.advance_virtual_time(budget);
                out.push(crate::msg::event_frame("Emulation.virtualTimeBudgetExpired", &json!({}), Some(&vt.session)));
                return false;
            }
            (None, None) => return false,
        };
        jumps += 1;
        if let Some(at) = thread.browser.advance_virtual_time(step) {
            thread.event_loop.run_timer_due_by(&mut thread.browser, at);
        }
    }
}
