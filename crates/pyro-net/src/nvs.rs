//! `No-Vary-Search` (https://httpwg.org/http-extensions/draft-ietf-httpbis-no-vary-search.html):
//! which query-string differences between two URLs a response does not vary on.
//!
//! Public interface (all pure, no I/O):
//! - [`NoVarySearch::parse`]`(&str) -> NoVarySearch`: "parse a URL search variance". Takes the header
//!   value (several header lines joined with `", "`, which is how a structured-field dictionary combines
//!   them). A value that is not a valid dictionary, or whose `key-order`, `params` or `except` members
//!   have the wrong type, yields [`NoVarySearch::default`] (no variance: the URLs must match exactly).
//! - [`NoVarySearch::from_headers`]`(&HeaderMap) -> NoVarySearch`: the same for a response's headers.
//! - [`NoVarySearch::equivalent`]`(&self, a: &Url, b: &Url) -> bool`: "equivalent modulo search variance".
//!   Used with the variance of the STORED response (the prefetched one, or the hint of a still pending
//!   prefetch) against the stored URL and the URL being looked up; the relation is symmetric.
//! - [`NoVarySearch::is_default`]: the variance is the default (callers may skip the secondary lookup).
//!
//! Matching ignores the fragment (it is never sent to the server). Paths, hosts and everything else
//! outside the query must be identical.

use http::header::HeaderMap;
use url::Url;

/// Which query parameters take part in the comparison.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Params {
    /// Every parameter varies except the named ones (`params=("a" "b")`; empty = the default).
    Ignore(Vec<String>),
    /// No parameter varies except the named ones (`params, except=("a")`; `params` alone = empty list).
    OnlyThese(Vec<String>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoVarySearch {
    vary_on_key_order: bool,
    params: Params,
}

impl Default for NoVarySearch {
    /// The default URL search variance: vary on every parameter and on their order.
    fn default() -> Self {
        NoVarySearch { vary_on_key_order: true, params: Params::Ignore(Vec::new()) }
    }
}

impl NoVarySearch {
    pub fn is_default(&self) -> bool {
        *self == NoVarySearch::default()
    }

    /// The response's `No-Vary-Search` field lines, combined; absent header = default.
    pub fn from_headers(headers: &HeaderMap) -> NoVarySearch {
        let mut lines = headers.get_all("no-vary-search").iter().map(|v| v.to_str().ok());
        let Some(first) = lines.next() else { return NoVarySearch::default() };
        let mut joined = match first {
            Some(s) => s.to_string(),
            None => return NoVarySearch::default(),
        };
        for line in lines {
            let Some(line) = line else { return NoVarySearch::default() };
            joined.push_str(", ");
            joined.push_str(line);
        }
        NoVarySearch::parse(&joined)
    }

    /// "Parse a URL search variance" from a header value.
    pub fn parse(value: &str) -> NoVarySearch {
        let Some(dict) = parse_dictionary(value) else { return NoVarySearch::default() };
        let mut variance = NoVarySearch::default();
        let member = |name: &str| dict.iter().find(|(k, _)| k == name).map(|(_, v)| v);
        // key-order: a boolean `true` means the order does not matter.
        if let Some(v) = member("key-order") {
            match v {
                Member::Item(Bare::Boolean(b)) => variance.vary_on_key_order = !*b,
                _ => return NoVarySearch::default(),
            }
        }
        // params: `true` = no parameter varies, an inner list of strings = those do not vary.
        let mut params_is_true = false;
        if let Some(v) = member("params") {
            match v {
                Member::Item(Bare::Boolean(b)) => {
                    if *b {
                        params_is_true = true;
                        variance.params = Params::OnlyThese(Vec::new());
                    }
                }
                Member::InnerList(items) => match strings(items) {
                    Some(keys) => variance.params = Params::Ignore(keys),
                    None => return NoVarySearch::default(),
                },
                _ => return NoVarySearch::default(),
            }
        }
        // except: only meaningful with `params` true; the listed parameters DO vary.
        if let Some(v) = member("except") {
            if !params_is_true {
                return NoVarySearch::default();
            }
            match v {
                Member::InnerList(items) => match strings(items) {
                    Some(keys) => variance.params = Params::OnlyThese(keys),
                    None => return NoVarySearch::default(),
                },
                _ => return NoVarySearch::default(),
            }
        }
        variance
    }

    /// "Equivalent modulo search variance" of `a` and `b`.
    pub fn equivalent(&self, a: &Url, b: &Url) -> bool {
        // 1. Everything but the query (and the fragment) must be the same.
        if !same_but_query(a, b) {
            return false;
        }
        // 2. Identical query strings (null and empty count as equal) are equivalent under any variance.
        if a.query().unwrap_or("") == b.query().unwrap_or("") {
            return true;
        }
        // 3-5. Parse, filter, optionally sort by key (UTF-16 code units, stable), compare pairwise.
        let la = self.prepared(a);
        let lb = self.prepared(b);
        la == lb
    }

    fn prepared(&self, url: &Url) -> Vec<(String, String)> {
        let mut list: Vec<(String, String)> = url
            .query_pairs()
            .filter(|(k, _)| match &self.params {
                Params::Ignore(skip) => !skip.iter().any(|s| s == k.as_ref()),
                Params::OnlyThese(keep) => keep.iter().any(|s| s == k.as_ref()),
            })
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        if !self.vary_on_key_order {
            list.sort_by(|x, y| x.0.encode_utf16().cmp(y.0.encode_utf16()));
        }
        list
    }
}

/// Scheme, credentials, host, port and path equal; query and fragment not looked at.
fn same_but_query(a: &Url, b: &Url) -> bool {
    a.scheme() == b.scheme()
        && a.username() == b.username()
        && a.password() == b.password()
        && a.host_str() == b.host_str()
        && a.port() == b.port()
        && a.path() == b.path()
}

/// The strings of an inner list, percent-decoded (so `"%C2%A2"` names the key `¢`); `None` if any item
/// is not a string.
fn strings(items: &[Bare]) -> Option<Vec<String>> {
    items
        .iter()
        .map(|i| match i {
            Bare::String(s) => Some(percent_decode(s)),
            _ => None,
        })
        .collect()
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push(h << 4 | l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(c: u8) -> Option<u8> {
    (c as char).to_digit(16).map(|d| d as u8)
}

// ---- RFC 8941 structured field dictionary (just enough to validate and read it) ----

#[derive(Debug)]
enum Bare {
    Boolean(bool),
    String(String),
    /// Integer, decimal, token or byte sequence: valid, but of no use here.
    Other,
}

#[derive(Debug)]
enum Member {
    Item(Bare),
    InnerList(Vec<Bare>),
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

/// `None` = parsing failed (the whole field is ignored).
fn parse_dictionary(input: &str) -> Option<Vec<(String, Member)>> {
    // Leading and trailing SP are discarded; other whitespace is not allowed.
    let trimmed = input.trim_matches(' ');
    let mut p = Parser { s: trimmed.as_bytes(), i: 0 };
    let mut out: Vec<(String, Member)> = Vec::new();
    while p.i < p.s.len() {
        let key = p.key()?;
        let member = if p.peek() == Some(b'=') {
            p.i += 1;
            if p.peek() == Some(b'(') { Member::InnerList(p.inner_list()?) } else { Member::Item(p.bare_item()?) }
        } else {
            Member::Item(Bare::Boolean(true))
        };
        p.parameters()?;
        // A repeated key overwrites the earlier value but keeps its place.
        if let Some(slot) = out.iter_mut().find(|(k, _)| *k == key) {
            slot.1 = member;
        } else {
            out.push((key, member));
        }
        p.ows();
        if p.i >= p.s.len() {
            return Some(out);
        }
        if p.peek() != Some(b',') {
            return None;
        }
        p.i += 1;
        p.ows();
        if p.i >= p.s.len() {
            return None; // trailing comma
        }
    }
    Some(out)
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn ows(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t')) {
            self.i += 1;
        }
    }

    fn sp(&mut self) {
        while self.peek() == Some(b' ') {
            self.i += 1;
        }
    }

    fn key(&mut self) -> Option<String> {
        let start = self.i;
        match self.peek() {
            Some(c) if c.is_ascii_lowercase() || c == b'*' => self.i += 1,
            _ => return None,
        }
        while matches!(self.peek(), Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'_' | b'-' | b'.' | b'*')) {
            self.i += 1;
        }
        Some(String::from_utf8_lossy(&self.s[start..self.i]).into_owned())
    }

    fn parameters(&mut self) -> Option<()> {
        while self.peek() == Some(b';') {
            self.i += 1;
            self.sp();
            self.key()?;
            if self.peek() == Some(b'=') {
                self.i += 1;
                self.bare_item()?;
            }
        }
        Some(())
    }

    fn inner_list(&mut self) -> Option<Vec<Bare>> {
        self.i += 1; // "("
        let mut items = Vec::new();
        loop {
            self.sp();
            match self.peek()? {
                b')' => {
                    self.i += 1;
                    self.parameters()?;
                    return Some(items);
                }
                _ => {
                    items.push(self.bare_item()?);
                    self.parameters()?;
                    // Items are separated by SP, or the list ends.
                    if !matches!(self.peek(), Some(b' ' | b')')) {
                        return None;
                    }
                }
            }
        }
    }

    fn bare_item(&mut self) -> Option<Bare> {
        match self.peek()? {
            b'"' => self.string(),
            b'?' => {
                self.i += 1;
                let v = match self.peek()? {
                    b'1' => true,
                    b'0' => false,
                    _ => return None,
                };
                self.i += 1;
                Some(Bare::Boolean(v))
            }
            b':' => {
                self.i += 1;
                let start = self.i;
                while matches!(self.peek(), Some(c) if c.is_ascii_alphanumeric() || matches!(c, b'+' | b'/' | b'=')) {
                    self.i += 1;
                }
                if self.peek() != Some(b':') {
                    return None;
                }
                let _ = start;
                self.i += 1;
                Some(Bare::Other)
            }
            c if c == b'-' || c.is_ascii_digit() => self.number(),
            c if c.is_ascii_alphabetic() || c == b'*' => {
                self.i += 1;
                while matches!(self.peek(), Some(c) if c.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~:/".contains(&c)) {
                    self.i += 1;
                }
                Some(Bare::Other)
            }
            _ => None,
        }
    }

    fn string(&mut self) -> Option<Bare> {
        self.i += 1; // opening quote
        let mut out = String::new();
        loop {
            let c = self.peek()?;
            self.i += 1;
            match c {
                b'\\' => {
                    let n = self.peek()?;
                    self.i += 1;
                    if n != b'"' && n != b'\\' {
                        return None;
                    }
                    out.push(n as char);
                }
                b'"' => return Some(Bare::String(out)),
                0x20..=0x7e => out.push(c as char),
                _ => return None,
            }
        }
    }

    fn number(&mut self) -> Option<Bare> {
        if self.peek() == Some(b'-') {
            self.i += 1;
        }
        let mut int_digits = 0;
        while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
            self.i += 1;
            int_digits += 1;
            if int_digits > 15 {
                return None;
            }
        }
        if int_digits == 0 {
            return None;
        }
        if self.peek() == Some(b'.') {
            if int_digits > 12 {
                return None;
            }
            self.i += 1;
            let mut frac = 0;
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                self.i += 1;
                frac += 1;
                if frac > 3 {
                    return None;
                }
            }
            if frac == 0 {
                return None;
            }
        }
        Some(Bare::Other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(query: &str) -> Url {
        let mut url = Url::parse("https://example.com/prefetch-executor.html").unwrap();
        if !query.is_empty() {
            url.set_query(Some(query));
        }
        url
    }

    fn check(nvs: &str, prefetch: &str, navigate: &str, expect: bool) {
        let v = NoVarySearch::parse(nvs);
        assert_eq!(v.equivalent(&u(prefetch), &u(navigate)), expect, "{nvs:?}: {prefetch:?} vs {navigate:?}");
        assert_eq!(v.equivalent(&u(navigate), &u(prefetch)), expect, "symmetric: {nvs:?}: {navigate:?} vs {prefetch:?}");
    }

    /// Rows of WPT speculation-rules/prefetch/no-vary-search/test-inputs.js (and the hint variants, which
    /// feed the same matcher with a hint instead of the header).
    #[test]
    fn wpt_test_inputs() {
        check(r#"params=("a")"#, "a=2&b=3", "b=3", true);
        check(r#"params("a")"#, "a=2&b=3", "b=2", false);
        check(r#"params=("a" "b")"#, "a=2&b=3", "b=2", true);
        check("params", "a=2&b=3", "b=4&c=5", true);
        check("params", "", "b=4&c=5", true);
        check("key-order", "c=4&b=3&a=2", "a=2&c=5&b=3", false);
        check("key-order", "b=5&a=3&a=4&d=6&c=5&b=3", "d=6&a=4&b=5&b=3&c=5&a=3", false);
        check("key-order", "b=5&a=3&a=4&d=6&c=5&b=3", "d=6&a=3&b=5&b=3&c=5&a=4", true);
        check("key-order=?1", "b=5&a=3&a=4&d=6&c=5&b=3", "d=6&a=3&b=5&b=3&c=5&a=4", true);
        check("key-order=?0", "b=5&a=3&a=4&d=6&c=5&b=3", "d=6&a=3&b=5&b=3&c=5&a=4", false);
        check(r#"params=("c")"#, "a=2&b=2&c=5", "a=2&c=3&b=2", true);
        check(r#"params=("a")"#, "a=2", "", true);
        check(r#"params=("a")"#, "", "a=2", true);
        check(r#"params, except=("c")"#, "b=5&a=3&d=6&c=3", "a=1&b=2&c=3", true);
        check(r#"params, except=("b"), except=("c")"#, "b=5&a=3&d=6&c=3", "a=1&b=2&c=3", true);
        check(r#"params, except=("c")"#, "b=5&a=3&d=6&c=3", "a=1&b=2&c=5", false);
        check(r#"params, except=("c" "d")"#, "b=5&a=3&d=6&c=5", "d=6&a=1&b=2&c=5", true);
        check(r#"params, except=("c" "d")"#, "b=5&a=3&a=4&d=6&c=5", "d=6&a=1&a=2&b=2&b=3&c=5", true);
        check(r#"params, except=("c";unknown)"#, "b=5&a=3&d=6&c=3", "a=1&b=2&c=3", true);
        check(r#"params=("c";unknown)"#, "a=2&b=2&c=5", "a=2&c=3&b=2", true);
        check("key-order;unknown", "b=5&a=3&a=4&d=6&c=5&b=3", "d=6&a=3&b=5&b=3&c=5&a=4", true);
        check("params;unknown", "", "b=4&c=5", true);
        check(r#"params;unknown, except=("c");unknown"#, "b=5&a=3&d=6&c=3", "a=1&b=2&c=3", true);
        check("", "b=5&a=3&d=6&c=3", "a=1&b=2&c=3", false);
        check("", "b=5&a=3&d=6&c=3", "b=5&a=3&d=6&c=3", true);
        check("", "", "", true);
        // Non-ASCII key, 2 UTF-8 code units: the header names it percent-encoded.
        check(r#"params=("%C2%A2")"#, "¢=3", "¢=4", true);
        check(r#"params=("%C2%A2")"#, "a=2&¢=3", "¢=4&a=2", true);
        check(r#"params, except=("%C2%A2")"#, "¢=3", "¢=4", false);
        check(r#"params, except=("%C2%A2")"#, "¢=3&a=4", "a=5&¢=3", true);
        // Hint-only malformed values fall back to the default.
        check("params=(a)", "a=1", "a=2", false);
        check("0", "a=1", "a=2", false);
    }

    /// WPT fetch/http-cache/no-vary-search.tentative.any.js headers.
    #[test]
    fn wpt_http_cache_headers() {
        check(r#"params, except=("dispatch" "uuid")"#, "dispatch=test&uuid=1&a=1&b=2", "dispatch=test&uuid=1", true);
        check(r#"params=?1, except=("dispatch" "uuid")"#, "dispatch=test&uuid=1&a=1&b=2", "dispatch=test&uuid=1", true);
        check("", "a=1&b=2", "b=2&a=1", false);
        check("key-order", "a=1&b=2", "b=2&a=1", true);
        check("key-order", "a=1&b=2", "b=2", false);
        check("key-order", "a=1&b=2", "a=2&b=2", false);
        check("key-order", "a=1&b=2", "a=1&b=2&c=3", false);
        check(r#"params=("a")"#, "a=1&b=2", "a=99&b=2", true);
        check(r#"params=("a")"#, "a=1&b=2", "a=1&b=99", false);
        check(r#"params=("a" "b")"#, "a=1&b=2&c=3", "a=9&b=8&c=3", true);
        check(r#"params, except=("dispatch" "uuid" "id")"#, "id=42&noise=abc", "id=99&noise=xyz", false);
        check(r#"params=("utm"), key-order"#, "utm=x&b=2&a=1", "a=1&b=2&utm=y", true);
        check(r#"params=("utm"), key-order"#, "utm=x&b=2&a=1", "a=1&b=99&utm=y", false);
        // params list together with except, and except without params: invalid, so exact match.
        check(r#"params=("a"), except=("b")"#, "a=1", "a=2", false);
        check(r#"except=("a")"#, "a=1", "a=2", false);
    }

    #[test]
    fn spec_examples() {
        // https://httpwg.org/http-extensions/draft-ietf-httpbis-no-vary-search.html#section-2.1
        check("params", "a=1", "a=2", true);
        check(r#"params=("a")"#, "a=1&b=2", "b=2", true);
        check("key-order", "a=1&b=2", "b=2&a=1", true);
        // A repeated key keeps its relative value order when sorting.
        check("key-order", "a=1&a=2", "a=2&a=1", false);
        // + and %20 are the same space.
        check("params=(\"a b\")", "a+b=1&c=2", "c=2", true);
    }

    #[test]
    fn non_query_parts_must_match() {
        let v = NoVarySearch::parse("params");
        let a = Url::parse("https://example.com/a?x=1").unwrap();
        assert!(!v.equivalent(&a, &Url::parse("https://example.com/b?x=1").unwrap()));
        assert!(!v.equivalent(&a, &Url::parse("http://example.com/a?x=1").unwrap()));
        assert!(!v.equivalent(&a, &Url::parse("https://example.org/a?x=1").unwrap()));
        assert!(v.equivalent(&a, &Url::parse("https://example.com/a?x=2#frag").unwrap()));
        assert!(NoVarySearch::default().equivalent(&a, &Url::parse("https://example.com/a?x=1#frag").unwrap()));
    }

    #[test]
    fn utf16_key_order() {
        // U+FF5E (BMP, one unit 0xFF5E) sorts after U+1F600 (surrogates 0xD83D..) in UTF-16 order.
        let v = NoVarySearch::parse("key-order");
        let a = Url::parse("https://e.com/?%EF%BD%9E=1&%F0%9F%98%80=2").unwrap();
        let b = Url::parse("https://e.com/?%F0%9F%98%80=2&%EF%BD%9E=1").unwrap();
        assert!(v.equivalent(&a, &b));
    }

    #[test]
    fn parsing_is_strict_where_the_spec_is() {
        assert!(NoVarySearch::parse("").is_default());
        assert!(NoVarySearch::parse("key-order=1").is_default()); // integer, not boolean
        assert!(NoVarySearch::parse("params=\"a\"").is_default()); // string, not list
        assert!(NoVarySearch::parse("params=(1)").is_default());
        assert!(NoVarySearch::parse("params, except=a").is_default());
        assert!(NoVarySearch::parse("params, ").is_default()); // trailing comma
        assert!(NoVarySearch::parse("Params").is_default()); // keys are lowercase
        assert!(NoVarySearch::parse("params=?0").is_default());
        assert!(NoVarySearch::parse("params=?0, except=(\"a\")").is_default());
        assert!(!NoVarySearch::parse("params").is_default());
        // Unknown members are ignored.
        assert_eq!(NoVarySearch::parse("key-order, unknown=5"), NoVarySearch::parse("key-order"));
        // Several header lines combine into one dictionary.
        let mut h = HeaderMap::new();
        h.append("no-vary-search", "key-order".parse().unwrap());
        h.append("no-vary-search", "params=(\"a\")".parse().unwrap());
        let v = NoVarySearch::from_headers(&h);
        assert!(v.equivalent(&u("a=1&b=2"), &u("b=2&a=3")));
        assert!(NoVarySearch::from_headers(&HeaderMap::new()).is_default());
    }

    #[test]
    fn percent_decoding_of_names() {
        assert_eq!(percent_decode("%C2%A2"), "¢");
        assert_eq!(percent_decode("a%2"), "a%2");
        assert_eq!(percent_decode("%"), "%");
        assert_eq!(percent_decode("%zz"), "%zz");
        assert_eq!(percent_decode("%41"), "A");
    }
}
