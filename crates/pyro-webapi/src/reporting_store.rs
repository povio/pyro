//! The Reporting API's generating side: the one interface Rust code uses to make a report of any
//! type, and the stores it delivers through. COOP and COEP reporting, Document-Isolation-Policy
//! reporting, Permissions Policy violations, NEL and CSP hash reports build on this module. CSP
//! violations and deprecations predate it and keep their own entry points (`crate::csp_report`,
//! `crate::reporting`), which share its delivery.
//!
//! # Queueing a report
//!
//! ```ignore
//! use pyro_webapi::reporting_store::{queue_report, queue_report_for_document, ReportInput};
//!
//! let mut body = serde_json::Map::new();
//! body.insert("blockedURL".into(), "https://other.test/x.png".into());
//! body.insert("destination".into(), "image".into());
//! body.insert("disposition".into(), "enforce".into());
//! body.insert("type".into(), "corp".into());
//! queue_report(&page, Some(&scope), ReportInput {
//!     report_type: "coep",
//!     url: strip_url_for_report(&document_url),   // the report's `url`, already stripped
//!     body,
//!     destination: Some("coep-endpoint".into()),  // None: observers only
//! });
//! ```
//!
//! - [`ReportInput::report_type`] is the Reporting API type string: `"test"`, `"coop"`, `"coep"`,
//!   `"permissions-policy-violation"`, `"network-error"`, `"csp-hash"`, ... A type with no entry in
//!   the `BODY_TYPES` table of reporting.js still reaches observers and endpoints: its body object
//!   (`ReportBody`'s `toJSON()` included) has the fields given. A type that wants Chrome's body
//!   interface (its class name, getters on the prototype) adds a class and a row to that table.
//! - [`ReportInput::body`] holds the body's fields exactly as Chrome serializes them (camelCase
//!   as the IDL has them). The same map is the `body` an endpoint receives and the fields the
//!   `ReportingObserver`'s `Report.body` reads, so `JSON.stringify(report)` and the POSTed JSON
//!   agree. Put `null` (not a missing key) for a nullable member that has no value.
//! - [`ReportInput::url`] is the `url` of the report, the generating global's URL already
//!   stripped for reports (`crate::csp_report::strip_url_for_report`: no fragment, no
//!   credentials).
//! - [`ReportInput::destination`] is the name of the endpoint group the report goes to over the
//!   network: a `Reporting-Endpoints` name of the document that generated the report, else a
//!   `Report-To` group of its origin (see below). `None`, or a name the document has neither
//!   way, delivers to observers only, as the spec says (a report with no destination is only
//!   observable).
//!
//! The two entry points differ in which observers hear the report:
//! - [`queue_report`]`(page, Some(&scope), r)`: the report of the realm `scope` (a Window's or a
//!   worker's global; for a worker `page` is its owner document, whose endpoints a worker's reports
//!   use). Call it with the scope you already hold when you run inside script, e.g. from a
//!   native method of a class; `scope` must be the realm of the global that generated the report.
//! - [`queue_report_for_document`]`(page, r)` and `queue_report(page, None, r)`: the report of
//!   `page`'s document, for code that runs in a task with no scope at hand (a navigation commit,
//!   a network callback hop). It finds the document's main-world realm itself and drops the
//!   observer half when the document has none yet. Do NOT call it from inside a native method
//!   that runs script: pass the scope instead.
//!
//! The call returns before anything is sent and never throws into script. Observers get their
//! callback in a task of their own (reporting.js). The network half is queued in the document
//! ([`PageDom`]`::csp_reports`) and sent at the end of the current task as one POST per
//! (endpoint URL, document) holding every report of that document for the endpoint that this task
//! made, `application/reports+json`, `cors`, credentials `same-origin`, no referrer, the
//! document's origin (`crate::csp_report::flush_pending`). Reports of different documents are
//! never in one request, even for the same URL (Chrome keys its queues by reporting source). The
//! document's cap `crate::csp_report::MAX_REPORTS_PER_DOCUMENT` covers every type. Each report
//! carries `age` (milliseconds since it was queued), `type`, `url`, `user_agent`, `body`.
//!
//! A report about a response that is not (yet) a document, such as COOP navigation reports for
//! the NEW document at commit time, is queued on the `PageDom` that will show it, after
//! `PageDom::set_reporting_endpoints` and `PageDom::set_report_to` ran for that response (the
//! loader calls both when it commits), so its `destination` resolves against the new response's
//! headers; a report for the OLD document is queued before that commit, as the previous
//! document's pending reports are flushed (to the endpoints they were queued for) when the commit
//! replaces its state.
//!
//! [`generate_test_report`] is the `test` report of the Reporting API's "generate test report"
//! command (`Page.generateTestReport` over CDP).
//!
//! # Endpoints
//!
//! - `Reporting-Endpoints` (a structured-field dictionary of name to URL, per document):
//!   `crate::csp_report::parse_reporting_endpoints`, set by `PageDom::set_reporting_endpoints`.
//! - `Report-To` (legacy JSON groups, per origin, surviving navigation): [`parse_report_to`],
//!   stored by `PageDom::set_report_to` in the browser context's [`ReportToStore`]
//!   (`ContextServices::report_to`). A group needs `max_age` and at least one usable endpoint; a
//!   header replaces all the origin's groups (Chrome); `max_age: 0` drops a group; only a
//!   potentially trustworthy response sets groups. [`report_to_endpoint`] resolves a name for a
//!   document: its own `Reporting-Endpoints` first (`ReportState::endpoint`), then the origin's
//!   groups (`include_subdomains` groups of a parent domain too). Of a group's endpoints the lowest
//!   `priority` number wins and, among those, the highest `weight` (the spec picks randomly by
//!   weight; one choice keeps requests predictable). No failover to the next endpoint.
//!   NEL (`network-error` reports) resolves its `report_to` through [`report_to_endpoint_for`].
//!
//! Not implemented: retry of failed uploads (Chrome keeps a failed report for later attempts with
//! exponential backoff, a minute and up to five failures; no WPT observes it), persistence of
//! `Report-To` groups across browser restarts, and endpoint failover.

use std::cmp::Reverse;
use std::rc::Rc;
use std::time::{Duration, Instant};

use pyro_js::Scope;
use pyro_net::Url;
use serde_json::{Map, Value as Json};

use crate::csp_report;
use crate::page::PageDom;

/// A report to make: see the module documentation.
#[derive(Clone, Debug, PartialEq)]
pub struct ReportInput {
    /// The report's type: `"test"`, `"coop"`, `"coep"`, `"permissions-policy-violation"`, ...
    pub report_type: &'static str,
    /// The report's `url` (stripped for reports).
    pub url: String,
    /// The body's fields, as Chrome serializes them.
    pub body: Map<String, Json>,
    /// The endpoint group the report is sent to; `None`: observers only.
    pub destination: Option<String>,
}

/// Queues `r` for the realm `scope`'s `ReportingObserver`s and for the document's endpoint named
/// `r.destination` (module documentation). `scope: None` finds the document's main-world realm.
pub fn queue_report(page: &Rc<PageDom>, scope: Option<&Scope<'_>>, r: ReportInput) {
    let ReportInput { report_type, url, body, destination } = r;
    let body = Json::Object(body);
    match scope {
        Some(s) => observe(s, page, report_type, &url, &body),
        None => {
            if let Some(realm) = page.main_world_realm() {
                if let Ok(s) = realm.scope() {
                    observe(&s, page, report_type, &url, &body);
                }
            }
        }
    }
    if let Some(group) = destination {
        csp_report::report_to_group(page, &group, report_type, &url, body);
    }
}

/// [`queue_report`] for the document `page` shows, from code with no scope at hand.
pub fn queue_report_for_document(page: &Rc<PageDom>, r: ReportInput) {
    queue_report(page, None, r);
}

/// The `test` report of "generate test report" (Reporting API; WebDriver's Generate Test Report
/// command, CDP's `Page.generateTestReport`): `message` as the body's `message`, to the
/// document's observers and to the endpoint `group` (`default` when the caller gave none).
pub fn generate_test_report(page: &Rc<PageDom>, message: &str, group: &str) {
    let mut body = Map::new();
    body.insert("message".to_owned(), message.into());
    queue_report_for_document(
        page,
        ReportInput {
            report_type: "test",
            url: csp_report::strip_url_for_report(&page.document_url()),
            body,
            destination: Some(group.to_owned()),
        },
    );
}

/// Hands the report to reporting.js `queue`, which makes its `Report` and body object and wakes
/// the observers of `s`'s realm. A failure (a realm going away) drops the observer half only.
fn observe(s: &Scope<'_>, page: &Rc<PageDom>, report_type: &str, url: &str, body: &Json) {
    let json = body.to_string();
    let _ = crate::reporting::MODULE.helper(s, page, "queue", &[s.string(report_type), s.string(url), s.string(&json)]);
}

/// One endpoint of a `Report-To` group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Endpoint {
    url: Url,
    priority: u64,
    weight: u64,
}

/// A `Report-To` endpoint group of one origin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Group {
    name: String,
    include_subdomains: bool,
    expires: Instant,
    endpoints: Vec<Endpoint>,
}

impl Group {
    /// The endpoint to use: lowest priority number, then highest weight.
    fn best(&self) -> Option<&Endpoint> {
        self.endpoints.iter().min_by_key(|e| (e.priority, Reverse(e.weight)))
    }
}

/// The groups of the `Report-To` header `value` (the comma-separated JSON objects of the header,
/// as a list without its brackets), the endpoint URLs resolved against `base`, expiring `max_age`
/// seconds after `now`. `None` when the value is not a JSON list. A member that is not an object,
/// has no non-negative numeric `max_age`, a non-string `group`, or no usable endpoint is skipped
/// (`max_age: 0` also removes an earlier group of the same name in this header); a group named
/// twice keeps the last; `group` defaults to `default`.
pub(crate) fn parse_report_to(value: &str, base: &Url, now: Instant) -> Option<Vec<Group>> {
    let Json::Array(items) = serde_json::from_str::<Json>(&format!("[{value}]")).ok()? else { return None };
    let mut groups: Vec<Group> = Vec::new();
    for item in items {
        let Json::Object(item) = item else { continue };
        let name = match item.get("group") {
            None => "default".to_owned(),
            Some(Json::String(name)) => name.clone(),
            Some(_) => continue,
        };
        let Some(max_age) = item.get("max_age").and_then(Json::as_f64).filter(|v| v.is_finite() && *v >= 0.0) else {
            continue;
        };
        if max_age == 0.0 {
            groups.retain(|g| g.name != name);
            continue;
        }
        let include_subdomains = item.get("include_subdomains").and_then(Json::as_bool).unwrap_or(false);
        let mut endpoints = Vec::new();
        if let Some(Json::Array(list)) = item.get("endpoints") {
            for endpoint in list {
                let Json::Object(endpoint) = endpoint else { continue };
                let Some(url) = endpoint.get("url").and_then(Json::as_str).and_then(|u| base.join(u).ok()) else {
                    continue;
                };
                if !matches!(url.scheme(), "http" | "https") || !pyro_net::is_potentially_trustworthy_url(&url) {
                    continue;
                }
                let number = |key: &str| endpoint.get(key).and_then(Json::as_u64).unwrap_or(1);
                endpoints.push(Endpoint { url, priority: number("priority"), weight: number("weight") });
            }
        }
        if endpoints.is_empty() {
            continue;
        }
        groups.retain(|g| g.name != name);
        // (Two years at most: keeps the sum an `Instant` can hold.)
        let lifetime = Duration::from_secs_f64(max_age.min(63_072_000.0));
        groups.push(Group { name, include_subdomains, expires: now + lifetime, endpoints });
    }
    Some(groups)
}

/// The `Report-To` groups a browser context's responses delivered, by the origin of the response.
#[derive(Default)]
pub(crate) struct ReportToStore {
    origins: Vec<OriginGroups>,
}

struct OriginGroups {
    /// The URL of the response that delivered the groups, for its origin.
    base: Url,
    groups: Vec<Group>,
}

impl ReportToStore {
    /// A response from `base`'s origin delivered `groups`: they replace everything that origin
    /// had; none leaves it with nothing.
    pub(crate) fn set(&mut self, base: &Url, groups: Vec<Group>) {
        let origin = base.origin();
        self.origins.retain(|o| o.base.origin() != origin);
        if !groups.is_empty() {
            self.origins.push(OriginGroups { base: base.clone(), groups });
        }
    }

    /// The endpoint for group `name` of a reporting source at `source`: the group the source's own
    /// origin delivered, else an `include_subdomains` group of a parent domain with the same scheme
    /// and port. Expired groups do not count.
    pub(crate) fn lookup(&self, source: &Url, name: &str, now: Instant) -> Option<Url> {
        let live = |o: &OriginGroups, subdomain: bool| {
            o.groups
                .iter()
                .find(|g| g.name == name && g.expires > now && (!subdomain || g.include_subdomains))
                .and_then(Group::best)
                .map(|e| e.url.clone())
        };
        let origin = source.origin();
        if let Some(url) = self.origins.iter().filter(|o| o.base.origin() == origin).find_map(|o| live(o, false)) {
            return Some(url);
        }
        let host = match source.host() {
            Some(url::Host::Domain(host)) => host,
            _ => return None,
        };
        self.origins
            .iter()
            .filter(|o| {
                o.base.scheme() == source.scheme()
                    && o.base.port_or_known_default() == source.port_or_known_default()
                    && matches!(o.base.host(), Some(url::Host::Domain(parent)) if host.len() > parent.len()
                        && host.ends_with(parent) && host.as_bytes()[host.len() - parent.len() - 1] == b'.')
            })
            .find_map(|o| live(o, true))
    }
}

/// The endpoint of group `name` for a reporting source at `source` (a document or a response URL
/// that has no document), from the browser context of `page`.
pub(crate) fn report_to_endpoint_for(page: &PageDom, source: &Url, name: &str) -> Option<Url> {
    page.context_services().report_to.borrow().lookup(source, name, Instant::now())
}

/// The `Report-To` endpoint of group `name` for the document `page` shows (the fallback of a
/// name its `Reporting-Endpoints` lacks).
pub(crate) fn report_to_endpoint(page: &PageDom, name: &str) -> Option<Url> {
    report_to_endpoint_for(page, &page.document_url_parsed()?, name)
}

impl PageDom {
    /// The response the page's document came from has the `Report-To` header `value` (`None`: it
    /// had none, which changes nothing). The loader calls this when it commits a document, next
    /// to `set_reporting_endpoints`. Only a potentially trustworthy document's origin gets
    /// groups, and a header that is not a JSON list is ignored.
    pub fn set_report_to(&self, value: Option<&str>) {
        let (Some(value), Some(base)) = (value, self.document_url_parsed()) else { return };
        if !pyro_net::is_potentially_trustworthy_url(&base) {
            return;
        }
        let Some(groups) = parse_report_to(value, &base, Instant::now()) else { return };
        self.context_services().report_to.borrow_mut().set(&base, groups);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Url {
        Url::parse("https://a.test/dir/page").unwrap()
    }

    #[test]
    fn report_to_groups() {
        let now = Instant::now();
        let value = r#"{"group":"g","max_age":10,"endpoints":[{"url":"/r1","priority":2},{"url":"https://b.test/r2","priority":1,"weight":3},{"url":"http://insecure.test/x"}]}, {"max_age":5,"endpoints":[{"url":"/d"}]}, {"group":"gone","max_age":0,"endpoints":[{"url":"/x"}]}, {"group":7,"max_age":1,"endpoints":[{"url":"/x"}]}, {"group":"noendpoints","max_age":1,"endpoints":[]}, {"group":"nomaxage","endpoints":[{"url":"/x"}]}, 5"#;
        let groups = parse_report_to(value, &base(), now).unwrap();
        let names: Vec<_> = groups.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, ["g", "default"]);
        assert_eq!(groups[0].endpoints.len(), 2);
        assert_eq!(groups[0].best().unwrap().url.as_str(), "https://b.test/r2");
        assert_eq!(groups[1].best().unwrap().url.as_str(), "https://a.test/d");
        assert!(parse_report_to("not json", &base(), now).is_none());
        assert_eq!(parse_report_to("", &base(), now).unwrap().len(), 0);
    }

    #[test]
    fn priority_then_weight() {
        let value = r#"{"group":"g","max_age":10,"endpoints":[{"url":"/a","priority":1,"weight":1},{"url":"/b","priority":1,"weight":5},{"url":"/c","priority":0,"weight":1}]}"#;
        let groups = parse_report_to(value, &base(), Instant::now()).unwrap();
        assert_eq!(groups[0].best().unwrap().url.path(), "/c");
        let value = r#"{"group":"g","max_age":10,"endpoints":[{"url":"/a","priority":1,"weight":1},{"url":"/b","priority":1,"weight":5}]}"#;
        let groups = parse_report_to(value, &base(), Instant::now()).unwrap();
        assert_eq!(groups[0].best().unwrap().url.path(), "/b");
    }

    #[test]
    fn store_replaces_expires_and_matches_subdomains() {
        let now = Instant::now();
        let mut store = ReportToStore::default();
        let header = r#"{"group":"g","max_age":10,"include_subdomains":true,"endpoints":[{"url":"/e"}]}"#;
        store.set(&base(), parse_report_to(header, &base(), now).unwrap());
        let same = Url::parse("https://a.test/other").unwrap();
        let sub = Url::parse("https://x.y.a.test/").unwrap();
        let other_port = Url::parse("https://a.test:8443/").unwrap();
        let unrelated = Url::parse("https://nota.test/").unwrap();
        assert_eq!(store.lookup(&same, "g", now).unwrap().as_str(), "https://a.test/e");
        assert_eq!(store.lookup(&sub, "g", now).unwrap().as_str(), "https://a.test/e");
        assert!(store.lookup(&other_port, "g", now).is_none());
        assert!(store.lookup(&unrelated, "g", now).is_none());
        assert!(store.lookup(&same, "missing", now).is_none());
        assert!(store.lookup(&same, "g", now + Duration::from_secs(11)).is_none());
        // A header without include_subdomains does not cover subdomains.
        let header = r#"{"group":"g","max_age":10,"endpoints":[{"url":"/e"}]}"#;
        store.set(&base(), parse_report_to(header, &base(), now).unwrap());
        assert!(store.lookup(&sub, "g", now).is_none());
        assert!(store.lookup(&same, "g", now).is_some());
        // A header with only deleted groups empties the origin.
        let header = r#"{"group":"g","max_age":0}"#;
        store.set(&base(), parse_report_to(header, &base(), now).unwrap());
        assert!(store.lookup(&same, "g", now).is_none());
    }
}
