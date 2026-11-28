//! WebAuthn authenticators behind `navigator.credentials`. A headless browser has no platform
//! authenticator and no security key to talk to, so a request is answered only by the virtual
//! authenticators the embedder adds to the browser context (WebDriver's Virtual Authenticators,
//! CDP `WebAuthn.*`): software CTAP authenticators with ES256 credentials (`authenticator`), whose
//! attestation objects and COSE keys are CBOR (`cbor`).

mod authenticator;
mod cbor;

use std::rc::Rc;

use serde_json::{json, Value};

use crate::page::PageDom;

pub(crate) use authenticator::State;

/// How a `WebAuthn.*` command failed, for the embedder to turn into its protocol error.
pub enum WebAuthnError {
    /// An argument is not valid (CDP: invalid params).
    Invalid(String),
    /// The command cannot be carried out in the current state (CDP: server error).
    Failed(String),
}

impl State {
    /// A CDP `WebAuthn.<method>` command.
    pub(crate) fn run(&mut self, method: &str, params: &Value) -> Result<Value, WebAuthnError> {
        self.command(method, params).map_err(|e| match e {
            authenticator::CommandError::Invalid(m) => WebAuthnError::Invalid(m),
            authenticator::CommandError::Failed(m) => WebAuthnError::Failed(m),
        })
    }
}

/// One `create` or `get` request of the client (credentials.js): the answer of the first virtual
/// authenticator that can give one, or `pending` when none can (no authenticator, none that fits the
/// request, none a user touches): the request then stays open until its timeout or abort.
pub(crate) fn authenticate(page: &Rc<PageDom>, request: &Value) -> Value {
    let services = page.context_services();
    let mut state = services.webauthn.borrow_mut();
    match request["op"].as_str() {
        Some("create") => state.make_credential(request),
        Some("get") => state.get_assertion(request),
        _ => json!({ "status": "pending" }),
    }
}

/// What a client can tell about the authenticators: a user-verifying platform authenticator
/// (`uvpaa`) and conditional mediation (a platform authenticator that holds discoverable credentials).
pub(crate) fn platform(page: &Rc<PageDom>) -> Value {
    let services = page.context_services();
    let state = services.webauthn.borrow();
    state.platform()
}

/// The Signal API calls (`signalUnknownCredential` and the others): the authenticators' discoverable
/// credentials are removed or updated.
pub(crate) fn signal(page: &Rc<PageDom>, request: &Value) -> Value {
    let services = page.context_services();
    let mut state = services.webauthn.borrow_mut();
    state.signal(request)
}
