//! lp-net: HTTP client (hyper on a tokio I/O thread), cookie jar, CORS and
//! the fetch request/response model for lightningpanda. No JS dependency.

pub mod client;
pub mod cookies;
pub mod cors;
pub mod data_url;
pub mod request;

pub use client::{
    InterceptDecision, Interceptor, NetClient, NetConfig, NetEvent, NetSettings, PausedRequest, RequestInfo,
    ResponseSink,
};
pub use cookies::CookieJar;
pub use request::*;
pub use url::Url;
