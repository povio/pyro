//! Response-header integrity: `Unencoded-Digest` (a digest of the decoded body the server
//! vouches for) and Signature-based SRI (https://wicg.github.io/signature-based-sri/: HTTP
//! message signatures, RFC 9421, over that digest and other response or request components).
//!
//! - A response with an `Unencoded-Digest` header (a structured dictionary of `sha-256` /
//!   `sha-512` byte sequences; other algorithms and malformed members are ignored) is blocked
//!   when a digest it lists differs from the body's: whatever the request's `integrity`.
//! - A response whose `Signature-Input` has members tagged `"sri"` with a `keyid` that is an
//!   Ed25519 public key (standard base64, 32 bytes) is blocked when one of those signatures
//!   does not verify: its signature base is built from the covered components (`"unencoded-digest";sf`
//!   is required, the derived `@status`, `@authority;req`, `@scheme;req`, `@path;req` and
//!   `@query;req`, and any header field), the `@signature-params` line being the parsed input
//!   serialized again (so spacing and parameter spelling of the header do not matter). A signature
//!   whose `expires` has passed does not verify. A covered component the response or request
//!   cannot supply fails the signature.
//! - A request whose `integrity` metadata has Ed25519 keys (`ed25519-<base64>`; a malformed one
//!   is ignored, as are duplicates) is blocked unless a signature that verified names one of
//!   them (so an unsigned response is blocked), and carries
//!   `Accept-Signature: sig0=("unencoded-digest";sf);keyid="<key>";tag="ed25519-integrity", sig1=...`.
//!
//! Everything here costs a header lookup for a response without the headers ([`wants_check`]).

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::{Signature, VerifyingKey};
use pyro_net::http::HeaderMap;
use pyro_net::Url;
use sha2::{Digest, Sha256, Sha512};

/// An Ed25519 public key.
pub type Key = [u8; 32];

/// What a response's checks read (kept by the loader for the element that asked).
#[derive(Clone, Debug)]
pub struct Facts {
    /// The response's URL (the last request's: `@path;req` and the like come from it).
    pub url: Url,
    pub status: u16,
    pub headers: HeaderMap,
}

/// Whether a response has headers that make it subject to a check: an `Unencoded-Digest`, or a
/// `Signature-Input` with a member tagged `"sri"` (an HTTP message signature for anything else
/// is none of this module's business: the response is not held for it).
pub fn wants_check(headers: &HeaderMap) -> bool {
    headers.contains_key("unencoded-digest")
        || (headers.contains_key("signature-input")
            && header_dictionary(headers, "signature-input").is_some_and(|inputs| {
                inputs.iter().any(|(_, member)| matches!(member, Member::List(_, params) if is_sri_tag(param_str(params, "tag"))))
            }))
}

/// The `tag` of a Signature-Based SRI signature: `sri`, and `ed25519-integrity`, the name the
/// request's `Accept-Signature` carries and older servers (WPT integrity-policy/tentative/signature) sign with.
fn is_sri_tag(tag: Option<&str>) -> bool {
    matches!(tag, Some("sri" | "ed25519-integrity"))
}

/// The facts of a response subject to a check, `None` otherwise (no copy of the headers).
pub fn capture(url: &Url, status: u16, headers: &HeaderMap) -> Option<Box<Facts>> {
    wants_check(headers).then(|| Box::new(Facts { url: url.clone(), status, headers: headers.clone() }))
}

fn decode_key(value: &str) -> Option<Key> {
    STANDARD.decode(value).ok()?.try_into().ok()
}

/// The Ed25519 keys of `integrity` metadata (`ed25519-<base64>` tokens, in order, once each).
pub fn parse_keys(integrity: &str) -> Vec<Key> {
    let mut keys: Vec<Key> = Vec::new();
    for token in integrity.split_ascii_whitespace() {
        let expression = token.split('?').next().unwrap_or("");
        let Some((algorithm, value)) = expression.split_once('-') else { continue };
        if !algorithm.eq_ignore_ascii_case("ed25519") {
            continue;
        }
        if let Some(key) = decode_key(value) {
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
    }
    keys
}

/// The `Accept-Signature` value for a request with these keys (`None`: no key).
pub fn accept_signature(keys: &[Key]) -> Option<String> {
    if keys.is_empty() {
        return None;
    }
    let members: Vec<String> = keys
        .iter()
        .enumerate()
        .map(|(i, key)| format!("sig{i}=(\"unencoded-digest\";sf);keyid=\"{}\";tag=\"ed25519-integrity\"", STANDARD.encode(key)))
        .collect();
    Some(members.join(", "))
}

/// [`accept_signature`] of the keys in an `integrity` attribute value.
pub fn accept_signature_for(integrity: &str) -> Option<String> {
    accept_signature(&parse_keys(integrity))
}

// ---- Structured Field Values (RFC 9651), as far as these headers use them ----

#[derive(Clone, Debug, PartialEq)]
enum Bare {
    Int(i64),
    Decimal(String),
    Str(String),
    Token(String),
    Bytes(Vec<u8>),
    Bool(bool),
}

type Params = Vec<(String, Bare)>;

#[derive(Clone, Debug)]
struct Item {
    bare: Bare,
    params: Params,
}

#[derive(Clone, Debug)]
enum Member {
    Item(Item),
    List(Vec<Item>, Params),
}

struct Cursor<'a> {
    s: &'a str,
    i: usize,
}

impl Cursor<'_> {
    fn peek(&self) -> Option<char> {
        self.s[self.i..].chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.i += c.len_utf8();
        Some(c)
    }

    fn eof(&self) -> bool {
        self.i >= self.s.len()
    }

    fn skip_sp(&mut self) {
        while self.peek() == Some(' ') {
            self.i += 1;
        }
    }

    fn skip_ows(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t')) {
            self.i += 1;
        }
    }

    fn key(&mut self) -> Option<String> {
        let start = self.i;
        match self.peek()? {
            c if c.is_ascii_lowercase() || c == '*' => self.i += 1,
            _ => return None,
        }
        while let Some(c) = self.peek() {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || "_-.*".contains(c) {
                self.i += 1;
            } else {
                break;
            }
        }
        Some(self.s[start..self.i].to_owned())
    }

    fn bare(&mut self) -> Option<Bare> {
        match self.peek()? {
            '"' => {
                self.i += 1;
                let mut out = String::new();
                loop {
                    match self.bump()? {
                        '"' => return Some(Bare::Str(out)),
                        '\\' => match self.bump()? {
                            c @ ('"' | '\\') => out.push(c),
                            _ => return None,
                        },
                        c if (' '..='~').contains(&c) => out.push(c),
                        _ => return None,
                    }
                }
            }
            ':' => {
                self.i += 1;
                let start = self.i;
                let end = start + self.s[start..].find(':')?;
                let bytes = STANDARD.decode(&self.s[start..end]).ok()?;
                self.i = end + 1;
                Some(Bare::Bytes(bytes))
            }
            '?' => {
                self.i += 1;
                match self.bump()? {
                    '1' => Some(Bare::Bool(true)),
                    '0' => Some(Bare::Bool(false)),
                    _ => None,
                }
            }
            c if c == '-' || c.is_ascii_digit() => {
                let start = self.i;
                self.i += 1;
                let mut decimal = false;
                while let Some(c) = self.peek() {
                    if c.is_ascii_digit() {
                        self.i += 1;
                    } else if c == '.' && !decimal {
                        decimal = true;
                        self.i += 1;
                    } else {
                        break;
                    }
                }
                let text = &self.s[start..self.i];
                if decimal {
                    canonical_decimal(text).map(Bare::Decimal)
                } else {
                    // RFC 9651 §3.3.1: at most 15 digits.
                    let digits = text.trim_start_matches('-');
                    if digits.is_empty() || digits.len() > 15 {
                        return None;
                    }
                    text.parse().ok().map(Bare::Int)
                }
            }
            c if c.is_ascii_alphabetic() || c == '*' => {
                let start = self.i;
                self.i += 1;
                while let Some(c) = self.peek() {
                    if c.is_ascii_alphanumeric() || ":/!#$%&'*+-.^_`|~".contains(c) {
                        self.i += 1;
                    } else {
                        break;
                    }
                }
                Some(Bare::Token(self.s[start..self.i].to_owned()))
            }
            _ => None,
        }
    }

    fn params(&mut self) -> Option<Params> {
        let mut out: Params = Vec::new();
        while self.peek() == Some(';') {
            self.i += 1;
            self.skip_sp();
            let key = self.key()?;
            let value = if self.peek() == Some('=') {
                self.i += 1;
                self.bare()?
            } else {
                Bare::Bool(true)
            };
            match out.iter_mut().find(|(name, _)| *name == key) {
                Some(entry) => entry.1 = value,
                None => out.push((key, value)),
            }
        }
        Some(out)
    }

    fn item(&mut self) -> Option<Item> {
        let bare = self.bare()?;
        let params = self.params()?;
        Some(Item { bare, params })
    }

    fn member(&mut self) -> Option<Member> {
        if self.peek() != Some('(') {
            return Some(Member::Item(self.item()?));
        }
        self.i += 1;
        let mut items = Vec::new();
        loop {
            self.skip_sp();
            if self.peek() == Some(')') {
                self.i += 1;
                break;
            }
            items.push(self.item()?);
            if !matches!(self.peek()?, ' ' | ')') {
                return None;
            }
        }
        let params = self.params()?;
        Some(Member::List(items, params))
    }
}

/// A structured dictionary: its members in order, a repeated key keeping its first place and its
/// last value. `None` when the text is not one.
fn parse_dictionary(text: &str) -> Option<Vec<(String, Member)>> {
    let mut cursor = Cursor { s: text, i: 0 };
    let mut out: Vec<(String, Member)> = Vec::new();
    cursor.skip_sp();
    if cursor.eof() {
        return Some(out);
    }
    loop {
        let key = cursor.key()?;
        let member = if cursor.peek() == Some('=') {
            cursor.i += 1;
            cursor.member()?
        } else {
            let params = cursor.params()?;
            Member::Item(Item { bare: Bare::Bool(true), params })
        };
        match out.iter_mut().find(|(name, _)| *name == key) {
            Some(entry) => entry.1 = member,
            None => out.push((key, member)),
        }
        cursor.skip_ows();
        if cursor.eof() {
            return Some(out);
        }
        if cursor.bump()? != ',' {
            return None;
        }
        cursor.skip_ows();
        if cursor.eof() {
            return None;
        }
    }
}

/// RFC 9651 §3.3.2 and §4.1.5.2: a decimal of at most 12 integer and 3 fractional digits (none
/// of either is a parse failure), serialized with no leading zeros and at least one fractional
/// digit, trailing zeros dropped (`-0.0` is `0.0`).
fn canonical_decimal(text: &str) -> Option<String> {
    let (negative, unsigned) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (int, frac) = unsigned.split_once('.')?;
    if int.is_empty() || int.len() > 12 || frac.is_empty() || frac.len() > 3 {
        return None;
    }
    let int = int.trim_start_matches('0');
    let int = if int.is_empty() { "0" } else { int };
    let frac = frac.trim_end_matches('0');
    let frac = if frac.is_empty() { "0" } else { frac };
    let zero = int == "0" && frac == "0";
    Some(format!("{}{int}.{frac}", if negative && !zero { "-" } else { "" }))
}

fn serialize_bare(bare: &Bare) -> String {
    match bare {
        Bare::Int(n) => n.to_string(),
        Bare::Decimal(text) | Bare::Token(text) => text.clone(),
        Bare::Str(text) => format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\"")),
        Bare::Bytes(bytes) => format!(":{}:", STANDARD.encode(bytes)),
        Bare::Bool(true) => "?1".into(),
        Bare::Bool(false) => "?0".into(),
    }
}

fn serialize_params(params: &Params) -> String {
    params
        .iter()
        .map(|(key, value)| match value {
            Bare::Bool(true) => format!(";{key}"),
            value => format!(";{key}={}", serialize_bare(value)),
        })
        .collect()
}

fn serialize_item(item: &Item) -> String {
    format!("{}{}", serialize_bare(&item.bare), serialize_params(&item.params))
}

fn serialize_member(member: &Member) -> String {
    match member {
        Member::Item(item) => serialize_item(item),
        Member::List(items, params) => {
            let items: Vec<String> = items.iter().map(serialize_item).collect();
            format!("({}){}", items.join(" "), serialize_params(params))
        }
    }
}

fn serialize_dictionary(entries: &[(String, Member)]) -> String {
    let members: Vec<String> = entries
        .iter()
        .map(|(key, member)| match member {
            Member::Item(Item { bare: Bare::Bool(true), params }) => format!("{key}{}", serialize_params(params)),
            member => format!("{key}={}", serialize_member(member)),
        })
        .collect();
    members.join(", ")
}

/// A header's field values joined (HTTP combines repeated fields with `, `), `None` when absent
/// or not text.
fn field_text(headers: &HeaderMap, name: &str) -> Option<String> {
    let mut values = Vec::new();
    for value in headers.get_all(name) {
        values.push(value.to_str().ok()?.trim_matches([' ', '\t']).to_owned());
    }
    (!values.is_empty()).then(|| values.join(", "))
}

fn header_dictionary(headers: &HeaderMap, name: &str) -> Option<Vec<(String, Member)>> {
    parse_dictionary(&field_text(headers, name)?)
}

// ---- Unencoded-Digest ----

fn digest_matches(entries: &[(String, Member)], body: &[&[u8]]) -> bool {
    fn digest<D: Digest>(body: &[&[u8]]) -> Vec<u8> {
        let mut d = D::new();
        for chunk in body {
            d.update(chunk);
        }
        d.finalize().to_vec()
    }
    for (algorithm, member) in entries {
        let Member::Item(Item { bare: Bare::Bytes(expected), .. }) = member else { continue };
        let actual = match algorithm.as_str() {
            "sha-256" => digest::<Sha256>(body),
            "sha-512" => digest::<Sha512>(body),
            _ => continue,
        };
        if actual != *expected {
            return false;
        }
    }
    true
}

// ---- Signatures ----

fn param<'a>(params: &'a Params, name: &str) -> Option<&'a Bare> {
    params.iter().find(|(key, _)| key == name).map(|(_, value)| value)
}

fn param_str<'a>(params: &'a Params, name: &str) -> Option<&'a str> {
    match param(params, name)? {
        Bare::Str(text) => Some(text),
        _ => None,
    }
}

/// The value of covered component `name` with `params` (RFC 9421 §2.1, §2.2): `None` when this
/// response and its request cannot supply it.
fn component_value(url: &Url, status: u16, headers: &HeaderMap, name: &str, params: &Params) -> Option<String> {
    let flag = |p: &str| matches!(param(params, p), Some(Bare::Bool(true)));
    if let Some(derived) = name.strip_prefix('@') {
        let request = flag("req");
        if params.iter().any(|(key, _)| key != "req") {
            return None;
        }
        return match derived {
            "status" if !request => Some(status.to_string()),
            "authority" if request => {
                let host = url.host_str()?;
                Some(match url.port() {
                    Some(port) => format!("{host}:{port}"),
                    None => host.to_owned(),
                })
            }
            "scheme" if request => Some(url.scheme().to_owned()),
            "path" if request => Some(url.path().to_owned()),
            "query" if request => Some(format!("?{}", url.query().unwrap_or(""))),
            _ => None,
        };
    }
    if name.bytes().any(|b| b.is_ascii_uppercase()) || params.iter().any(|(key, _)| key != "sf") {
        return None;
    }
    let text = field_text(headers, name)?;
    if flag("sf") {
        // A structured field: its serialization (a dictionary, which `Unencoded-Digest` is; a
        // single item).
        if let Some(entries) = parse_dictionary(&text) {
            return Some(serialize_dictionary(&entries));
        }
        let mut cursor = Cursor { s: &text, i: 0 };
        let item = cursor.item()?;
        return cursor.eof().then(|| serialize_item(&item));
    }
    Some(text)
}

/// RFC 9421 §2.5 "Creating the signature base" for the covered `components` and `params` of
/// one signature.
fn signature_base(url: &Url, status: u16, headers: &HeaderMap, components: &[Item], params: &Params) -> Option<String> {
    let mut base = String::new();
    for item in components {
        let Bare::Str(name) = &item.bare else { return None };
        let value = component_value(url, status, headers, name, &item.params)?;
        base.push_str(&serialize_item(item));
        base.push_str(": ");
        base.push_str(&value);
        base.push('\n');
    }
    base.push_str("\"@signature-params\": ");
    base.push_str(&serialize_member(&Member::List(components.to_vec(), params.clone())));
    Some(base)
}

/// Whether the covered components include `"unencoded-digest";sf`.
fn covers_digest(components: &[Item]) -> bool {
    components.iter().any(|item| {
        matches!(&item.bare, Bare::Str(name) if name == "unencoded-digest")
            && item.params.len() == 1
            && matches!(param(&item.params, "sf"), Some(Bare::Bool(true)))
    })
}

fn now_seconds() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// The keys of the response's `"sri"`-tagged signatures, `None` when one of them does not verify.
/// A member that is not tagged `"sri"`, has no usable `keyid` or no matching `Signature` is not a
/// signature of this kind.
fn verified_keys(url: &Url, status: u16, headers: &HeaderMap) -> Option<Vec<Key>> {
    let mut verified = Vec::new();
    let Some(inputs) = header_dictionary(headers, "signature-input") else { return Some(verified) };
    let Some(signatures) = header_dictionary(headers, "signature") else { return Some(verified) };
    for (label, member) in &inputs {
        let Member::List(components, params) = member else { continue };
        if !is_sri_tag(param_str(params, "tag")) {
            continue;
        }
        let Some(key) = param_str(params, "keyid").and_then(decode_key) else { continue };
        let Some((_, Member::Item(Item { bare: Bare::Bytes(signature), .. }))) =
            signatures.iter().find(|(name, _)| name == label)
        else {
            continue;
        };
        if let Some(Bare::Int(expires)) = param(params, "expires") {
            if *expires < now_seconds() {
                return None;
            }
        }
        if !covers_digest(components) {
            return None;
        }
        let base = signature_base(url, status, headers, components, params)?;
        let signature = Signature::from_slice(signature).ok()?;
        let public = VerifyingKey::from_bytes(&key).ok()?;
        public.verify_strict(base.as_bytes(), &signature).ok()?;
        verified.push(key);
    }
    Some(verified)
}

/// Whether a response may be used: its `Unencoded-Digest` matches `body`, every signature it
/// carries verifies, and when the request asserted `keys` one of them signed it. `url` and
/// `status` are the response's.
pub fn verify(url: &Url, status: u16, headers: &HeaderMap, body: &[&[u8]], keys: &[Key]) -> bool {
    if let Some(entries) = header_dictionary(headers, "unencoded-digest") {
        if !digest_matches(&entries, body) {
            return false;
        }
    }
    let Some(verified) = verified_keys(url, status, headers) else { return false };
    keys.is_empty() || keys.iter().any(|key| verified.contains(key))
}

/// [`verify`] for a response the loader kept ([`capture`]; `None`: it had neither header).
pub fn verify_facts(facts: Option<&Facts>, body: &[u8], keys: &[Key]) -> bool {
    match facts {
        Some(facts) => verify(&facts.url, facts.status, &facts.headers, &[body], keys),
        None => keys.is_empty(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RFC_KEY: &str = "JrQLj5P/89iXES9+vFgrIy29clF9CC/oPPsw3c5D0bs=";
    const INVALID_KEY: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
    const BODY: &[u8] = br#"{"hello": "world"}"#;
    const DIGEST: &str = "sha-256=:X48E9qOokqqrvdts8nOJRJN3OWDUoyWxBf7kbu9DBPE=:";
    // The WPT subresource-integrity/signatures canonical response (RFC 9421 test key).
    const SIGNATURE: &str = "signature=:gHim9e5Pk2H7c9BStOmxSmkyc8+ioZgoxynu3d4INAT4dwfj5LhvaV9DFnEQ9p7C0hzW4o4Qpkm5aApd6WLLCw==:";

    fn url() -> Url {
        Url::parse("https://a.test/subresource-integrity/signatures/resource.py?x=1").unwrap()
    }

    fn headers(digest: Option<&str>, input: Option<&str>, signature: Option<&str>) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (name, value) in [("unencoded-digest", digest), ("signature-input", input), ("signature", signature)] {
            if let Some(value) = value {
                h.append(name, value.parse().unwrap());
            }
        }
        h
    }

    fn input(spacing: &str) -> String {
        format!("signature=(\"unencoded-digest\";sf);{spacing}keyid=\"{RFC_KEY}\";{spacing}tag=\"sri\"")
    }

    #[test]
    fn unencoded_digest() {
        let ok = |digest: &str| verify(&url(), 200, &headers(Some(digest), None, None), &[BODY], &[]);
        assert!(ok(DIGEST));
        assert!(ok(&format!("{DIGEST},{DIGEST}")));
        assert!(!ok("sha-256=:vT+a3uWsoxRxVJEINKfH4XZpLqsneOzhFVY98Y3iIz0=:"));
        // The last of a repeated key counts; an unknown algorithm is ignored; chunks add up.
        assert!(!ok(&format!("{DIGEST}, sha-256=:vT+a3uWsoxRxVJEINKfH4XZpLqsneOzhFVY98Y3iIz0=:")));
        assert!(ok(&format!("{DIGEST}, md5=:AAAA:")));
        assert!(verify(&url(), 200, &headers(Some(DIGEST), None, None), &[&BODY[..5], &BODY[5..]], &[]));
        assert!(verify(&url(), 200, &HeaderMap::new(), &[BODY], &[]));
    }

    #[test]
    fn signatures_verify_and_keys_are_required() {
        let key = parse_keys(&format!("ed25519-{RFC_KEY}"));
        let invalid = parse_keys(&format!("ed25519-{INVALID_KEY}"));
        assert_eq!(key.len(), 1);
        for spacing in ["", " "] {
            let h = headers(Some(DIGEST), Some(&input(spacing)), Some(SIGNATURE));
            assert!(verify(&url(), 200, &h, &[BODY], &[]));
            assert!(verify(&url(), 200, &h, &[BODY], &key));
            assert!(!verify(&url(), 200, &h, &[BODY], &invalid));
            let both: Vec<Key> = key.iter().chain(&invalid).copied().collect();
            assert!(verify(&url(), 200, &h, &[BODY], &both));
            // A body the digest does not cover.
            assert!(!verify(&url(), 200, &h, &[b"{}"], &[]));
        }
        // No signature: fine until a key is asserted.
        let none = headers(Some(DIGEST), None, None);
        assert!(verify(&url(), 200, &none, &[BODY], &[]));
        assert!(!verify(&url(), 200, &none, &[BODY], &key));
        // A forged signature blocks even without keys.
        let forged = format!("signature=:{}:", "A".repeat(86) + "==");
        assert!(!verify(&url(), 200, &headers(Some(DIGEST), Some(&input("")), Some(&forged)), &[BODY], &[]));
        // Not tagged "sri": not ours.
        let other = input("").replace("tag=\"sri\"", "tag=\"other\"");
        assert!(verify(&url(), 200, &headers(Some(DIGEST), Some(&other), Some(&forged)), &[BODY], &[]));
        // A covered header the response lacks fails the signature.
        let missing = input("").replace("(\"unencoded-digest\";sf)", "(\"x-missing\" \"unencoded-digest\";sf)");
        assert!(!verify(&url(), 200, &headers(Some(DIGEST), Some(&missing), Some(SIGNATURE)), &[BODY], &[]));
    }

    #[test]
    fn only_digests_and_sri_signatures_hold_a_response() {
        assert!(!wants_check(&HeaderMap::new()));
        assert!(wants_check(&headers(Some(DIGEST), None, None)));
        assert!(wants_check(&headers(None, Some(&input("")), None)));
        // An unrelated message signature (another tag, none, or not a dictionary): not held.
        assert!(!wants_check(&headers(None, Some("sig1=(\"@status\");keyid=\"k\";tag=\"app\""), None)));
        assert!(!wants_check(&headers(None, Some("sig1=(\"@status\");keyid=\"k\""), None)));
        assert!(!wants_check(&headers(None, Some("(((("), None)));
    }

    #[test]
    fn decimals_serialize_canonically() {
        for (written, canonical) in [("1.50", "1.5"), ("001.000", "1.0"), ("-0.0", "0.0"), ("-12.340", "-12.34"), ("0.001", "0.001")] {
            assert_eq!(canonical_decimal(written).as_deref(), Some(canonical), "{written}");
        }
        for bad in ["1.2345", "1234567890123.0", "1.", ".5"] {
            assert_eq!(canonical_decimal(bad), None, "{bad}");
        }
        let mut cursor = Cursor { s: "4.50", i: 0 };
        assert_eq!(serialize_item(&cursor.item().unwrap()), "4.5");
    }

    #[test]
    fn accept_signature_header() {
        let both = format!("ed25519-{INVALID_KEY} malformed ed25519-{RFC_KEY} ed25519-{RFC_KEY}");
        assert_eq!(
            accept_signature_for(&both).unwrap(),
            format!(
                "sig0=(\"unencoded-digest\";sf);keyid=\"{INVALID_KEY}\";tag=\"ed25519-integrity\", sig1=(\"unencoded-digest\";sf);keyid=\"{RFC_KEY}\";tag=\"ed25519-integrity\""
            )
        );
        assert!(accept_signature_for("sha256-abc ed25519-!!!").is_none());
    }

    #[test]
    fn components() {
        let h = headers(None, None, None);
        let none = Params::new();
        let req: Params = vec![("req".into(), Bare::Bool(true))];
        assert_eq!(component_value(&url(), 201, &h, "@status", &none).as_deref(), Some("201"));
        assert_eq!(component_value(&url(), 200, &h, "@authority", &req).as_deref(), Some("a.test"));
        assert_eq!(component_value(&url(), 200, &h, "@scheme", &req).as_deref(), Some("https"));
        assert_eq!(component_value(&url(), 200, &h, "@path", &req).as_deref(), Some("/subresource-integrity/signatures/resource.py"));
        assert_eq!(component_value(&url(), 200, &h, "@query", &req).as_deref(), Some("?x=1"));
        assert_eq!(component_value(&Url::parse("http://a.test:8000/").unwrap(), 200, &h, "@authority", &req).as_deref(), Some("a.test:8000"));
        assert_eq!(component_value(&Url::parse("http://a.test/").unwrap(), 200, &h, "@query", &req).as_deref(), Some("?"));
        // Request components need `;req`, `@status` forbids it.
        assert!(component_value(&url(), 200, &h, "@path", &none).is_none());
        assert!(component_value(&url(), 200, &h, "@status", &req).is_none());
    }
}
