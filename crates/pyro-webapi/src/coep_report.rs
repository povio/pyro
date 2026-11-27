//! Cross-Origin-Embedder-Policy reporting (HTML § 7.1.4 "queue a cross-origin embedder policy
//! CORP violation report" and "... navigation ..."): the reports a context makes when its
//! enforced policy blocked a response, or its report-only policy would have, as `type: "coep"`
//! reports with a body `{type, blockedURL, destination, disposition}`.
//!
//! The Cross-Origin Resource Policy check runs on the network thread
//! (`pyro_net::client::corp_check`), which hands each violation to the [`CorpReport`] a request
//! carries ([`PageDom::apply_embedder_defaults`] makes it). That closure posts to the context's
//! own thread, where [`queue_violation`] queues the report for the context's observers and for
//! the endpoint its policy's `report-to` names (`Reporting-Endpoints` of the document, else
//! `Report-To`), through [`crate::reporting_store`]. A worker's reports are its own (its scope's
//! observers); a nested navigation's or a worker script's go to the embedding document or the
//! worker's owner, which is the context whose policy blocked them.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::Arc;

use pyro_net::{CorpReport, CorpViolation, EmbedderPolicy, EmbedderReporting, RequestDefaults};
use serde_json::Map;

use crate::csp_report::strip_url_for_report;
use crate::page::PageDom;
use crate::reporting_store::{queue_report, ReportInput};

thread_local! {
    /// The pages of this thread that handed out a [`CorpReport`], by id: the posted task finds its
    /// page here (a page that is gone drops the report).
    static PAGES: RefCell<HashMap<u64, Weak<PageDom>>> = RefCell::new(HashMap::new());
}

impl PageDom {
    /// Fills `defaults` with the embedder policy this context's requests are subject to: the
    /// enforced value, the report-only value and where violations go (`None` when the context
    /// has neither policy). A nested navigation's request is made with the embedding document's.
    pub fn apply_embedder_defaults(&self, defaults: &mut RequestDefaults) {
        let policy = self.embedder_policy();
        let reporting = self.embedder_reporting();
        defaults.embedder_policy = policy;
        defaults.corp_report = if policy != EmbedderPolicy::UnsafeNone || reporting.report_only != EmbedderPolicy::UnsafeNone {
            self.corp_report(reporting.report_only)
        } else {
            None
        };
    }

    fn corp_report(&self, report_only: EmbedderPolicy) -> Option<CorpReport> {
        let poster = self.remote_poster()?;
        let id = self.id();
        PAGES.with(|pages| {
            let mut pages = pages.borrow_mut();
            if pages.len() > 64 {
                pages.retain(|_, page| page.strong_count() > 0);
            }
            pages.insert(id, self.weak_self());
        });
        Some(CorpReport {
            report_only,
            first_url: None,
            sink: Arc::new(move |violation: CorpViolation| {
                poster(Box::new(move || {
                    let page = PAGES.with(|pages| pages.borrow().get(&id).and_then(Weak::upgrade));
                    if let Some(page) = page {
                        queue_violation(&page, violation.kind, &violation.blocked_url, violation.destination, violation.report_only);
                    }
                }));
            }),
        })
    }
}

/// Queues the COEP violation report of `page`'s context, of body type `kind` (`"corp"`, `"navigation"`
/// or `"worker initialization"`): `blocked_url` (stripped for reports) was
/// blocked by its enforced policy, or with `report_only` would have been by its report-only one.
/// It goes to the context's observers and to the `report-to` endpoint of that policy.
pub fn queue_violation(page: &Rc<PageDom>, kind: &str, blocked_url: &str, destination: Option<&str>, report_only: bool) {
    let reporting = page.embedder_reporting();
    let endpoint = if report_only { reporting.report_only_report_to } else { reporting.report_to };
    let mut body = Map::new();
    body.insert("type".into(), kind.into());
    body.insert("blockedURL".into(), strip_url_for_report(blocked_url).into());
    // Only a CORP violation has a destination (the Fetch destination of the blocked request).
    if let Some(destination) = destination {
        body.insert("destination".into(), destination.into());
    }
    body.insert("disposition".into(), if report_only { "reporting" } else { "enforce" }.into());
    queue_report(
        page,
        None,
        ReportInput {
            report_type: "coep",
            url: report_url(page),
            body,
            destination: endpoint,
        },
    );
}

/// The `url` of a report of `page`'s context: its document's URL stripped for reports; Chrome keeps
/// `about:blank` whole (the scheme alone says nothing there).
fn report_url(page: &PageDom) -> String {
    let url = page.document_url();
    if url == "about:blank" {
        url
    } else {
        strip_url_for_report(&url)
    }
}
