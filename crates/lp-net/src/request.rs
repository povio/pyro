//! Request/response data model shared by the loader, fetch/XHR bindings and
//! the CDP Network/Fetch domains.

use bytes::Bytes;
use http::{HeaderMap, HeaderValue, Method, StatusCode};
use url::Url;

/// Request id, unique per `NetClient` (each context client counts from 1). Also used as CDP `Network.requestId` and as
/// `Fetch.requestPaused.networkId`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RequestId(pub u64);

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Fetch "destination" / CDP resource type.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Destination {
    Document,
    Iframe,
    Script,
    Style,
    Image,
    /// An `<audio>` element's resource.
    Audio,
    /// A `<video>` element's resource.
    Video,
    Font,
    Xhr,
    Fetch,
    EventSource,
    WebSocket,
    /// `navigator.sendBeacon`.
    Beacon,
    Other,
}

impl Destination {
    /// CDP `Network.ResourceType` name.
    pub fn cdp_type(self) -> &'static str {
        match self {
            Destination::Document | Destination::Iframe => "Document",
            Destination::Script => "Script",
            Destination::Style => "Stylesheet",
            Destination::Image => "Image",
            Destination::Audio | Destination::Video => "Media",
            Destination::Font => "Font",
            Destination::Xhr => "XHR",
            Destination::Fetch => "Fetch",
            Destination::EventSource => "EventSource",
            Destination::WebSocket => "WebSocket",
            Destination::Beacon => "Ping",
            Destination::Other => "Other",
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RequestMode {
    Navigate,
    SameOrigin,
    NoCors,
    Cors,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Credentials {
    Omit,
    SameOrigin,
    Include,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RedirectMode {
    Follow,
    Error,
    Manual,
}

/// Browser/page defaults, distinct from author headers for CORS preflight.
/// Cloned into a request so later page changes do not alter an in-flight hop.
#[derive(Clone, Debug, Default)]
pub struct RequestDefaults {
    pub user_agent: Option<HeaderValue>,
    pub accept_language: Option<HeaderValue>,
    /// Page-scoped extra headers (CDP `Network.setExtraHTTPHeaders` on a page
    /// session). Applied after the client's context-wide `NetSettings`
    /// extra headers, so a page value replaces a context value of the same name.
    pub extra_headers: HeaderMap,
    /// Page-scoped HTTP cache bypass (CDP `Network.setCacheDisabled` on a page
    /// session): no cache lookup or store for this request, plus `Pragma:
    /// no-cache` / `Cache-Control: no-cache` like DevTools. Sibling pages in the
    /// same context keep using the context cache.
    pub cache_disabled: bool,
    /// Low-entropy User-Agent client hints (`Sec-CH-UA`, `-Mobile`,
    /// `-Platform`) of a page whose agent override carries metadata. Sent
    /// only to potentially trustworthy origins, never over request headers.
    pub client_hints: Option<std::sync::Arc<Vec<(http::HeaderName, HeaderValue)>>>,
}

#[derive(Clone, Debug)]
pub struct Request {
    pub method: Method,
    pub url: Url,
    pub headers: HeaderMap,
    pub defaults: RequestDefaults,
    pub body: Option<Bytes>,
    pub destination: Destination,
    pub mode: RequestMode,
    pub credentials: Credentials,
    pub redirect: RedirectMode,
    /// Serialized origin of the requesting document ("null" for opaque);
    /// None for browser-initiated navigations.
    pub origin: Option<String>,
    /// The referrer source (document or init URL) before its policy, which
    /// the client applies per hop; a navigation with none is browser-initiated.
    pub referrer: Option<Url>,
    /// Updated by a redirect's `Referrer-Policy` header.
    pub referrer_policy: crate::referrer::ReferrerPolicy,
    /// Frame / loader identifiers for CDP correlation.
    pub frame_id: Option<String>,
    pub loader_id: Option<String>,
    /// Upper bound on the decoded body; exceeded => `NetError::TooLarge`.
    pub max_body: Option<usize>,
    /// Fetch "cache mode" for the context's HTTP cache.
    pub cache: CacheMode,
    /// Never offered to the [`crate::Interceptor`]: the caller's thread, which
    /// answers interception decisions, is blocked on this request (sync XHR).
    pub skip_interception: bool,
    /// The object behind a `blob:` URL, looked up by the caller in its
    /// document's blob URL store (the client has no access to it). A `blob:`
    /// request without one fails as for a revoked URL.
    pub blob: Option<BlobBody>,
}

/// A `Blob` served for a `blob:` URL: its bytes and its `type`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlobBody {
    pub bytes: Bytes,
    pub mime: String,
}

/// Fetch request cache mode (WHATWG Fetch, "cache mode"), applied to the
/// context's private HTTP cache.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CacheMode {
    /// Use fresh entries, revalidate stale ones that have validators, store.
    #[default]
    Default,
    /// Bypass the cache entirely: no lookup, no store.
    NoStore,
    /// No lookup, but store the response.
    Reload,
    /// Revalidate any matching entry (conditional request), store.
    NoCache,
    /// Use any matching entry, even stale, without revalidation; else network.
    ForceCache,
    /// Use any matching entry, even stale; else a network error.
    OnlyIfCached,
}

impl Request {
    pub fn get(url: Url, destination: Destination) -> Self {
        let mode = match destination {
            Destination::Document | Destination::Iframe => RequestMode::Navigate,
            Destination::Xhr | Destination::Fetch | Destination::EventSource => RequestMode::Cors,
            _ => RequestMode::NoCors,
        };
        Request {
            method: Method::GET,
            url,
            headers: HeaderMap::new(),
            defaults: RequestDefaults::default(),
            body: None,
            destination,
            mode,
            credentials: Credentials::SameOrigin,
            redirect: RedirectMode::Follow,
            origin: None,
            referrer: None,
            referrer_policy: Default::default(),
            frame_id: None,
            loader_id: None,
            max_body: None,
            cache: CacheMode::Default,
            skip_interception: false,
            blob: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ResponseHead {
    /// Final URL after redirects.
    pub url: Url,
    pub status: StatusCode,
    pub status_text: String,
    pub headers: HeaderMap,
    /// Lowercased essence of Content-Type, e.g. "text/html".
    pub mime: String,
    pub charset: Option<String>,
    pub redirected: bool,
    pub from_cache: bool,
    /// Response tainting for fetch: basic/cors/opaque/opaqueredirect.
    pub kind: ResponseKind,
    /// Remote address "ip:port" when known (CDP remoteIPAddress).
    pub remote_addr: Option<String>,
    /// HTTP version of the response (CDP `protocol`).
    pub protocol: http::Version,
    /// When the fetch's phases happened (Resource Timing).
    pub timing: crate::timing::NetTiming,
}

impl ResponseHead {
    /// This head as sent over `protocol`. HTTP/2 has no reason phrase.
    pub(crate) fn over(mut self, protocol: http::Version) -> Self {
        self.protocol = protocol;
        if protocol == http::Version::HTTP_2 {
            self.status_text.clear();
        }
        self
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ResponseKind {
    Basic,
    Cors,
    Opaque,
    OpaqueRedirect,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetError {
    /// CDP errorText "net::ERR_ABORTED".
    Aborted,
    Timeout,
    ConnectionRefused,
    NameNotResolved,
    InvalidUrl,
    UnsupportedScheme(String),
    TooManyRedirects,
    RedirectNotAllowed,
    /// CORS check failed; message is diagnostic only.
    Cors(String),
    TooLarge,
    Decode(String),
    /// Request blocked or failed by CDP Fetch.failRequest.
    Blocked(String),
    /// The server certificate did not verify; message is diagnostic only.
    Certificate(String),
    /// The proxy failed: its CDP errorText (net::ERR_PROXY_CONNECTION_FAILED,
    /// net::ERR_TUNNEL_CONNECTION_FAILED).
    Proxy(&'static str),
    Other(String),
}

impl NetError {
    /// Chromium-style net error text used in CDP `loadingFailed.errorText`.
    pub fn error_text(&self) -> &str {
        match self {
            NetError::Aborted => "net::ERR_ABORTED",
            NetError::Timeout => "net::ERR_TIMED_OUT",
            NetError::ConnectionRefused => "net::ERR_CONNECTION_REFUSED",
            NetError::NameNotResolved => "net::ERR_NAME_NOT_RESOLVED",
            NetError::InvalidUrl => "net::ERR_INVALID_URL",
            NetError::UnsupportedScheme(_) => "net::ERR_UNKNOWN_URL_SCHEME",
            NetError::TooManyRedirects => "net::ERR_TOO_MANY_REDIRECTS",
            NetError::RedirectNotAllowed => "net::ERR_FAILED",
            NetError::Cors(_) => "net::ERR_FAILED",
            NetError::TooLarge => "net::ERR_INSUFFICIENT_RESOURCES",
            NetError::Decode(_) => "net::ERR_CONTENT_DECODING_FAILED",
            NetError::Blocked(r) => r,
            NetError::Certificate(_) => "net::ERR_CERT_AUTHORITY_INVALID",
            NetError::Proxy(text) => text,
            NetError::Other(_) => "net::ERR_FAILED",
        }
    }
}

impl std::fmt::Display for NetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NetError::Cors(m) | NetError::Decode(m) | NetError::Certificate(m) | NetError::Other(m) => {
                write!(f, "{}: {m}", self.error_text())
            }
            NetError::UnsupportedScheme(s) => write!(f, "{}: {s}", self.error_text()),
            _ => f.write_str(self.error_text()),
        }
    }
}

impl std::error::Error for NetError {}

/// Cumulative transport progress exposed by a response consumer on its owner
/// thread. Bytes count decoded transport data, even when the consumer discards
/// it. `terminal` means an observed transport Finished/Failed, never a local
/// abort, disposal or consumer quota error. Request IDs are context-local.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NetworkDelivery {
    pub response: bool,
    pub data_bytes: u64,
    pub terminal: bool,
}

/// Events delivered to the request's consumer, in order:
/// `Redirect*`, `Response`, `Data*`, then exactly one of `Finished`/`Failed`
/// (`Failed` may also occur before `Response`).
#[derive(Clone, Debug)]
pub enum ResponseEvent {
    Redirect {
        from: Url,
        to: Url,
        status: StatusCode,
        headers: HeaderMap,
    },
    Response(ResponseHead),
    /// Decoded body chunk. `Bytes` is refcounted: forwarding it to consumers
    /// and the CDP body store does not copy.
    Data(Bytes),
    Finished {
        encoded_len: u64,
        decoded_len: u64,
    },
    Failed(NetError),
}

/// Parses `Content-Type` into (essence, charset).
pub fn parse_content_type(v: &str) -> (String, Option<String>) {
    let mut parts = v.split(';');
    let essence = parts.next().unwrap_or("").trim().to_ascii_lowercase();
    let charset = parts.find_map(|p| {
        let (k, val) = p.split_once('=')?;
        k.trim().eq_ignore_ascii_case("charset").then(|| val.trim().trim_matches('"').to_ascii_lowercase())
    });
    (essence, charset)
}

/// MIME Sniffing "JavaScript MIME type essence match" for a lowercased
/// essence (as [`parse_content_type`] returns it).
pub fn is_javascript_mime(essence: &str) -> bool {
    matches!(
        essence,
        "application/ecmascript"
            | "application/javascript"
            | "application/x-ecmascript"
            | "application/x-javascript"
            | "text/ecmascript"
            | "text/javascript"
            | "text/javascript1.0"
            | "text/javascript1.1"
            | "text/javascript1.2"
            | "text/javascript1.3"
            | "text/javascript1.4"
            | "text/javascript1.5"
            | "text/jscript"
            | "text/livescript"
            | "text/x-ecmascript"
            | "text/x-javascript"
    )
}

/// MIME Sniffing "JSON MIME type" for a lowercased essence: a valid MIME type
/// whose essence is `application/json` or `text/json`, or whose subtype ends
/// in `+json`.
pub fn is_json_mime(essence: &str) -> bool {
    // HTTP token code points (RFC 9110 tchar).
    let token =
        |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b));
    let Some((ty, subtype)) = essence.split_once('/') else { return false };
    if !token(ty) || !token(subtype) {
        return false;
    }
    essence == "application/json" || essence == "text/json" || subtype.ends_with("+json")
}

/// Decodes a body to UTF-8 using the response charset (BOM wins), falling
/// back to UTF-8 with replacement. Borrowed when no transcoding is needed.
pub fn decode_text<'a>(body: &'a [u8], charset: Option<&str>) -> std::borrow::Cow<'a, str> {
    let enc = charset.and_then(|c| encoding_rs::Encoding::for_label(c.as_bytes())).unwrap_or(encoding_rs::UTF_8);
    let (text, _, _) = enc.decode(body);
    text
}

#[cfg(test)]
mod mime_tests {
    use super::{is_javascript_mime, is_json_mime, parse_content_type};

    fn essence(content_type: &str) -> String {
        parse_content_type(content_type).0
    }

    #[test]
    fn json_mime_types_follow_mime_sniffing() {
        for ok in ["application/json", "text/json", "application/vnd.api+json", "Application/LD+JSON; charset=utf-8"] {
            assert!(is_json_mime(&essence(ok)), "{ok}");
        }
        for bad in [
            "application/json+protobuf",
            "text/x-json",
            "image/json",
            "text+json",
            "json+json",
            "text/json/json+json",
            "text/html;+json",
            "applic ation/vnd.api+json",
            "applic\"ation/vnd.api+json",
            "application/vnd.api\u{2122}+json",
            "/vnd.api+json",
            "",
            "text/javascript",
        ] {
            assert!(!is_json_mime(&essence(bad)), "{bad:?}");
        }
    }

    #[test]
    fn javascript_mime_types_are_the_mime_sniffing_list() {
        assert!(is_javascript_mime(&essence("text/javascript; charset=utf-8")));
        assert!(is_javascript_mime(&essence("Application/X-JavaScript")));
        for bad in ["", "text/plain", "application/json", "text/css", "text/javascript2"] {
            assert!(!is_javascript_mime(&essence(bad)), "{bad}");
        }
    }
}
