//! CSP violation report serialization and delivery: the reporting half of CSP 3 "report a
//! violation", after the `securitypolicyviolation` event. If the violated policy has
//! `report-to`, the report goes to that Reporting API endpoint as `application/reports+json`, and
//! its `report-uri` is ignored. Otherwise each `report-uri` gets an `application/csp-report` POST.
//!
//! This module decides nothing and fires no event. Its caller has already found the violation
//! and dispatched the event, then hands over a [`Violation`] and the policy's [`ReportTargets`]:
//! - Trusted Types (`trusted_types.js`): after its event, in the same task, through the
//!   module's `deliverReport` native.
//! - The CSP fetch/inline/eval core (`crate::csp::report`): after `dispatch`, in the same
//!   task, a document's violation through `crate::csp::report_parts`. Its `report_uris`
//!   already leave out a `<meta>` policy's `report-uri`, as [`ReportTargets::from_policy`] does
//!   here.
//!
//! `crate::reporting` shares two pieces: the `CSPViolationReportBody` fields its
//! `ReportingObserver`s get (`csp_violation_body`), and the Reporting API send of a report of
//! another type to a named group (`report_to_group`, for `deprecation` reports).
//!
//! Reports of every other type come through `crate::reporting_store` (its top documents the
//! interface), which hands them to [`report_to_group`]: those wait in the document's queue until
//! the end of the task that made them, and go out as ONE list per endpoint
//! ([`flush_pending`]). A group name the document's `Reporting-Endpoints` does not have is looked
//! up in the browser context's `Report-To` groups (`crate::reporting_store::ReportToStore`).
//!
//! Requests go out through the page's network client, shaped like a `navigator.sendBeacon`
//! POST that outlives its document, with no Resource Timing entry (as in Chrome). The two kinds
//! stay apart ([`Delivery`]). A `report-uri` report is `no-cors` with credentials `same-origin`
//! and the document's referrer (CSP 3). A `report-to` report is `cors` with credentials
//! `same-origin` (as Chrome, which sends a same-origin endpoint its cookies; the Reporting API
//! says `omit`) and no referrer. Only `http:` and `https:` endpoints are used, and a
//! `report-to` endpoint only when the document is potentially trustworthy.
//!
//! Bookkeeping is per document and lives on the [`PageDom`] (`csp_reports`): the endpoints from
//! the document's `Reporting-Endpoints` header, which the loader sets when it commits the
//! document ([`PageDom::set_reporting_endpoints`]), and a count of reports sent, capped at
//! [`MAX_REPORTS_PER_DOCUMENT`]. Both reset when the page shows another document, and they go
//! when the page does.

use std::rc::Rc;

use bytes::Bytes;
use pyro_dom::NodeId;
use pyro_net::http::{header, HeaderValue, Method};
use pyro_net::{Credentials, Destination, Request, RequestMode, Url};
use serde_json::{Map, Value as Json};

use crate::page::PageDom;

/// The most reports one document sends (both kinds together), so a violation in a loop cannot
/// flood an endpoint.
pub const MAX_REPORTS_PER_DOCUMENT: u32 = 100;

/// A violation as CSP 3 describes it: the fields its report and its event share.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Violation {
    /// The URL of the violation's global, i.e. the document's URL or a worker's. Empty: the
    /// page's document URL.
    pub url: String,
    /// The document's referrer. Empty: the page document's referrer, if any.
    pub referrer: String,
    /// A URL, or one of the non-URL resources (`inline`, `eval`, `wasm-eval`,
    /// `trusted-types-policy`, `trusted-types-sink`); empty for none.
    pub blocked: String,
    pub effective_directive: String,
    /// The violated policy's serialization.
    pub policy: String,
    /// Whether the policy was delivered as report-only.
    pub report_only: bool,
    pub source_file: String,
    /// Already clipped by the enforcement point. For CSP: the first 40 characters of the code.
    /// For Trusted Types: the sink name, `|`, and the first 40 characters of the value.
    pub sample: String,
    pub status_code: u16,
    pub line: u32,
    pub column: u32,
}

/// Where a policy sends its reports: its `report-uri` references as written, and its `report-to`
/// group. A policy with a group reports there only.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReportTargets {
    pub uris: Vec<String>,
    pub group: Option<String>,
}

fn is_ascii_ws(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\x0c' | '\r' | ' ')
}

impl ReportTargets {
    /// The reporting directives of one serialized policy (CSP 3 "parse a serialized CSP", as
    /// `pyro_net::csp::Policy::parse` reads them). Names are ASCII case-insensitive. A name that
    /// is not alphanumeric-and-dash is skipped. The first directive of a name wins.
    /// `report-to` takes its first token. With `meta`, the policy came from a
    /// `<meta http-equiv>`, which cannot carry `report-uri` (CSP 3 §3.3); its `report-to`
    /// still counts.
    pub fn from_policy(policy: &str, meta: bool) -> ReportTargets {
        let mut seen: Vec<String> = Vec::new();
        let mut targets = ReportTargets::default();
        for part in policy.split(';') {
            let mut tokens = part.split(is_ascii_ws).filter(|t| !t.is_empty());
            let Some(name) = tokens.next() else { continue };
            let name = name.to_ascii_lowercase();
            if !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') || seen.contains(&name) {
                continue;
            }
            match name.as_str() {
                "report-uri" if meta => continue,
                "report-uri" => targets.uris = tokens.map(str::to_owned).collect(),
                "report-to" => targets.group = tokens.next().map(str::to_owned),
                _ => {}
            }
            seen.push(name);
        }
        targets
    }
}

/// CSP 3 "strip URL for use in reports". A URL whose scheme is not HTTP(S) becomes just its
/// scheme. Any other URL loses its fragment and credentials. A value that is not an absolute
/// URL (`inline`, `eval`, `trusted-types-sink`, empty) is kept as it is.
pub fn strip_url_for_report(value: &str) -> String {
    let Ok(mut url) = Url::parse(value) else { return value.to_owned() };
    if !matches!(url.scheme(), "http" | "https") {
        return url.scheme().to_owned();
    }
    url.set_fragment(None);
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.into()
}

fn disposition(v: &Violation) -> &'static str {
    if v.report_only {
        "report"
    } else {
        "enforce"
    }
}

/// The `application/csp-report` body for `report-uri` (CSP 3 "obtain the deprecated
/// serialization of violation"). Like Chrome, `source-file`, `line-number` and `column-number`
/// appear only when the violation has a source location.
pub fn serialize_csp_report(v: &Violation) -> String {
    let mut body = Map::new();
    let mut put = |k: &str, value: Json| {
        body.insert(k.to_owned(), value);
    };
    put("document-uri", strip_url_for_report(&v.url).into());
    put("referrer", strip_url_for_report(&v.referrer).into());
    put("violated-directive", v.effective_directive.clone().into());
    put("effective-directive", v.effective_directive.clone().into());
    put("original-policy", v.policy.clone().into());
    put("disposition", disposition(v).into());
    put("blocked-uri", strip_url_for_report(&v.blocked).into());
    put("status-code", v.status_code.into());
    put("script-sample", v.sample.clone().into());
    if !v.source_file.is_empty() {
        put("source-file", strip_url_for_report(&v.source_file).into());
        put("line-number", v.line.into());
        put("column-number", v.column.into());
    }
    let mut outer = Map::new();
    outer.insert("csp-report".to_owned(), Json::Object(body));
    Json::Object(outer).to_string()
}

/// The `application/reports+json` body for `report-to`: a list holding one Reporting API report
/// of type `csp-violation` (CSP 3 `CSPViolationReportBody`). `user_agent` is the sender's.
pub fn serialize_reporting_api(v: &Violation, user_agent: &str) -> String {
    reporting_api_list("csp-violation", &strip_url_for_report(&v.url), user_agent, Json::Object(csp_violation_body(v)))
}

/// An `application/reports+json` body holding one report of `report_type` from `url`.
fn reporting_api_list(report_type: &str, url: &str, user_agent: &str, body: Json) -> String {
    Json::Array(vec![report_value(report_type, url, user_agent, 0, body)]).to_string()
}

/// One report of an `application/reports+json` list (Reporting API "serialize reports"). `age`
/// is in milliseconds.
fn report_value(report_type: &str, url: &str, user_agent: &str, age: u64, body: Json) -> Json {
    let mut report = Map::new();
    report.insert("type".to_owned(), report_type.into());
    report.insert("age".to_owned(), age.into());
    report.insert("url".to_owned(), url.into());
    report.insert("user_agent".to_owned(), user_agent.into());
    report.insert("body".to_owned(), body);
    Json::Object(report)
}

/// The fields of `v`'s `CSPViolationReportBody`, as a `report-to` endpoint and a
/// `ReportingObserver` (`crate::reporting`) both get them. Without a source location,
/// `sourceFile`, `lineNumber` and `columnNumber` are null.
pub(crate) fn csp_violation_body(v: &Violation) -> Map<String, Json> {
    let located = !v.source_file.is_empty();
    let mut body = Map::new();
    let mut put = |k: &str, value: Json| {
        body.insert(k.to_owned(), value);
    };
    put("documentURL", strip_url_for_report(&v.url).into());
    put("referrer", strip_url_for_report(&v.referrer).into());
    put("blockedURL", strip_url_for_report(&v.blocked).into());
    put("effectiveDirective", v.effective_directive.clone().into());
    put("originalPolicy", v.policy.clone().into());
    put("sourceFile", if located { strip_url_for_report(&v.source_file).into() } else { Json::Null });
    put("sample", v.sample.clone().into());
    put("disposition", disposition(v).into());
    put("statusCode", v.status_code.into());
    put("lineNumber", if located { v.line.into() } else { Json::Null });
    put("columnNumber", if located { v.column.into() } else { Json::Null });
    body
}

/// The endpoints of a `Reporting-Endpoints` header value (Reporting API "process reporting
/// endpoints"). The value is a structured-field dictionary of `name="url"` members, each URL
/// resolved against `base`. A member is left out when its value is not a string, does not
/// resolve, or is not a potentially trustworthy http(s) URL. A repeated name keeps its last
/// value.
pub fn parse_reporting_endpoints(value: &str, base: &Url) -> Vec<(String, Url)> {
    let mut out: Vec<(String, Url)> = Vec::new();
    for member in split_dictionary(value) {
        let member = member.trim_matches(|c| c == ' ' || c == '\t');
        let Some((name, rest)) = member.split_once('=') else { continue };
        let name = name.trim();
        if !name.bytes().next().is_some_and(|b| b.is_ascii_lowercase() || b == b'*')
            || !name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-.*".contains(&b))
        {
            continue;
        }
        // The value is an sf-string; parameters after it (`;...`) are ignored.
        let Some(url) = parse_sf_string(rest.trim_start()) else { continue };
        let Ok(url) = base.join(&url) else { continue };
        if !matches!(url.scheme(), "http" | "https") || !pyro_net::is_potentially_trustworthy_url(&url) {
            continue;
        }
        out.retain(|(n, _)| n != name);
        out.push((name.to_owned(), url));
    }
    out
}

/// The value the loader hands [`PageDom::set_reporting_endpoints`]: the `Reporting-Endpoints`
/// header value and the `Report-To` one of a document's response, `None` when it has neither.
pub fn pack_reporting_headers(endpoints: Option<String>, report_to: Option<String>) -> Option<String> {
    match (endpoints, report_to) {
        (None, None) => None,
        (endpoints, None) => endpoints,
        (endpoints, Some(report_to)) => Some(format!("{}\n{report_to}", endpoints.unwrap_or_default())),
    }
}

/// Splits a structured-field dictionary at the commas outside its strings.
fn split_dictionary(value: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut start, mut in_string, mut escaped) = (0, false, false);
    for (i, c) in value.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' if in_string => escaped = true,
            '"' => in_string = !in_string,
            ',' if !in_string => {
                parts.push(&value[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&value[start..]);
    parts
}

/// The leading sf-string of `s` (`"..."` with `\"` and `\\` escapes, printable ASCII only).
fn parse_sf_string(s: &str) -> Option<String> {
    let mut chars = s.strip_prefix('"')?.chars();
    let mut out = String::new();
    loop {
        match chars.next()? {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                c @ ('"' | '\\') => out.push(c),
                _ => return None,
            },
            c if (' '..='~').contains(&c) => out.push(c),
            _ => return None,
        }
    }
}

/// A page's report bookkeeping for the document it shows (`PageDom::csp_reports`).
#[derive(Default)]
pub(crate) struct ReportState {
    /// The document this state belongs to; another document starts afresh.
    doc: Option<NodeId>,
    /// The document's `Reporting-Endpoints`: name → URL.
    endpoints: Vec<(String, Url)>,
    sent: u32,
    /// Reporting API reports of this document waiting for the end of their task, one batch per
    /// endpoint ([`enqueue_report`]); `flush_queued`: that task's flush is already posted.
    pending: Vec<Batch>,
    flush_queued: bool,
    /// `<img>` URLs whose request the element's own CSP check blocked and reported, which the
    /// layout engine's later fetch of the same image must not report again
    /// ([`PageDom::note_blocked_image`]).
    blocked_images: Vec<String>,
}

/// The most blocked-image URLs remembered per document.
const MAX_BLOCKED_IMAGES: usize = 64;

/// The reports of one document for one endpoint, waiting to be sent together.
struct Batch {
    endpoint: Url,
    /// The document's origin when the first report was queued, the report request's origin even
    /// if the document has been replaced by the time it goes out.
    origin: String,
    reports: Vec<Queued>,
}

struct Queued {
    report_type: String,
    url: String,
    body: Json,
    at: std::time::Instant,
}

impl ReportState {
    /// This state for `doc`, cleared first if it held another document's.
    fn for_doc(&mut self, doc: NodeId) -> &mut ReportState {
        if self.doc != Some(doc) {
            // (Queued reports carry their endpoint and origin: they go out whoever's they were.)
            let (pending, flush_queued) = (std::mem::take(&mut self.pending), self.flush_queued);
            *self = ReportState { doc: Some(doc), pending, flush_queued, ..ReportState::default() };
        }
        self
    }

    /// Takes one report slot; false once [`MAX_REPORTS_PER_DOCUMENT`] were taken.
    fn take_slot(&mut self) -> bool {
        if self.sent >= MAX_REPORTS_PER_DOCUMENT {
            return false;
        }
        self.sent += 1;
        true
    }

    fn endpoint(&self, group: &str) -> Option<Url> {
        self.endpoints.iter().find(|(n, _)| n == group).map(|(_, u)| u.clone())
    }
}

impl PageDom {
    /// The document the page shows now has the reporting headers `value` (`None`: it had none):
    /// the `Reporting-Endpoints` value, and when the response had a `Report-To` header a newline
    /// and its value ([`pack_reporting_headers`]; a header value has no newline). The loader calls
    /// this when it commits a document; the document's earlier report state, and any other
    /// document's, goes. Only a document whose URL is potentially trustworthy gets endpoints
    /// (Reporting API "process reporting endpoints"). `Report-To` groups go to the browser
    /// context's store (`crate::reporting_store`), which outlives the document.
    pub fn set_reporting_endpoints(&self, value: Option<&str>) {
        let (value, report_to) = match value.and_then(|v| v.split_once('\n')) {
            Some((endpoints, report_to)) => ((!endpoints.is_empty()).then_some(endpoints), Some(report_to)),
            None => (value, None),
        };
        self.set_report_to(report_to);
        let endpoints = match (value, self.document_url_parsed()) {
            (Some(value), Some(base)) if pyro_net::is_potentially_trustworthy_url(&base) => {
                parse_reporting_endpoints(value, &base)
            }
            _ => Vec::new(),
        };
        // Reports the previous document queued still go out, to the endpoints it had.
        flush_pending(self);
        *self.csp_reports.borrow_mut() = ReportState { doc: Some(self.document()), endpoints, ..ReportState::default() };
    }

    /// The `<img>` request for `url` was blocked by CSP and its violation reported by the
    /// element's own check. The layout engine fetches the same image again when it draws the
    /// element ([`PageDom::take_blocked_image`]); that second request is the same one, not a
    /// second violation (Chrome reports an image once).
    pub fn note_blocked_image(&self, url: &str) {
        let mut state = self.csp_reports.borrow_mut();
        let state = state.for_doc(self.document());
        if state.blocked_images.len() < MAX_BLOCKED_IMAGES {
            state.blocked_images.push(url.to_owned());
        }
    }

    /// Whether `url` was noted by [`PageDom::note_blocked_image`] and not yet taken; taking
    /// forgets one note, so a later request for the same URL is reported again.
    pub fn take_blocked_image(&self, url: &str) -> bool {
        let mut state = self.csp_reports.borrow_mut();
        let state = state.for_doc(self.document());
        match state.blocked_images.iter().position(|u| u == url) {
            Some(i) => {
                state.blocked_images.remove(i);
                true
            }
            None => false,
        }
    }
}

/// How a report goes out. The two kinds share nothing but the POST.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Delivery {
    /// `report-uri` (CSP 3 "report a violation"): `application/csp-report`, `no-cors`,
    /// credentials `same-origin`, with the document's referrer.
    Legacy,
    /// `report-to` (Reporting API "send reports"): `application/reports+json`, `cors` (so a
    /// cross-origin endpoint answers its preflight), credentials `same-origin` (as Chrome), no
    /// referrer.
    ReportingApi,
}

impl Delivery {
    fn content_type(self) -> &'static str {
        match self {
            Delivery::Legacy => "application/csp-report",
            Delivery::ReportingApi => "application/reports+json",
        }
    }
}

/// One report request to make: endpoint, kind, body.
type Planned = (Url, Delivery, String);

/// The requests violation `v` makes, taking their slots from the document's cap. If `targets`
/// names a group, that group's endpoint gets the report, and nothing goes out when the document
/// has no endpoint of that name. Otherwise each http(s) `report-uri` gets one, resolved against
/// the violation's URL.
fn plan(page: &PageDom, v: &Violation, targets: &ReportTargets) -> Vec<Planned> {
    let mut state = page.csp_reports.borrow_mut();
    let state = state.for_doc(page.document());
    if let Some(group) = &targets.group {
        let Some(endpoint) = state.endpoint(group).or_else(|| crate::reporting_store::report_to_endpoint(page, group)) else {
            return Vec::new();
        };
        if !state.take_slot() {
            return Vec::new();
        }
        let user_agent = crate::window::user_agent(page);
        return vec![(endpoint, Delivery::ReportingApi, serialize_reporting_api(v, &user_agent))];
    }
    let Some(base) = Url::parse(&v.url).ok() else { return Vec::new() };
    let mut planned = Vec::new();
    for uri in &targets.uris {
        let Ok(endpoint) = base.join(uri) else { continue };
        if !matches!(endpoint.scheme(), "http" | "https") {
            continue;
        }
        if !state.take_slot() {
            break;
        }
        planned.push((endpoint, Delivery::Legacy, serialize_csp_report(v)));
    }
    planned
}

/// Fills in the violation's URL and referrer from the page's document when the caller left
/// them empty.
pub(crate) fn completed(page: &PageDom, v: &Violation) -> Violation {
    let mut v = v.clone();
    if v.url.is_empty() {
        v.url = page.document_url();
    }
    if v.referrer.is_empty() {
        v.referrer = page.document_referrer();
    }
    v
}

/// Delivers the reports of violation `v` of the page's document to `targets` (CSP 3 "report a
/// violation", after its event). Returns the number of requests started.
pub fn report_violation(page: &Rc<PageDom>, v: &Violation, targets: &ReportTargets) -> usize {
    report_violation_from(page, v, targets, None)
}

/// [`report_violation`] for a violation of a response that never became a document (a
/// `frame-ancestors` one): `page` only supplies the network and the slots, and `origin` (the
/// response's, `None`: the page's) is the report request's origin, so a report to the embedder's
/// origin is cross-origin for it and carries no cookies.
pub fn report_violation_from(page: &Rc<PageDom>, v: &Violation, targets: &ReportTargets, origin: Option<&str>) -> usize {
    if targets.group.is_none() && targets.uris.is_empty() {
        return 0;
    }
    let Some(net) = page.net.borrow().clone() else { return 0 };
    let v = completed(page, v);
    let mut started = 0;
    for (endpoint, delivery, body) in plan(page, &v, targets) {
        let mut request = report_request(page, endpoint, delivery, body);
        if let Some(origin) = origin {
            request.origin = Some(origin.to_owned());
        }
        net.fetch(request, Box::new(|_| {}));
        started += 1;
    }
    started
}

/// Queues one Reporting API report of `report_type` from `url`, with `body`, for the endpoint
/// the page's document names `group` (a `deprecation` report goes to `default`), sharing the
/// document's cap with its CSP reports. It goes out at the end of the current task, in one
/// request with the other reports of this document for the same endpoint ([`flush_pending`]).
/// Whether it was queued: not without a network, an endpoint of that name (the document's
/// `Reporting-Endpoints`, else a `Report-To` group of its origin), or a free slot.
pub(crate) fn report_to_group(page: &Rc<PageDom>, group: &str, report_type: &str, url: &str, body: Json) -> bool {
    enqueue_report(page, group, report_type, url, body)
}

fn enqueue_report(page: &Rc<PageDom>, group: &str, report_type: &str, url: &str, body: Json) -> bool {
    if page.net.borrow().is_none() {
        return false;
    }
    let from_document = page.csp_reports.borrow_mut().for_doc(page.document()).endpoint(group);
    let Some(endpoint) = from_document.or_else(|| crate::reporting_store::report_to_endpoint(page, group)) else {
        return false;
    };
    let origin = page.document_url_parsed().map(|u| u.origin().ascii_serialization()).unwrap_or_else(|| "null".into());
    let first = {
        let mut state = page.csp_reports.borrow_mut();
        let state = state.for_doc(page.document());
        if !state.take_slot() {
            return false;
        }
        let report = Queued { report_type: report_type.to_owned(), url: url.to_owned(), body, at: std::time::Instant::now() };
        match state.pending.iter_mut().find(|b| b.endpoint == endpoint) {
            Some(batch) => batch.reports.push(report),
            None => state.pending.push(Batch { endpoint, origin, reports: vec![report] }),
        }
        !std::mem::replace(&mut state.flush_queued, true)
    };
    if first {
        let weak = Rc::downgrade(page);
        page.post_task(Box::new(move || {
            if let Some(page) = weak.upgrade() {
                flush_pending(&page);
            }
        }));
    }
    true
}

/// Sends the reports [`report_to_group`] queued: one `application/reports+json` list per
/// endpoint, each report's `age` the milliseconds since it was generated. Runs at the end of the
/// task that queued them, before the document is replaced (`set_reporting_endpoints`) and when
/// it is disposed. A page without a network drops them. Only the first attempt is made: Chrome's
/// retry of a failed upload (a minute's backoff, five attempts) is not implemented.
pub(crate) fn flush_pending(page: &PageDom) {
    let batches = {
        let mut state = page.csp_reports.borrow_mut();
        state.flush_queued = false;
        std::mem::take(&mut state.pending)
    };
    if batches.is_empty() {
        return;
    }
    let Some(net) = page.net.borrow().clone() else { return };
    let user_agent = crate::window::user_agent(page);
    let now = std::time::Instant::now();
    for batch in batches {
        let list: Vec<Json> = batch
            .reports
            .into_iter()
            .map(|r| {
                let age = now.saturating_duration_since(r.at).as_millis() as u64;
                report_value(&r.report_type, &r.url, &user_agent, age, r.body)
            })
            .collect();
        let mut request = report_request(page, batch.endpoint, Delivery::ReportingApi, Json::Array(list).to_string());
        request.origin = Some(batch.origin);
        net.fetch(request, Box::new(|_| {}));
    }
}

/// CSP Integrity: the `csp-hash` reports a fetched script owes the document's policies that ask
/// for its hashes (`'report-sha256'`...), sent to each policy's `report-to` endpoint. `data` is
/// the response body, `None` for one the page may not read (a cross-origin no-CORS response:
/// reported with an empty hash). No `securitypolicyviolation` and no `ReportingObserver` entry:
/// nothing is blocked, the report only goes out. `url`: the script's request URL.
pub fn report_script_hashes(page: &Rc<PageDom>, list: &pyro_net::csp::PolicyList, url: &str, data: Option<&[u8]>) {
    for (group, algorithms) in list.script_hash_reports() {
        let Some(group) = group else { continue };
        for algorithm in algorithms {
            let hash = data.and_then(|data| pyro_net::csp::hash_expression(algorithm, data)).unwrap_or_default();
            let mut body = Map::new();
            body.insert("documentURL".to_owned(), strip_url_for_report(&page.document_url()).into());
            body.insert("subresourceURL".to_owned(), strip_url_for_report(url).into());
            body.insert("hash".to_owned(), hash.into());
            body.insert("type".to_owned(), "subresource".into());
            body.insert("destination".to_owned(), "script".into());
            let document = strip_url_for_report(&page.document_url());
            report_to_group(page, &group, "csp-hash", &document, Json::Object(body));
        }
    }
}

/// The report POST of `body` to `endpoint`, as `delivery` makes it.
fn report_request(page: &PageDom, endpoint: Url, delivery: Delivery, body: String) -> Request {
    // Both specs use destination "report" (`Sec-Fetch-Dest: report`).
    let mut request = Request::get(endpoint, Destination::Report);
    request.method = Method::POST;
    request.headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(delivery.content_type()));
    request.body = Some(Bytes::from(body));
    let document_url = page.document_url_parsed();
    request.origin =
        Some(document_url.as_ref().map(|u| u.origin().ascii_serialization()).unwrap_or_else(|| "null".into()));
    match delivery {
        Delivery::Legacy => {
            request.mode = RequestMode::NoCors;
            request.credentials = Credentials::SameOrigin;
            request.referrer = page.client_referrer();
            request.referrer_policy = page.referrer_policy();
        }
        Delivery::ReportingApi => {
            request.mode = RequestMode::Cors;
            // (Chrome sends a same-origin endpoint its cookies; the Reporting API says `omit`.)
            request.credentials = Credentials::SameOrigin;
            request.referrer = None;
        }
    }
    let context = page.request_context();
    request.frame_id = context.frame_id;
    request.loader_id = context.loader_id;
    page.apply_request_defaults(&mut request);
    request
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use pyro_dom::{DocumentKind, Dom};

    use super::*;

    fn violation() -> Violation {
        Violation {
            url: "https://a.test/page.html#frag".into(),
            referrer: "https://user:pw@b.test/ref#x".into(),
            blocked: "https://c.test/img.png#y".into(),
            effective_directive: "img-src".into(),
            policy: "img-src 'none'; report-uri /r".into(),
            report_only: true,
            ..Default::default()
        }
    }

    fn page(url: &str) -> (Rc<PageDom>, Rc<RefCell<Dom>>) {
        let dom = Rc::new(RefCell::new(Dom::new()));
        let doc = dom.borrow_mut().create_document(DocumentKind::Html, url);
        (PageDom::new(dom.clone(), doc), dom)
    }

    #[test]
    fn targets_follow_first_directive_and_case() {
        let t = ReportTargets::from_policy("REPORT-URI /a  //b.test/c; report-uri /ignored; img-src *", false);
        assert_eq!(t.uris, ["/a", "//b.test/c"]);
        assert_eq!(t.group, None);
        let t = ReportTargets::from_policy("report-to g1 g2; report-uri /a", false);
        assert_eq!(t.group.as_deref(), Some("g1"));
        assert_eq!(t.uris, ["/a"]);
        assert_eq!(ReportTargets::from_policy("default-src 'self'", false), ReportTargets::default());
        // An invalid name is not a directive and does not shadow a later valid one.
        let t = ReportTargets::from_policy("report-uri! /x; report-uri /y", false);
        assert_eq!(t.uris, ["/y"]);
    }

    #[test]
    fn meta_policy_drops_report_uri_but_keeps_report_to() {
        let t = ReportTargets::from_policy("img-src 'none'; report-uri /r", true);
        assert_eq!(t, ReportTargets::default());
        let t = ReportTargets::from_policy("img-src 'none'; report-to g; report-uri /r", true);
        assert_eq!(t.group.as_deref(), Some("g"));
        assert!(t.uris.is_empty());
    }

    #[test]
    fn strip_keeps_non_urls_and_drops_fragment_and_credentials() {
        assert_eq!(strip_url_for_report("inline"), "inline");
        assert_eq!(strip_url_for_report("trusted-types-sink"), "trusted-types-sink");
        assert_eq!(strip_url_for_report(""), "");
        assert_eq!(strip_url_for_report("data:image/png;base64,AAAA"), "data");
        assert_eq!(strip_url_for_report("ftp://blah.test/"), "ftp");
        assert_eq!(strip_url_for_report("https://u:p@a.test/x?q#f"), "https://a.test/x?q");
    }

    #[test]
    fn deprecated_serialization() {
        let mut v = violation();
        v.sample = "Element innerHTML|<b>".into();
        let json: Json = serde_json::from_str(&serialize_csp_report(&v)).unwrap();
        let r = &json["csp-report"];
        assert_eq!(r["document-uri"], "https://a.test/page.html");
        assert_eq!(r["referrer"], "https://b.test/ref");
        assert_eq!(r["blocked-uri"], "https://c.test/img.png");
        assert_eq!(r["violated-directive"], "img-src");
        assert_eq!(r["effective-directive"], "img-src");
        assert_eq!(r["disposition"], "report");
        assert_eq!(r["status-code"], 0);
        assert_eq!(r["script-sample"], "Element innerHTML|<b>");
        assert!(r.get("line-number").is_none());
        v.source_file = "https://a.test/s.js#h".into();
        v.line = 3;
        v.column = 7;
        let json: Json = serde_json::from_str(&serialize_csp_report(&v)).unwrap();
        assert_eq!(json["csp-report"]["source-file"], "https://a.test/s.js");
        assert_eq!(json["csp-report"]["line-number"], 3);
        assert_eq!(json["csp-report"]["column-number"], 7);
    }

    #[test]
    fn reporting_api_serialization() {
        let json: Json = serde_json::from_str(&serialize_reporting_api(&violation(), "UA")).unwrap();
        let r = &json[0];
        assert_eq!(r["type"], "csp-violation");
        assert_eq!(r["url"], "https://a.test/page.html");
        assert_eq!(r["user_agent"], "UA");
        assert_eq!(r["body"]["blockedURL"], "https://c.test/img.png");
        assert_eq!(r["body"]["effectiveDirective"], "img-src");
        assert_eq!(r["body"]["disposition"], "report");
        assert!(r["body"]["sourceFile"].is_null());
    }

    #[test]
    fn reporting_endpoints_header() {
        let base = Url::parse("https://a.test/dir/page").unwrap();
        let e = parse_reporting_endpoints(
            r#"csp="/r?x=1,2", other="http://insecure.test/", Bad="/no", n=1, dup="/a", dup="https://b.test/b";p=1"#,
            &base,
        );
        let names: Vec<_> = e.iter().map(|(n, u)| (n.as_str(), u.as_str())).collect();
        assert_eq!(names, [("csp", "https://a.test/r?x=1,2"), ("dup", "https://b.test/b")]);
    }

    #[test]
    fn state_is_per_document_and_capped() {
        let mut dom = Dom::new();
        let first = dom.create_document(DocumentKind::Html, "https://a.test/1");
        let second = dom.create_document(DocumentKind::Html, "https://a.test/2");
        let mut state = ReportState::default();
        state.for_doc(first).endpoints = vec![("g".into(), Url::parse("https://e.test/").unwrap())];
        for _ in 0..MAX_REPORTS_PER_DOCUMENT {
            assert!(state.for_doc(first).take_slot());
        }
        assert!(!state.for_doc(first).take_slot());
        assert!(state.for_doc(first).endpoint("g").is_some());
        // Another document starts with no endpoints and a fresh count.
        assert!(state.for_doc(second).endpoint("g").is_none());
        assert!(state.for_doc(second).take_slot());
    }

    #[test]
    fn plan_uses_report_to_endpoint_and_ignores_report_uri() {
        let (page, _dom) = page("https://a.test/doc");
        let v = violation();
        let targets = ReportTargets::from_policy("img-src 'none'; report-to g; report-uri /r", false);
        // No Reporting-Endpoints: nothing is sent, and report-uri stays unused.
        assert!(plan(&page, &v, &targets).is_empty());
        page.set_reporting_endpoints(Some(r#"g="/endpoint""#));
        let planned = plan(&page, &v, &targets);
        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].0.as_str(), "https://a.test/endpoint");
        assert_eq!(planned[0].1, Delivery::ReportingApi);
        // A new commit without the header forgets the endpoint.
        page.set_reporting_endpoints(None);
        assert!(plan(&page, &v, &targets).is_empty());
    }

    #[test]
    fn plan_resolves_report_uris_against_the_violation_url() {
        let (page, _dom) = page("https://a.test/doc");
        let v = violation();
        let targets = ReportTargets::from_policy("report-uri /r //b.test/s ftp://c.test/ https://d.test/t", false);
        let planned: Vec<_> = plan(&page, &v, &targets).into_iter().map(|(u, ty, _)| (u.to_string(), ty)).collect();
        assert_eq!(
            planned,
            [
                ("https://a.test/r".to_owned(), Delivery::Legacy),
                ("https://b.test/s".to_owned(), Delivery::Legacy),
                ("https://d.test/t".to_owned(), Delivery::Legacy),
            ]
        );
    }

    #[test]
    fn untrustworthy_document_gets_no_reporting_endpoints() {
        let (page, _dom) = page("http://a.test/doc");
        page.set_reporting_endpoints(Some(r#"g="https://e.test/endpoint""#));
        let targets = ReportTargets::from_policy("img-src 'none'; report-to g; report-uri /r", false);
        // No endpoint, and report-to still overrides report-uri: nothing goes out.
        assert!(plan(&page, &violation(), &targets).is_empty());
        // The legacy path does not depend on a trustworthy document.
        let legacy = ReportTargets::from_policy("img-src 'none'; report-uri /r", false);
        assert_eq!(plan(&page, &violation(), &legacy).len(), 1);
    }

    #[test]
    fn legacy_and_reporting_api_requests_differ() {
        let (page, _dom) = page("https://a.test/doc");
        let endpoint = Url::parse("https://e.test/r").unwrap();
        let legacy = report_request(&page, endpoint.clone(), Delivery::Legacy, "{}".into());
        assert_eq!(legacy.method, Method::POST);
        assert_eq!(legacy.mode, RequestMode::NoCors);
        assert_eq!(legacy.credentials, Credentials::SameOrigin);
        assert_eq!(legacy.headers.get(header::CONTENT_TYPE).unwrap(), "application/csp-report");
        assert!(legacy.referrer.is_some());
        assert_eq!(legacy.origin.as_deref(), Some("https://a.test"));
        let api = report_request(&page, endpoint, Delivery::ReportingApi, "[]".into());
        assert_eq!(api.method, Method::POST);
        assert_eq!(api.mode, RequestMode::Cors);
        assert_eq!(api.credentials, Credentials::SameOrigin);
        assert_eq!(api.headers.get(header::CONTENT_TYPE).unwrap(), "application/reports+json");
        assert!(api.referrer.is_none());
        assert_eq!(api.origin.as_deref(), Some("https://a.test"));
        assert_eq!(api.body.as_deref(), Some(&b"[]"[..]));
    }

    #[test]
    fn report_violation_without_network_sends_nothing_and_keeps_slots() {
        let (page, _dom) = page("https://a.test/doc");
        let targets = ReportTargets::from_policy("report-uri /r", false);
        assert_eq!(report_violation(&page, &violation(), &targets), 0);
        assert_eq!(page.csp_reports.borrow().sent, 0);
    }

    #[test]
    fn empty_url_and_referrer_come_from_the_document() {
        let (page, _dom) = page("https://a.test/doc#f");
        let v = completed(&page, &Violation::default());
        assert_eq!(strip_url_for_report(&v.url), "https://a.test/doc");
    }

    #[test]
    fn observer_body_matches_the_endpoint_body() {
        let v = Violation { source_file: "https://a.test/s.js#x".into(), line: 3, column: 7, ..violation() };
        let body = csp_violation_body(&v);
        let json: Json = serde_json::from_str(&serialize_reporting_api(&v, "UA")).unwrap();
        assert_eq!(json[0]["body"], Json::Object(body.clone()));
        assert_eq!(body["documentURL"], "https://a.test/page.html");
        assert_eq!(body["sourceFile"], "https://a.test/s.js");
        assert_eq!(body["lineNumber"], 3);
        assert_eq!(body["columnNumber"], 7);
    }

    #[test]
    fn report_value_carries_age_and_fields() {
        let json = report_value("test", "https://a.test/", "UA", 1500, serde_json::json!({"message": "m"}));
        assert_eq!(json["age"], 1500);
        assert_eq!(json["type"], "test");
        assert_eq!(json["body"]["message"], "m");
    }

    #[test]
    fn reporting_headers_pack_and_unpack() {
        assert_eq!(pack_reporting_headers(None, None), None);
        assert_eq!(pack_reporting_headers(Some("a=\"/x\"".into()), None).as_deref(), Some("a=\"/x\""));
        assert_eq!(pack_reporting_headers(None, Some("{}".into())).as_deref(), Some("\n{}"));
        // A Report-To only response gives the document no Reporting-Endpoints, and its groups
        // reach the store, resolving a name the document lacks.
        let (page, _dom) = page("https://a.test/doc");
        page.set_reporting_endpoints(pack_reporting_headers(
            Some(r#"own="/own""#.into()),
            Some(r#"{"group":"legacy","max_age":60,"endpoints":[{"url":"/legacy"}]}"#.into()),
        ).as_deref());
        let targets = ReportTargets::from_policy("img-src 'none'; report-to legacy", false);
        let planned = plan(&page, &violation(), &targets);
        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].0.as_str(), "https://a.test/legacy");
        let own = ReportTargets::from_policy("img-src 'none'; report-to own", false);
        assert_eq!(plan(&page, &violation(), &own)[0].0.as_str(), "https://a.test/own");
    }

    #[test]
    fn blocked_image_notes_are_taken_once() {
        let (page, _dom) = page("https://a.test/doc");
        assert!(!page.take_blocked_image("https://a.test/i.png"));
        page.note_blocked_image("https://a.test/i.png");
        assert!(page.take_blocked_image("https://a.test/i.png"));
        assert!(!page.take_blocked_image("https://a.test/i.png"));
    }

    #[test]
    fn report_to_group_without_network_sends_nothing_and_keeps_slots() {
        let (page, _dom) = page("https://a.test/doc");
        page.set_reporting_endpoints(Some(r#"default="/r""#));
        assert!(!report_to_group(&page, "default", "deprecation", "https://a.test/doc", Json::Null));
        assert_eq!(page.csp_reports.borrow().sent, 0);
    }

    #[test]
    fn reporting_api_list_wraps_one_report() {
        let json: Json = serde_json::from_str(&reporting_api_list(
            "deprecation",
            "https://a.test/",
            "UA",
            serde_json::json!({"id": "x"}),
        ))
        .unwrap();
        assert_eq!(json.as_array().map(Vec::len), Some(1));
        assert_eq!(json[0]["type"], "deprecation");
        assert_eq!(json[0]["age"], 0);
        assert_eq!(json[0]["url"], "https://a.test/");
        assert_eq!(json[0]["user_agent"], "UA");
        assert_eq!(json[0]["body"]["id"], "x");
    }
}
