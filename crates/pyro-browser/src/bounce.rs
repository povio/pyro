//! Bounce Tracking Mitigations in the browser: the recording at a top-level page's navigations and its close
//! (`pyro_webapi::bounce_tracking` has the data model and the user activation side), and the run that deletes the
//! state of the sites that were only bounced through.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use pyro_webapi::bounce_tracking::site_of_url;

use crate::frames::FrameKey;
use crate::{Browser, ContextId, NavigateOpts, NavigationReason, PageId};

impl Browser {
    /// Whether a cookie of `site` was created in `context` after its jar had `sequence`.
    fn cookie_written_since(&self, context: ContextId, sequence: u64, site: &str) -> bool {
        self.context_net(context).is_some_and(|net| net.cookies().lock().is_ok_and(|jar| jar.site_written_since(sequence, site)))
    }

    fn cookie_sequence(&self, context: ContextId) -> u64 {
        self.context_net(context).map_or(0, |net| net.cookies().lock().map_or(0, |jar| jar.sequence()))
    }

    /// Spec "navigation start", for a top-level page's document navigation (`start_document_fetch`): a navigation
    /// with transient activation, or one the browser makes (`Page.navigate`, the address bar, a traversal), ends the
    /// extended navigation in progress; any other is a client redirect, and the document it leaves bounced.
    pub(crate) fn bounce_navigation_started(&self, page: FrameKey, opts: &NavigateOpts) {
        if page.child.is_some() {
            return;
        }
        let Some(context) = self.page(page.page).map(|p| p.context) else { return };
        let Some(services) = self.context_services(context) else { return };
        let dom = self.doc(page).and_then(|d| d.main_frame.page_dom.clone());
        let source = dom.as_ref().and_then(|d| site_of_url(&d.document_url()));
        let user_initiated = opts.cdp_request_id.is_some()
            || matches!(opts.reason, NavigationReason::AddressBar | NavigationReason::History)
            || dom.as_ref().is_some_and(|d| d.has_transient_activation());
        let sequence = self.cookie_sequence(context);
        let written = |site: &str, since: u64| self.cookie_written_since(context, since, site);
        services.bounce_tracker().navigation_started(page.page.0, source, user_initiated, sequence, Instant::now(), &written);
    }

    /// The sites of the URLs a top-level navigation's response went through (its redirects, then the document's
    /// own URL), from the response `coop` chain and `url`.
    pub(crate) fn bounce_response_sites(coop: &[crate::loader::CoopResponse], url: &str) -> Vec<String> {
        let mut sites: Vec<String> = coop.iter().filter_map(|r| site_of_url(&r.url)).collect();
        if sites.is_empty() {
            sites.extend(site_of_url(url));
        }
        sites
    }

    /// Spec "response received" and "document load": the committed top-level document's response chain.
    pub(crate) fn bounce_response_committed(&self, page: FrameKey, sites: Vec<String>) {
        if page.child.is_some() || sites.is_empty() {
            return;
        }
        let Some(context) = self.page(page.page).map(|p| p.context) else { return };
        let Some(services) = self.context_services(context) else { return };
        services.bounce_tracker().response_committed(page.page.0, sites, self.cookie_sequence(context), Instant::now());
    }

    /// Spec "record bounces" at the close of a tab.
    pub(crate) fn bounce_page_closed(&self, page: PageId) {
        let Some(context) = self.page(page).map(|p| p.context) else { return };
        let Some(services) = self.context_services(context) else { return };
        let written = |site: &str, since: u64| self.cookie_written_since(context, since, site);
        services.bounce_tracker().finish(page.0, Instant::now(), &written);
    }

    /// The bounce tracking timer's run for `context`: the sites that were bounced through at least `grace` ago, were
    /// not activated by the user since, and that no top-level page of the context shows lose their cookies, the
    /// storage of their origins (Web Storage, IndexedDB, Cache Storage, the origin private file system, storage
    /// buckets), their service worker registrations and their HTTP cache entries. Returns the sites, in order.
    /// `None`: the context is gone. (`Storage.runBounceTrackingMitigations` passes no grace period, as the WebDriver
    /// command does.)
    pub fn run_bounce_tracking_mitigations(&self, context: ContextId, grace: Duration) -> Option<Vec<String>> {
        let services = self.context_services(context)?;
        let now = Instant::now();
        {
            let written = |site: &str, since: u64| self.cookie_written_since(context, since, site);
            services.bounce_tracker().settle(now, &written);
        }
        let shown: HashSet<String> = self
            .pages
            .iter()
            .filter(|p| p.context == context)
            .filter_map(|p| p.main_frame.page_dom.as_ref().and_then(|d| site_of_url(&d.document_url())))
            .collect();
        let due = services.bounce_tracker().take_expired(now, grace, &|site| shown.contains(site));
        let local = self.contexts.iter().find(|c| c.id == context).map(|c| c.local_storage.clone());
        for bounce in &due {
            if let Some(net) = self.context_net(context) {
                if let Ok(mut jar) = net.cookies().lock() {
                    jar.delete_site(&bounce.site);
                }
                net.clear_http_cache_site(&bounce.site);
            }
            if let Some(local) = &local {
                local.clear_site(&bounce.site);
            }
            services.unregister_site_service_workers(&bounce.site);
        }
        Some(due.into_iter().map(|b| b.site).collect())
    }
}
