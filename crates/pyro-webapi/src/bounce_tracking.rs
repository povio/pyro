//! Bounce Tracking Mitigations (https://privacycg.github.io/nav-tracking-mitigations/#bounce-tracking-mitigations),
//! as Chrome ships them: a browser context remembers the sites a navigation only passed through, and when none of
//! the user's interaction protects a site, deletes the state it holds.
//!
//! The data model is the spec's, per browser context ([`BounceTracker`], in [`ContextServices`]):
//!
//! - the *user activation map*: the sites a top-level document received trusted input at;
//! - the *bounce tracking map*: the sites that were passed through (a server redirect or a client redirect no
//!   user activation started) since the last run, with when it was first seen. A site is *stateful* when a
//!   cookie of it was created while the extended navigation that passed through it lasted (Chrome tells the
//!   bounces that accessed state from those that did not; the deletion applies to both, as the WPT pages
//!   `nav-tracking-mitigations/` ask: a stateless bounce is reported and its state, if any, cleared);
//! - per top-level page, the *bounce tracking record* of the extended navigation in progress: its initial site,
//!   its final site and the sites it bounced through. An extended navigation is a chain of navigations joined by
//!   redirects; a navigation with transient activation (a click, the address bar) ends it and starts the next.
//!   Where the spec's timer would end it after a client bounce detection period of 10 seconds, a record whose
//!   last response is older than that counts as ended when the map is next read or the page navigates.
//!
//! Sites are registrable domains (Chrome keys by site), schemeless: cookies go to `http` and `https` alike.
//!
//! The recording costs one small table update per top-level navigation and per trusted input; nothing is stored
//! for subresources or frames. The run ([`BounceTracker::take_expired`], driven by the embedder with the context's
//! cookie jar, storage and service workers) removes the sites whose grace period has elapsed and no top-level page
//! shows; production would call it on a timer, the WebDriver command and CDP's `Storage.runBounceTrackingMitigations`
//! call it with no grace period.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::navigator_extras::ContextServices;
use crate::page::PageDom;

/// The time after a navigation's last response that a client redirect may still follow (spec: "client bounce
/// detection timer"): the extended navigation has ended after it.
pub const CLIENT_BOUNCE_WINDOW: Duration = Duration::from_secs(10);

/// How long an activation protects a site (spec: "activation lifetime").
pub const ACTIVATION_LIFETIME: Duration = Duration::from_secs(45 * 24 * 60 * 60);

/// The site (registrable domain, or the IP address) of an http(s) URL; `None` for other URLs.
pub fn site_of_url(url: &str) -> Option<String> {
    let url = url::Url::parse(url).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = url.host_str()?.to_ascii_lowercase();
    Some(pyro_net::cookies::site_of(&host).to_owned())
}

/// A site in the bounce tracking map that is due.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DueBounce {
    pub site: String,
    /// Some cookie of the site was created while the navigation bounced through it.
    pub stateful: bool,
}

#[derive(Clone, Copy)]
struct Bounce {
    since: Instant,
    stateful: bool,
}

/// The extended navigation of one top-level page.
struct Record {
    initial: Option<String>,
    last_site: Option<String>,
    bounces: Vec<String>,
    /// The cookie jar's sequence number when the record began (`CookieJar::sequence`).
    cookie_sequence: u64,
    last_response: Option<Instant>,
}

impl Record {
    fn new(initial: Option<String>, cookie_sequence: u64) -> Self {
        Record { initial, last_site: None, bounces: Vec::new(), cookie_sequence, last_response: None }
    }

    fn bounce(&mut self, site: String) {
        if !self.bounces.contains(&site) {
            self.bounces.push(site);
        }
    }
}

/// A browser context's bounce tracking state.
#[derive(Default)]
pub struct BounceTracker {
    activation: HashMap<String, Instant>,
    bounces: HashMap<String, Bounce>,
    records: HashMap<u32, Record>,
}

impl BounceTracker {
    /// Spec "activation notification": `site` is the top-level document's. It leaves the bounce tracking map and
    /// is protected for the activation lifetime.
    pub fn activated(&mut self, site: &str, now: Instant) {
        self.bounces.remove(site);
        self.activation.insert(site.to_owned(), now);
    }

    /// Spec "navigation start" for the top-level page `page` leaving a document of `source` (`None`: no http(s)
    /// document). `user_initiated`: the navigation has transient activation, or the browser started it (address
    /// bar, `Page.navigate`, a history traversal): it ends the extended navigation in progress and begins another.
    /// Without activation it is a client redirect, and `source` bounced. `written(site, sequence)`: a cookie of
    /// `site` was created after the jar had `sequence`. `cookie_sequence` is the jar's now.
    pub fn navigation_started(
        &mut self,
        page: u32,
        source: Option<String>,
        user_initiated: bool,
        cookie_sequence: u64,
        now: Instant,
        written: &dyn Fn(&str, u64) -> bool,
    ) {
        let continues = !user_initiated && self.records.get(&page).is_some_and(|record| !Self::settled(record, now));
        if continues {
            if let (Some(site), Some(record)) = (source, self.records.get_mut(&page)) {
                record.bounce(site);
            }
            return;
        }
        // (No record, or its extended navigation ended: `finish` is then a no-op or the end of the old one.)
        self.finish(page, now, written);
        self.records.insert(page, Record::new(source, cookie_sequence));
    }

    /// Spec "response received" and "document load": the sites of the response's URL list (every server redirect
    /// and the final URL, in order) joined the extended navigation; the last is where it stands now.
    pub fn response_committed(&mut self, page: u32, sites: Vec<String>, cookie_sequence: u64, now: Instant) {
        let record = self.records.entry(page).or_insert_with(|| Record::new(None, cookie_sequence));
        record.last_site = sites.last().cloned();
        for site in sites {
            record.bounce(site);
        }
        record.last_response = Some(now);
    }

    fn settled(record: &Record, now: Instant) -> bool {
        record.last_response.is_some_and(|at| now.duration_since(at) >= CLIENT_BOUNCE_WINDOW)
    }

    /// Spec "record bounces", at the end of a page's extended navigation (a navigation the user started, the
    /// detection window passing, the page closing): the sites it bounced through, except its first and last and
    /// those the user activated, are in the bounce tracking map, the time they were first seen kept.
    pub fn finish(&mut self, page: u32, now: Instant, written: &dyn Fn(&str, u64) -> bool) {
        let Some(record) = self.records.remove(&page) else { return };
        for site in &record.bounces {
            if Some(site) == record.initial.as_ref() || Some(site) == record.last_site.as_ref() || self.activation.contains_key(site) {
                continue;
            }
            let stateful = written(site, record.cookie_sequence);
            let entry = self.bounces.entry(site.clone()).or_insert(Bounce { since: now, stateful });
            entry.stateful |= stateful;
        }
    }

    /// Ends the extended navigations whose client bounce detection window has passed.
    pub fn settle(&mut self, now: Instant, written: &dyn Fn(&str, u64) -> bool) {
        let ended: Vec<u32> = self.records.iter().filter(|(_, r)| Self::settled(r, now)).map(|(page, _)| *page).collect();
        for page in ended {
            self.finish(page, now, written);
        }
    }

    /// The bounce tracking timer: forgets the activations past their lifetime, and takes out of the map the
    /// sites seen at least `grace` ago that `shown` says no top-level page shows. They are returned in site order;
    /// the embedder deletes their state.
    pub fn take_expired(&mut self, now: Instant, grace: Duration, shown: &dyn Fn(&str) -> bool) -> Vec<DueBounce> {
        self.activation.retain(|_, at| now.duration_since(*at) < ACTIVATION_LIFETIME);
        let mut due: Vec<DueBounce> = self
            .bounces
            .iter()
            .filter(|(site, bounce)| now.duration_since(bounce.since) >= grace && !shown(site))
            .map(|(site, bounce)| DueBounce { site: site.clone(), stateful: bounce.stateful })
            .collect();
        due.sort_by(|a, b| a.site.cmp(&b.site));
        for d in &due {
            self.bounces.remove(&d.site);
        }
        due
    }
}

impl ContextServices {
    /// The context's Bounce Tracking Mitigations state.
    pub fn bounce_tracker(&self) -> std::cell::RefMut<'_, BounceTracker> {
        self.bounce.borrow_mut()
    }
}

/// Trusted input reached `page` (`PageDom::notify_input_activation`): its top-level site is activated.
pub(crate) fn input_activated(page: &PageDom) {
    let origin = page.hooks().and_then(|h| h.top_level_origin()).or_else(|| crate::storage::document_origin(page));
    if let Some(site) = origin.as_deref().and_then(site_of_url) {
        page.context_services().bounce_tracker().activated(&site, Instant::now());
    }
}
