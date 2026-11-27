//! pyro-net: HTTP client (hyper on a tokio I/O thread), cookie jar, CORS and
//! the fetch request/response model for pyro. No JS dependency.

mod auth;
mod cache;
mod client_hints;
mod dictionary;
pub mod client;
pub mod cookies;
pub mod cors;
pub mod csp;
pub mod data_url;
mod h2_probe;
mod inflate;
pub mod proxy;
pub mod range;
pub mod referrer;
pub mod request;
pub mod secure_context;
pub mod static_files;
mod subscription;
pub mod timing;
mod ws_deflate;
mod zstd;
pub use subscription::{NetSubscription, NetWake, SubscriptionError, SubscriptionLimits};
pub use timing::{timing_allowed, ConnectionTiming, NetTiming};

pub use cache::{parse_http_date, SharedAssetCache, SHARED_ASSET_CACHE_ENV};
pub use client::set_idle_release;
pub use client::websocket::{
    accept_key as websocket_accept_key, is_bad_port, WebSocketActivity, WebSocketHandle, WebSocketRequest, WsEvent,
    WsSink,
};
pub use client::{
    AuthChallenge, AuthDecision, InterceptDecision, Interceptor, NetActivity, NetClient, NetConfig, NetEvent,
    NetSettings, PausedRequest, PausedResponse, RequestCanceller, RequestInfo, ResponseDecision, ResponseSink,
};
pub use cookies::CookieJar;
/// HTTP types used by Request/ResponseHead for native consumers.
pub use http;
pub use proxy::ProxyConfig;
pub use referrer::ReferrerPolicy;
pub use request::*;
pub use secure_context::{is_potentially_trustworthy_origin, is_potentially_trustworthy_url};
pub use static_files::{StaticFile, StaticFiles, StaticHit, StaticMemoryHit};
pub use url::Url;
