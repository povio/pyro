//! HTML "close requests and close watchers": the per-window close watcher manager that dialogs,
//! popovers, customizable select pickers and the `CloseWatcher` API share, and `CloseWatcher`.
//!
//! The manager keeps ordered groups of watchers. A close request (Escape) acts on the newest
//! group, its watchers newest first, and stops at the first one whose `cancel` was canceled.
//! Every activating input lets the page open one more group: a watcher established while such
//! an allowance is left (and the manager is not empty) starts a new group and uses one up, any
//! other joins the newest group (an empty manager always gets a new group). `cancel` of a close
//! request is cancelable only while the window has history-action activation (an allowance left),
//! and a canceled one uses the activation up. These are the rules WPT close-watcher/user-activation
//! (and Chrome) show; the allowance count is what tells `yy` (two groups, both cancelable) from `ny`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use pyro_dom::events::TargetKey;
use pyro_dom::NodeId;
use pyro_js::{ClassDef, JsClass, JsError, Value};
use rustc_hash::FxHashMap;

use crate::abort::{AbortRegistration, AbortSignal};
use crate::event::{EventCore, EventInterface};
use crate::page::PageDom;
use crate::target::{self, page_of, pin_target, unpin_target, EventTarget, EventTargetData};

/// One close watcher of the manager: an element's (an open dialog, a showing auto or hint
/// popover, an open select picker; what closes it follows from the element's state) or a
/// `CloseWatcher` object's (its id in [`CloseWatchers::scripts`]).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Watcher {
    Element(NodeId),
    Script(u64),
}

/// A `CloseWatcher` object's state while it is active.
pub(crate) struct ScriptWatcher {
    key: TargetKey,
    realm: pyro_js::WeakRealm,
    /// HTML "is running cancel action".
    running_cancel: Cell<bool>,
    /// The `signal` option's abort step, dropped with the watcher.
    abort: RefCell<Option<AbortRegistration>>,
}

/// HTML "close watcher manager" of a window.
#[derive(Default)]
pub(crate) struct CloseWatchers {
    /// HTML "groups": the newest group is last, a group's newest watcher is last.
    groups: RefCell<Vec<Vec<Watcher>>>,
    /// The activating inputs the window got that no new group and no canceled `cancel` has used
    /// up yet (HTML "history-action activation" and "next user interaction allows a new group").
    allowances: Cell<u64>,
    /// The active `CloseWatcher` objects.
    scripts: RefCell<FxHashMap<u64, Rc<ScriptWatcher>>>,
    next_id: Cell<u64>,
}

impl CloseWatchers {
    /// HTML "activation notification": the window got an activating input.
    pub(crate) fn activate(&self) {
        self.allowances.set(self.allowances.get().saturating_add(1));
    }

    /// HTML "has history-action activation".
    pub(crate) fn has_activation(&self) -> bool {
        self.allowances.get() > 0
    }

    /// HTML "consume history-action user activation".
    pub(crate) fn consume_activation(&self) {
        self.allowances.set(0);
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.groups.borrow().is_empty()
    }

    pub(crate) fn contains(&self, w: Watcher) -> bool {
        self.groups.borrow().iter().any(|g| g.contains(&w))
    }

    /// HTML "establish a close watcher" (a watcher already present moves to its new place).
    pub(crate) fn establish(&self, w: Watcher) {
        self.remove(w);
        let mut groups = self.groups.borrow_mut();
        if groups.is_empty() {
            groups.push(vec![w]);
        } else if self.allowances.get() > 0 {
            self.allowances.set(self.allowances.get() - 1);
            groups.push(vec![w]);
        } else if let Some(last) = groups.last_mut() {
            last.push(w);
        }
    }

    /// The watcher leaves its group (HTML "destroy"); an empty group goes.
    pub(crate) fn remove(&self, w: Watcher) {
        let mut groups = self.groups.borrow_mut();
        for g in groups.iter_mut() {
            g.retain(|&x| x != w);
        }
        groups.retain(|g| !g.is_empty());
    }

    /// Drops the element watchers `keep` refuses.
    pub(crate) fn retain_elements(&self, keep: impl Fn(NodeId) -> bool) {
        let mut groups = self.groups.borrow_mut();
        for g in groups.iter_mut() {
            g.retain(|w| match *w {
                Watcher::Element(e) => keep(e),
                Watcher::Script(_) => true,
            });
        }
        groups.retain(|g| !g.is_empty());
    }

    /// The element watchers, the oldest first.
    pub(crate) fn elements(&self) -> Vec<NodeId> {
        self.groups
            .borrow()
            .iter()
            .flatten()
            .filter_map(|w| match *w {
                Watcher::Element(e) => Some(e),
                Watcher::Script(_) => None,
            })
            .collect()
    }

    /// The watchers a close request acts on, the oldest first: those `listens` accepts of the
    /// newest group that has one (a non-modal dialog without `closedby` has no watcher to act).
    fn newest_group(&self, listens: impl Fn(Watcher) -> bool) -> Option<Vec<Watcher>> {
        let groups = self.groups.borrow();
        groups
            .iter()
            .rev()
            .map(|g| g.iter().copied().filter(|&w| listens(w)).collect::<Vec<_>>())
            .find(|g| !g.is_empty())
    }

    fn script(&self, id: u64) -> Option<Rc<ScriptWatcher>> {
        self.scripts.borrow().get(&id).cloned()
    }
}

fn fire_event(page: &Rc<PageDom>, key: TargetKey, ty: &str, cancelable: bool) -> bool {
    let ev = EventCore::trusted(page, EventInterface::Event, ty, false, cancelable);
    target::fire(page, key, &ev)
}

/// HTML "process close watchers": a close request. Returns whether some watcher was asked.
pub(crate) fn process(page: &Rc<PageDom>) -> bool {
    let group = page.popover.watchers.newest_group(|w| match w {
        Watcher::Element(e) => crate::popover::listens_for_close_request(page, e),
        Watcher::Script(_) => true,
    });
    let Some(group) = group else { return false };
    for w in group.into_iter().rev() {
        // An earlier watcher's events may have destroyed this one.
        if !page.popover.watchers.contains(w) {
            continue;
        }
        let proceed = match w {
            Watcher::Script(id) => request_close(page, id, true),
            Watcher::Element(e) => {
                let can_prevent = page.popover.watchers.has_activation();
                let proceed = crate::popover::request_close_element(page, e, can_prevent);
                if !proceed && can_prevent {
                    page.popover.watchers.consume_activation();
                }
                proceed
            }
        };
        if !proceed {
            break;
        }
    }
    true
}

/// HTML "destroy" of a `CloseWatcher`: it is inactive from now on and fires nothing.
pub(crate) fn destroy(page: &PageDom, id: u64) {
    let removed = page.popover.watchers.scripts.borrow_mut().remove(&id);
    let Some(w) = removed else { return };
    page.popover.watchers.remove(Watcher::Script(id));
    if let Some(realm) = w.realm.upgrade() {
        if let Ok(s) = realm.scope() {
            unpin_target(&s, page, w.key);
        }
    }
    // The abort step goes with the watcher (after the borrows above ended: it may be this very step running).
    let registration = w.abort.borrow_mut().take();
    drop(registration);
}

/// HTML "close a close watcher": it is destroyed, then `close` fires, unless its document is no
/// longer fully active. `close` fires before the wrapper is released.
pub(crate) fn close(page: &Rc<PageDom>, id: u64) {
    let Some(w) = page.popover.watchers.script(id) else { return };
    if !page.is_fully_active() {
        destroy(page, id);
        return;
    }
    // Inactive first: a listener asking to close or destroy it again does nothing.
    page.popover.watchers.scripts.borrow_mut().remove(&id);
    page.popover.watchers.remove(Watcher::Script(id));
    fire_event(page, w.key, "close", false);
    if let Some(realm) = w.realm.upgrade() {
        if let Ok(s) = realm.scope() {
            unpin_target(&s, page, w.key);
        }
    }
    let registration = w.abort.borrow_mut().take();
    drop(registration);
}

/// HTML "request to close a close watcher". `from_close_request`: a user's close request, whose
/// `cancel` is cancelable only with history-action activation (a canceled one uses it up); the
/// script's `requestClose()` always fires a cancelable one. Returns false when `cancel` was canceled.
pub(crate) fn request_close(page: &Rc<PageDom>, id: u64, from_close_request: bool) -> bool {
    let Some(w) = page.popover.watchers.script(id) else { return true };
    if w.running_cancel.get() {
        return true;
    }
    if !page.is_fully_active() {
        destroy(page, id);
        return true;
    }
    let cancelable = !from_close_request || page.popover.watchers.has_activation();
    w.running_cancel.set(true);
    let proceed = fire_event(page, w.key, "cancel", cancelable);
    w.running_cancel.set(false);
    if !proceed {
        if from_close_request && cancelable {
            page.popover.watchers.consume_activation();
        }
        return false;
    }
    close(page, id);
    true
}

pub(crate) struct CloseWatcherData {
    target: EventTargetData,
    id: u64,
}

pub(crate) struct CloseWatcher;
impl JsClass for CloseWatcher {
    const NAME: &'static str = "CloseWatcher";
    type Data = CloseWatcherData;
    type Parent = EventTarget;
    fn define(def: &mut ClassDef<Self>) {
        def.upcast(|d: &CloseWatcherData| &d.target);
        def.constructor(0, |s, nt, a| {
            let page = page_of(s)?;
            if page.is_disposed() {
                return Err(JsError::ContextDestroyed);
            }
            if !page.is_fully_active() {
                return Err(JsError::dom("InvalidStateError", "The document is not fully active."));
            }
            // CloseWatcherOptions { AbortSignal signal }.
            let mut signal: Option<Value<'_>> = None;
            if let Some(init) = a.opt(0).filter(|v| !v.is_nullish()) {
                let init = init.as_object().ok_or_else(|| {
                    JsError::Type("Failed to construct 'CloseWatcher': the options must be an object.".into())
                })?;
                let value = init.get_atom("signal")?;
                if !value.is_undefined() {
                    if s.unwrap::<AbortSignal>(value).is_none() {
                        return Err(JsError::Type(
                            "Failed to construct 'CloseWatcher': the signal must be an AbortSignal.".into(),
                        ));
                    }
                    signal = Some(value);
                }
            }
            let target = EventTargetData::new(&page);
            let key = target.key;
            let TargetKey::Other(k) = key else { unreachable!() };
            let identity = page.other_identity(k);
            let id = page.popover.watchers.next_id.get() + 1;
            page.popover.watchers.next_id.set(id);
            let object = s.wrap_new::<CloseWatcher>(CloseWatcherData { target, id }, nt, Some(identity))?;
            let watcher = ScriptWatcher {
                key,
                realm: s.realm().downgrade(),
                running_cancel: Cell::new(false),
                abort: RefCell::new(None),
            };
            page.popover.watchers.scripts.borrow_mut().insert(id, Rc::new(watcher));
            page.popover.watchers.establish(Watcher::Script(id));
            // An active watcher stays alive, with its listeners, until it closes or is destroyed.
            pin_target(s, &page, key, object);
            if let Some(signal) = signal.and_then(|v| s.unwrap::<AbortSignal>(v)) {
                let weak = Rc::downgrade(&page);
                // An already aborted signal runs the step now: the watcher is destroyed again at once.
                let registration = signal.register_native(Box::new(move |_: &PageDom| {
                    if let Some(page) = weak.upgrade() {
                        destroy(&page, id);
                    }
                }));
                let watcher = page.popover.watchers.script(id);
                if let (Some(registration), Some(watcher)) = (registration, watcher) {
                    *watcher.abort.borrow_mut() = Some(registration);
                }
            }
            Ok(object)
        });
        def.method("requestClose", 0, |s, d, _a| {
            let page = d.target.page.clone();
            request_close(&page, d.id, false);
            Ok(s.undefined())
        });
        def.method("close", 0, |s, d, _a| {
            let page = d.target.page.clone();
            close(&page, d.id);
            Ok(s.undefined())
        });
        def.method("destroy", 0, |s, d, _a| {
            destroy(&d.target.page, d.id);
            Ok(s.undefined())
        });
        def.accessor(
            "oncancel",
            |s, d| target::event_handler(s, &d.target, "cancel"),
            |s, d, v| target::set_event_handler(s, &d.target, "cancel", v),
        );
        def.accessor(
            "onclose",
            |s, d| target::event_handler(s, &d.target, "close"),
            |s, d, v| target::set_event_handler(s, &d.target, "close", v),
        );
    }
}

pub(crate) fn register(engine: &pyro_js::Engine) {
    engine.register::<CloseWatcher>();
}
