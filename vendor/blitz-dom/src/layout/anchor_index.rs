use crate::{BaseDocument, NodeId};
use std::collections::HashMap;
use std::ops::Range;
use style::values::DashedIdent;

type Name = (Option<NodeId>, DashedIdent);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Exposure { Ordered, InFlow(Option<NodeId>) }

#[derive(Default)]
pub(crate) struct AnchorIndex {
    order: Option<Order>,
    groups: HashMap<Name, HashMap<(Option<NodeId>, Exposure), Bucket>>,
    pub(crate) work: std::cell::Cell<u64>,
}

fn tick(work: &std::cell::Cell<u64>) {
    work.set(work.get().saturating_add(1));
}

struct Order {
    boundaries: HashMap<Name, Vec<(usize, Option<NodeId>)>>,
    dom: HashMap<NodeId, usize>,
    boxes: HashMap<NodeId, Range<usize>>,
    islands: HashMap<NodeId, Option<NodeId>>,
    ids: Vec<NodeId>,
}

impl Order {
    fn new(doc: &BaseDocument, work: &std::cell::Cell<u64>) -> Self {
        let mut dom = HashMap::new();
        let mut ids = Vec::new();
        let mut boundaries: HashMap<Name, Vec<_>> = HashMap::new();
        let mut active: HashMap<Name, Vec<NodeId>> = HashMap::new();
        let mut stack = vec![(doc.root_node().id, false)];
        while let Some((id, exit)) = stack.pop() {
            tick(work);
            let node = &doc.nodes[id];
            if !exit && dom.contains_key(&id) {
                continue;
            }
            if let Some(values) = node.primary_styles() {
                let scope = &values.get_box().anchor_scope;
                if !scope.value.is_none() {
                    let root = scope.scope.get_shadow_root_for_scoped(node).map(|root| root.id);
                    for name in scope.value.names() {
                        let key = (root, DashedIdent(name.clone()));
                        let path = active.entry(key.clone()).or_default();
                        if exit { path.pop(); } else { path.push(id); }
                        boundaries.entry(key).or_default().push((ids.len(), path.last().copied()));
                        tick(work);
                    }
                }
            }
            if exit { continue; }
            dom.insert(id, ids.len());
            ids.push(id);
            stack.push((id,true));
            // pyro: `::before` precedes and `::after` follows the children in tree order (css-anchor-position-1
            // §anchor-name: pseudo-elements are ordered like their originating element's box tree position).
            let (before, after) = (node.before(), node.after());
            stack.extend(after.into_iter().chain(node.children.iter().rev().copied()).chain(before).map(|child|(child,false)));
        }
        let mut children: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        let mut roots = Vec::new();
        let mut claims = HashMap::new();
        for (id, node) in doc.nodes.iter() {
            tick(work);
            if !dom.contains_key(&id) {
                dom.insert(id, ids.len());
                ids.push(id);
            }
            let mut parent = node.layout_parent.get();
            if node.is_out_of_flow() {
                let fixed = node.taffy_position() == taffy::Position::Fixed;
                let mut cursor = node.layout_parent.get();
                let mut path = Vec::new();
                while let Some(id) = cursor {
                    tick(work);
                    if let Some(&cached) = claims.get(&(id, fixed)) {
                        cursor = cached;
                        break;
                    }
                    path.push(id);
                    let ancestor = &doc.nodes[id];
                    if ancestor.is_root_element()
                        || ancestor.primary_styles().is_some_and(|_| {
                            taffy::CoreStyle::is_containing_block(&ancestor.layout_style())
                                .for_position(node.taffy_position())
                        })
                    {
                        break;
                    }
                    cursor = ancestor.layout_parent.get();
                }
                for ancestor in path {
                    claims.insert((ancestor, fixed), cursor);
                }
                parent = if fixed && doc.is_in_top_layer(id) { Some(doc.root_element().id) } else { cursor };
            }
            if let Some(parent) = parent {
                children.entry(parent).or_default().push(id);
            } else {
                roots.push((id, false, None));
            }
        }
        let mut boxes: HashMap<NodeId, Range<usize>> = HashMap::new();
        let mut clock = 0;
        let mut islands = HashMap::new();
        while let Some((id, exit, inherited)) = roots.pop() {
            tick(work);
            if exit {
                boxes.get_mut(&id).unwrap().end = clock;
                continue;
            }
            if boxes.contains_key(&id) {
                continue;
            }
            boxes.insert(id, clock..clock + 1);
            clock += 1;
            let island = if doc.nodes[id].is_out_of_flow() { Some(id) } else { inherited };
            islands.insert(id, island);
            roots.push((id, true, island));
            if let Some(kids) = children.remove(&id) {
                roots.extend(kids.into_iter().map(|id| (id, false, island)));
            }
        }
        Self { dom, boxes, ids, boundaries, islands }
    }
}

struct Bucket {
    positions: Vec<usize>,
    ranks: Vec<Vec<usize>>,
}

impl Bucket {
    fn new(mut entries: Vec<(usize, usize)>, work: &std::cell::Cell<u64>) -> Self {
        entries.sort_unstable_by(|a, b| {
            tick(work);
            a.cmp(b)
        });
        let count = entries.len();
        let positions = entries.iter().map(|entry| entry.0).collect();
        let mut ranks = vec![Vec::new(); count * 2];
        for (index, (_, rank)) in entries.into_iter().enumerate() {
            ranks[count + index].push(rank);
        }
        for index in (1..count).rev() {
            let mut left = ranks[index * 2].iter().copied().peekable();
            let mut right = ranks[index * 2 + 1].iter().copied().peekable();
            let mut merged = Vec::with_capacity(left.len() + right.len());
            while let (Some(a), Some(b)) = (left.peek(), right.peek()) {
                tick(work);
                merged.push(if a <= b { left.next().unwrap() } else { right.next().unwrap() });
            }
            merged.extend(left);
            merged.extend(right);
            ranks[index] = merged;
        }
        Self { positions, ranks }
    }

    fn best(&self, interval: Range<usize>, before: usize, work: &std::cell::Cell<u64>) -> Option<usize> {
        let count = self.positions.len();
        let mut left = count
            + self.positions.partition_point(|&pos| {
                tick(work);
                pos < interval.start
            });
        let mut right = count
            + self.positions.partition_point(|&pos| {
                tick(work);
                pos < interval.end
            });
        let mut best = None;
        let mut take = |index: usize| {
            let ranks = &self.ranks[index];
            if let Some(last) = ranks
                .partition_point(|&rank| {
                    tick(work);
                    rank < before
                })
                .checked_sub(1)
            {
                best = best.max(Some(ranks[last]));
            }
        };
        while left < right {
            if left % 2 != 0 {
                take(left);
                left += 1;
            }
            if right % 2 != 0 {
                right -= 1;
                take(right);
            }
            left /= 2;
            right /= 2;
        }
        best
    }
}

impl AnchorIndex {
    pub(crate) fn boundary_entries(&self) -> usize {
        self.order.as_ref().map_or(0, |order| order.boundaries.values().map(Vec::len).sum())
    }

    pub(crate) fn clear(&mut self) {
        self.order = None;
        self.groups.clear();
    }

    pub(super) fn lookup(
        &mut self,
        doc: &BaseDocument,
        key: Name,
        origin: NodeId,
        target: NodeId,
        container: NodeId,
    ) -> Option<NodeId> {
        let work = &self.work;
        let order = self.order.get_or_insert_with(|| Order::new(doc, work));
        let candidates = doc.named_anchor_nodes.get(&key)?;
        let events = [order.boundaries.get(&key), order.boundaries.get(&(key.0, DashedIdent("all".into())))];
        let boundary = |start| {
            let rank = order.dom[&start];
            events.iter().flatten().filter_map(|events| {
                let index = events.partition_point(|(at,_)| { tick(work); *at <= rank });
                events.get(index.checked_sub(1)? )?.1
            }).max_by_key(|id| order.dom[id])
        };
        let buckets = self.groups.entry(key).or_insert_with(|| {
            let mut entries: HashMap<_, Vec<_>> = HashMap::new();
            for &id in candidates {
                doc.anchor_lookup_visits.set(doc.anchor_lookup_visits.get().saturating_add(1));
                let node = &doc.nodes[id];
                if !node.flags.is_in_document() && !node.is_anonymous() { continue; }
                if !node.primary_styles().is_some_and(|s| !s.get_box().display.is_none()) {
                    continue;
                }
                let geometry_root = if node.is_non_atomic_inline() {
                    node.inline_root_ancestor().map_or(id, |root| root.id)
                } else {
                    id
                };
                if let Some(span) = order.boxes.get(&geometry_root) {
                    let boundary = boundary(id);
                    let entry = (span.start, order.dom[&id]);
                    entries.entry((boundary, Exposure::Ordered)).or_default().push(entry);
                    if !node.is_out_of_flow() {
                        entries.entry((boundary, Exposure::InFlow(order.islands[&geometry_root]))).or_default().push(entry);
                    }
                }
            }
            entries.into_iter().map(|(key, values)| (key, Bucket::new(values, work))).collect()
        });
        let full = order.boxes.get(&container)?;
        let exclude = order.boxes.get(&target).filter(|span| full.contains(&span.start));
        let boundary = boundary(origin);
        let mut best = None;
        for (exposure, limit) in [(Exposure::InFlow(order.islands[&container]), usize::MAX), (Exposure::Ordered, *order.dom.get(&origin)?)] {
            if let Some(bucket) = buckets.get(&(boundary, exposure)) {
                if let Some(span) = exclude {
                    best = best.max(bucket.best(full.start..span.start, limit, work)).max(bucket.best(
                        span.end..full.end,
                        limit,
                        work,
                    ));
                } else {
                    best = best.max(bucket.best(full.clone(), limit, work));
                }
            }
        }
        best.map(|rank| order.ids[rank])
    }
}
