//! Speculation-rules prefetch (WICG nav-speculation "prefetch"): the driver that turns a
//! document's `prefetch` rules into fetches, the prefetch cache that keeps their responses, and
//! the claim a later navigation makes on one so that it does not touch the network again.
//!
//! # The driver
//! `Browser::speculation_changed` (`speculation_rules`) calls [`Browser::prefetch_rules_changed`]
//! when a rule set is added to or removed from a TOP-LEVEL document (Chrome ignores the rules of
//! nested documents). An added set starts a fetch for each URL of its `prefetch` list rules with
//! `eagerness` `immediate` (the others, `eager`, `moderate` and `conservative`, start on pointer
//! or visibility events: the document-rules / eagerness lane calls [`Browser::start_prefetch_of`]
//! with the candidate when that happens). A candidate is NOT prefetched when
//! - the initiator or the URL is not potentially trustworthy (https, or localhost): a prefetch
//!   from or to a plain http page is never made;
//! - the URL has credentials, or only differs from the document's URL by a fragment (that is a
//!   same-document navigation; the document's own URL without a fragment is prefetchable: a
//!   reload uses it);
//! - the rule `requires: ["anonymous-client-ip-when-cross-origin"]` and the URL is cross-origin
//!   (it needs a proxy this browser has not);
//! - the URL is cross-site and the effective referrer policy (the rule's, else the document's) is
//!   not `strict-origin-when-cross-origin` or stricter (`no-referrer`, `same-origin`,
//!   `strict-origin`): a cross-site request must not leak a path;
//! - a service worker would handle a navigation to it (its fetch event belongs to the
//!   service-worker lane); or the same document already has it (prefetched or in flight).
//!
//! # The request
//! A [`ResourceKind::Document`] navigation request carrying [`Purpose::Prefetch`]: `Sec-Purpose:
//! prefetch` (on every redirect hop), `Sec-Fetch-Dest: document`, `Sec-Fetch-Mode: navigate`, no
//! `Sec-Fetch-User`, the navigation `Accept`, the initiator document's URL as referrer under the
//! rule's `referrer_policy` (else the document's), cookies and client hints as a navigation of
//! the same site has them. A CROSS-SITE prefetch sends no cookies and no non-default client hints
//! (the cookies its response sets are stored, as they take effect when the navigation uses it).
//! It goes through the context's HTTP cache like any navigation (a fresh cached response answers
//! it, and its response is stored) but its result is kept in this cache, not in the HTTP cache's
//! hands: a response that is not 2xx is dropped, never served.
//!
//! # The cache and the claim
//! One cache per [`Browser`], entries tagged with their browsing context; an entry is usable
//! once, by a navigation of the same context started by the document that asked for it (matched
//! by the document's URL: the `Referer` source of the navigation, which for a traversal is the
//! document of the frame that called `history.go()`), of the top-level frame, with GET, to the
//! requested URL (the first one of a redirect chain: a navigation to a URL in the middle of the
//! chain, or redirected to the prefetched URL, does not match), through [`NoVarySearch`]
//! equivalence (the rule's `expects_no_vary_search` hint while the response is pending, the
//! response's own `No-Vary-Search` after). A ready entry answers the navigation as a task; a
//! pending one is joined (the navigation waits for it, then gets its response, or goes to the
//! network when the prefetch fails or is not 2xx). The navigation's timing is rebased on the
//! navigation (`deliveryType` `navigational-prefetch`). An entry expires after five minutes;
//! `Clear-Site-Data` naming `"prefetchCache"` or `"cache"` (`NetClient::prefetch_cache_cleared_since`)
//! drops the origin's entries, in flight ones included. At most [`MAX_ENTRIES`] live.

use std::time::{Duration, Instant};

use pyro_net::nvs::NoVarySearch;
use pyro_net::ReferrerPolicy;
use url::Url;

use crate::frames::FrameKey;
use crate::loader::{Purpose, ResourceCancellation};
use crate::speculation_rules::{Action, Candidate, Eagerness, Requirement, RuleSetId, SpeculationChange};
use crate::{Browser, ContextId, FetchRequest, NavigateOpts, NavigationReason, PageId, Resource, ResourceKind};

/// How long a prefetch may wait for its navigation.
const TTL: Duration = Duration::from_secs(300);
/// The most prefetches alive at once; past it the oldest unclaimed one goes.
const MAX_ENTRIES: usize = 64;

/// What a navigation that joined a pending prefetch is given: the response (rebased on the
/// navigation), or `None` when the prefetch failed, was not usable for the navigation's URL, or
/// was cleared, and the navigation must fetch.
pub(crate) type Waiter = Box<dyn FnOnce(&mut Browser, Option<Resource>)>;

enum State {
    /// In flight. `waiter`: a navigation (to this URL) took it and waits.
    Pending { waiter: Option<(Url, Waiter)>, cancel: Option<ResourceCancellation> },
    Ready(Resource),
}

impl State {
    fn claimed(&self) -> bool {
        matches!(self, State::Pending { waiter: Some(_), .. })
    }
}

struct Entry {
    id: u64,
    context: ContextId,
    /// The top-level document that asked: its page and loader id (an entry dies with it) and its
    /// URL without fragment (what a navigation it starts names it by).
    page: PageId,
    initiator_loader: String,
    initiator: Url,
    set: RuleSetId,
    /// The requested URL without fragment: the only one a navigation matches, up to `nvs`.
    url: Url,
    /// The variance responses to `url` are matched under: the rule's hint until the response says.
    nvs: NoVarySearch,
    started: Instant,
    state: State,
}

/// The prefetched responses of a browser (see the module header).
#[derive(Default)]
pub(crate) struct PrefetchCache {
    entries: Vec<Entry>,
    next_id: u64,
}

/// What a navigation got from the cache.
pub(crate) enum Claim {
    /// The response, already rebased on the navigation.
    Ready(Resource),
    /// An entry still in flight: join it with [`Browser::prefetch_wait`]. The navigation's start,
    /// which the response's timing is rebased on.
    Pending(u64, Instant),
}

/// `url` without its fragment.
fn without_fragment(mut url: Url) -> Url {
    url.set_fragment(None);
    url
}

/// Whether a cross-site prefetch may go out under `policy`: only policies that never put a path
/// in a cross-site `Referer` (the default and stricter ones).
fn cross_site_policy_ok(policy: ReferrerPolicy) -> bool {
    matches!(
        policy,
        ReferrerPolicy::NoReferrer
            | ReferrerPolicy::SameOrigin
            | ReferrerPolicy::StrictOrigin
            | ReferrerPolicy::StrictOriginWhenCrossOrigin
    )
}

/// Whether `resource` may answer a navigation: a 2xx response (a prefetch's 3xx were followed,
/// 4xx and 5xx are never served) from a trustworthy final URL, and not a download.
fn usable(resource: &Resource) -> bool {
    (200..300).contains(&resource.status)
        && Url::parse(&resource.url).is_ok_and(|u| pyro_net::cookies::is_trustworthy(&u))
        && !resource.content_disposition.as_deref().is_some_and(|d| d.trim_start().to_ascii_lowercase().starts_with("attachment"))
}

/// Navigation Timing of a prefetched response as the navigation sees it: the fetch is the
/// navigation's, started at `start`, with no connection behind it and each later step no earlier
/// than the navigation (a response that arrived before it has `connectEnd`, `requestStart` and
/// `responseStart` all at `start`; one still to come keeps its own `responseStart`, and a redirect
/// chain's last hop its own start). `deliveryType` says `navigational-prefetch`.
pub(crate) fn rebase_timing(resource: &mut Resource, start: Instant) {
    let Some(report) = resource.timing.as_mut() else { return };
    report.start = start;
    report.end = report.end.max(start);
    let Some(response) = report.response.as_mut() else { return };
    response.prefetched = true;
    let timing = &mut response.timing;
    let not_before = |at: Option<Instant>| Some(at.map_or(start, |at| at.max(start)));
    timing.start = Some(start);
    timing.fetch_start = not_before(timing.fetch_start);
    timing.request_start = not_before(timing.request_start);
    timing.response_start = not_before(timing.response_start);
    timing.connection = None;
}

impl PrefetchCache {
    fn find(&self, id: u64) -> Option<usize> {
        self.entries.iter().position(|e| e.id == id)
    }

    /// Makes room for one more: the oldest entry no navigation waits on goes.
    fn make_room(&mut self) {
        while self.entries.len() >= MAX_ENTRIES {
            let at = self.entries.iter().position(|e| !e.state.claimed()).unwrap_or(0);
            self.entries.remove(at);
        }
    }
}

impl Browser {
    /// A rule set was added to or removed from `page`'s document (`Browser::speculation_changed`).
    pub(crate) fn prefetch_rules_changed(&mut self, page: FrameKey, change: SpeculationChange) {
        if page.child.is_some() {
            return;
        }
        match change {
            SpeculationChange::Added(set) => {
                let candidates: Vec<Candidate> =
                    self.speculation_candidates(page).into_iter().filter(|c| c.set == set).collect();
                for candidate in candidates {
                    if candidate.action == Action::Prefetch && candidate.eagerness == Eagerness::Immediate {
                        self.start_prefetch_of(page, &candidate);
                    }
                }
            }
            SpeculationChange::Removed(set) => {
                // The prefetches of a rule set that went away stop (one a navigation waits on
                // keeps going for it).
                let Some(loader) = self.doc(page).map(|d| d.main_frame.loader_id.0.clone()) else { return };
                self.prefetches.entries.retain(|e| e.initiator_loader != loader || e.set != set || e.state.claimed());
            }
        }
    }

    /// Starts the prefetch `candidate` of `page`'s (top-level) document asks for, unless it is
    /// not eligible (see the module header).
    pub(crate) fn start_prefetch_of(&mut self, page: FrameKey, candidate: &Candidate) {
        let Some(frame) = self.doc(page).map(|d| d.main_frame.clone()) else { return };
        let Some(loader) = self.loader_for(page) else { return };
        let Some(context) = self.page(page.page).map(|p| p.context) else { return };
        // The document's URL as it is now (a `pushState` changed it), which a navigation it starts
        // names as its referrer.
        let Some(document_url) = self.document_url_of(page) else { return };
        let initiator = without_fragment(document_url.clone());
        let trustworthy = pyro_net::cookies::is_trustworthy;
        if !trustworthy(&document_url) || !trustworthy(&candidate.url) {
            return;
        }
        let mut url = candidate.url.clone();
        if !url.username().is_empty() || url.password().is_some() {
            return;
        }
        let has_fragment = url.fragment().is_some();
        url.set_fragment(None);
        // A link that only adds a fragment to the document's URL navigates within the document.
        if has_fragment && url == initiator {
            return;
        }
        let cross_origin = url.origin() != document_url.origin();
        if cross_origin && candidate.requires.contains(&Requirement::AnonymousClientIpWhenCrossOrigin) {
            return;
        }
        let cross_site = pyro_net::cookies::partition_key(&url) != pyro_net::cookies::partition_key(&document_url);
        let policy = candidate
            .referrer_policy
            .or_else(|| frame.page_dom.as_ref().map(|d| d.referrer_policy()))
            .unwrap_or_default();
        if cross_site && !cross_site_policy_ok(policy) {
            return;
        }
        if self.navigation_service_worker_controls(page.page, &url) {
            return;
        }
        self.sweep_prefetches();
        if self.prefetches.entries.iter().any(|e| e.context == context && e.initiator_loader == frame.loader_id.0 && e.url == url) {
            return;
        }
        self.prefetches.make_room();
        let id = self.prefetches.next_id;
        self.prefetches.next_id += 1;
        let nvs = candidate.expects_no_vary_search.as_deref().map(NoVarySearch::parse).unwrap_or_default();
        self.prefetches.entries.push(Entry {
            id,
            context,
            page: page.page,
            initiator_loader: frame.loader_id.0.clone(),
            initiator,
            set: candidate.set,
            url: url.clone(),
            nvs,
            started: Instant::now(),
            state: State::Pending { waiter: None, cancel: None },
        });
        let request = FetchRequest {
            csp: None,
            required_csp: None,
            purpose: Some(Purpose::Prefetch { cross_site }),
            parser_priority: false,
            credentials: None,
            post: None,
            url: url.to_string(),
            kind: ResourceKind::Document,
            page: page.page,
            referrer: frame.navigation_referrer(),
            referrer_policy: Some(policy),
            frame_id: None,
            loader_id: None,
            crossorigin: None,
            initiator: None,
            module: None,
            part: None,
        };
        let cancel = loader.fetch_cancellable(self, request, Box::new(move |b, result| b.prefetch_finished(id, result)));
        match self.prefetches.find(id).map(|at| &mut self.prefetches.entries[at].state) {
            Some(State::Pending { cancel: slot, .. }) => *slot = cancel,
            // It went away meanwhile (cleared, expired): the transport stops with the handle.
            _ => drop(cancel),
        }
    }

    /// The URL of `page`'s current document (`location.href`).
    fn document_url_of(&self, page: FrameKey) -> Option<Url> {
        Url::parse(&self.doc(page)?.nav.url()).ok()
    }

    /// Drops the entries that expired, whose browsing context is gone, or whose document is no
    /// longer its page's (an entry belongs to the document that asked), but not one a navigation
    /// waits on.
    fn sweep_prefetches(&mut self) {
        let now = Instant::now();
        let keep: Vec<bool> = self
            .prefetches
            .entries
            .iter()
            .map(|e| {
                e.state.claimed()
                    || (now.saturating_duration_since(e.started) < TTL
                        && self.contexts.iter().any(|c| c.id == e.context)
                        && self.doc(FrameKey::from(e.page)).is_some_and(|d| d.main_frame.loader_id.0 == e.initiator_loader))
            })
            .collect();
        let mut keep = keep.into_iter();
        self.prefetches.entries.retain(|_| keep.next().unwrap_or(true));
    }

    /// Whether a service worker would take a navigation to `url` (its fetch event is not given
    /// prefetches here, and a navigation it handles never uses one).
    fn navigation_service_worker_controls(&self, page: PageId, url: &Url) -> bool {
        let probe = pyro_net::Request::get(url.clone(), pyro_net::Destination::Document);
        crate::loader::service_worker_route(self, page, &probe, true).is_some()
    }

    /// Whether `Clear-Site-Data` named the prefetch cache for the origins of `urls` in `context`
    /// since `since`.
    fn prefetch_cleared(&self, context: ContextId, since: Instant, urls: &[&Url]) -> bool {
        let Some(net) = self.context_net(context) else { return false };
        urls.iter().any(|u| net.prefetch_cache_cleared_since(&u.origin().ascii_serialization(), since))
    }

    /// The prefetch `id` got its result.
    fn prefetch_finished(&mut self, id: u64, result: Result<Resource, String>) {
        let Some(at) = self.prefetches.find(id) else { return };
        let mut entry = self.prefetches.entries.remove(at);
        let State::Pending { waiter, cancel } = std::mem::replace(&mut entry.state, State::Pending { waiter: None, cancel: None }) else {
            return;
        };
        if let Some(cancel) = cancel {
            cancel.disarm();
        }
        // The response's `No-Vary-Search` replaces the rule's hint.
        let nvs = result.as_ref().ok().and_then(|r| r.no_vary_search.clone()).unwrap_or_default();
        let resource = result.ok().filter(usable).filter(|r| {
            let final_url = Url::parse(&r.url).ok();
            let mut urls = vec![&entry.url];
            urls.extend(final_url.as_ref());
            !self.prefetch_cleared(entry.context, entry.started, &urls)
        });
        match (waiter, resource) {
            (Some((target, waiter)), resource) => {
                // The navigation joined under the hint: the response may not allow its URL.
                waiter(self, resource.filter(|_| nvs.equivalent(&entry.url, &target)));
            }
            (None, Some(resource)) => {
                entry.nvs = nvs;
                entry.state = State::Ready(resource);
                self.prefetches.entries.insert(at.min(self.prefetches.entries.len()), entry);
            }
            (None, None) => {}
        }
    }

    /// The prefetch a navigation of `page` to `opts.url` may use (see the module header), taken
    /// out of the cache: its response, or an entry in flight to join.
    pub(crate) fn claim_prefetch(&mut self, page: FrameKey, opts: &NavigateOpts) -> Option<Claim> {
        if page.child.is_some() || opts.post.is_some() || opts.cdp_request_id.is_some() || self.prefetches.entries.is_empty() {
            return None;
        }
        let context = self.page(page.page)?.context;
        let started = Instant::now();
        self.sweep_prefetches();
        // The document that navigates: a traversal's is the one that called `history.go()`;
        // otherwise the navigation's referrer source is the initiator document's URL (a popup's
        // is its opener's), and a traversal or reload with none is the frame's own document.
        let source = self.prefetch_navigation_source.clone().or_else(|| opts.referrer.clone()).or_else(|| {
            (opts.reason == NavigationReason::History).then(|| self.document_url_of(page).map(|u| u.to_string())).flatten()
        })?;
        let initiator = without_fragment(Url::parse(&source).ok()?);
        let target = without_fragment(Url::parse(&opts.url).ok()?);
        if !pyro_net::cookies::is_trustworthy(&target) || self.navigation_service_worker_controls(page.page, &target) {
            return None;
        }
        let matching = |e: &Entry| e.context == context && e.initiator == initiator && !e.state.claimed() && e.nvs.equivalent(&e.url, &target);
        // The entry of exactly this URL first, else one the variance makes equivalent.
        let at = self
            .prefetches
            .entries
            .iter()
            .position(|e| matching(e) && e.url == target)
            .or_else(|| self.prefetches.entries.iter().position(|e| matching(e)))?;
        let entry = &self.prefetches.entries[at];
        if self.prefetch_cleared(entry.context, entry.started, &[&entry.url]) {
            self.prefetches.entries.remove(at);
            return None;
        }
        if let State::Pending { .. } = entry.state {
            return Some(Claim::Pending(entry.id, started));
        }
        let State::Ready(mut resource) = self.prefetches.entries.remove(at).state else { return None };
        rebase_timing(&mut resource, started);
        Some(Claim::Ready(resource))
    }

    /// Gives the navigation to `target` that claimed the pending prefetch `id` its waiter (it
    /// runs when the prefetch finishes, with the response if it may serve `target`).
    pub(crate) fn prefetch_wait(&mut self, id: u64, target: Url, waiter: Waiter) {
        if let Some(at) = self.prefetches.find(id) {
            if let State::Pending { waiter: slot, .. } = &mut self.prefetches.entries[at].state {
                *slot = Some((without_fragment(target), waiter));
                return;
            }
        }
        // It finished or went away in between (not within one task): nothing to wait for.
        self.scheduler().post(pyro_runtime::TaskSource::Networking, move |b| waiter(b, None));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cross_site_prefetch_needs_a_policy_that_leaks_no_path() {
        assert!(cross_site_policy_ok(ReferrerPolicy::StrictOriginWhenCrossOrigin));
        assert!(cross_site_policy_ok(ReferrerPolicy::NoReferrer));
        assert!(cross_site_policy_ok(ReferrerPolicy::StrictOrigin));
        assert!(!cross_site_policy_ok(ReferrerPolicy::UnsafeUrl));
        assert!(!cross_site_policy_ok(ReferrerPolicy::OriginWhenCrossOrigin));
        assert!(!cross_site_policy_ok(ReferrerPolicy::NoReferrerWhenDowngrade));
    }

    #[test]
    fn fragments_are_not_part_of_a_prefetch() {
        let url = Url::parse("https://a.test/p?x=1#frag").unwrap();
        assert_eq!(without_fragment(url).as_str(), "https://a.test/p?x=1");
    }

    #[test]
    fn an_entry_expires_and_the_cache_is_bounded() {
        let mut cache = PrefetchCache::default();
        let now = Instant::now();
        for id in 0..(MAX_ENTRIES as u64 + 3) {
            cache.make_room();
            cache.entries.push(Entry {
                id,
                context: ContextId(0),
                page: PageId(1),
                initiator_loader: String::new(),
                initiator: Url::parse("https://a.test/").unwrap(),
                set: RuleSetId(0),
                url: Url::parse("https://a.test/x").unwrap(),
                nvs: NoVarySearch::default(),
                started: now,
                state: State::Pending { waiter: None, cancel: None },
            });
        }
        assert_eq!(cache.entries.len(), MAX_ENTRIES);
        // The oldest were evicted.
        assert_eq!(cache.entries[0].id, 3);
        assert!(cache.entries.iter().all(|e| now.saturating_duration_since(e.started) < TTL));
    }
}
