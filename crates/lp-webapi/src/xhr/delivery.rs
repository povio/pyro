//! Network-thread buffering; only IDs cross into queued browser tasks.
use super::*;

pub(super) struct Delivery {
    frontier: NetworkDelivery,
    pub head: Option<ResponseHead>,
    pub chunks: Vec<Bytes>,
    pub bytes: usize,
    pub terminal: Option<Result<(), NetError>>,
    pub closed: bool,
    scheduled: bool,
    last_progress: Option<Instant>,
    budget: Arc<AtomicUsize>,
}
impl Delivery {
    pub fn new(budget: Arc<AtomicUsize>) -> Self {
        Self {
            frontier: NetworkDelivery::default(),
            head: None,
            chunks: Vec::new(),
            bytes: 0,
            terminal: None,
            closed: false,
            scheduled: false,
            last_progress: None,
            budget,
        }
    }
    pub fn clear(&mut self) {
        self.budget.fetch_sub(self.bytes, Ordering::AcqRel);
        self.bytes = 0;
        self.chunks = Vec::new();
    }
    pub fn close(&mut self) {
        self.closed = true;
        self.head = None;
        self.clear();
    }
}
impl Drop for Delivery {
    fn drop(&mut self) {
        self.clear();
    }
}

pub(super) fn receive(
    wire: &Arc<Mutex<Delivery>>,
    page: u64,
    id: RequestId,
    poster: &RemotePoster,
    limits: XhrLimits,
    event: ResponseEvent,
) {
    let wake = {
        let mut wire = wire.lock().unwrap();
        if wire.closed {
            return;
        }
        let notify = match event {
            ResponseEvent::Redirect { .. } => false,
            ResponseEvent::Response(head) => {
                wire.frontier.response = true;
                wire.head = Some(head);
                true
            }
            ResponseEvent::Data(bytes) if bytes.is_empty() => false,
            ResponseEvent::Data(bytes) => {
                wire.frontier.data_bytes = wire.frontier.data_bytes.saturating_add(bytes.len() as u64);
                let fits = wire.chunks.len() < limits.max_body_chunks
                    && wire.bytes.saturating_add(bytes.len()) <= limits.max_body_bytes;
                let reserved = fits
                    && wire
                        .budget
                        .try_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                            n.checked_add(bytes.len()).filter(|n| *n <= limits.max_total_body_bytes)
                        })
                        .is_ok();
                if !reserved {
                    wire.terminal = Some(Err(NetError::TooLarge));
                    wire.closed = true;
                    // Keep already-visible bytes and their budget charge until owner delivery.
                    true
                } else {
                    wire.bytes += bytes.len();
                    wire.chunks.push(bytes);
                    let notify = wire.last_progress.is_none_or(|t| t.elapsed() >= Duration::from_millis(50));
                    if notify {
                        wire.last_progress = Some(Instant::now());
                    }
                    notify
                }
            }
            ResponseEvent::Finished { .. } => {
                wire.frontier.terminal = true;
                wire.terminal = Some(Ok(()));
                wire.closed = true;
                true
            }
            ResponseEvent::Failed(error) => {
                wire.frontier.terminal = true;
                wire.terminal = Some(Err(error));
                wire.closed = true;
                // Owner delivery clears buffers after making the failure visible.
                true
            }
        };
        if notify && !wire.scheduled {
            wire.scheduled = true;
            true
        } else {
            false
        }
    };
    if wake {
        poster(Box::new(move || pump(page, id)));
    }
}

fn pump(page_id: u64, id: RequestId) {
    let Some(page) = PAGES.with(|p| p.borrow().get(&page_id).and_then(Weak::upgrade)) else { return };
    let Some(job) = page.xhr_state.jobs.borrow().get(&id).cloned() else { return };
    let xhr = &job.xhr;
    let generation = xhr.generation.get();
    if xhr.active.get() != Some(id) {
        return;
    }
    let Some(realm) = xhr.realm.upgrade() else {
        xhr.dispose();
        return;
    };
    let Ok(scope) = realm.scope() else {
        xhr.dispose();
        return;
    };
    let Some(wire) = xhr.wire.borrow().clone() else { return };
    let (head, bytes, terminal, chunks, frontier) = {
        let mut wire = wire.lock().unwrap();
        wire.scheduled = false;
        (wire.head.take(), wire.bytes, wire.terminal.clone(), wire.chunks.len(), wire.frontier)
    };
    xhr.visible_chunks.set(chunks);
    page.network_delivery(id, frontier);
    if !xhr.current(generation, id) {
        return;
    }
    if let Some(head) = head {
        *xhr.head.borrow_mut() = Some(head);
        xhr.ready.set(2);
        xhr.fire("readystatechange", false, 0);
        if !xhr.current(generation, id) {
            return;
        }
    }
    if let Some(Err(_)) = terminal {
        xhr.fail("error");
        return;
    }
    let progressed = bytes > 0 && bytes != xhr.reported.replace(bytes);
    if progressed {
        xhr.ready.set(3);
        xhr.fire("readystatechange", false, bytes);
        if !xhr.current(generation, id) {
            return;
        }
        xhr.fire("progress", true, bytes);
        if !xhr.current(generation, id) {
            return;
        }
    }
    if terminal.is_some() {
        if !progressed {
            xhr.fire("progress", true, bytes);
        }
        if !xhr.current(generation, id) {
            return;
        }
        xhr.active.set(None);
        xhr.sent.set(false);
        xhr.ready.set(4);
        xhr.clear_timer(&scope);
        let finished = page.xhr_state.jobs.borrow_mut().remove(&id);
        drop(finished);
        xhr.fire("readystatechange", false, bytes);
        if xhr.generation.get() != generation {
            return;
        }
        xhr.fire("load", true, bytes);
        if xhr.generation.get() != generation {
            return;
        }
        xhr.fire("loadend", true, bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_data_wakes_and_later_progress_coalesces_with_bounded_buffers() {
        let budget = Arc::new(AtomicUsize::new(0));
        let wire = Arc::new(Mutex::new(Delivery::new(budget.clone())));
        let wakes = Arc::new(AtomicUsize::new(0));
        let counter = wakes.clone();
        let poster: RemotePoster = Arc::new(move |_| {
            counter.fetch_add(1, Ordering::Relaxed);
        });
        let limits = XhrLimits { max_body_chunks: 3, ..Default::default() };
        let data = || receive(&wire, 0, RequestId(0), &poster, limits, ResponseEvent::Data(Bytes::from_static(b"x")));
        data();
        assert_eq!(wakes.load(Ordering::Relaxed), 1, "first body bytes must wake even immediately after headers");
        {
            let mut wire = wire.lock().unwrap();
            wire.scheduled = false;
            wire.last_progress = Some(Instant::now() + Duration::from_secs(60));
        }
        data();
        assert_eq!(wakes.load(Ordering::Relaxed), 1);
        wire.lock().unwrap().last_progress = Some(Instant::now() - Duration::from_millis(51));
        data();
        assert_eq!(wakes.load(Ordering::Relaxed), 2);
        assert_eq!(budget.load(Ordering::Acquire), 3);
        data();
        assert_eq!(budget.load(Ordering::Acquire), 3, "visible bytes remain charged until owner cleanup");
        assert!(matches!(wire.lock().unwrap().terminal, Some(Err(NetError::TooLarge))));
        let frontier = wire.lock().unwrap().frontier;
        assert_eq!(
            frontier,
            NetworkDelivery { response: false, data_bytes: 4, terminal: false },
            "quota is not transport terminal"
        );
        data();
        assert_eq!(wakes.load(Ordering::Relaxed), 2, "terminal notification coalesces and late data is ignored");
        wire.lock().unwrap().close();
        assert_eq!(budget.load(Ordering::Acquire), 0, "owner cleanup refunds retained bytes");
    }
}
