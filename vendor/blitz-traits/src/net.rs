//! Abstractions of networking so that custom networking implementations can be provided

pub use bytes::Bytes;
pub use http::{self, HeaderMap, Method};
use serde::{
    Serialize,
    ser::{SerializeSeq, SerializeTuple},
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::{ops::Deref, path::PathBuf};
pub use url::Url;

/// A type that fetches resources for a Document.
///
/// This may be over the network via http(s), via the filesystem, or some other method.
pub trait NetProvider: Send + Sync + 'static {
    fn fetch(&self, doc_id: usize, request: Request, handler: Box<dyn NetHandler>);

    /// Whether this provider is a no-op (e.g. `DummyNetProvider`) that will never
    /// deliver resources. When true, callers must NOT register resources as
    /// "pending critical" — doing so blocks painting forever, since the
    /// completion callback never fires. Used by integrations that feed a
    /// pre-rendered DOM and perform no sub-fetches (e.g. aginxbrowser).
    fn is_noop(&self) -> bool {
        false
    }

    /// pyro: the document `doc_id` is a frame's, whose requests name `client`
    /// ([`FetchOptions::client`]): a provider that sends requests as a document's gives every
    /// request of `doc_id` that names no client this one.
    fn document_client(&self, _doc_id: usize, _client: String) {}
}

/// A type that parses raw bytes from a network request into a Data and then calls
/// the NetCallack with the result.
pub trait NetHandler: Send + Sync + 'static {
    fn bytes(self: Box<Self>, resolved_url: String, bytes: Bytes);

    /// pyro: how long after the first request for the resource its response is still of use,
    /// for a font of a `font-display: optional` face (its block period): a provider delivers a
    /// response that came later as a failed load to such handlers (CSS Fonts 4 § 4.10: the font
    /// is not swapped in). `None` (the default): no limit.
    fn block_period(&self) -> Option<std::time::Duration> {
        None
    }

    /// Called when fetching fails, including HTTP error statuses and cancellation.
    fn error(self: Box<Self>, _url: String, _error: String) {}

    /// pyro: [`Self::bytes`] for a response whose URL after redirects is `final_url`;
    /// `resolved_url` stays the URL asked for, which the document keys its loads by. A handler
    /// that resolves URLs in the body (a style sheet's `@import`s and `url()`s) resolves them
    /// against `final_url` (the default ignores it).
    fn bytes_at(self: Box<Self>, resolved_url: String, final_url: String, bytes: Bytes) {
        let _ = final_url;
        self.bytes(resolved_url, bytes)
    }

    /// pyro: [`Self::bytes_at`] for a response whose tainting is known: `opaque` when it is
    /// "opaque" (a no-cors request that reached another origin at any hop, though it ended back
    /// at the document's). An image handler shows an opaque image as its EXIF orientation says.
    fn bytes_from(self: Box<Self>, resolved_url: String, final_url: String, opaque: bool, bytes: Bytes) {
        let _ = opaque;
        self.bytes_at(resolved_url, final_url, bytes)
    }

    /// pyro: [`Self::bytes_from`] with the `Referrer-Policy` the response was served with: a
    /// style sheet's own requests (its imports, its `url()`s) are sent under it.
    fn bytes_meta(
        self: Box<Self>,
        resolved_url: String,
        final_url: String,
        opaque: bool,
        referrer_policy: Option<String>,
        bytes: Bytes,
    ) {
        let _ = referrer_policy;
        self.bytes_from(resolved_url, final_url, opaque, bytes)
    }
}

/// A callback which gets called every time a network request completes
// Q: Should we use std::task::Waker for this?
pub trait NetWaker: Send + Sync + 'static {
    fn wake(&self, client_id: usize);
}

impl<F: Fn(usize) + Send + Sync + 'static> NetWaker for F {
    fn wake(&self, doc_id: usize) {
        self(doc_id)
    }
}

#[non_exhaustive]
#[derive(Debug, Clone)]
/// A request type loosely representing <https://fetch.spec.whatwg.org/#requests>
pub struct Request {
    pub url: Url,
    pub method: Method,
    pub content_type: Option<String>,
    pub headers: HeaderMap,
    pub body: Body,
    pub signal: Option<AbortSignal>,
    /// pyro: how the request is made beyond its URL (CORS mode, integrity, referrer), as a CSS
    /// `url()`'s request URL modifiers ask (CSS Values 5 § 4.5.4).
    pub options: FetchOptions,
}

/// pyro: the credentials mode of a request in CORS mode (Fetch § 2.2.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CorsCredentials {
    /// "same-origin" (`crossorigin=anonymous`, `cross-origin(anonymous)`).
    SameOrigin,
    /// "include" (`crossorigin=use-credentials`, `cross-origin(use-credentials)`).
    Include,
}

/// pyro: a request's mode, integrity metadata and referrer, where they are not a sub-resource's
/// defaults (no-cors, no integrity, the document as referrer with its policy).
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct FetchOptions {
    /// `Some`: the request's mode is "cors", with these credentials.
    pub cors: Option<CorsCredentials>,
    /// Integrity metadata (Subresource Integrity): a response whose body matches none of it is
    /// a network error, as is a cross-origin one fetched without CORS.
    pub integrity: Option<String>,
    /// The request's referrer policy keyword (`None`: the document's policy).
    pub referrer_policy: Option<String>,
    /// The request's referrer (`None`: the document): the style sheet a `url()` was written in.
    pub referrer: Option<Url>,
    /// The id of the frame whose document the request is made for (`None`: the page's own):
    /// the request is that frame's, with its origin (opaque in a sandboxed frame), which a CORS
    /// check compares the response with. Never a URL, which several frames can share.
    pub client: Option<String>,
    /// pyro: what the response is used as (Fetch's destination), from where the request is made,
    /// never from its URL: its `Accept`, `Sec-Fetch-Dest` and credentials follow it. Not part
    /// of [`Self::is_default`] (it says what the request is for, not how it is made).
    pub destination: Destination,
}

/// pyro: a request's destination (Fetch § 2.2.5), as the code that makes it knows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Destination {
    /// Not said (an embedder's own request).
    #[default]
    Unknown,
    /// A CSS image, an `<img>`, an SVG reference (`image`).
    Image,
    /// A web font (`font`).
    Font,
    /// A style sheet, an `@import`ed one included (`style`).
    Style,
    /// A frame's document (`iframe`).
    Document,
}

impl FetchOptions {
    /// Whether these are a sub-resource's defaults (the destination aside).
    pub fn is_default(&self) -> bool {
        Self { destination: Destination::Unknown, ..self.clone() } == Self::default()
    }

    /// pyro: these, for a request whose response is used as `destination`.
    pub fn for_destination(mut self, destination: Destination) -> Self {
        self.destination = destination;
        self
    }

    /// The key a response to `url` fetched with these options is cached by: one fetched with
    /// other options never stands in for it (a CORS response for a no-cors one, a response
    /// checked against other integrity metadata, or one sent another referrer). A space is
    /// never in a serialized URL.
    pub fn cache_key(&self, url: &str) -> String {
        // pyro: a response used as one destination (fetched with its `Accept`, credentials)
        // never stands in for another's.
        match self.destination {
            Destination::Unknown => self.request_key(url),
            destination => format!("dest={destination:?} {}", self.request_key(url)),
        }
    }

    fn request_key(&self, url: &str) -> String {
        if self.is_default() {
            return url.to_owned();
        }
        if self.integrity.is_none() && self.referrer_policy.is_none() && self.referrer.is_none() && self.client.is_none()
        {
            return match self.cors {
                Some(CorsCredentials::SameOrigin) => format!("cors-anonymous {url}"),
                Some(CorsCredentials::Include) => format!("cors-use-credentials {url}"),
                None => url.to_owned(),
            };
        }
        format!(
            "cors={:?} integrity={:?} referrer-policy={:?} referrer={:?} client={:?} {url}",
            self.cors,
            self.integrity,
            self.referrer_policy,
            self.referrer.as_ref().map(Url::as_str),
            self.client.as_deref(),
        )
    }
}

impl Request {
    /// A get request to the specified Url and an empty body
    pub fn get(url: Url) -> Self {
        Self {
            url,
            method: Method::GET,
            content_type: None,
            headers: HeaderMap::new(),
            body: Body::Empty,
            signal: None,
            options: FetchOptions::default(),
        }
    }

    pub fn signal(mut self, signal: AbortSignal) -> Self {
        self.signal = Some(signal);
        self
    }
}

#[derive(Debug, Clone)]
pub enum Body {
    Bytes(Bytes),
    Form(FormData),
    Empty,
}

/// A list of form entries used for form submission
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FormData(pub Vec<Entry>);
impl FormData {
    /// Creates a new empty FormData
    pub fn new() -> Self {
        FormData(Vec::new())
    }
}
impl Serialize for FormData {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut seq_serializer = serializer.serialize_seq(Some(self.len()))?;
        for entry in &self.0 {
            seq_serializer.serialize_element(entry)?;
        }
        seq_serializer.end()
    }
}
impl Deref for FormData {
    type Target = Vec<Entry>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// A single form entry consisting of a name and value
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub name: String,
    pub value: EntryValue,
}
impl Serialize for Entry {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut serializer = serializer.serialize_tuple(2)?;
        serializer.serialize_element(&self.name)?;
        match &self.value {
            EntryValue::String(s) => serializer.serialize_element(s)?,
            EntryValue::File(p) => serializer.serialize_element(p.to_str().unwrap_or_default())?,
            EntryValue::EmptyFile => serializer.serialize_element("")?,
        }
        serializer.end()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum EntryValue {
    String(String),
    File(PathBuf),
    EmptyFile,
}
impl AsRef<str> for EntryValue {
    fn as_ref(&self) -> &str {
        match self {
            EntryValue::String(s) => s,
            EntryValue::File(p) => p.to_str().unwrap_or_default(),
            EntryValue::EmptyFile => "",
        }
    }
}

impl From<&str> for EntryValue {
    fn from(value: &str) -> Self {
        EntryValue::String(value.to_string())
    }
}
impl From<PathBuf> for EntryValue {
    fn from(value: PathBuf) -> Self {
        EntryValue::File(value)
    }
}

/// A default noop NetProvider
#[derive(Default)]
pub struct DummyNetProvider;
impl NetProvider for DummyNetProvider {
    fn fetch(&self, _doc_id: usize, _request: Request, _handler: Box<dyn NetHandler>) {}
    fn is_noop(&self) -> bool {
        true
    }
}

/// The AbortController interface represents a controller object that
/// allows you to abort one or more Web requests as and when desired.
///
/// <https://developer.mozilla.org/en-US/docs/Web/API/AbortController>
#[derive(Debug, Default)]
pub struct AbortController {
    pub signal: AbortSignal,
}

impl AbortController {
    /// The abort() method of the AbortController interface aborts
    /// an asynchronous operation before it has completed.
    /// This is able to abort fetch requests.
    ///
    /// <https://developer.mozilla.org/en-US/docs/Web/API/AbortController/abort>
    pub fn abort(self) {
        self.signal.0.store(true, Ordering::SeqCst);
    }
}

/// The AbortSignal interface represents a signal object that allows you to
/// communicate with an asynchronous operation (such as a fetch request) and
/// abort it if required via an AbortController object.
///
/// <https://developer.mozilla.org/en-US/docs/Web/API/AbortSignal>
#[derive(Debug, Default, Clone)]
pub struct AbortSignal(Arc<AtomicBool>);

impl AbortSignal {
    /// The aborted read-only property returns a value that indicates whether
    /// the asynchronous operations the signal is communicating with are
    /// aborted (true) or not (false).
    ///
    /// <https://developer.mozilla.org/en-US/docs/Web/API/AbortSignal/aborted>
    pub fn aborted(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}
