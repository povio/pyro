//! The Reporting API's script side: `ReportingObserver`, and the reports the engine generates
//! for it. The interfaces live in reporting.js, made in a realm on its first use: the first read
//! of one of the names, or the first report the realm generates. A realm with neither never
//! makes it.
//!
//! Reports reach a realm's observers from two places:
//! - `crate::csp::report`: a CSP violation, of a document or a worker, in the task that fired
//!   its `securitypolicyviolation` event, after the event and after `crate::csp_report`
//!   delivered the policy's reports ([`queue_csp_violation`]).
//! - [`deprecation`]: a deprecated API that pyro-browser defines (Chrome's prefixed
//!   `webkitRequestAnimationFrame` and `webkitCancelAnimationFrame`). Each id is reported once
//!   per realm, to the realm's observers and, as Chrome does, to the document's `default`
//!   Reporting-Endpoints endpoint.
use std::rc::Rc;

use pyro_js::{ClassDef, JsClass, JsResult, Scope, Value};
use serde_json::Value as Json;

use crate::csp_report;
use crate::lazy_module::{lazy_globals, LazyModule};
use crate::page::PageDom;
use crate::window::WindowHost;

pub(crate) static MODULE: LazyModule = LazyModule {
    key: "lpReporting",
    source: include_str!("reporting.js"),
    url: "pyro://internal/reporting.js",
    natives: None,
};

/// The Window's interface names (Chrome exposes `ReportBody` and `CSPViolationReportBody`, but
/// neither `Report` nor `DeprecationReportBody`).
pub(crate) fn define<W: JsClass>(def: &mut ClassDef<W>)
where
    W::Data: WindowHost,
{
    lazy_globals!(def, MODULE, "ReportingObserver", "ReportBody", "CSPViolationReportBody");
}

/// The same names in a worker realm, any kind of worker (`[Exposed=(Window,Worker)]`).
pub(crate) fn install_worker(s: &Scope<'_>) -> JsResult<()> {
    macro_rules! interface {
        ($name:ident) => {{
            struct $name;
            impl pyro_js::LazyGlobalInterface for $name {
                const NAME: &'static str = stringify!($name);
                fn value<'s>(s: &Scope<'s>) -> JsResult<Value<'s>> {
                    MODULE.module(s, &crate::target::page_of(s)?)?.get_atom(Self::NAME)
                }
            }
            s.define_lazy_global_interface::<$name>()?;
        }};
    }
    interface!(ReportingObserver);
    interface!(ReportBody);
    interface!(CSPViolationReportBody);
    Ok(())
}

/// Queues the `csp-violation` report of `v` to the observers of `s`, the realm of `page`'s
/// document or worker. Reporting API: the report's URL is the global's, a worker's being its
/// script's URL after redirects. The body has the fields `crate::csp_report` sends to a
/// `report-to` endpoint. A failure (a realm going away) drops the report.
pub(crate) fn queue_csp_violation(s: &Scope<'_>, page: &Rc<PageDom>, v: &csp_report::Violation) {
    let v = match crate::worker_scope::nested_referrer_source(s) {
        Some(url) => csp_report::Violation { url: url.to_string(), ..v.clone() },
        None => csp_report::completed(page, v),
    };
    let url = csp_report::strip_url_for_report(&v.url);
    let body = Json::Object(csp_report::csp_violation_body(&v)).to_string();
    let _ = MODULE.helper(s, page, "queue", &[s.string("csp-violation"), s.string(&url), s.string(&body)]);
}

/// A `deprecation` report for `id` from `page`'s document, made in `s`, the realm of the
/// script that called the deprecated API (its innermost page frame is the report's source
/// location). The first time per realm only. It goes to the realm's observers, then to the
/// document's `default` endpoint when it has one. Nothing here throws into the caller.
pub fn deprecation(s: &Scope<'_>, page: &Rc<PageDom>, id: &str, message: &str) {
    let url = csp_report::strip_url_for_report(&page.document_url());
    let Ok(body) = MODULE.helper(s, page, "deprecation", &[s.string(id), s.string(message), s.string(&url)]) else {
        return;
    };
    let Some(body) = body.as_string() else { return };
    let Ok(body) = serde_json::from_str::<Json>(&body) else { return };
    csp_report::report_to_group(page, "default", "deprecation", &url, body);
}
