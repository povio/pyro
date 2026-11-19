//! HTML's promise rejection tracking for a realm (HostPromiseRejectionTracker and "notify about
//! rejected promises"), on JavaScriptCore's tracker callback (patch 0036,
//! `JSGlobalContextSetPromiseRejectionTracker`).
//!
//! - JavaScriptCore keeps the promises rejected without a handler and, once the microtask queue
//!   is empty (the end of a microtask checkpoint), calls the realm's unhandled-rejection function
//!   for those that still have none. With [`RejectionHooks`] installed they join the realm's
//!   *about-to-be-notified* list here instead of being reported at once, and the embedder is
//!   asked to queue a task for each ([`RejectionHooks::schedule_notify`]). (HTML queues one task
//!   per checkpoint for that checkpoint's promises. JavaScriptCore names the promises one by one,
//!   so each gets a task: they are queued together, in order, which keeps every promise's place
//!   among the tasks queued before and after its checkpoint.)
//! - The task calls [`Scope::notify_rejected_promise`]: a promise still on the list fires
//!   `unhandledrejection` ([`RejectionHooks::unhandled`]), is reported unless that was canceled,
//!   and, if it has no handler by then, joins the *outstanding rejected promises* weak set.
//! - The tracker callback's Handle operation (a handler first added to a rejected promise) takes
//!   the promise off the about-to-be-notified list (it is never reported), or off the outstanding
//!   set, in which case the embedder queues a task that fires `rejectionhandled`
//!   ([`RejectionHooks::handled`]).
//!
//! The lists hold what the specification's hold: the about-to-be-notified promises strongly (a
//! task is queued for them), the outstanding ones weakly (`JSWeak`), swept as the set grows.
//! Without the patch (the system framework, an older archive) nothing here runs and rejections
//! are reported as before, when the microtask queue empties.

use crate::realm::{RealmInner, Scope, UnhandledRejectionHandler};
use crate::{sys, JsException, Persistent, Value};

/// What the embedder does for a realm's rejected promises ([`crate::Realm::set_rejection_hooks`]).
#[derive(Clone, Copy)]
pub struct RejectionHooks {
    /// A promise joined the about-to-be-notified list as entry `id`: queue a task that calls
    /// [`Scope::notify_rejected_promise`] with it in this realm.
    pub schedule_notify: for<'s> fn(&Scope<'s>, u64),
    /// Fires `unhandledrejection` for (promise, reason); true when it was canceled.
    pub unhandled: UnhandledRejectionHandler,
    /// A handler was added to (promise, reason), reported as unhandled before: queue a task that
    /// fires `rejectionhandled`. Called from inside promise internals: it may not run JavaScript.
    pub handled: for<'s> fn(&Scope<'s>, Persistent, Persistent),
}

thread_local! {
    /// How many promises the realms of this thread have on their lists: the Handle operation of
    /// any other promise (every `then` on a rejected promise without a handler, every `await` of
    /// one) returns on this count alone.
    static TRACKED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[derive(Default)]
pub(crate) struct Rejections {
    hooks: Option<RejectionHooks>,
    /// The about-to-be-notified list: (entry id, promise, reason).
    pending: Vec<(u64, Persistent, Persistent)>,
    next_id: u64,
    /// The promise whose `unhandledrejection` is being dispatched, and whether it got a handler
    /// meanwhile.
    notifying: Option<(sys::JSValueRef, bool)>,
    /// The outstanding rejected promises weak set.
    outstanding: Vec<sys::JSWeakRef>,
    /// The size at which the dead entries of `outstanding` are swept next.
    sweep_at: usize,
    /// This realm's part of [`TRACKED`].
    counted: usize,
}

impl Rejections {
    pub(crate) fn set_hooks(&mut self, hooks: RejectionHooks) {
        self.hooks = Some(hooks);
    }

    /// Brings [`TRACKED`] up to date with the lists, after any change of them.
    fn count(&mut self) {
        let now = self.pending.len() + self.outstanding.len() + usize::from(self.notifying.is_some());
        let before = std::mem::replace(&mut self.counted, now);
        TRACKED.with(|tracked| tracked.set(tracked.get() + now - before));
    }

    /// Whether rejections go through the lists (hooks installed and the tracker available).
    pub(crate) fn tracked(&self) -> bool {
        self.hooks.is_some() && available()
    }

    /// Drops everything (the realm is disposed): no task finds a promise to report.
    pub(crate) fn clear(&mut self, ctx: sys::JSContextRef) {
        self.hooks = None;
        self.pending.clear();
        self.notifying = None;
        let outstanding = std::mem::take(&mut self.outstanding);
        self.count();
        if let (Some(release), false) = (sys::spi::get().weak_release, ctx.is_null()) {
            // SAFETY: the realm's context is live while it is disposed; the handles are this set's own.
            let group = unsafe { sys::JSContextGetGroup(ctx) };
            for weak in outstanding {
                unsafe { release(group, weak) };
            }
        }
    }
}

/// Whether this JavaScriptCore has the promise rejection tracker (patch 0036).
pub fn available() -> bool {
    let spi = sys::spi::get();
    spi.set_promise_rejection_tracker.is_some()
        && spi.weak_create.is_some()
        && spi.weak_release.is_some()
        && spi.weak_get_object.is_some()
}

/// Installs the tracker callback on a new realm's global.
pub(crate) fn install(ctx: sys::JSGlobalContextRef) {
    if !available() {
        return;
    }
    if let Some(set) = sys::spi::get().set_promise_rejection_tracker {
        // SAFETY: the realm's live global context; the callback finds the realm by the context
        // it is called with. Handle alone: JavaScriptCore keeps the rejected promises itself.
        unsafe { set(ctx, Some(tracker), 1 << HANDLE) };
    }
}

/// A promise JavaScriptCore found without a handler when the microtask queue emptied joins the
/// about-to-be-notified list. False when the realm does not track rejections (the caller reports
/// it as before).
pub(crate) fn about_to_notify<'s>(scope: &Scope<'s>, promise: Value<'s>, reason: Value<'s>) -> bool {
    let schedule = {
        let mut state = scope.realm.rejections.borrow_mut();
        if !state.tracked() {
            return false;
        }
        let id = state.next_id;
        state.next_id += 1;
        state.pending.push((id, Persistent::new(scope, promise), Persistent::new(scope, reason)));
        state.count();
        state.hooks.map(|hooks| (hooks.schedule_notify, id))
    };
    if let Some((schedule, id)) = schedule {
        schedule(scope, id);
    }
    true
}

/// The tracker's Handle operation.
const HANDLE: std::os::raw::c_int = 1;

/// The tracker callback. Only Handle matters here: JavaScriptCore keeps the rejected promises
/// itself until the microtask queue empties.
unsafe extern "C" fn tracker(
    ctx: sys::JSContextRef,
    promise: sys::JSObjectRef,
    reason: sys::JSValueRef,
    operation: std::os::raw::c_int,
) {
    if operation != HANDLE || TRACKED.with(std::cell::Cell::get) == 0 {
        return;
    }
    let Some(realm) = crate::realm::realm_for_ctx(ctx) else { return };
    handle(ctx, &realm, promise, reason);
}

fn handle(ctx: sys::JSContextRef, realm: &std::rc::Rc<RealmInner>, promise: sys::JSObjectRef, reason: sys::JSValueRef) {
    let raw = promise as sys::JSValueRef;
    let handled = {
        let Ok(mut state) = realm.rejections.try_borrow_mut() else { return };
        if !state.tracked() {
            return;
        }
        if let Some((current, handled)) = state.notifying.as_mut() {
            if *current == raw {
                *handled = true;
                return;
            }
        }
        if let Some(at) = state.pending.iter().position(|(_, p, _)| p.as_raw() == raw) {
            state.pending.remove(at);
            state.count();
            return;
        }
        let spi = sys::spi::get();
        let (Some(get), Some(release)) = (spi.weak_get_object, spi.weak_release) else { return };
        // SAFETY: the handles are this set's own, of this realm's VM.
        let Some(at) = state.outstanding.iter().position(|&weak| unsafe { get(weak) } == promise) else { return };
        let weak = state.outstanding.swap_remove(at);
        unsafe { release(sys::JSContextGetGroup(ctx), weak) };
        state.count();
        state.hooks.map(|hooks| hooks.handled)
    };
    let Some(handled) = handled else { return };
    // SAFETY: JavaScriptCore passes the live promise and its reason, with the lock held.
    let (Some(promise), Some(reason)) = (unsafe { Persistent::from_raw(ctx, raw) }, unsafe { Persistent::from_raw(ctx, reason) })
    else {
        return;
    };
    handled(&Scope::new(ctx, realm), promise, reason);
}

impl<'s> Scope<'s> {
    /// HTML "notify about rejected promises", the task's part, for the entry `id` of the
    /// about-to-be-notified list: if the promise is still on it (no handler was added since), it
    /// fires `unhandledrejection`, is reported unless that was canceled, and joins the outstanding
    /// rejected promises if it has no handler by then.
    pub fn notify_rejected_promise(&self, id: u64) {
        let (hooks, promise, reason) = {
            let mut state = self.realm.rejections.borrow_mut();
            let Some(hooks) = state.hooks else { return };
            let Some(at) = state.pending.iter().position(|(entry, _, _)| *entry == id) else { return };
            let (_, promise, reason) = state.pending.remove(at);
            state.notifying = Some((promise.as_raw(), false));
            state.count();
            (hooks, promise, reason)
        };
        let (promise_value, reason_value) = (promise.get(self), reason.get(self));
        let canceled = match (hooks.unhandled)(self, promise_value, reason_value) {
            Ok(canceled) => canceled,
            Err(error) => {
                self.report_exception(&error);
                false
            }
        };
        if !canceled {
            if let Some(h) = self.realm.engine.hooks() {
                let details = JsException::from_value(self, reason_value);
                h.unhandled_rejection(&self.realm(), &details);
            }
        }
        let mut state = self.realm.rejections.borrow_mut();
        let handled = state.notifying.take().is_some_and(|(_, handled)| handled);
        if !handled && state.hooks.is_some() {
            self.add_outstanding(&mut state, promise.as_raw());
        }
        state.count();
    }

    fn add_outstanding(&self, state: &mut Rejections, promise: sys::JSValueRef) {
        let spi = sys::spi::get();
        let (Some(create), Some(get), Some(release)) = (spi.weak_create, spi.weak_get_object, spi.weak_release) else {
            return;
        };
        // SAFETY: this scope's live context; `promise` is a live promise object of its VM, and the
        // handles swept are this set's own.
        unsafe {
            let group = sys::JSContextGetGroup(self.ctx);
            if state.outstanding.len() >= state.sweep_at.max(64) {
                state.outstanding.retain(|&weak| {
                    let live = !get(weak).is_null();
                    if !live {
                        release(group, weak);
                    }
                    live
                });
                state.sweep_at = state.outstanding.len() * 2;
            }
            state.outstanding.push(create(group, promise as sys::JSObjectRef));
        }
    }
}
