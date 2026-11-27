//! MutationObserver bookkeeping (DOM §4.3). Records are queued natively during
//! tree operations; bindings drain them in a microtask. When no observer is
//! registered anywhere, queueing is a single map-emptiness check.

use markup5ever::{LocalName, Namespace, QualName};
use rustc_hash::FxHashMap;

use crate::dom::Dom;
use crate::error::{DomError, DomResult};
use crate::node::{NodeId, NIL};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ObserverId(u32);

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RecordKind {
    Attributes,
    CharacterData,
    ChildList,
}

#[derive(Debug, Clone)]
pub struct MutationRecord {
    pub kind: RecordKind,
    pub target: NodeId,
    pub added: Vec<NodeId>,
    pub removed: Vec<NodeId>,
    pub previous_sibling: Option<NodeId>,
    pub next_sibling: Option<NodeId>,
    pub attribute_name: Option<LocalName>,
    pub attribute_namespace: Option<Namespace>,
    pub old_value: Option<String>,
}

/// `MutationObserverInit` as passed from JS (members omitted = `None`).
#[derive(Debug, Clone, Default)]
pub struct ObserveInit {
    pub child_list: bool,
    pub attributes: Option<bool>,
    pub character_data: Option<bool>,
    pub subtree: bool,
    pub attribute_old_value: Option<bool>,
    pub character_data_old_value: Option<bool>,
    pub attribute_filter: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
struct Options {
    child_list: bool,
    attributes: bool,
    character_data: bool,
    subtree: bool,
    attribute_old_value: bool,
    character_data_old_value: bool,
    attribute_filter: Option<Vec<LocalName>>,
}

#[derive(Debug, Clone)]
struct Registration {
    observer: u32,
    opts: Options,
    /// Source node index for transient registrations.
    transient_source: u32,
}

#[derive(Default)]
struct Observer {
    alive: bool,
    /// Creation order (ids are reused): "notify mutation observers" runs the callbacks in it.
    seq: u64,
    records: Vec<MutationRecord>,
    /// Nodes holding (non-transient or transient) registrations for us.
    nodes: Vec<u32>,
}

#[derive(Default)]
pub(crate) struct MutationState {
    regs: FxHashMap<u32, Vec<Registration>>,
    observers: Vec<Observer>,
    free: Vec<u32>,
    /// The creation sequence number of the last observer.
    last_seq: u64,
    /// Observers with pending records awaiting the notify microtask.
    pending: Vec<u32>,
    /// Count of queued-record references per node index.
    pinned: FxHashMap<u32, u32>,
}

impl MutationState {
    /// Whether any node has an observer registration: no record is queued
    /// without one.
    pub(crate) fn any_registration(&self) -> bool {
        !self.regs.is_empty()
    }
    // Both run per node of a collected tree: no lookup in an empty table.
    pub(crate) fn pins(&self, idx: u32) -> bool {
        !self.pinned.is_empty() && self.pinned.contains_key(&idx)
    }
    pub(crate) fn forget_node(&mut self, idx: u32) {
        if self.regs.is_empty() {
            return;
        }
        if let Some(regs) = self.regs.remove(&idx) {
            for r in regs {
                if let Some(o) = self.observers.get_mut(r.observer as usize) {
                    o.nodes.retain(|&n| n != idx);
                }
            }
        }
    }
    fn pin(&mut self, idx: u32) {
        if idx != NIL {
            *self.pinned.entry(idx).or_insert(0) += 1;
        }
    }
}

impl Dom {
    pub fn create_observer(&mut self) -> ObserverId {
        let m = &mut self.mutation;
        m.last_seq += 1;
        let o = Observer { alive: true, seq: m.last_seq, ..Default::default() };
        let id = match m.free.pop() {
            Some(i) => {
                m.observers[i as usize] = o;
                i
            }
            None => {
                m.observers.push(o);
                (m.observers.len() - 1) as u32
            }
        };
        ObserverId(id)
    }

    /// `MutationObserver.observe(target, options)`.
    pub fn observe(&mut self, obs: ObserverId, target: NodeId, init: ObserveInit) -> DomResult<()> {
        let t = self.check(target)?;
        let mut attributes = init.attributes;
        let mut character_data = init.character_data;
        if init.attribute_old_value.is_some() || init.attribute_filter.is_some() {
            attributes.get_or_insert(true);
        }
        if init.character_data_old_value.is_some() {
            character_data.get_or_insert(true);
        }
        let attributes = attributes.unwrap_or(false);
        let character_data = character_data.unwrap_or(false);
        if !init.child_list && !attributes && !character_data {
            return Err(DomError::Type(
                "The options object must set at least one of 'attributes', 'characterData', or 'childList' to true.",
            ));
        }
        if init.attribute_old_value == Some(true) && !attributes {
            return Err(DomError::Type("attributeOldValue requires attributes"));
        }
        if init.attribute_filter.is_some() && !attributes {
            return Err(DomError::Type("attributeFilter requires attributes"));
        }
        if init.character_data_old_value == Some(true) && !character_data {
            return Err(DomError::Type("characterDataOldValue requires characterData"));
        }
        let opts = Options {
            child_list: init.child_list,
            attributes,
            character_data,
            subtree: init.subtree,
            attribute_old_value: init.attribute_old_value.unwrap_or(false),
            character_data_old_value: init.character_data_old_value.unwrap_or(false),
            attribute_filter: init.attribute_filter.map(|v| v.iter().map(|s| LocalName::from(&**s)).collect()),
        };
        let m = &mut self.mutation;
        let o = m.observers.get_mut(obs.0 as usize).filter(|o| o.alive).ok_or(DomError::Type("observer is gone"))?;
        let regs = m.regs.entry(t).or_default();
        if let Some(existing) = regs.iter_mut().find(|r| r.observer == obs.0 && r.transient_source == NIL) {
            // Re-observing replaces options and drops transient registrations
            // whose source is this node.
            existing.opts = opts;
            let observer = obs.0;
            // The transient registrations whose source is this node, and those made from them
            // (a node removed from a removed subtree): all stop observing.
            let mut sources = vec![t];
            let mut i = 0;
            while i < sources.len() {
                let source = sources[i];
                i += 1;
                let holders: Vec<u32> = m
                    .regs
                    .iter()
                    .filter(|(_, list)| list.iter().any(|r| r.observer == observer && r.transient_source == source))
                    .map(|(&node, _)| node)
                    .collect();
                for node in holders {
                    if let Some(list) = m.regs.get_mut(&node) {
                        list.retain(|r| !(r.observer == observer && r.transient_source == source));
                    }
                    if !sources.contains(&node) {
                        sources.push(node);
                    }
                }
            }
            m.regs.retain(|_, list| !list.is_empty());
        } else {
            regs.push(Registration { observer: obs.0, opts, transient_source: NIL });
            o.nodes.push(t);
        }
        Ok(())
    }

    /// `MutationObserver.disconnect()`: drops registrations and records.
    pub fn disconnect_observer(&mut self, obs: ObserverId) {
        let records = {
            let m = &mut self.mutation;
            let Some(o) = m.observers.get_mut(obs.0 as usize) else { return };
            let nodes = std::mem::take(&mut o.nodes);
            let records = std::mem::take(&mut o.records);
            for n in nodes {
                if let Some(list) = m.regs.get_mut(&n) {
                    list.retain(|r| r.observer != obs.0);
                    if list.is_empty() {
                        m.regs.remove(&n);
                    }
                }
            }
            records
        };
        self.unpin_records(&records);
    }

    /// Called when the JS MutationObserver object is finalized.
    pub fn drop_observer(&mut self, obs: ObserverId) {
        self.disconnect_observer(obs);
        let m = &mut self.mutation;
        if let Some(o) = m.observers.get_mut(obs.0 as usize) {
            if o.alive {
                o.alive = false;
                m.pending.retain(|&p| p != obs.0);
                m.free.push(obs.0);
            }
        }
    }

    /// `MutationObserver.takeRecords()`. Node ids in the records stay valid
    /// until the next `collect_garbage`; wrap them (add_ref) before that.
    pub fn take_records(&mut self, obs: ObserverId) -> Vec<MutationRecord> {
        let records = match self.mutation.observers.get_mut(obs.0 as usize) {
            Some(o) => std::mem::take(&mut o.records),
            None => return Vec::new(),
        };
        self.unpin_records(&records);
        records
    }

    /// Drains the "mutation observers to notify" list. Bindings call this from
    /// the notify microtask, then for each observer: `clear_transient`,
    /// `take_records`, and invoke the callback if non-empty.
    pub fn take_pending_observers(&mut self) -> Vec<ObserverId> {
        // "Notify mutation observers": the observers are notified in the order they were created,
        // not the order in which they got their first record.
        let mut pending = std::mem::take(&mut self.mutation.pending);
        let observers = &self.mutation.observers;
        pending.sort_by_key(|&id| observers.get(id as usize).map_or(u64::MAX, |o| o.seq));
        pending.into_iter().map(ObserverId).collect()
    }
    /// Snapshot only newly queued observer ids for binding lifetime roots.
    pub fn pending_observers_from(&self, start: usize) -> Vec<ObserverId> {
        self.mutation.pending.get(start..).unwrap_or_default().iter().copied().map(ObserverId).collect()
    }
    pub fn has_pending_mutations(&self) -> bool {
        !self.mutation.pending.is_empty()
    }
    /// Whether any observer was queued after the first `start` entries
    /// (what [`Dom::pending_observers_from`] would return), without building
    /// the list. Runs after every native callback, so it must stay O(1).
    pub fn has_pending_observers_from(&self, start: usize) -> bool {
        self.mutation.pending.len() > start
    }

    /// Removes the transient registrations of `obs` (notify step 3.2).
    pub fn clear_transient(&mut self, obs: ObserverId) {
        let m = &mut self.mutation;
        let Some(o) = m.observers.get_mut(obs.0 as usize) else { return };
        let regs = &mut m.regs;
        o.nodes.retain(|n| {
            let Some(list) = regs.get_mut(n) else { return false };
            list.retain(|r| !(r.observer == obs.0 && r.transient_source != NIL));
            let keep = list.iter().any(|r| r.observer == obs.0);
            if list.is_empty() {
                regs.remove(n);
            }
            keep
        });
    }

    fn unpin_records(&mut self, records: &[MutationRecord]) {
        let unpin = |id: Option<NodeId>, dom: &mut Dom| {
            if let Some(id) = id {
                if let Some(c) = dom.mutation.pinned.get_mut(&id.idx) {
                    *c -= 1;
                    if *c == 0 {
                        dom.mutation.pinned.remove(&id.idx);
                        dom.release_candidate(id.idx);
                    }
                }
            }
        };
        for r in records {
            unpin(Some(r.target), self);
            for &a in r.added.iter().chain(r.removed.iter()) {
                unpin(Some(a), self);
            }
            unpin(r.previous_sibling, self);
            unpin(r.next_sibling, self);
        }
    }

    // ----- queueing (called from tree operations) -----------------------

    pub(crate) fn add_transient_observers(&mut self, parent: u32, node: u32) {
        if self.mutation.regs.is_empty() {
            return;
        }
        let mut anc = parent;
        let mut add = Vec::new();
        while anc != NIL {
            if let Some(list) = self.mutation.regs.get(&anc) {
                for r in list.iter().filter(|r| r.opts.subtree) {
                    add.push(Registration { observer: r.observer, opts: r.opts.clone(), transient_source: anc });
                }
            }
            anc = self.n(anc).parent;
        }
        if add.is_empty() {
            return;
        }
        let m = &mut self.mutation;
        for r in &add {
            if let Some(o) = m.observers.get_mut(r.observer as usize) {
                if !o.nodes.contains(&node) {
                    o.nodes.push(node);
                }
            }
            // "Remove": the mutation observer microtask is queued, so that the notify step
            // drops this observer's transient registrations even if no record is queued.
            if !m.pending.contains(&r.observer) {
                m.pending.push(r.observer);
            }
        }
        m.regs.entry(node).or_default().extend(add);
    }

    fn interested(&self, target: u32, kind: RecordKind, attr: Option<&QualName>) -> Vec<(u32, bool)> {
        let mut out: Vec<(u32, bool)> = Vec::new();
        let mut node = target;
        while node != NIL {
            if let Some(list) = self.mutation.regs.get(&node) {
                for r in list {
                    let o = &r.opts;
                    if node != target && !o.subtree {
                        continue;
                    }
                    let (ok, want_old) = match kind {
                        RecordKind::Attributes => {
                            let filtered = match (&o.attribute_filter, attr) {
                                (Some(f), Some(a)) => a.ns != markup5ever::ns!() || !f.contains(&a.local),
                                _ => false,
                            };
                            (o.attributes && !filtered, o.attribute_old_value)
                        }
                        RecordKind::CharacterData => (o.character_data, o.character_data_old_value),
                        RecordKind::ChildList => (o.child_list, false),
                    };
                    if !ok {
                        continue;
                    }
                    match out.iter_mut().find(|(ob, _)| *ob == r.observer) {
                        Some(e) => e.1 |= want_old,
                        None => out.push((r.observer, want_old)),
                    }
                }
            }
            node = self.n(node).parent;
        }
        out
    }

    fn enqueue(&mut self, interested: Vec<(u32, bool)>, mut make: impl FnMut(bool) -> MutationRecord) {
        for (obs, want_old) in interested {
            let rec = make(want_old);
            let pins: Vec<u32> = std::iter::once(rec.target.idx)
                .chain(rec.added.iter().map(|n| n.idx))
                .chain(rec.removed.iter().map(|n| n.idx))
                .chain(rec.previous_sibling.map(|n| n.idx))
                .chain(rec.next_sibling.map(|n| n.idx))
                .collect();
            let m = &mut self.mutation;
            for p in pins {
                m.pin(p);
            }
            if let Some(o) = m.observers.get_mut(obs as usize) {
                o.records.push(rec);
                if !m.pending.contains(&obs) {
                    m.pending.push(obs);
                    self.touch_hook();
                }
            }
        }
    }

    pub(crate) fn queue_attribute(&mut self, target: u32, name: &QualName, old: Option<&str>) {
        if self.mutation.regs.is_empty() {
            return;
        }
        let interested = self.interested(target, RecordKind::Attributes, Some(name));
        if interested.is_empty() {
            return;
        }
        let tid = self.id_of(target);
        self.enqueue(interested, |want_old| MutationRecord {
            kind: RecordKind::Attributes,
            target: tid,
            added: Vec::new(),
            removed: Vec::new(),
            previous_sibling: None,
            next_sibling: None,
            attribute_name: Some(name.local.clone()),
            attribute_namespace: (name.ns != markup5ever::ns!()).then(|| name.ns.clone()),
            old_value: if want_old { old.map(str::to_owned) } else { None },
        });
    }

    pub(crate) fn queue_character_data(&mut self, target: u32, old: &str) {
        if self.mutation.regs.is_empty() {
            return;
        }
        let interested = self.interested(target, RecordKind::CharacterData, None);
        if interested.is_empty() {
            return;
        }
        let tid = self.id_of(target);
        self.enqueue(interested, |want_old| MutationRecord {
            kind: RecordKind::CharacterData,
            target: tid,
            added: Vec::new(),
            removed: Vec::new(),
            previous_sibling: None,
            next_sibling: None,
            attribute_name: None,
            attribute_namespace: None,
            old_value: want_old.then(|| old.to_owned()),
        });
    }

    pub(crate) fn queue_child_list(&mut self, target: u32, added: &[u32], removed: &[u32], prev: u32, next: u32) {
        if self.mutation.regs.is_empty() {
            return;
        }
        let interested = self.interested(target, RecordKind::ChildList, None);
        if interested.is_empty() {
            return;
        }
        let tid = self.id_of(target);
        let added: Vec<NodeId> = added.iter().map(|&i| self.id_of(i)).collect();
        let removed: Vec<NodeId> = removed.iter().map(|&i| self.id_of(i)).collect();
        let prev = self.opt_id(prev);
        let next = self.opt_id(next);
        self.enqueue(interested, |_| MutationRecord {
            kind: RecordKind::ChildList,
            target: tid,
            added: added.clone(),
            removed: removed.clone(),
            previous_sibling: prev,
            next_sibling: next,
            attribute_name: None,
            attribute_namespace: None,
            old_value: None,
        });
    }
}
