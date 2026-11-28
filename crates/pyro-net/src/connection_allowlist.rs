//! The `Connection-Allowlist` response header (an experimental origin allowlist for every
//! connection a document makes) and its report-only twin `Connection-Allowlist-Report-Only`.
//!
//! The value is a Structured Field inner list of URL pattern strings and the token
//! `response-origin` (the origin of the response that carried the header), with parameters:
//! `redirects=allow|block` (default `block`) and `report-to=<endpoint>` (a `Reporting-Endpoints`
//! name). `()` allows no connection at all. A pattern is a URLPattern constructor string
//! (`*://:subdomain.alt.test:*`) matched against the request's URL, where the WebSocket schemes
//! count as their HTTP twins (`ws:` as `http:`, `wss:` as `https:`), so one pattern covers an
//! origin's fetches and sockets, as Chrome. A value that does not parse as one inner list is
//! ignored.
//!
//! Enforcement lives in [`crate::csp::PolicyList::pre_request`], which every initiator that
//! checks CSP already calls (fetch, XHR, beacons, `EventSource`, `WebSocket`, subresources,
//! preloads, workers, frames) and which the client runs again at each redirect hop
//! ([`crate::csp::RequestCsp::allows_redirect`]). A redirect hop is not matched against the
//! patterns: it is refused unless the list says `redirects=allow`, and then it is followed
//! wherever it goes. Only requests to network schemes are governed: `data:`, `blob:`, `about:`
//! and `javascript:` open no connection.
//!
//! A violation is reported by the URL the request started with (fragment and credentials
//! stripped) and does not fire `securitypolicyviolation`: [`DIRECTIVE`] tells the reporter to
//! make a `connection-allowlist` Reporting API report instead (`{connection, disposition}`).

use std::sync::Arc;

use url::Url;

use crate::csp::{CheckOutcome, Disposition, PolicySource, Violation};

/// The `Violation::effective_directive` of an allowlist violation.
pub const DIRECTIVE: &str = "connection-allowlist";

/// One thing a list allows.
enum Matcher {
    /// `response-origin`: every URL of this tuple origin.
    Origin { scheme: String, host: String, port: u16 },
    Pattern(urlpattern::UrlPattern<regex::Regex>),
}

impl Matcher {
    /// Whether `url` (its scheme already normalized, [`normalized`]) is allowed by this entry.
    fn matches(&self, url: &Url) -> bool {
        match self {
            Matcher::Origin { scheme, host, port } => match url.origin() {
                url::Origin::Tuple(s, h, p) => s == *scheme && h.to_string() == *host && p == *port,
                url::Origin::Opaque(_) => false,
            },
            Matcher::Pattern(pattern) => pattern.test(urlpattern::UrlPatternMatchInput::Url(url.clone())).unwrap_or(false),
        }
    }
}

/// One parsed header: the enforced or the report-only one.
pub struct ConnectionAllowlist {
    matchers: Vec<Matcher>,
    /// What each matcher was made from (`o` for `response-origin`, `p` and the pattern string),
    /// which [`ConnectionAllowlist::subsumed_by`] compares: a pattern's own text is all there is to
    /// compare two patterns by.
    entries: Vec<(char, String)>,
    allow_redirects: bool,
    report_to: Option<String>,
    disposition: Disposition,
    /// The header value as delivered (the violation's `original_policy`).
    text: String,
}

impl std::fmt::Debug for ConnectionAllowlist {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectionAllowlist")
            .field("text", &self.text)
            .field("allow_redirects", &self.allow_redirects)
            .field("report_to", &self.report_to)
            .field("disposition", &self.disposition)
            .finish()
    }
}

/// An item of the inner list.
enum Item {
    Token(String),
    Str(String),
}

/// A parameter's value.
enum Param {
    Bool,
    Token(String),
    Str(String),
    Other,
}

/// The leading sf-string of `s` (`"..."` with `\"` and `\\` escapes, printable ASCII) and what
/// follows it.
fn take_string(s: &str) -> Option<(String, &str)> {
    let mut chars = s.strip_prefix('"')?.char_indices();
    let mut out = String::new();
    while let Some((i, c)) = chars.next() {
        match c {
            // `i` counts from after the opening quote: the closing quote is at `i + 1` of `s`.
            '"' => return Some((out, &s[i + 2..])),
            '\\' => match chars.next()?.1 {
                c @ ('"' | '\\') => out.push(c),
                _ => return None,
            },
            c if (' '..='~').contains(&c) => out.push(c),
            _ => return None,
        }
    }
    None
}

/// The leading sf-token of `s` and what follows it.
fn take_token(s: &str) -> Option<(&str, &str)> {
    let mut end = 0;
    for (i, c) in s.char_indices() {
        let ok = if i == 0 {
            c.is_ascii_alphabetic() || c == '*'
        } else {
            c.is_ascii_alphanumeric() || ":/!#$%&'*+-.^_`|~".contains(c)
        };
        if !ok {
            break;
        }
        end = i + c.len_utf8();
    }
    (end > 0).then(|| (&s[..end], &s[end..]))
}

/// The inner list and the parameters of a header value, `None` when it is anything else.
fn parse_inner_list(value: &str) -> Option<(Vec<Item>, Vec<(String, Param)>)> {
    let mut rest = value.trim_matches([' ', '\t']).strip_prefix('(')?;
    let mut items = Vec::new();
    loop {
        rest = rest.trim_start_matches(' ');
        if let Some(after) = rest.strip_prefix(')') {
            rest = after;
            break;
        }
        if rest.starts_with('"') {
            let (string, after) = take_string(rest)?;
            items.push(Item::Str(string));
            rest = after;
        } else {
            let (token, after) = take_token(rest)?;
            items.push(Item::Token(token.to_owned()));
            rest = after;
        }
        if !(rest.starts_with(' ') || rest.starts_with(')')) {
            return None;
        }
    }
    let mut params = Vec::new();
    loop {
        rest = rest.trim_start_matches([' ', '\t']);
        if rest.is_empty() {
            break;
        }
        rest = rest.strip_prefix(';')?.trim_start_matches(' ');
        let end = rest.find(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit() || "_-.*".contains(c))).unwrap_or(rest.len());
        if end == 0 || !rest.starts_with(|c: char| c.is_ascii_lowercase() || c == '*') {
            return None;
        }
        let key = rest[..end].to_owned();
        rest = &rest[end..];
        let param = match rest.strip_prefix('=') {
            Some(after) if after.starts_with('"') => {
                let (string, after) = take_string(after)?;
                rest = after;
                Param::Str(string)
            }
            Some(after) => match take_token(after) {
                Some((token, after)) => {
                    rest = after;
                    Param::Token(token.to_owned())
                }
                None => {
                    // A number, a boolean, a byte sequence: no parameter reads one.
                    rest = &after[after.find(';').unwrap_or(after.len())..];
                    Param::Other
                }
            },
            None => Param::Bool,
        };
        params.push((key, param));
    }
    Some((items, params))
}

/// `url` with a WebSocket scheme turned into its HTTP twin, which patterns are written for.
fn normalized(url: &Url) -> Url {
    let twin = match url.scheme() {
        "ws" => "http",
        "wss" => "https",
        _ => return url.clone(),
    };
    let mut url = url.clone();
    let _ = url.set_scheme(twin);
    url
}

fn compile(pattern: &str) -> Option<Matcher> {
    let init = urlpattern::UrlPatternInit::parse_constructor_string::<regex::Regex>(pattern, None).ok()?;
    let pattern = urlpattern::UrlPattern::<regex::Regex>::parse(init, Default::default()).ok()?;
    Some(Matcher::Pattern(pattern))
}

impl ConnectionAllowlist {
    /// Parses a header value delivered with `response_url` (what `response-origin` means).
    pub fn parse(value: &str, disposition: Disposition, response_url: &Url) -> Option<ConnectionAllowlist> {
        let (items, params) = parse_inner_list(value)?;
        let mut matchers = Vec::new();
        let mut entries: Vec<(char, String)> = Vec::new();
        for item in items {
            match item {
                Item::Token(token) if token == "response-origin" => {
                    if let url::Origin::Tuple(scheme, host, port) = response_url.origin() {
                        matchers.push(Matcher::Origin { scheme, host: host.to_string(), port });
                        entries.push(('o', String::new()));
                    }
                }
                Item::Token(_) => {}
                // A pattern that does not parse allows nothing.
                Item::Str(pattern) => {
                    if let Some(matcher) = compile(&pattern) {
                        matchers.push(matcher);
                        entries.push(('p', pattern));
                    }
                }
            }
        }
        let mut allow_redirects = false;
        let mut report_to = None;
        for (key, param) in params {
            match (key.as_str(), param) {
                ("redirects", Param::Token(t)) => allow_redirects = t == "allow",
                ("report-to", Param::Token(t) | Param::Str(t)) => report_to = Some(t),
                _ => {}
            }
        }
        Some(ConnectionAllowlist { matchers, entries, allow_redirects, report_to, disposition, text: value.trim().to_owned() })
    }

    /// Whether this list allows nothing `required` does not: each of its entries is one of
    /// `required`'s (`response-origin` is the origin of the response either was parsed for, which
    /// is the same document's for a list a framed document delivers against the one its embedder
    /// requires; a pattern is compared by its text, so a narrower pattern is never taken for a
    /// subset: it errs towards refusing), and it follows redirects only where `required` does.
    pub fn subsumed_by(&self, required: &ConnectionAllowlist) -> bool {
        (!self.allow_redirects || required.allow_redirects) && self.entries.iter().all(|e| required.entries.contains(e))
    }

    /// Whether a request for `url` may be made. `redirected`: `url` is a redirect's target.
    pub fn allows(&self, url: &Url, redirected: bool) -> bool {
        if !matches!(url.scheme(), "http" | "https" | "ws" | "wss" | "ftp") {
            return true;
        }
        if redirected {
            return self.allow_redirects;
        }
        let url = normalized(url);
        self.matchers.iter().any(|m| m.matches(&url))
    }

    fn violation(&self, blocked: &Url) -> Violation {
        let mut url = blocked.clone();
        url.set_fragment(None);
        let _ = url.set_username("");
        let _ = url.set_password(None);
        Violation {
            effective_directive: DIRECTIVE.to_owned(),
            violated_directive: DIRECTIVE.to_owned(),
            original_policy: self.text.clone(),
            disposition: self.disposition,
            policy_source: PolicySource::Header,
            blocked_url: url.to_string(),
            checked_url: url.to_string(),
            sample: String::new(),
            source_file: String::new(),
            line: 0,
            column: 0,
            status_code: 0,
            report_uris: Vec::new(),
            report_to: self.report_to.clone(),
        }
    }
}

/// A document's allowlists: the enforced one and the report-only one (either may be missing).
#[derive(Debug)]
pub struct ConnectionPolicy {
    enforce: Option<ConnectionAllowlist>,
    report_only: Option<ConnectionAllowlist>,
}

impl ConnectionPolicy {
    /// The policy of a response from `response_url` with these header values (`None`: absent),
    /// `None` when neither parses.
    pub fn from_headers(enforce: Option<&str>, report_only: Option<&str>, response_url: &Url) -> Option<Arc<ConnectionPolicy>> {
        let enforce = enforce.and_then(|v| ConnectionAllowlist::parse(v, Disposition::Enforce, response_url));
        let report_only = report_only.and_then(|v| ConnectionAllowlist::parse(v, Disposition::Report, response_url));
        (enforce.is_some() || report_only.is_some()).then(|| Arc::new(ConnectionPolicy { enforce, report_only }))
    }

    /// Whether the enforced list of this policy is at least as strict as `required`, a value of the
    /// embedding `iframe`'s `connectionallowlist` attribute read against `response_url` (Embedded
    /// Enforcement: a framed document delivers a list that subsumes it). No enforced list, or a
    /// `required` that is not a valid value: not subsumed.
    pub fn subsumes(&self, required: &str, response_url: &Url) -> bool {
        let Some(required) = ConnectionAllowlist::parse(required, Disposition::Enforce, response_url) else { return false };
        self.enforce.as_ref().is_some_and(|own| own.subsumed_by(&required))
    }

    /// This policy with the enforced list replaced by `required` (the policy of a framed document
    /// that opted in to its embedder's requirement and delivered no list as strict): its
    /// report-only list is kept. `None` when `required` is not a valid value.
    pub fn with_required(this: Option<&Arc<ConnectionPolicy>>, required: &str, response_url: &Url) -> Option<Arc<ConnectionPolicy>> {
        let enforce = ConnectionAllowlist::parse(required, Disposition::Enforce, response_url)?;
        let report_only = this.and_then(|p| p.report_only.as_ref()).and_then(|r| {
            // `ConnectionAllowlist` is not `Clone`: the report-only list is parsed again from its text.
            ConnectionAllowlist::parse(&r.text, Disposition::Report, response_url)
        });
        Some(Arc::new(ConnectionPolicy { enforce: Some(enforce), report_only }))
    }

    /// Whether `value` is a Structured Field inner list `Connection-Allowlist` reads (an iframe's
    /// `connectionallowlist` attribute is held to the same grammar: CR, LF and anything else that is
    /// not one list never become a request header).
    pub fn is_valid_value(value: &str) -> bool {
        parse_inner_list(value).is_some()
    }

    /// The check of a request for `url`, which started as one for `original_url` (a redirect's
    /// violation names the URL before its redirects). Blocked when the enforced list refuses it.
    pub fn check(&self, url: &Url, original_url: &Url, redirected: bool) -> CheckOutcome {
        let mut outcome = CheckOutcome::default();
        for list in [&self.enforce, &self.report_only].into_iter().flatten() {
            if !list.allows(url, redirected) {
                outcome.violations.push(list.violation(original_url));
                if list.disposition == Disposition::Enforce {
                    outcome.blocked = true;
                }
            }
        }
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(value: &str) -> Arc<ConnectionPolicy> {
        ConnectionPolicy::from_headers(Some(value), None, &Url::parse("http://a.test:8000/page").unwrap()).unwrap()
    }

    fn allowed(value: &str, url: &str) -> bool {
        let url = Url::parse(url).unwrap();
        policy(value).check(&url, &url, false).allowed()
    }

    #[test]
    fn response_origin_and_patterns() {
        assert!(allowed("(response-origin)", "http://a.test:8000/x?y#z"));
        assert!(!allowed("(response-origin)", "http://www.a.test:8000/x"));
        assert!(!allowed("(response-origin)", "https://a.test:8000/x"));
        assert!(!allowed("()", "http://a.test:8000/x"));
        // Local schemes make no connection.
        assert!(allowed("()", "data:text/plain,1"));
        assert!(allowed("(response-origin \"*://:sub.b.test:*\")", "http://www.b.test:81/"));
        assert!(!allowed("(response-origin \"*://:sub.b.test:*\")", "http://b.test:81/"));
        // WebSocket schemes count as HTTP ones.
        assert!(allowed("(response-origin \"http://a.test:9000\")", "ws://a.test:9000/echo"));
        assert!(allowed("(response-origin)", "ws://a.test:8000/echo"));
    }

    #[test]
    fn embedded_enforcement_subsumption() {
        let url = Url::parse("http://a.test:8000/page").unwrap();
        // A framed document's delivered list subsumes the embedder's requirement when it allows
        // nothing the requirement does not (WPT connection-allowlist iframe-connectionallowlist-embedded-enforcement).
        assert!(policy("(response-origin)").subsumes("(response-origin)", &url));
        assert!(policy("()").subsumes("(response-origin)", &url));
        assert!(!policy("(response-origin \"https://extra.test/\")").subsumes("(response-origin)", &url));
        assert!(policy("(response-origin \"https://extra.test/\")").subsumes("(response-origin \"https://extra.test/\")", &url));
        assert!(!policy("(response-origin);redirects=allow").subsumes("(response-origin)", &url));
        assert!(policy("(response-origin)").subsumes("(response-origin);redirects=allow", &url));
        // No enforced list, or a requirement that is not a list: nothing is subsumed.
        let report_only = ConnectionPolicy::from_headers(None, Some("(response-origin)"), &url).unwrap();
        assert!(!report_only.subsumes("(response-origin)", &url));
        assert!(!policy("(response-origin)").subsumes("not a list", &url));
        assert!(ConnectionPolicy::is_valid_value("(\"https://example.test/\")"));
        assert!(!ConnectionPolicy::is_valid_value("(\"https://example.test/\")\nX-Injected: evil"));
        // An opted-in document enforces the requirement.
        let enforced = ConnectionPolicy::with_required(None, "(response-origin)", &url).unwrap();
        let blocked = Url::parse("http://www.a.test:8000/").unwrap();
        assert!(enforced.check(&blocked, &blocked, false).blocked);
        assert!(enforced.check(&url, &url, false).allowed());
    }

    #[test]
    fn parameters_and_garbage() {
        let url = Url::parse("http://a.test:8000/x").unwrap();
        assert!(!policy("(response-origin); redirects=block").check(&url, &url, true).allowed());
        assert!(!policy("(response-origin)").check(&url, &url, true).allowed());
        assert!(policy("(response-origin);redirects=allow").check(&url, &url, true).allowed());
        let blocked = policy("(response-origin); report-to=endpoint")
            .check(&Url::parse("http://b.test/").unwrap(), &Url::parse("http://b.test/#f").unwrap(), false);
        assert!(blocked.blocked);
        assert_eq!(blocked.violations[0].report_to.as_deref(), Some("endpoint"));
        assert_eq!(blocked.violations[0].blocked_url, "http://b.test/");
        assert!(ConnectionPolicy::from_headers(Some("response-origin"), None, &url).is_none());
        assert!(ConnectionPolicy::from_headers(Some("(response-origin), (x)"), None, &url).is_none());
        let report_only = ConnectionPolicy::from_headers(None, Some("(response-origin)"), &url).unwrap();
        let outcome = report_only.check(&Url::parse("http://b.test/").unwrap(), &Url::parse("http://b.test/").unwrap(), false);
        assert!(!outcome.blocked && outcome.violations.len() == 1);
    }
}
