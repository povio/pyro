//! Browser-context Web Locks state. The mutex protects queue transitions and
//! the order of notices in each client's inbox. Posting tasks and invoking JS
//! happen after it is unlocked, on the receiving agent's thread.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::thread::ThreadId;

use rustc_hash::FxHashMap;

use crate::page::RemotePoster;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct StorageKey {
    pub origin: String,
    pub top_level_site: Option<String>,
    pub cross_site_ancestor: bool,
    /// Embedders without a cross-thread wakeup route can participate only
    /// with other local-only clients on their own thread.
    pub local_thread: Option<ThreadId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mode {
    Exclusive,
    Shared,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Exclusive => "exclusive",
            Self::Shared => "shared",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Notice {
    Grant(u64),
    Stolen(u64),
}

pub(super) struct Client {
    pub page: u64,
    pub thread: ThreadId,
    pub poster: Option<RemotePoster>,
    inbox: Mutex<VecDeque<Notice>>,
}

impl Client {
    pub fn new(page: u64, poster: Option<RemotePoster>) -> Self {
        Self { page, thread: std::thread::current().id(), poster, inbox: Default::default() }
    }

    pub fn next_notice(&self) -> Option<Notice> {
        self.inbox.lock().unwrap().pop_front()
    }

    fn notify(self: &Arc<Self>, notice: Notice, wake: &mut Vec<Arc<Self>>) {
        self.inbox.lock().unwrap().push_back(notice);
        wake.push(self.clone());
    }
}

pub(super) struct Request {
    pub id: u64,
    pub mode: Mode,
    pub client_id: String,
    pub client: Arc<Client>,
}

#[derive(Default)]
struct Queue {
    held: Vec<Request>,
    pending: VecDeque<Request>,
}

impl Queue {
    fn grantable(&self, mode: Mode) -> bool {
        self.held.is_empty() || (mode == Mode::Shared && self.held.iter().all(|r| r.mode == Mode::Shared))
    }

    fn process(&mut self, wake: &mut Vec<Arc<Client>>) {
        while self.pending.front().is_some_and(|r| self.grantable(r.mode)) {
            let req = self.pending.pop_front().unwrap();
            req.client.notify(Notice::Grant(req.id), wake);
            self.held.push(req);
        }
    }

    fn empty(&self) -> bool {
        self.held.is_empty() && self.pending.is_empty()
    }
}

type Manager = FxHashMap<String, Queue>;
pub(super) type Rows = Vec<(String, &'static str, String)>;

#[derive(Clone, Default)]
pub(crate) struct LockService(Arc<Mutex<FxHashMap<StorageKey, Manager>>>);

impl LockService {
    pub(super) fn acquire(
        &self,
        key: StorageKey,
        name: String,
        req: Request,
        steal: bool,
        if_available: bool,
    ) -> (bool, Vec<Arc<Client>>) {
        let mut managers = self.0.lock().unwrap();
        let queue = managers.entry(key).or_default().entry(name).or_default();
        let mut wake = Vec::new();
        if steal {
            for held in queue.held.drain(..) {
                held.client.notify(Notice::Stolen(held.id), &mut wake);
            }
            queue.pending.push_front(req);
        } else if if_available && !(queue.pending.is_empty() && queue.grantable(req.mode)) {
            return (false, wake);
        } else {
            queue.pending.push_back(req);
        }
        queue.process(&mut wake);
        (true, wake)
    }

    /// A release removes a held request. An abort, checked on its owner
    /// thread before delivering the grant, may remove a waiting one too.
    pub(super) fn remove(&self, key: &StorageKey, name: &str, id: u64, abort: bool) -> (bool, Vec<Arc<Client>>) {
        let mut managers = self.0.lock().unwrap();
        let Some(manager) = managers.get_mut(key) else { return (false, vec![]) };
        let Some(queue) = manager.get_mut(name) else { return (false, vec![]) };
        if let Some(i) = queue.held.iter().position(|r| r.id == id) {
            queue.held.remove(i);
        } else if let Some(i) = queue.pending.iter().position(|r| abort && r.id == id) {
            queue.pending.remove(i);
        } else {
            return (false, vec![]);
        }
        let mut wake = Vec::new();
        queue.process(&mut wake);
        if queue.empty() {
            manager.remove(name);
        }
        if manager.is_empty() {
            managers.remove(key);
        }
        (true, wake)
    }

    pub(super) fn query(&self, key: &StorageKey) -> (Rows, Rows) {
        let managers = self.0.lock().unwrap();
        let mut rows = (Vec::new(), Vec::new());
        if let Some(manager) = managers.get(key) {
            let mut names: Vec<_> = manager.keys().collect();
            names.sort();
            for name in names {
                let queue = &manager[name];
                let info = |r: &Request| (name.clone(), r.mode.as_str(), r.client_id.clone());
                rows.0.extend(queue.held.iter().map(info));
                rows.1.extend(queue.pending.iter().map(info));
            }
        }
        rows
    }

    pub(super) fn dispose(&self, page: u64) -> Vec<Arc<Client>> {
        let mut wake = Vec::new();
        let mut managers = self.0.lock().unwrap();
        managers.retain(|_, manager| {
            manager.retain(|_, queue| {
                queue.held.retain(|r| r.client.page != page);
                queue.pending.retain(|r| r.client.page != page);
                queue.process(&mut wake);
                !queue.empty()
            });
            !manager.is_empty()
        });
        wake
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> StorageKey {
        StorageKey {
            origin: "https://example.test".into(),
            top_level_site: Some("https://example.test".into()),
            cross_site_ancestor: false,
            local_thread: None,
        }
    }

    fn request(id: u64, client: &Arc<Client>, mode: Mode) -> Request {
        Request { id, mode, client_id: format!("client-{}", client.page), client: client.clone() }
    }

    #[test]
    fn queues_are_shared_across_threads_and_exclusive_waiters_keep_their_turn() {
        let service = LockService::default();
        let document = Arc::new(Client::new(1, None));
        assert!(service.acquire(key(), "resource".into(), request(1, &document, Mode::Shared), false, false).0);
        assert_eq!(document.next_notice(), Some(Notice::Grant(1)));
        let worker_service = service.clone();
        let worker = std::thread::spawn(move || {
            let worker = Arc::new(Client::new(2, None));
            assert!(
                worker_service.acquire(key(), "resource".into(), request(2, &worker, Mode::Exclusive), false, false).0
            );
            assert_eq!(worker.next_notice(), None);
            worker
        })
        .join()
        .unwrap();
        assert!(!service.acquire(key(), "resource".into(), request(3, &document, Mode::Shared), false, true).0);
        assert!(service.acquire(key(), "resource".into(), request(4, &document, Mode::Shared), false, false).0);
        let (held, pending) = service.query(&key());
        assert_eq!(held.len(), 1);
        assert_eq!(
            pending.iter().map(|r| (r.1, r.2.as_str())).collect::<Vec<_>>(),
            [("exclusive", "client-2"), ("shared", "client-1")]
        );
        assert!(service.remove(&key(), "resource", 1, false).0);
        assert_eq!(worker.next_notice(), Some(Notice::Grant(2)));
        assert_eq!(document.next_notice(), None);
        let wake = service.dispose(2);
        assert_eq!(wake.len(), 1);
        assert_eq!(document.next_notice(), Some(Notice::Grant(4)));
        service.dispose(1);
        assert!(service.0.lock().unwrap().is_empty());
    }

    #[test]
    fn steal_and_abort_preserve_notification_order_and_drop_pending_requests() {
        let service = LockService::default();
        let client = Arc::new(Client::new(1, None));
        for id in [1, 2] {
            assert!(service.acquire(key(), "resource".into(), request(id, &client, Mode::Exclusive), false, false).0);
        }
        assert!(service.acquire(key(), "resource".into(), request(3, &client, Mode::Exclusive), true, false).0);
        assert_eq!(client.next_notice(), Some(Notice::Grant(1)));
        assert_eq!(client.next_notice(), Some(Notice::Stolen(1)));
        assert_eq!(client.next_notice(), Some(Notice::Grant(3)));
        assert!(!service.remove(&key(), "resource", 1, false).0);
        assert!(service.remove(&key(), "resource", 2, true).0);
        assert!(service.remove(&key(), "resource", 3, false).0);
        assert_eq!(client.next_notice(), None);
        assert!(service.0.lock().unwrap().is_empty());
    }

    #[test]
    fn posterless_embedders_do_not_admit_requests_into_other_threads_queues() {
        let service = LockService::default();
        let local_key = StorageKey { local_thread: Some(std::thread::current().id()), ..key() };
        let local = Arc::new(Client::new(1, None));
        service.acquire(local_key.clone(), "k".into(), request(1, &local, Mode::Exclusive), false, false);
        let remote_service = service.clone();
        std::thread::spawn(move || {
            let other = Arc::new(Client::new(2, None));
            let other_key = StorageKey { local_thread: Some(std::thread::current().id()), ..key() };
            for scope in [other_key, key()] {
                assert!(remote_service.query(&scope).0.is_empty());
                assert!(remote_service.acquire(scope, "k".into(), request(2, &other, Mode::Exclusive), false, true).0);
            }
            remote_service.dispose(2);
        }).join().unwrap();
        assert_eq!(service.query(&local_key).0.len(), 1);
        assert_eq!(local.next_notice(), Some(Notice::Grant(1)));
        assert_eq!(local.next_notice(), None);
        service.dispose(1);
    }

    #[test]
    fn context_origin_top_level_site_and_cross_site_ancestor_bound_the_manager() {
        let service = LockService::default();
        let client = Arc::new(Client::new(1, None));
        assert!(service.acquire(key(), "resource".into(), request(1, &client, Mode::Exclusive), false, false).0);
        for other in [
            StorageKey { origin: "https://other.test".into(), ..key() },
            StorageKey { top_level_site: Some("https://other.test".into()), ..key() },
            StorageKey { cross_site_ancestor: true, ..key() },
        ] {
            assert!(service.query(&other).0.is_empty());
            assert!(service.acquire(other, "resource".into(), request(2, &client, Mode::Exclusive), false, true).0);
        }
        assert!(LockService::default().query(&key()).0.is_empty());
    }
}
