//! Challenge parsing only runs for intercepted 401/407 responses.
use http::{HeaderMap, HeaderName};

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
