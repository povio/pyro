//! Challenge parsing for 401/407 responses, and the session's Basic credentials.
use std::collections::HashMap;

use http::{HeaderMap, HeaderName, HeaderValue};
use url::Url;

/// The HTTP authentication cache of a context's session (Chrome `HttpAuthCache`, Basic only):
/// the `Authorization` value a server's realm accepted, found again for the same origin and
/// realm on a challenge, and for a path under the directory it was accepted at, preemptively.
#[derive(Default)]
pub(crate) struct BasicSessions {
    /// Per origin (`scheme://host:port`): (realm, directory path, `Authorization` value).
    entries: HashMap<String, Vec<(String, String, HeaderValue)>>,
}

/// The directory of a URL path (up to and including its last `/`).
fn directory(path: &str) -> &str {
    path.rfind('/').map_or("/", |i| &path[..=i])
}

impl BasicSessions {
    /// Whether the origin of `url` has credentials stored (a 401 from it may be answered).
    pub(crate) fn knows(&self, url: &Url) -> bool {
        if self.entries.is_empty() {
            return false;
        }
        self.entries.contains_key(&url.origin().ascii_serialization())
    }

    /// The credentials for a request to `url` sent before any challenge: those of the entry whose
    /// directory is the longest prefix of its path.
    pub(crate) fn preemptive(&self, url: &Url) -> Option<HeaderValue> {
        if self.entries.is_empty() {
            return None;
        }
        let path = url.path();
        self.entries
            .get(&url.origin().ascii_serialization())?
            .iter()
            .filter(|(_, dir, _)| path.starts_with(dir.as_str()))
            .max_by_key(|(_, dir, _)| dir.len())
            .map(|(_, _, value)| value.clone())
    }

    /// The credentials of `realm` at the origin of `url`, for answering its challenge.
    pub(crate) fn for_realm(&self, url: &Url, realm: &str) -> Option<HeaderValue> {
        self.entries
            .get(&url.origin().ascii_serialization())?
            .iter()
            .find(|(r, _, _)| r == realm)
            .map(|(_, _, value)| value.clone())
    }

    /// Remembers `value` for `realm` at the origin of `url`, replacing the realm's entry (its
    /// directory is the shorter of the two when the realm is already known).
    pub(crate) fn store(&mut self, url: &Url, realm: &str, value: HeaderValue) {
        let mut dir = directory(url.path()).to_owned();
        let list = self.entries.entry(url.origin().ascii_serialization()).or_default();
        if let Some(i) = list.iter().position(|(r, _, _)| r == realm) {
            let old = list.remove(i).1;
            if old.len() < dir.len() && dir.starts_with(&old) {
                dir = old;
            }
        }
        list.push((realm.to_owned(), dir, value));
    }

    /// Forgets what `value` was stored as at the origin of `url` (a server refused it).
    pub(crate) fn evict(&mut self, url: &Url, value: &HeaderValue) {
        if let Some(list) = self.entries.get_mut(&url.origin().ascii_serialization()) {
            list.retain(|(_, _, v)| v != value);
        }
    }
}

/// The user name and password in `url`'s userinfo, percent-decoded (`None` without either), and
/// `url` without them.
pub(crate) fn take_userinfo(url: &mut Url) -> Option<(String, String)> {
    if url.username().is_empty() && url.password().is_none() {
        return None;
    }
    let decode = |s: &str| percent_encoding::percent_decode_str(s).decode_utf8_lossy().into_owned();
    let credentials = (decode(url.username()), decode(url.password().unwrap_or_default()));
    let _ = url.set_username("");
    let _ = url.set_password(None);
    Some(credentials)
}

/// Prefer Basic when a server advertises several schemes. Preserve an
/// unsupported challenge so the interceptor can explicitly decline it.
pub(crate) fn auth_challenge(headers: &HeaderMap, name: HeaderName) -> Option<(String, String)> {
    let mut first = None;
    for header in headers.get_all(name) {
        let Ok(value) = header.to_str() else { continue };
        let mut current: Option<(String, String)> = None;
        let mut quoted = false;
        let mut escaped = false;
        let mut start = 0;
        for (pos, ch) in value.char_indices().chain(std::iter::once((value.len(), ','))) {
            if escaped {
                escaped = false;
                continue;
            }
            if quoted && ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                quoted = !quoted;
            } else if ch == ',' && !quoted {
                let part = value[start..pos].trim();
                start = pos + 1;
                let end = part.find(|c: char| c.is_ascii_whitespace() || c == '=').unwrap_or(part.len());
                let token = &part[..end];
                let rest = part[end..].trim_start();
                if !token.is_empty() && !rest.starts_with('=') {
                    if let Some(challenge) = current.take() {
                        if challenge.0 == "basic" {
                            return Some(challenge);
                        }
                        first.get_or_insert(challenge);
                    }
                    current = Some((token.to_ascii_lowercase(), String::new()));
                    read_realm(rest, &mut current);
                } else {
                    read_realm(part, &mut current);
                }
            }
        }
        if let Some(challenge) = current {
            if challenge.0 == "basic" {
                return Some(challenge);
            }
            first.get_or_insert(challenge);
        }
    }
    first
}

fn read_realm(parameter: &str, challenge: &mut Option<(String, String)>) {
    let Some((name, value)) = parameter.split_once('=') else { return };
    if !name.trim().eq_ignore_ascii_case("realm") {
        return;
    }
    let Some((_, realm)) = challenge else { return };
    let value = value.trim();
    if let Some(value) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
        let mut escaped = false;
        for ch in value.chars() {
            if !escaped && ch == '\\' {
                escaped = true;
                continue;
            }
            realm.push(ch);
            escaped = false;
        }
    } else {
        realm.push_str(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::{header::WWW_AUTHENTICATE, HeaderValue};

    #[test]
    fn sessions_follow_origin_realm_and_directory() {
        let mut sessions = BasicSessions::default();
        let a = Url::parse("http://h.test/dir/sub/x.py").unwrap();
        sessions.store(&a, "r", HeaderValue::from_static("Basic one"));
        assert_eq!(sessions.preemptive(&Url::parse("http://h.test/dir/sub/y").unwrap()).unwrap(), "Basic one");
        assert!(sessions.preemptive(&Url::parse("http://h.test/dir/y").unwrap()).is_none());
        assert!(sessions.preemptive(&Url::parse("http://other.test/dir/sub/y").unwrap()).is_none());
        assert_eq!(sessions.for_realm(&Url::parse("http://h.test/elsewhere").unwrap(), "r").unwrap(), "Basic one");
        sessions.store(&Url::parse("http://h.test/dir/z").unwrap(), "r", HeaderValue::from_static("Basic two"));
        assert_eq!(sessions.preemptive(&Url::parse("http://h.test/dir/sub/y").unwrap()).unwrap(), "Basic two");
    }

    #[test]
    fn userinfo_is_decoded_and_removed() {
        let mut url = Url::parse("http://us%20er:p%40ss@h.test/p").unwrap();
        assert_eq!(take_userinfo(&mut url), Some(("us er".into(), "p@ss".into())));
        assert_eq!(url.as_str(), "http://h.test/p");
        assert_eq!(take_userinfo(&mut url), None);
    }

    #[test]
    fn multiple_schemes_quoted_commas_and_escapes() {
        let mut h = HeaderMap::new();
        h.append(WWW_AUTHENTICATE, HeaderValue::from_static("Negotiate"));
        h.append(
            WWW_AUTHENTICATE,
            HeaderValue::from_static(r#"Digest realm="a,b", nonce="x", Basic realm="c,\"d", charset="UTF-8""#),
        );
        assert_eq!(auth_challenge(&h, WWW_AUTHENTICATE), Some(("basic".into(), "c,\"d".into())));
        h.insert(WWW_AUTHENTICATE, HeaderValue::from_static("Digest realm = example, nonce=x"));
        assert_eq!(auth_challenge(&h, WWW_AUTHENTICATE), Some(("digest".into(), "example".into())));
        h.clear();
        assert_eq!(auth_challenge(&h, WWW_AUTHENTICATE), None);
    }
}
