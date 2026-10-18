//! lp-net: HTTP client (hyper on a tokio I/O thread), cookie jar, CORS and
//! the fetch request/response model for lightningpanda. No JS dependency.

mod auth;
pub mod client;
pub mod cookies;
pub mod cors;
pub mod data_url;
pub mod request;
mod subscription;
pub use subscription::{NetSubscription, NetWake, SubscriptionError, SubscriptionLimits};

pub use client::{
    AuthChallenge, AuthDecision, InterceptDecision, Interceptor, NetActivity, NetClient, NetConfig, NetEvent,
    NetSettings, PausedRequest, RequestInfo, ResponseSink,
};
pub use cookies::CookieJar;
/// HTTP types used by Request/ResponseHead for native consumers.
pub use http;
pub use request::*;
pub use url::Url;
