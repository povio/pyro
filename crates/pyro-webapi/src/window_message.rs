//! `window.postMessage` (HTML "window post message steps"): argument
//! handling, the structured clone into the target window's realm and the
//! `message` task. The embedder resolves which window is the target and
//! which (the incumbent) is the source; every realm of a browser thread is
//! in one VM, so the message is cloned straight into the target's realm.
//!
//! Cost when unused: none. A primitive message without a transfer list is
//! not cloned (it is the same value in every realm), so `postMessage` used
//! as a macrotask scheduler (`setImmediate` polyfills) stays one task.
use std::rc::Rc;

use pyro_js::{Args, JsError, JsResult, Persistent, Realm, Scope, Value};

use crate::event::{EventCore, EventInterface, MessageSource};
use crate::page::PageDom;

/// The parsed `targetOrigin`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TargetOrigin {
    /// `"*"`: any document.
    Any,
    /// An origin, by key (`PageDom::origin_key`: a tuple origin's serialization, or an opaque
    /// origin's identity for "/" from an opaque document); only a document of that origin receives it.
    Origin(String),
    /// An absolute URL with an opaque origin: no document matches it.
    Opaque,
}

impl TargetOrigin {
    /// Whether a document of origin key `origin` (`PageDom::origin_key`) may receive the message.
    pub fn allows(&self, origin: &str) -> bool {
        match self {
            Self::Any => true,
            Self::Origin(o) => o == origin,
            Self::Opaque => false,
        }
    }
}

/// `postMessage(message, targetOrigin, transfer)` or
/// `postMessage(message, {targetOrigin, transfer})`: the target origin
/// (`"/"` is `incumbent`, the source's serialized origin) and the options
/// for the structured clone. SyntaxError for a target origin that is not
/// `"*"`, `"/"` or an absolute URL.
pub fn post_message_args<'s>(
    s: &Scope<'s>,
    a: &Args<'s>,
    incumbent: &str,
) -> JsResult<(Value<'s>, TargetOrigin, Value<'s>)> {
    a.require(1, "Window.postMessage")?;
    // WebIDL overload resolution: three arguments are (message, targetOrigin,
    // transfer); otherwise an object (an array too) or a nullish second
    // argument is the options dictionary, anything else the target origin.
    let second = a.get(1);
    let (text, options) = if a.len() >= 3 {
        let transfer = a.get(2);
        let options = if transfer.is_undefined() {
            s.undefined()
        } else {
            let o = s.object();
            o.set_atom("transfer", transfer)?;
            o.value()
        };
        (crate::util::dom_string(second)?, options)
    } else if second.is_nullish() {
        ("/".to_owned(), s.undefined())
    } else if let Some(o) = second.as_object() {
        let t = o.get_atom("targetOrigin")?;
        (if t.is_undefined() { "/".to_owned() } else { crate::util::dom_string(t)? }, second)
    } else {
        (crate::util::dom_string(second)?, s.undefined())
    };
    let origin = match text.as_str() {
        "*" => TargetOrigin::Any,
        "/" => TargetOrigin::Origin(incumbent.to_owned()),
        other => {
            let url = pyro_net::Url::parse(other).map_err(|_| {
                JsError::dom("SyntaxError", format!("Invalid target origin '{other}' in a call to 'postMessage'."))
            })?;
            let origin = url.origin();
            if origin.is_tuple() {
                TargetOrigin::Origin(origin.ascii_serialization())
            } else {
                TargetOrigin::Opaque
            }
        }
    };
    Ok((a.get(0), origin, options))
}

/// Serializes `message` with `options` (`{transfer}`) into `realm`, the
/// target window's, and queues the `message` event at `page`'s window.
/// `origin_key` is the source's origin identity at send time; `source` resolves the
/// source window in the realm reading `event.source` (`None`: the target
/// window itself). Errors (DataCloneError, a getter's exception) propagate
/// in the realm of `s`, the poster's scope (see `structured_clone::clone_into`).
/// Without `deliver` (the target origin did not match) the message is still
/// serialized, transferring what it lists, and then dropped.
pub fn send_window_message<'s>(
    s: &Scope<'s>,
    message: Value<'s>,
    options: Value<'s>,
    page: &Rc<PageDom>,
    realm: &Realm,
    origin_key: Option<String>,
    source: Option<MessageSource>,
    deliver: bool,
) -> JsResult<()> {
    let rs = realm.scope()?;
    let payload = if message.as_object().is_none() && options.is_nullish() {
        crate::structured_clone::MessagePayload::Local(Persistent::new(&rs, rs.rehome(message)), None)
    } else {
        crate::structured_clone::clone_into(s, &rs, message, options)?
    };
    if !deliver {
        return Ok(());
    }
    let weak = Rc::downgrade(page);
    let realm = realm.downgrade();
    page.post_task(Box::new(move || {
        let Some(page) = weak.upgrade() else { return };
        if page.is_disposed() {
            return;
        }
        let Some(window) = page.window_target() else { return };
        let Some(realm) = realm.upgrade() else { return };
        let Ok(scope) = realm.scope() else { return };
        let (ty, data, ports) = match payload.read(&scope) {
            Ok((value, ports)) => ("message", Some(value), ports),
            Err(JsError::Terminated) => return,
            Err(_) => ("messageerror", None, None),
        };
        let ev = EventCore::trusted(&page, EventInterface::MessageEvent, ty, false, false);
        {
            let mut f = ev.fields.borrow_mut();
            f.message_data = data;
            f.message_ports = ports;
            if let Some(key) = origin_key {
                f.set_sender_origin(key);
            } else {
                f.origin = "null".into();
            }
            match source {
                Some(source) => f.message_source_window = Some(source),
                None => f.message_source = Some(window),
            }
        }
        crate::target::fire(&page, window, &ev);
    }));
    Ok(())
}
