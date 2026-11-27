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

use pyro_net::{CorpReport, CorpViolation, EmbedderPolicy, RequestDefaults};
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
                        queue_violation(&page, None, violation.kind, &violation.blocked_url, violation.destination, violation.report_only);
                    }
                }));
            }),
        })
    }
}

/// Queues the COEP violation report of `page`'s context, of body type `kind` (`"corp"`, `"navigation"`
/// or `"worker initialization"`): `blocked_url` (stripped for reports) was
/// blocked by its enforced policy, or with `report_only` would have been by its report-only one.
/// It goes to the context's observers and to the `report-to` endpoint of that policy. `scope` is the
/// realm's when the caller runs inside one of its natives (see [`queue_report`]).
pub fn queue_violation(page: &Rc<PageDom>, scope: Option<&pyro_js::Scope<'_>>, kind: &str, blocked_url: &str, destination: Option<&str>, report_only: bool) {
    let reporting = page.embedder_reporting();
    let (dip, endpoint) = if report_only {
        (reporting.report_only_dip, reporting.report_only_report_to)
    } else {
        (reporting.dip, reporting.report_to)
    };
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
        scope,
        ReportInput {
            report_type: if dip { "dip" } else { "coep" },
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

/// HTML "obtain an embedder policy" of a response for `url`: the `Cross-Origin-Embedder-Policy` header as a
/// structured-field token (parameters allowed, so `require-corp; report-to="x"` is `require-corp`;
/// a repeated header, a string or byte-sequence item or a malformed field is none), `unsafe-none` when
/// it is absent, unrecognised or sent from a context that is not potentially trustworthy.
pub fn obtain_embedder_policy(headers: &pyro_net::http::HeaderMap, url: &pyro_net::Url) -> pyro_net::EmbedderPolicy {
    embedder_policy_field(headers, url, "cross-origin-embedder-policy").0
}

/// The embedder policy header `name` (the enforced one or its `-report-only` twin) as
/// [`obtain_embedder_policy`] reads it, and its `report-to` parameter when that is a string.
fn embedder_policy_field(headers: &pyro_net::http::HeaderMap, url: &pyro_net::Url, name: &str) -> (pyro_net::EmbedderPolicy, Option<String>) {
    policy_field(headers, url, name, "require-corp", "credentialless")
}

/// The same for a `Document-Isolation-Policy` header, whose values `isolate-and-require-corp` and
/// `isolate-and-credentialless` are the embedder policies `require-corp` and `credentialless` (with
/// the isolation of the agent cluster, which is not this module's).
fn dip_field(headers: &pyro_net::http::HeaderMap, url: &pyro_net::Url, name: &str) -> (pyro_net::EmbedderPolicy, Option<String>) {
    policy_field(headers, url, name, "isolate-and-require-corp", "isolate-and-credentialless")
}

fn policy_field(
    headers: &pyro_net::http::HeaderMap,
    url: &pyro_net::Url,
    name: &str,
    require_corp: &str,
    credentialless: &str,
) -> (pyro_net::EmbedderPolicy, Option<String>) {
    let none = (pyro_net::EmbedderPolicy::UnsafeNone, None);
    let trustworthy = url.scheme() == "https"
        || url.host_str().is_some_and(|h| h == "localhost" || h == "127.0.0.1" || h == "[::1]" || h.ends_with(".localhost"));
    if !trustworthy {
        return none;
    }
    // Repeated headers combine with ", ", which no single structured item parses as.
    let Some(values) = headers.get_all(name).iter().map(|v| v.to_str().ok()).collect::<Option<Vec<&str>>>() else {
        return none;
    };
    if values.is_empty() {
        return none;
    }
    let joined = values.join(", ");
    let value = match structured_item_token(&joined) {
        Some(token) if token == require_corp => pyro_net::EmbedderPolicy::RequireCorp,
        Some(token) if token == credentialless => pyro_net::EmbedderPolicy::Credentialless,
        _ => return none,
    };
    (value, report_to_param(&joined))
}

/// The embedder policy a response gives its document or worker, and what it reports with: its
/// `Cross-Origin-Embedder-Policy` and `-Report-Only` headers, each (when it has no value) taken from
/// the `Document-Isolation-Policy` header that is its twin, whose reports are of type `dip`.
pub fn obtain_embedder(headers: &pyro_net::http::HeaderMap, url: &pyro_net::Url) -> (pyro_net::EmbedderPolicy, pyro_net::EmbedderReporting) {
    use pyro_net::EmbedderPolicy::UnsafeNone;
    let (coep, coep_to) = embedder_policy_field(headers, url, "cross-origin-embedder-policy");
    let (dip, dip_to) = dip_field(headers, url, "document-isolation-policy");
    let (coep_ro, coep_ro_to) = embedder_policy_field(headers, url, "cross-origin-embedder-policy-report-only");
    let (dip_ro, dip_ro_to) = dip_field(headers, url, "document-isolation-policy-report-only");
    let (enforced_dip, report_only_dip) = (coep == UnsafeNone && dip != UnsafeNone, coep_ro == UnsafeNone && dip_ro != UnsafeNone);
    let policy = if enforced_dip { dip } else { coep };
    let reporting = pyro_net::EmbedderReporting {
        report_to: if enforced_dip { dip_to } else { coep_to },
        dip: enforced_dip,
        report_only: if report_only_dip { dip_ro } else { coep_ro },
        report_only_report_to: if report_only_dip { dip_ro_to } else { coep_ro_to },
        report_only_dip,
    };
    (policy, reporting)
}

/// The first item of a structured field value that is a bare item (RFC 8941 `sf-item`), when it is
/// a Token: the text of that token. `None` for a string, byte sequence, number or boolean first
/// item and for a field that does not parse (malformed parameters included).
pub fn structured_item_token(value: &str) -> Option<&str> {
    // Leading and trailing HTTP whitespace is not part of the field value; a structured field
    // then discards leading and trailing SP only.
    let value = value.trim_matches(|c| c == ' ' || c == '\t');
    let tchar = |c: char| c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c);
    let (token, mut rest) = match value.chars().next()? {
        c if c.is_ascii_alphabetic() || c == '*' => {
            let end = value.find(|c: char| !(tchar(c) || c == ':' || c == '/')).unwrap_or(value.len());
            (Some(&value[..end]), &value[end..])
        }
        _ => (None, parse_sf_non_token(value)?),
    };
    // parameters = *( ";" *SP key [ "=" bare-item ] )
    while !rest.is_empty() {
        rest = rest.strip_prefix(';')?.trim_start_matches(' ');
        let key_end = {
            let mut chars = rest.char_indices();
            match chars.next() {
                Some((_, c)) if c.is_ascii_lowercase() || c == '*' => {}
                _ => return None,
            }
            chars
                .find(|&(_, c)| !(c.is_ascii_lowercase() || c.is_ascii_digit() || "_-.*".contains(c)))
                .map_or(rest.len(), |(i, _)| i)
        };
        rest = &rest[key_end..];
        if let Some(after) = rest.strip_prefix('=') {
            rest = match after.chars().next() {
                Some(c) if c.is_ascii_alphabetic() || c == '*' => {
                    let end = after.find(|c: char| !(tchar(c) || c == ':' || c == '/')).unwrap_or(after.len());
                    &after[end..]
                }
                _ => parse_sf_non_token(after)?,
            };
        }
    }
    token
}

/// The rest of `value` after a leading bare item that is not a token (string, byte sequence,
/// number, boolean); `None` if it is none of those.
fn parse_sf_non_token(value: &str) -> Option<&str> {
    let mut chars = value.char_indices();
    match chars.next()? {
        (_, '"') => {
            let mut escaped = false;
            for (i, c) in chars {
                if !(' '..='~').contains(&c) {
                    return None;
                }
                match (escaped, c) {
                    (true, '"' | '\\') => escaped = false,
                    (true, _) => return None,
                    (false, '\\') => escaped = true,
                    (false, '"') => return Some(&value[i + 1..]),
                    (false, _) => {}
                }
            }
            None
        }
        (_, ':') => {
            let end = value[1..].find(':')? + 1;
            value[1..end].chars().all(|c| c.is_ascii_alphanumeric() || "+/=".contains(c)).then(|| &value[end + 1..])
        }
        (_, '?') => value.get(1..2).filter(|b| *b == "0" || *b == "1").map(|_| &value[2..]),
        (_, c) if c == '-' || c.is_ascii_digit() => {
            let end = value.find(|c: char| !(c.is_ascii_digit() || c == '-' || c == '.')).unwrap_or(value.len());
            Some(&value[end..])
        }
        _ => None,
    }
}

/// The `report-to` parameter of a COOP header value that has parsed (`structured_item_token`
/// accepted it): its string, `None` when it is absent or not a string (a token or byte sequence
/// is ignored). The last of a repeated parameter counts.
pub fn report_to_param(value: &str) -> Option<String> {
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

