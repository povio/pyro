//! Asymmetric public-key derivation. Only owned key bytes travel to the
//! background worker. Promise handles stay on the requesting JS thread.
use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use pyro_js::{Deferred, JsResult, Persistent, RealmId, Scope, Value, WeakRealm};
use rustc_hash::FxHashMap;

use super::{buffer, operation_error, public_material};

type Work = Box<dyn FnOnce() + Send>;
static NEXT: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy)]
struct Owner {
    page: u64,
    realm: RealmId,
}

impl Owner {
    fn matches(self, page: u64, realm: Option<RealmId>) -> bool {
        self.page == page && realm.is_none_or(|id| self.realm == id)
    }
}

#[derive(Default)]
struct Queue {
    running: bool,
    jobs: VecDeque<(Owner, Work)>,
}

struct Pending {
    owner: Owner,
    realm: WeakRealm,
    deferred: Deferred,
    wrap: Persistent,
}

thread_local! {
    static QUEUE: Arc<Mutex<Queue>> = Arc::default();
    static PENDING: RefCell<FxHashMap<u64, Pending>> = RefCell::default();
}

// One transient worker per requesting thread, irrespective of the number of
// concurrent requests. It exits when the queue empties.
fn submit(owner: Owner, job: Work) -> std::io::Result<()> {
    QUEUE.with(|shared| {
        let mut queue = shared.lock().unwrap();
        if !queue.running {
            let worker = Arc::clone(shared);
            let thread = std::thread::Builder::new().name("pyro-public-key".into()).spawn(move || loop {
                let job = {
                    let mut queue = worker.lock().unwrap();
                    let Some((_, job)) = queue.jobs.pop_front() else {
                        queue.running = false;
                        break;
                    };
                    job
                };
                job();
            })?;
            crate::worker::track_thread(thread);
            queue.running = true;
        }
        queue.jobs.push_back((owner, job));
        Ok(())
    })
}

fn finish(id: u64, public: Option<Vec<u8>>) {
    let Some(pending) = PENDING.with(|pending| pending.borrow_mut().remove(&id)) else { return };
    let Some(realm) = pending.realm.upgrade() else { return };
    let Ok(s) = realm.scope() else { return };
    let result = (|| {
        let bytes = public.ok_or_else(|| operation_error("Could not derive the public key"))?;
        let wrap =
            pending.wrap.get(&s).as_object().ok_or_else(|| operation_error("Public key packaging unavailable"))?;
        wrap.call(None, &[buffer(&s, &bytes)?])
    })();
    match result {
        Ok(value) => {
            let _ = pending.deferred.resolve(&s, value);
        }
        Err(error) => {
            let _ = pending.deferred.reject(&s, error.to_value(&s));
        }
    }
}

pub(super) fn request<'s>(
    s: &Scope<'s>,
    kind: String,
    curve: String,
    material: Vec<u8>,
    wrap: Value<'s>,
) -> JsResult<Value<'s>> {
    let page = crate::target::page_of(s)?;
    let deferred = s.deferred()?;
    let promise = deferred.promise.get(s);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let owner = Owner { page: page.id(), realm: s.realm_id() };
    let pending = Pending { owner, realm: s.realm().downgrade(), deferred, wrap: Persistent::new(s, wrap) };
    PENDING.with(|p| p.borrow_mut().insert(id, pending));
    let derive = move || public_material(&kind, &curve, &material).ok();
    if let Some(post) = page.thread_poster().filter(|_| !crate::worker::is_held()) {
        // The thread poster also runs after a document navigation, so even a
        // closed realm can release its outstanding handles.
        if submit(
            owner,
            Box::new(move || {
                let public = std::panic::catch_unwind(derive).ok().flatten();
                post(Box::new(move || finish(id, public)));
            }),
        )
        .is_err()
        {
            finish(id, None);
        }
    } else {
        // Fork templates must remain single-threaded. They and minimal embedders
        // without a cross-thread dispatcher use their owner task queue.
        page.post_task(Box::new(move || finish(id, derive())));
    }
    Ok(promise)
}

/// Drop roots when their realm/page is detached. Late deliveries use globally
/// unique IDs and simply find no matching request.
pub(crate) fn cancel(page: u64, realm: Option<RealmId>) {
    let removed = PENDING.with(|p| {
        let mut pending = p.borrow_mut();
        let ids: Vec<_> = pending.iter().filter(|(_, v)| v.owner.matches(page, realm)).map(|(&id, _)| id).collect();
        ids.into_iter().filter_map(|id| pending.remove(&id)).collect::<Vec<_>>()
    });
    if !removed.is_empty() {
        QUEUE.with(|q| q.lock().unwrap().jobs.retain(|(owner, _)| !owner.matches(page, realm)));
    }
}

pub(crate) fn thread_leftover() -> Option<&'static str> {
    if PENDING.with(|p| !p.borrow().is_empty()) {
        return Some("public_key::PENDING");
    }
    QUEUE.with(|q| {
        let queue = q.lock().unwrap();
        (queue.running || !queue.jobs.is_empty()).then_some("public_key::QUEUE")
    })
}
