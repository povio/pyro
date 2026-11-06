//! lp-net: HTTP client (hyper on a tokio I/O thread), cookie jar, CORS and
//! the fetch request/response model for lightningpanda. No JS dependency.

mod auth;
mod cache;
pub mod client;
pub mod cookies;
pub mod cors;
pub mod data_url;
pub mod proxy;
pub mod range;
pub mod request;
pub mod static_files;
mod subscription;
pub use subscription::{NetSubscription, NetWake, SubscriptionError, SubscriptionLimits};

pub use cache::{SharedAssetCache, SHARED_ASSET_CACHE_ENV};
pub use client::set_idle_release;
pub use client::{
    AuthChallenge, AuthDecision, InterceptDecision, Interceptor, NetActivity, NetClient, NetConfig, NetEvent,
    NetSettings, PausedRequest, PausedResponse, RequestInfo, ResponseDecision, ResponseSink,
};
pub use cookies::CookieJar;
/// HTTP types used by Request/ResponseHead for native consumers.
pub use http;
pub use proxy::ProxyConfig;
pub use request::*;
pub use static_files::{StaticFile, StaticFiles, StaticHit, StaticMemoryHit};
pub use url::Url;
