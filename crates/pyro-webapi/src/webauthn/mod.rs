//! WebAuthn authenticators behind `navigator.credentials`. A headless browser has no platform
//! authenticator and no security key to talk to, so a request is answered only by the virtual
//! authenticators the embedder adds (WebDriver's Virtual Authenticators, CDP `WebAuthn.*`).

use std::rc::Rc;

use serde_json::{json, Value};

use crate::page::PageDom;

/// One `create` or `get` request of the client (credentials.js): without an authenticator that can
/// answer it, `pending`: the request stays open until its timeout or abort.
pub(crate) fn authenticate(_page: &Rc<PageDom>, _request: &Value) -> Value {
    json!({ "status": "pending" })
}

/// What a client can tell about the authenticators: a user-verifying platform authenticator
/// (`uvpaa`) and conditional mediation (a platform authenticator that holds discoverable credentials).
pub(crate) fn platform(_page: &Rc<PageDom>) -> Value {
    json!({ "uvpaa": false, "conditional": false })
}

/// The Signal API calls (`signalUnknownCredential` and the others): the authenticators' discoverable
/// credentials are removed or updated.
pub(crate) fn signal(_page: &Rc<PageDom>, _request: &Value) -> Value {
    json!({})
}
