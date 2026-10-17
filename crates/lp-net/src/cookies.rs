//! Cookie store (RFC 6265bis subset). Cookies are indexed by domain so a
//! request only examines cookies of its host and parent domains, instead of
//! upstream's full linear scan per request
//! (browser/src/browser/webapi/storage/Cookie.zig:774-803).
//!
//! Domain cookies and schemeful sites use the embedded Public Suffix List
//! (including private suffixes), pinned through the psl dependency lockfile.

use std::time::{SystemTime, UNIX_EPOCH};

use rustc_hash::FxHashMap;
use url::{Host, Url};

pub const MAX_COOKIE_BYTES: usize = 4096;
pub const MAX_PER_DOMAIN: usize = 180;
pub const MAX_TOTAL: usize = 3300;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SameSite {
    Strict,
    Lax,
    None,
    /// Attribute absent: treated as Lax for cross-site requests (Chrome default).
    Unspecified,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    /// Lowercase host or domain without leading dot.
    pub domain: String,
    pub host_only: bool,
    pub path: String,
    /// Expiry in seconds since the Unix epoch; None = session cookie.
    pub expires: Option<f64>,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: SameSite,
    creation: u64,
}

impl Cookie {
    pub fn size(&self) -> usize {
        self.name.len() + self.value.len()
    }
}

/// How the cookie is being accessed.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Access {
    /// HTTP request/response (HttpOnly visible).
    Http,
    /// `document.cookie` / CookieStore from script.
    Script,
}

/// Request context for SameSite enforcement.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SiteContext {
    SameSite,
    /// Cross-site top-level navigation with a safe method: Lax cookies sent.
    CrossSiteTopLevelSafe,
    CrossSite,
}

#[derive(Default)]
pub struct CookieJar {
    by_domain: FxHashMap<String, Vec<Cookie>>,
    count: usize,
    seq: u64,
}

pub fn now_secs() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

fn host_str(url: &Url) -> Option<String> {
    match url.host()? {
        Host::Domain(d) => Some(d.trim_end_matches('.').to_ascii_lowercase()),
        Host::Ipv4(ip) => Some(ip.to_string()),
        Host::Ipv6(ip) => Some(format!("[{ip}]")),
    }
}

fn is_ip(host: &str) -> bool {
    host.starts_with('[') || host.parse::<std::net::Ipv4Addr>().is_ok()
}

/// Secure-context check used for Secure cookies: https, or loopback hosts
/// (Chromium treats http://localhost as potentially trustworthy).
pub fn is_trustworthy(url: &Url) -> bool {
    match url.scheme() {
        "https" | "wss" => true,
        "http" | "ws" => match url.host() {
            Some(Host::Domain(d)) => d == "localhost" || d.ends_with(".localhost"),
            Some(Host::Ipv4(ip)) => ip.is_loopback(),
            Some(Host::Ipv6(ip)) => ip.is_loopback(),
            None => false,
        },
        _ => false,
    }
}

/// Registrable domain using the embedded PSL; IPs and single-label hosts
/// remain their own site.
pub fn site_of(host: &str) -> &str {
    if is_ip(host) {
        host
    } else {
        psl::domain_str(host).unwrap_or(host)
    }
}

fn default_path(url: &Url) -> String {
    let p = url.path();
    if !p.starts_with('/') {
        return "/".into();
    }
    match p.rfind('/') {
        Some(0) | None => "/".into(),
        Some(i) => p[..i].to_owned(),
    }
}

fn domain_matches(host: &str, domain: &str) -> bool {
    host == domain || (!is_ip(host) && host.strip_suffix(domain).is_some_and(|prefix| prefix.ends_with('.')))
}

fn domain_match(host: &str, c: &Cookie) -> bool {
    if c.host_only {
        host == c.domain
    } else {
        domain_matches(host, &c.domain)
    }
}

fn path_match(req: &str, cookie: &str) -> bool {
    req == cookie
        || (req.starts_with(cookie) && (cookie.ends_with('/') || req.as_bytes().get(cookie.len()) == Some(&b'/')))
}

impl CookieJar {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn len(&self) -> usize {
        self.count
    }
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Processes one `Set-Cookie` header value (or a `document.cookie`
    /// assignment when `access == Script`). Returns whether it was stored.
    pub fn set_from_header(&mut self, url: &Url, header: &str, access: Access, now: f64) -> bool {
        let Ok(parsed) = cookie::Cookie::parse(header.to_owned()) else { return false };
        let Some(host) = host_str(url) else { return false };
        if parsed.name().is_empty() && parsed.value().is_empty() {
            return false;
        }
        if parsed.name().len() + parsed.value().len() > MAX_COOKIE_BYTES {
            return false;
        }
        let (domain, host_only) = match parsed.domain() {
            Some(d) if !d.is_empty() => {
                let d = d.trim_start_matches('.').trim_end_matches('.').to_ascii_lowercase();
                let public_suffix = !is_ip(&d) && psl::suffix_str(&d) == Some(d.as_str());
                if public_suffix && d != host {
                    return false;
                }
                let probe = Cookie {
                    name: String::new(),
                    value: String::new(),
                    domain: d.clone(),
                    host_only: false,
                    path: String::new(),
                    expires: None,
                    secure: false,
                    http_only: false,
                    same_site: SameSite::Unspecified,
                    creation: 0,
                };
                if !domain_match(&host, &probe) {
                    return false;
                }
                (d, public_suffix)
            }
            _ => (host.clone(), true),
        };
        let path = match parsed.path() {
            Some(p) if p.starts_with('/') => p.to_owned(),
            _ => default_path(url),
        };
        let secure = parsed.secure().unwrap_or(false);
        if secure && !is_trustworthy(url) {
            return false;
        }
        let http_only = parsed.http_only().unwrap_or(false);
        if http_only && access == Access::Script {
            return false;
        }
        let same_site = match parsed.same_site() {
            Some(cookie::SameSite::Strict) => SameSite::Strict,
            Some(cookie::SameSite::Lax) => SameSite::Lax,
            Some(cookie::SameSite::None) => SameSite::None,
            None => SameSite::Unspecified,
        };
        if same_site == SameSite::None && !secure {
            return false;
        }
        let name = parsed.name();
        if name.starts_with("__Secure-") && !secure {
            return false;
        }
        if name.starts_with("__Host-") && (!secure || !host_only || path != "/") {
            return false;
        }
        let expires = if let Some(max_age) = parsed.max_age() {
            Some(now + max_age.whole_seconds() as f64)
        } else {
            parsed.expires_datetime().map(|t| t.unix_timestamp() as f64)
        };
        // An insecure response must not overlay an existing Secure cookie,
        // including a narrower path or a domain/host-only variant.
        if !secure
            && !is_trustworthy(url)
            && self.by_domain.values().flatten().any(|old| {
                old.secure
                    && old.name == name
                    && old.expires.is_none_or(|e| e > now)
                    && (domain_matches(&domain, &old.domain) || domain_matches(&old.domain, &domain))
                    && path_match(&path, &old.path)
            })
        {
            return false;
        }
        self.seq += 1;
        let c = Cookie {
            name: parsed.name().to_owned(),
            value: parsed.value().to_owned(),
            domain,
            host_only,
            path,
            expires,
            secure,
            http_only,
            same_site,
            creation: self.seq,
        };
        self.store(c, access, now)
    }

    fn store(&mut self, mut c: Cookie, access: Access, now: f64) -> bool {
        let list = self.by_domain.entry(c.domain.clone()).or_default();
        if let Some(pos) = list.iter().position(|o| o.name == c.name && o.path == c.path && o.host_only == c.host_only)
        {
            if list[pos].http_only && access == Access::Script {
                return false;
            }
            c.creation = list[pos].creation;
            list.remove(pos);
            self.count -= 1;
        }
        if c.expires.is_some_and(|e| e <= now) {
            // Expired: acts as deletion.
            if list.is_empty() {
                self.by_domain.remove(&c.domain);
            }
            return true;
        }
        list.push(c);
        if list.len() > MAX_PER_DOMAIN {
            // Evict the least recently created cookie of this domain.
            if let Some((i, _)) = list.iter().enumerate().min_by_key(|(_, c)| c.creation) {
                list.remove(i);
                self.count -= 1;
            }
        }
        self.count += 1;
        if self.count > MAX_TOTAL {
            self.evict_oldest();
        }
        true
    }

    fn evict_oldest(&mut self) {
        let victim = self
            .by_domain
            .iter()
            .flat_map(|(d, l)| l.iter().map(move |c| (c.creation, d.clone())))
            .min_by_key(|(c, _)| *c);
        if let Some((creation, d)) = victim {
            if let Some(l) = self.by_domain.get_mut(&d) {
                l.retain(|c| c.creation != creation);
                if l.is_empty() {
                    self.by_domain.remove(&d);
                }
            }
            self.count -= 1;
        }
    }

    /// Cookies applicable to `url`, sorted per RFC 6265 (longer paths first,
    /// then earlier creation). Expired cookies encountered are purged.
    pub fn matching(&mut self, url: &Url, access: Access, site: SiteContext, now: f64) -> Vec<&Cookie> {
        let Some(host) = host_str(url) else { return Vec::new() };
        let secure_ok = is_trustworthy(url);
        let path = url.path();
        // Purge expired cookies in the candidate domains first.
        let mut domains: Vec<String> = vec![host.clone()];
        if !is_ip(&host) {
            let mut rest = host.as_str();
            while let Some(i) = rest.find('.') {
                rest = &rest[i + 1..];
                domains.push(rest.to_owned());
            }
        }
        for d in &domains {
            if let Some(l) = self.by_domain.get_mut(d) {
                let before = l.len();
                l.retain(|c| c.expires.is_none_or(|e| e > now));
                self.count -= before - l.len();
                if l.is_empty() {
                    self.by_domain.remove(d);
                }
            }
        }
        let mut out: Vec<&Cookie> = domains
            .iter()
            .filter_map(|d| self.by_domain.get(d))
            .flat_map(|l| l.iter())
            .filter(|c| domain_match(&host, c))
            .filter(|c| path_match(path, &c.path))
            .filter(|c| !c.secure || secure_ok)
            .filter(|c| !(c.http_only && access == Access::Script))
            .filter(|c| {
                matches!(
                    (site, c.same_site),
                    (SiteContext::SameSite, _)
                        | (_, SameSite::None)
                        | (SiteContext::CrossSiteTopLevelSafe, SameSite::Lax | SameSite::Unspecified)
                )
            })
            .collect();
        out.sort_by(|a, b| b.path.len().cmp(&a.path.len()).then(a.creation.cmp(&b.creation)));
        out
    }

    /// `Cookie` request header value, or None if no cookie applies.
    pub fn header_for(&mut self, url: &Url, site: SiteContext, now: f64) -> Option<String> {
        let list = self.matching(url, Access::Http, site, now);
        if list.is_empty() {
            return None;
        }
        let mut s = String::with_capacity(list.iter().map(|c| c.size() + 2).sum());
        for (i, c) in list.iter().enumerate() {
            if i > 0 {
                s.push_str("; ");
            }
            if !c.name.is_empty() {
                s.push_str(&c.name);
                s.push('=');
            }
            s.push_str(&c.value);
        }
        Some(s)
    }

    /// `document.cookie` getter.
    pub fn document_cookie(&mut self, url: &Url, now: f64) -> String {
        if !matches!(url.scheme(), "http" | "https") {
            return String::new();
        }
        let list = self.matching(url, Access::Script, SiteContext::SameSite, now);
        list.iter()
            .map(|c| if c.name.is_empty() { c.value.clone() } else { format!("{}={}", c.name, c.value) })
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// All cookies (CDP Storage.getCookies / Network.getAllCookies).
    pub fn all(&self) -> Vec<Cookie> {
        let mut v: Vec<Cookie> = self.by_domain.values().flatten().cloned().collect();
        v.sort_by_key(|c| c.creation);
        v
    }

    /// CDP Network.setCookie / Storage.setCookies. `domain` with a leading
    /// dot (or explicit `host_only=false`) creates a domain cookie.
    #[allow(clippy::too_many_arguments)]
    pub fn set_cookie(
        &mut self,
        name: &str,
        value: &str,
        domain: &str,
        path: &str,
        expires: Option<f64>,
        secure: bool,
        http_only: bool,
        same_site: SameSite,
    ) -> bool {
        self.seq += 1;
        let host_only = !domain.starts_with('.');
        let c = Cookie {
            name: name.to_owned(),
            value: value.to_owned(),
            domain: domain.trim_start_matches('.').to_ascii_lowercase(),
            host_only,
            path: if path.is_empty() { "/".into() } else { path.to_owned() },
            expires,
            secure,
            http_only,
            same_site,
            creation: self.seq,
        };
        if c.domain.is_empty()
            || (!c.host_only && !is_ip(&c.domain) && psl::suffix_str(&c.domain) == Some(c.domain.as_str()))
        {
            return false;
        }
        self.store(c, Access::Http, now_secs())
    }

    /// Deletes cookies by name, optionally restricted by domain and path.
    pub fn delete(&mut self, name: &str, domain: Option<&str>, path: Option<&str>) -> usize {
        let mut removed = 0;
        self.by_domain.retain(|d, l| {
            let before = l.len();
            l.retain(|c| {
                !(c.name == name
                    && domain.is_none_or(|x| x.trim_start_matches('.').eq_ignore_ascii_case(d))
                    && path.is_none_or(|p| c.path == p))
            });
            removed += before - l.len();
            !l.is_empty()
        });
        self.count -= removed;
        removed
    }

    pub fn clear(&mut self) {
        self.by_domain.clear();
        self.count = 0;
    }
}

/// Determines the SameSite context of a request initiated by a document at
/// `initiator` (None = browser-initiated / CDP navigation, treated same-site).
pub fn site_context(initiator: Option<&Url>, target: &Url, top_level_nav: bool, safe_method: bool) -> SiteContext {
    let (Some(init), Some(th)) = (initiator.and_then(host_str), host_str(target)) else {
        return SiteContext::SameSite;
    };
    // Schemeful same-site: registrable domain and secure-ness must match.
    let secure = |u: &Url| matches!(u.scheme(), "https" | "wss");
    let same = site_of(&init) == site_of(&th) && initiator.map(secure) == Some(secure(target));
    if same {
        SiteContext::SameSite
    } else if top_level_nav && safe_method {
        SiteContext::CrossSiteTopLevelSafe
    } else {
        SiteContext::CrossSite
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expired_domain_buckets_are_reclaimed() {
        let mut jar = CookieJar::new();
        for i in 0..1000 {
            let url = Url::parse(&format!("https://h{i}.example.com/")).unwrap();
            assert!(jar.set_from_header(&url, "x=1; Max-Age=1", Access::Http, 1.0));
            jar.header_for(&url, SiteContext::SameSite, 3.0);
        }
        assert!(jar.by_domain.is_empty());
        assert_eq!(jar.count, 0);
    }

    #[test]
    fn psl_sites_and_private_suffixes_are_distinct() {
        assert_eq!(site_of("a.example.co.uk"), "example.co.uk");
        assert_eq!(site_of("a.github.io"), "a.github.io");
        let a = Url::parse("https://a.github.io/").unwrap();
        let b = Url::parse("https://b.github.io/").unwrap();
        assert_eq!(site_context(Some(&a), &b, false, true), SiteContext::CrossSite);
        let mut jar = CookieJar::new();
        assert!(!jar.set_from_header(&a, "x=1; Domain=github.io", Access::Http, 1.0));
        assert!(!jar.set_cookie("x", "1", ".co.uk", "/", None, false, false, SameSite::Lax));
        let suffix_host = Url::parse("https://co.uk/").unwrap();
        assert!(jar.set_from_header(&suffix_host, "x=1; Domain=co.uk", Access::Http, 1.0));
        assert!(jar.all()[0].host_only);
    }
}
