//! Cross-Origin-Opener-Policy reporting (HTML § 7.1.3 "cross-origin opener policy reporting"):
//! the `report-to` parameter of `Cross-Origin-Opener-Policy` and of
//! `Cross-Origin-Opener-Policy-Report-Only`, the navigation reports a browsing context group
//! switch makes, and the access reports a window makes when it reaches into a window that the
//! reported policy would have kept out of reach.
//!
//! Every top-level document has a *virtual browsing context group* number (`CoopState::vbcg`):
//! the group its report-only policy would have put it in. A navigation (and a popup) that would
//! switch groups under the report-only policy, or under the enforced one, gets a new number; one
//! that would not keeps its predecessor's. Two windows with different numbers that nevertheless
//! reach each other are the accesses reported ([`access`]).
//!
//! Navigation reports (`type: "coop"`, [`queue_navigation_reports`]) go to the endpoints of the
//! documents on either side of the switch, `navigation-from-response` to the document that is
//! left (its `url` is that document's) and `navigation-to-response` to the one that comes. A
//! redirect response is such a document too, with the `Reporting-Endpoints` of its own response.
//! Access reports are `access-from-coop-page-to-{opener,openee,other}` to the reported
//! document of the window that accessed and `access-to-coop-page-from-...` to the reported
//! document of the window that was accessed; the first kind is also what a `ReportingObserver`
//! of that document sees, as `coop-access-violation`.
//!
//! While no document names a COOP reporting endpoint nothing here runs on an access
//! (`pyro_webapi::coop_reporting_active`); the loader's navigation bookkeeping is a comparison of
//! two small values.

use std::cell::Cell;
use std::rc::{Rc, Weak};

use pyro_webapi::csp_report::strip_url_for_report;
use pyro_webapi::reporting_store::{queue_endpoint_report, queue_endpoint_report_at, queue_report, ReportInput};
use pyro_webapi::PageDom;
use serde_json::Map;

use crate::frames::{frames_same_origin, FrameHost, FrameKey};
use crate::loader::{coop_requires_switch, CoopPolicy, CoopResponse, CoopState, CoopValue};

thread_local! {
    static NEXT_GROUP: Cell<u64> = const { Cell::new(1) };
}

/// A virtual browsing context group number nobody has.
fn new_group() -> u64 {
    NEXT_GROUP.with(|n| n.replace(n.get() + 1))
}

/// The policy name reports use (HTML "cross-origin opener policy value" serialization).
fn policy_name(value: CoopValue) -> &'static str {
    match value {
        CoopValue::UnsafeNone => "unsafe-none",
        CoopValue::SameOriginAllowPopups => "same-origin-allow-popups",
        CoopValue::SameOrigin => "same-origin",
        CoopValue::SameOriginPlusCoep => "same-origin-plus-coep",
        CoopValue::NoopenerAllowPopups => "noopener-allow-popups",
    }
}

/// The `report-to` parameter of a COOP header value that has parsed (`structured_item_token`
/// accepted it): its string, `None` when it is absent or not a string (a token or byte sequence
/// is ignored). The last of a repeated parameter counts.
pub(crate) fn report_to_param(value: &str) -> Option<String> {
    let mut rest = &value[value.find(';')?..];
    let mut found = None;
    while let Some(after) = rest.strip_prefix(';') {
        let after = after.trim_start_matches(' ');
        let key_end = after.find(|c: char| c == '=' || c == ';').unwrap_or(after.len());
        let key = &after[..key_end];
        rest = &after[key_end..];
        let Some(after) = rest.strip_prefix('=') else {
            // A boolean parameter: no value.
            if key == "report-to" {
                found = None;
            }
            continue;
        };
        if let Some(string) = after.strip_prefix('"') {
            // sf-string: `\"` and `\\` are the escapes.
            let mut out = String::new();
            let mut chars = string.char_indices();
            let mut end = string.len();
            while let Some((i, c)) = chars.next() {
                match c {
                    '\\' => {
                        if let Some((_, escaped)) = chars.next() {
                            out.push(escaped);
                        }
                    }
                    '"' => {
                        end = i + 1;
                        break;
                    }
                    c => out.push(c),
                }
            }
            if key == "report-to" {
                found = Some(out);
            }
            rest = &string[end..];
        } else {
            let end = after.find(';').unwrap_or(after.len());
            if key == "report-to" {
                found = None;
            }
            rest = &after[end..];
        }
    }
    found
}

/// What [`queue_navigation_reports`] decided.
pub(crate) struct NavigationReports {
    /// Some response of the chain would have switched the browsing context group under the
    /// enforced or the report-only policy: the committed document is in a new virtual group.
    pub virtual_switch: bool,
    /// The reports the new document receives, for its own endpoints (queued once the loader
    /// has given the document its `Reporting-Endpoints`).
    pub to_document: Vec<ReportInput>,
}

/// HTML "coop navigation reporting" for a top-level navigation whose responses (the redirects
/// and the final one) are `chain`: for each response, under the enforced and under the
/// report-only policies, a browsing context group switch is reported to the document left and the
/// document entered. Reports about the document being replaced, and about redirect responses,
/// are queued now (on the document that left, which is still the page's active one); the
/// returned `to_document` is for the new document. `referrer` is the one the request was sent
/// with; `requested_url` is what the navigation asked for (a popup's `initialPopupURL`).
pub(crate) fn queue_navigation_reports(
    host: &Rc<FrameHost>,
    carrier: Option<&Rc<PageDom>>,
    chain: &[CoopResponse],
    referrer: &str,
    requested_url: &str,
) -> NavigationReports {
    let mut out = NavigationReports { virtual_switch: false, to_document: Vec::new() };
    let (old_doc, mut initial_blank, mut active_enforced, mut active_report_only, mut active_origin) = {
        let mut state = host.coop.borrow_mut();
        if state.initial_blank && state.initial_popup_url.is_none() {
            state.initial_popup_url = Some(requested_url.to_owned());
        }
        (
            state.reporter.as_ref().and_then(Weak::upgrade),
            state.initial_blank,
            CoopPolicy { value: state.value, report_to: state.report_to.clone() },
            state.report_only.clone(),
            state.origin.clone(),
        )
    };
    let old_doc = old_doc.or_else(|| carrier.cloned());
    let mut active_url = old_doc.as_ref().map(|d| d.document_url());
    // The endpoints of the response before: its own for a redirect, the old document's for none.
    let mut active_endpoints: Option<&[(String, pyro_net::Url)]> = None;
    for (i, response) in chain.iter().enumerate() {
        let last = i + 1 == chain.len();
        let enforced_switch =
            coop_requires_switch(initial_blank, active_enforced.value, &active_origin, response.enforced.value, &response.origin);
        let report_only_switch = coop_requires_switch(
            initial_blank,
            active_report_only.value,
            &active_origin,
            response.report_only.value,
            &response.origin,
        );
        out.virtual_switch |= enforced_switch || report_only_switch;
        let same_origin = active_origin == response.origin && response.origin != "null";
        let previous_url = match &active_url {
            Some(url) if same_origin => strip_url_for_report(url),
            _ => String::new(),
        };
        for (switch, disposition, from, to) in [
            (enforced_switch, "enforce", &active_enforced, &response.enforced),
            (report_only_switch, "reporting", &active_report_only, &response.report_only),
        ] {
            if !switch {
                continue;
            }
            if let Some(group) = &from.report_to {
                let mut body = Map::new();
                body.insert("type".into(), "navigation-from-response".into());
                body.insert("disposition".into(), disposition.into());
                body.insert("effectivePolicy".into(), policy_name(from.value).into());
                body.insert("nextResponseURL".into(), strip_url_for_report(&response.url).into());
                let report = ReportInput {
                    report_type: "coop",
                    url: active_url.as_deref().map(strip_url_for_report).unwrap_or_default(),
                    body,
                    destination: Some(group.clone()),
                };
                deliver(old_doc.as_ref(), active_endpoints, group, report);
            }
            if let Some(group) = &to.report_to {
                let mut body = Map::new();
                body.insert("type".into(), "navigation-to-response".into());
                body.insert("disposition".into(), disposition.into());
                body.insert("effectivePolicy".into(), policy_name(to.value).into());
                body.insert("previousResponseURL".into(), previous_url.clone().into());
                body.insert("referrer".into(), referrer.into());
                let report = ReportInput {
                    report_type: "coop",
                    url: strip_url_for_report(&response.url),
                    body,
                    destination: Some(group.clone()),
                };
                if last {
                    out.to_document.push(report);
                } else {
                    deliver(old_doc.as_ref(), Some(response.endpoints.as_slice()), group, report);
                }
            }
        }
        initial_blank = false;
        active_enforced = response.enforced.clone();
        active_report_only = response.report_only.clone();
        active_origin = response.origin.clone();
        active_url = Some(response.url.clone());
        active_endpoints = Some(response.endpoints.as_slice());
    }
    out
}

/// Queues `report` for the endpoint `group`: of a redirect response (`endpoints`), through
/// `carrier`, or of `carrier`'s own document when there are none.
fn deliver(carrier: Option<&Rc<PageDom>>, endpoints: Option<&[(String, pyro_net::Url)]>, group: &str, report: ReportInput) {
    let Some(carrier) = carrier else { return };
    match endpoints {
        None => queue_endpoint_report(carrier, report),
        Some(endpoints) => {
            if let Some((_, url)) = endpoints.iter().find(|(name, _)| name == group) {
                queue_endpoint_report_at(carrier, url, report);
            }
        }
    }
}

impl FrameHost {
    /// The COOP state of a popup this page's top-level document creates, for the popup's initial
    /// about:blank document: the creator's policies, reporting endpoints and virtual group, with
    /// the creator document's `origin`.
    pub(crate) fn coop_for_popup(&self, origin: String) -> CoopState {
        let vbcg = self.vbcg();
        let state = self.coop.borrow();
        CoopState {
            value: state.value,
            origin,
            initial_blank: true,
            report_to: state.report_to.clone(),
            report_only: state.report_only.clone(),
            reporter: state.reporter.clone().or_else(|| {
                self.page_dom(FrameKey::from(self.page)).map(|dom| Rc::downgrade(&dom))
            }),
            vbcg,
            initial_popup_url: None,
            reported: Vec::new(),
        }
    }

    /// This top-level document's virtual browsing context group, numbered on first use.
    pub(crate) fn vbcg(&self) -> u64 {
        let mut state = self.coop.borrow_mut();
        if state.vbcg == 0 {
            state.vbcg = new_group();
        }
        state.vbcg
    }

    /// The top-level document `page_dom` committed with the response `last`: its reporting
    /// endpoints and report-only policy are now the ones its navigation reports and its access
    /// reports go by. `virtual_switch`: its navigation would have changed the browsing context
    /// group (`NavigationReports::virtual_switch`, or a real switch).
    pub(crate) fn coop_note_commit(&self, last: &CoopResponse, page_dom: &Rc<PageDom>, virtual_switch: bool) {
        let vbcg = {
            let state = self.coop.borrow();
            if virtual_switch || state.vbcg == 0 { new_group() } else { state.vbcg }
        };
        let current = self.coop.borrow().clone();
        self.set_coop(CoopState {
            report_to: last.enforced.report_to.clone(),
            report_only: last.report_only.clone(),
            reporter: Some(Rc::downgrade(page_dom)),
            vbcg,
            reported: Vec::new(),
            ..current
        });
    }
}

/// How a window's document is known to another document's `coop_access`.
pub(crate) struct CoopSide {
    pub host: Weak<FrameHost>,
    pub key: FrameKey,
}

/// How two top-level windows are related, from the point of view of the one whose policy reports.
#[derive(Clone, Copy)]
enum Relation {
    /// The other window was opened by this one.
    Opener,
    /// The other window opened this one.
    Openee,
    Other,
}

/// HTML "cross-origin opener policy access reporting": a script of the window `accessor_key` of
/// `accessor_host`, running in `scope`, reached `property` of the window `target` (a frame of
/// `target_page`'s host). When the two are in different virtual browsing context groups, the
/// report-only policy of either side's top-level document that names an endpoint gets a report.
pub(crate) fn access(
    accessor_host: &Rc<FrameHost>,
    accessor_key: FrameKey,
    scope: &pyro_js::Scope<'_>,
    target_page: &Rc<PageDom>,
    target: &CoopSide,
    property: &str,
) {
    let Some(target_host) = target.host.upgrade() else { return };
    if Rc::ptr_eq(accessor_host, &target_host) {
        // Frames of one page are in one group.
        return;
    }
    if accessor_host.vbcg() == target_host.vbcg() {
        return;
    }
    let opener_of = |host: &Rc<FrameHost>| host.opener.borrow().as_ref().and_then(Weak::upgrade);
    let relation = |coop: &Rc<FrameHost>, other: &Rc<FrameHost>| {
        if opener_of(other).is_some_and(|o| Rc::ptr_eq(&o, coop)) {
            Relation::Opener
        } else if opener_of(coop).is_some_and(|o| Rc::ptr_eq(&o, other)) {
            Relation::Openee
        } else {
            Relation::Other
        }
    };
    let Some(accessor_page) = accessor_host.page_dom(accessor_key) else { return };
    let accessor_side = Side { host: accessor_host, key: accessor_key, page: &accessor_page };
    let target_side = Side { host: &target_host, key: target.key, page: target_page };
    // The accessing window's own policy (and the observers of its document), then the accessed's.
    report_access(&accessor_side, &target_side, relation(accessor_host, &target_host), property, Some(scope));
    report_access(&target_side, &accessor_side, relation(&target_host, accessor_host), property, None);
}

struct Side<'a> {
    host: &'a Rc<FrameHost>,
    key: FrameKey,
    page: &'a Rc<PageDom>,
}

/// The access report of the window `coop` (whose top-level document's report-only policy is
/// reported to) about the window `other`. `scope`: `coop` is the window that accessed, and this
/// is its running script (the report's source location, and its `ReportingObserver`s).
fn report_access(coop: &Side<'_>, other: &Side<'_>, relation: Relation, property: &str, scope: Option<&pyro_js::Scope<'_>>) {
    let from_coop_page = scope.is_some();
    let (policy, reporter) = {
        let state = coop.host.coop.borrow();
        let Some(group) = state.report_only.report_to.clone() else { return };
        let Some(reporter) = state.reporter.as_ref().and_then(Weak::upgrade) else { return };
        (CoopPolicy { value: state.report_only.value, report_to: Some(group) }, reporter)
    };
    // A frame reports for its top-level document only when it is same origin with it.
    if coop.key.child.is_some() && !coop.host.same_origin(coop.key, FrameKey::from(coop.host.page)) {
        return;
    }
    let key = (from_coop_page, other.page.id(), property.to_owned());
    {
        let mut state = coop.host.coop.borrow_mut();
        if state.reported.contains(&key) {
            return;
        }
        state.reported.push(key);
    }
    let (Some(coop_frame), Some(other_frame)) = (coop.host.frame(coop.key), other.host.frame(other.key)) else { return };
    let visible_url =
        |url: String| if frames_same_origin(&coop_frame, &other_frame) { strip_url_for_report(&url) } else { String::new() };
    let suffix = match relation {
        Relation::Opener => "openee",
        Relation::Openee => "opener",
        Relation::Other => "other",
    };
    let report_type = if from_coop_page {
        format!("access-from-coop-page-to-{suffix}")
    } else {
        format!("access-to-coop-page-from-{suffix}")
    };
    let mut body = Map::new();
    body.insert("type".into(), report_type.into());
    body.insert("property".into(), property.into());
    body.insert("disposition".into(), "reporting".into());
    body.insert("effectivePolicy".into(), policy_name(policy.value).into());
    if let Some(scope) = scope {
        if let Some((file, line, column)) = pyro_webapi::csp::running_script_location(scope) {
            body.insert("sourceFile".into(), file.split('#').next().unwrap_or_default().into());
            body.insert("lineNumber".into(), line.into());
            // JavaScriptCore places a property read at the `.` of `w.blur`, V8 (what a report's
            // `columnNumber` is specified by, and what pages expect) at the property's name.
            body.insert("columnNumber".into(), (column + 1).into());
        }
    }
    let other_url = visible_url(other.page.document_url());
    match relation {
        Relation::Opener => {
            body.insert("openeeURL".into(), other_url.into());
            if let Some(url) = other.host.coop.borrow().initial_popup_url.clone() {
                body.insert("initialPopupURL".into(), url.into());
            }
        }
        Relation::Openee => {
            body.insert("openerURL".into(), other_url.into());
            body.insert("referrer".into(), reporter.document_referrer().into());
        }
        Relation::Other => {
            body.insert("otherDocumentURL".into(), other_url.into());
        }
    }
    let url = strip_url_for_report(&reporter.document_url());
    let group = policy.report_to.clone();
    // The observers hear the report of the window that accessed, under the type they know.
    if let Some(scope) = scope {
        let observed = ReportInput { report_type: "coop-access-violation", url: url.clone(), body: body.clone(), destination: None };
        let own_realm = Rc::ptr_eq(&reporter, coop.page);
        queue_report(&reporter, own_realm.then_some(scope), observed);
    }
    queue_endpoint_report(&reporter, ReportInput { report_type: "coop", url, body, destination: group });
}
