//! Minimal HTTP/1.1 request-head parsing and the DevTools discovery endpoints.
//!
//! Only what CDP clients need: `GET /json/version`, `GET /json[/list]` and the
//! WebSocket upgrade. Requests are rejected unless their `Host` names a
//! loopback/IP address, and WebSocket upgrades carrying an `Origin` are
//! rejected, mirroring Chrome's DNS-rebinding and cross-origin protections.

use crate::handshake_crypto::websocket_accept;

/// Upper bound for a request head; anything larger is rejected.
pub const MAX_HEAD: usize = 16 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub struct RequestHead {
    pub method: String,
    pub path: String,
    /// `HTTP/1.0` or `HTTP/1.1`.
    pub version: String,
    headers: Vec<(String, String)>,
}

impl RequestHead {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    fn header_count(&self, name: &str) -> usize {
        self.headers.iter().filter(|(k, _)| k.eq_ignore_ascii_case(name)).count()
    }

    fn header_has_token(&self, name: &str, token: &str) -> bool {
        self.headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case(name))
            .flat_map(|(_, v)| v.split(','))
            .any(|t| t.trim().eq_ignore_ascii_case(token))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum HeadError {
    TooLarge,
    Malformed,
}

/// Parses a request head from `buf`. Returns the head and its byte length
/// (including the terminating blank line) once complete.
pub fn parse_head(buf: &[u8]) -> Result<Option<(RequestHead, usize)>, HeadError> {
    let Some(end) = find_head_end(buf) else {
        return if buf.len() > MAX_HEAD { Err(HeadError::TooLarge) } else { Ok(None) };
    };
    if end > MAX_HEAD {
        return Err(HeadError::TooLarge);
    }
    let text = std::str::from_utf8(&buf[..end - 4]).map_err(|_| HeadError::Malformed)?;
    let mut lines = text.split("\r\n");
    let request_line = lines.next().ok_or(HeadError::Malformed)?;
    let mut parts = request_line.split(' ');
    let (Some(method), Some(path), Some(version), None) = (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(HeadError::Malformed);
    };
    if !matches!(version, "HTTP/1.0" | "HTTP/1.1") || !method.bytes().all(is_tchar) || method.is_empty() {
        return Err(HeadError::Malformed);
    }
    if !path.starts_with('/') || path.bytes().any(|b| b.is_ascii_control()) {
        return Err(HeadError::Malformed);
    }
    let mut headers = Vec::new();
    for line in lines {
        let (name, value) = line.split_once(':').ok_or(HeadError::Malformed)?;
        if name.is_empty() || !name.bytes().all(is_tchar) || value.bytes().any(|b| b.is_ascii_control() && b != b'\t') {
            return Err(HeadError::Malformed);
        }
        headers.push((name.to_owned(), value.trim().to_owned()));
    }
    let head = RequestHead { method: method.to_owned(), path: path.to_owned(), version: version.to_owned(), headers };
    Ok(Some((head, end)))
}

/// RFC 9110 `tchar`.
fn is_tchar(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)
}

fn find_head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
}

/// Facts the discovery endpoints report.
pub struct ServerInfo<'a> {
    /// `host:port` the server is reachable at, as configured (not taken from the request).
    pub authority: &'a str,
    pub product: &'a str,
    pub user_agent: &'a str,
    pub protocol_version: &'a str,
}

pub enum Routed {
    /// Complete HTTP response bytes; the connection is closed afterwards.
    Response(Vec<u8>),
    /// Switch to WebSocket after writing these handshake bytes.
    Upgrade(Vec<u8>),
}

pub fn route(head: &RequestHead, info: &ServerInfo<'_>) -> Routed {
    if !host_is_allowed(head.header("host")) {
        return Routed::Response(response(403, "text/plain", b"Host header is not an IP address or localhost\n"));
    }
    if head.header_has_token("upgrade", "websocket") {
        return upgrade(head);
    }
    if head.method != "GET" && head.method != "PUT" {
        return Routed::Response(response(405, "text/plain", b"method not allowed\n"));
    }
    let path = head.path.split('?').next().unwrap_or("");
    let body = match path.trim_end_matches('/') {
        "/json/version" => serde_json::json!({
            "Browser": info.product,
            "Protocol-Version": info.protocol_version,
            "User-Agent": info.user_agent,
            "V8-Version": "",
            "WebKit-Version": "",
            "webSocketDebuggerUrl": format!("ws://{}/", info.authority),
        }),
        // Page targets live in the attached connection's browser; a fresh
        // process has none to list.
        "/json" | "/json/list" => serde_json::json!([]),
        _ => return Routed::Response(response(404, "text/plain", b"not found\n")),
    };
    let bytes = serde_json::to_vec_pretty(&body).expect("static JSON serializes");
    Routed::Response(response(200, "application/json; charset=UTF-8", &bytes))
}

fn upgrade(head: &RequestHead) -> Routed {
    if head.method != "GET" {
        return Routed::Response(response(405, "text/plain", b"method not allowed\n"));
    }
    if head.header("origin").is_some() {
        return Routed::Response(response(403, "text/plain", b"WebSocket connections with an Origin are rejected\n"));
    }
    // RFC 6455 4.2.1: HTTP/1.1 GET with exactly one Host, Sec-WebSocket-Key
    // (16 random bytes, base64) and Sec-WebSocket-Version 13.
    let key = head.header("sec-websocket-key").unwrap_or("");
    let valid = head.version == "HTTP/1.1"
        && head.header_count("host") == 1
        && head.header_count("sec-websocket-key") == 1
        && head.header_count("sec-websocket-version") == 1
        && head.header("sec-websocket-version") == Some("13")
        && key.len() == 24
        && crate::fetch_protocol::base64_decode(key).is_ok_and(|k| k.len() == 16)
        && head.header_has_token("connection", "upgrade");
    if !valid {
        return Routed::Response(response(400, "text/plain", b"invalid WebSocket upgrade\n"));
    }
    let handshake = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
        websocket_accept(key)
    );
    Routed::Upgrade(handshake.into_bytes())
}

/// Accepts IP literals and `localhost` (with optional port); a missing Host is
/// accepted as HTTP/1.0 clients may omit it.
fn host_is_allowed(host: Option<&str>) -> bool {
    let Some(host) = host else { return true };
    let name = if let Some(rest) = host.strip_prefix('[') {
        match rest.split_once(']') {
            Some((ip, port)) if port.is_empty() || port.starts_with(':') => {
                return ip.parse::<std::net::Ipv6Addr>().is_ok()
            }
            _ => return false,
        }
    } else {
        host.rsplit_once(':').map_or(host, |(name, _)| name)
    };
    name.eq_ignore_ascii_case("localhost") || name.parse::<std::net::Ipv4Addr>().is_ok()
}

pub fn response(status: u16, content_type: &str, body: &[u8]) -> Vec<u8> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        _ => "Error",
    };
    let mut out = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    out.extend_from_slice(body);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const INFO: ServerInfo<'static> = ServerInfo {
        authority: "127.0.0.1:9222",
        product: "Pyro/0.1.0",
        user_agent: "UA",
        protocol_version: "1.3",
    };

    fn head(raw: &str) -> RequestHead {
        parse_head(raw.as_bytes()).unwrap().unwrap().0
    }

    fn body_of(routed: Routed) -> (u16, serde_json::Value) {
        let Routed::Response(bytes) = routed else { panic!("expected a response") };
        let text = String::from_utf8(bytes).unwrap();
        let status = text[9..12].parse().unwrap();
        let body = text.split_once("\r\n\r\n").unwrap().1;
        (status, serde_json::from_str(body).unwrap_or(serde_json::Value::Null))
    }

    #[test]
    fn parses_incrementally_and_reports_length() {
        let raw = b"GET /json/version HTTP/1.1\r\nHost: 127.0.0.1:9222\r\n\r\nEXTRA";
        assert_eq!(parse_head(&raw[..20]), Ok(None));
        let (h, len) = parse_head(raw).unwrap().unwrap();
        assert_eq!(len, raw.len() - 5);
        assert_eq!(h.path, "/json/version");
        assert_eq!(h.header("HOST"), Some("127.0.0.1:9222"));
    }

    #[test]
    fn rejects_malformed_and_oversized_heads() {
        assert_eq!(parse_head(b"GET\r\n\r\n"), Err(HeadError::Malformed));
        assert_eq!(parse_head(b"GET / HTTP/1.1\r\nbad header\r\n\r\n"), Err(HeadError::Malformed));
        assert_eq!(parse_head(&vec![b'a'; MAX_HEAD + 1]), Err(HeadError::TooLarge));
    }

    #[test]
    fn version_endpoint_uses_configured_authority() {
        let (status, json) = body_of(route(&head("GET /json/version HTTP/1.1\r\nHost: localhost:9222\r\n\r\n"), &INFO));
        assert_eq!(status, 200);
        assert_eq!(json["webSocketDebuggerUrl"], "ws://127.0.0.1:9222/");
        assert_eq!(json["Browser"], "Pyro/0.1.0");
    }

    #[test]
    fn rejects_rebinding_hosts_and_unknown_paths() {
        let (status, _) = body_of(route(&head("GET /json/version HTTP/1.1\r\nHost: evil.example:9222\r\n\r\n"), &INFO));
        assert_eq!(status, 403);
        let (status, _) = body_of(route(&head("GET /nope HTTP/1.1\r\nHost: [::1]:9222\r\n\r\n"), &INFO));
        assert_eq!(status, 404);
        assert!(host_is_allowed(Some("127.0.0.1")));
        assert!(!host_is_allowed(Some("[::1]x")));
        assert!(!host_is_allowed(Some("localhost.evil.example")));
    }

    #[test]
    fn websocket_upgrade_handshake() {
        let raw = "GET / HTTP/1.1\r\nHost: 127.0.0.1:9222\r\nUpgrade: websocket\r\nConnection: keep-alive, Upgrade\r\n\
                   Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n";
        let Routed::Upgrade(bytes) = route(&head(raw), &INFO) else { panic!("expected upgrade") };
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.starts_with("HTTP/1.1 101 "));
        assert!(text.contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n"));
    }

    #[test]
    fn websocket_upgrade_requires_rfc6455_handshake() {
        let ok_key = "dGhlIHNhbXBsZSBub25jZQ==";
        let build = |version: &str, extra: &str, key: &str| {
            format!(
                "GET / {version}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\n\
                 Sec-WebSocket-Key: {key}\r\n{extra}\r\n"
            )
        };
        let status = |raw: String| body_of(route(&head(&raw), &INFO)).0;
        assert_eq!(status(build("HTTP/1.0", "", ok_key)), 400, "HTTP/1.0 upgrade");
        assert_eq!(status(build("HTTP/1.1", "", ok_key)), 400, "missing Host");
        assert_eq!(status(build("HTTP/1.1", "Host: 127.0.0.1\r\nHost: 127.0.0.2\r\n", ok_key)), 400, "duplicate Host");
        assert_eq!(status(build("HTTP/1.1", "Host: 127.0.0.1\r\n", "x")), 400, "short key");
        assert_eq!(
            status(build("HTTP/1.1", "Host: 127.0.0.1\r\n", "AAAAAAAAAAAAAAAAAAAAAAAAAA==")),
            400,
            "key not 16 bytes"
        );
        assert!(matches!(route(&head(&build("HTTP/1.1", "Host: 127.0.0.1\r\n", ok_key)), &INFO), Routed::Upgrade(_)));
        assert_eq!(parse_head(b"GET / HTTP/2.0\r\n\r\n"), Err(HeadError::Malformed));
        assert_eq!(parse_head(b"GET / HTTP/1.1\r\nBad Name: x\r\n\r\n"), Err(HeadError::Malformed));
        assert_eq!(parse_head(b"GET / HTTP/1.1\r\nX: a\x01b\r\n\r\n"), Err(HeadError::Malformed));
    }

    #[test]
    fn websocket_upgrade_rejects_origin_and_bad_version() {
        let base = "GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n";
        let (status, _) =
            body_of(route(&head(&format!("{base}Sec-WebSocket-Version: 13\r\nOrigin: http://a\r\n\r\n")), &INFO));
        assert_eq!(status, 403);
        let (status, _) = body_of(route(&head(&format!("{base}Sec-WebSocket-Version: 8\r\n\r\n")), &INFO));
        assert_eq!(status, 400);
    }
}
