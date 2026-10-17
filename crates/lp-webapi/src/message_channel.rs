//! `MessageChannel` / `MessagePort` (HTML §9.4) for same-realm messaging.
//!
//! Each port is a non-node EventTarget. `postMessage` structured-clones the
//! message (the realm's original `structuredClone`, so non-cloneable values
//! throw DataCloneError) and queues it on the entangled port. A started port
//! delivers each message as its own task, in FIFO order, as a trusted
//! `MessageEvent` (`origin` "", no `source`, no `ports`). Messages wait while
//! the port is not started; `start()` or assigning `onmessage` starts it,
//! `addEventListener("message")` does not.
//!
//! `close()` disentangles both ports; later posts on either side are dropped
//! and the closed port drops its undelivered messages (upstream
//! MessagePort.zig, Chromium).
//!
//! Lifetime: a started, entangled, open port is pinned so its listeners keep
//! running without a JS reference (HTML keeps such ports alive); it is
//! unpinned on close, and every pin is released with the page.
//!
//! Gaps (explicit): transferring anything, including MessagePorts, throws
//! NotSupportedError (`postMessage(m, [port])` or `{transfer}`), so ports never
//! leave their realm; a MessagePort is not cloneable as data
//! (DataCloneError); there is no `messageerror` source because same-realm
//! deserialization cannot fail.
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::{Rc, Weak};

use lp_js::{ClassDef, JsClass, JsError, JsResult, NoParent, Object, Persistent, Scope, WeakRealm};

use crate::event::{EventCore, EventInterface};
use crate::page::PageDom;
use crate::target::{fire, page_of, pin_target, target_object, unpin_target, EventTarget, EventTargetData};
use lp_dom::events::TargetKey;

pub struct PortInner {
    target: EventTargetData,
    realm: WeakRealm,
    peer: RefCell<Weak<PortInner>>,
    queue: RefCell<VecDeque<Persistent>>,
    started: Cell<bool>,
    closed: Cell<bool>,
}

impl PortInner {
    fn page(&self) -> &Rc<PageDom> {
        &self.target.page
    }

    /// Queues `data` (already cloned into this port's realm).
    fn enqueue(self: &Rc<Self>, data: Persistent) {
        if self.closed.get() {
            return;
        }
        self.queue.borrow_mut().push_back(data);
        if self.started.get() {
            self.schedule_one();
        }
    }

    /// One task per queued message keeps FIFO order and task boundaries.
    fn schedule_one(self: &Rc<Self>) {
        let port = Rc::downgrade(self);
        self.page().post_task(Box::new(move || {
            if let Some(port) = port.upgrade() {
                port.deliver_next();
            }
        }));
    }

    fn deliver_next(&self) {
        if self.closed.get() || self.page().is_disposed() {
            return;
        }
        let Some(data) = self.queue.borrow_mut().pop_front() else { return };
        let ev = EventCore::trusted(self.page(), EventInterface::MessageEvent, "message", false, false);
        {
            let mut f = ev.fields.borrow_mut();
            f.message_data = Some(data);
            f.origin = String::new();
        }
        fire(self.page(), self.target.key, &ev);
    }

    fn start(self: &Rc<Self>, s: &Scope<'_>) -> JsResult<()> {
        if self.started.get() || self.closed.get() {
            return Ok(());
        }
        self.started.set(true);
        if self.peer.borrow().upgrade().is_some() {
            if let Some(wrapper) = target_object(s, self.page(), self.target.key)? {
                pin_target(s, self.page(), self.target.key, wrapper);
            }
        }
        for _ in 0..self.queue.borrow().len() {
            self.schedule_one();
        }
        Ok(())
    }

    fn close(&self, s: &Scope<'_>) {
        if self.closed.replace(true) {
            return;
        }
        self.queue.borrow_mut().clear();
        unpin_target(s, self.page(), self.target.key);
        if let Some(peer) = self.peer.take().upgrade() {
            peer.peer.replace(Weak::new());
            // The peer can no longer receive anything; let it be collected.
            if let Some(realm) = peer.realm.upgrade() {
                if let Ok(ps) = realm.scope() {
                    unpin_target(&ps, peer.page(), peer.target.key);
                }
            }
        }
    }
}

pub struct PortData(Rc<PortInner>);

pub struct MessagePort;
impl JsClass for MessagePort {
    const NAME: &'static str = "MessagePort";
    type Data = PortData;
    type Parent = EventTarget;
    fn define(def: &mut ClassDef<Self>) {
        def.upcast(|d: &PortData| &d.0.target);
        def.method("postMessage", 1, |s, d, a| {
            a.require(1, "MessagePort.postMessage")?;
            let options = match a.opt(1) {
                // `postMessage(message, transfer)`: a sequence is the transfer list.
                Some(v) if v.is_array() => {
                    let o = s.object();
                    o.set("transfer", v)?;
                    o.value()
                }
                Some(v) => v,
                None => s.undefined(),
            };
            // Clone even when the message will be dropped, as in the spec
            // (errors are reported regardless of entanglement).
            let cloned = crate::structured_clone::clone_value(s, a.get(0), options)?;
            if d.0.closed.get() {
                return Ok(s.undefined());
            }
            if let Some(peer) = d.0.peer.borrow().upgrade() {
                peer.enqueue(Persistent::new(s, cloned));
            }
            Ok(s.undefined())
        });
        def.method("start", 0, |s, d, _a| {
            d.0.start(s)?;
            Ok(s.undefined())
        });
        def.method("close", 0, |s, d, _a| {
            d.0.close(s);
            Ok(s.undefined())
        });
        def.accessor(
            "onmessage",
            |s, d| crate::target::event_handler(s, &d.0.target, "message"),
            |s, d, value| {
                crate::target::set_event_handler(s, &d.0.target, "message", value)?;
                // Setting onmessage implicitly starts the port (HTML §9.4.4).
                if value.as_object().is_some() {
                    d.0.start(s)?;
                }
                Ok(())
            },
        );
        def.accessor(
            "onmessageerror",
            |s, d| crate::target::event_handler(s, &d.0.target, "messageerror"),
            |s, d, value| crate::target::set_event_handler(s, &d.0.target, "messageerror", value),
        );
    }
}

fn new_port<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<(Object<'s>, Rc<PortInner>)> {
    let target = EventTargetData::new(page);
    let TargetKey::Other(k) = target.key else { unreachable!() };
    let key = page.other_identity(k);
    let inner = Rc::new(PortInner {
        target,
        realm: s.realm().downgrade(),
        peer: RefCell::new(Weak::new()),
        queue: RefCell::new(VecDeque::new()),
        started: Cell::new(false),
        closed: Cell::new(false),
    });
    Ok((s.wrap::<MessagePort>(PortData(inner.clone()), Some(key))?, inner))
}

/// The channel references its ports only through traced hidden properties
/// on its wrapper (as URL/URLSearchParams), so a listener or expando that
/// references the channel cannot form a GC-invisible cycle.
pub struct ChannelData {
    realm: WeakRealm,
    identity: lp_js::IdentityKey,
}

fn channel_port<'s>(s: &Scope<'s>, c: &ChannelData, name: &str) -> JsResult<lp_js::Value<'s>> {
    let realm = c.realm.upgrade().ok_or(JsError::ContextDestroyed)?;
    let owner = realm.scope()?;
    let object = owner.lookup_identity(c.identity).ok_or(JsError::ContextDestroyed)?;
    let value = owner.get_hidden(object, name)?.ok_or(JsError::ContextDestroyed)?;
    // SAFETY: both realms share the VM; the receiver's traced property roots
    // the port wrapper throughout this getter.
    Ok(unsafe { lp_js::Value::from_raw(s.ctx(), value.as_raw()) })
}

pub struct MessageChannel;
impl JsClass for MessageChannel {
    const NAME: &'static str = "MessageChannel";
    type Data = ChannelData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(0, |s, nt, _a| {
            let page = page_of(s)?;
            if page.is_disposed() {
                return Err(JsError::ContextDestroyed);
            }
            let (o1, p1) = new_port(s, &page)?;
            let (o2, p2) = new_port(s, &page)?;
            p1.peer.replace(Rc::downgrade(&p2));
            p2.peer.replace(Rc::downgrade(&p1));
            let identity = page.event_identity(page.next_event_id());
            let data = ChannelData { realm: s.realm().downgrade(), identity };
            let object = s.wrap_new::<MessageChannel>(data, nt, Some(identity))?;
            s.set_hidden(object, "port1", o1.value())?;
            s.set_hidden(object, "port2", o2.value())?;
            Ok(object)
        });
        def.getter("port1", |s, c| channel_port(s, c, "port1"));
        def.getter("port2", |s, c| channel_port(s, c, "port2"));
    }
}

pub(crate) fn register(engine: &lp_js::Engine) {
    engine.register::<MessagePort>();
    engine.register::<MessageChannel>();
}
