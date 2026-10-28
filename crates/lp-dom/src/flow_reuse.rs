//! Subtree reuse across flow passes. Laying out an element's contents (its
//! children in its content box) is a function of the subtree (DOM, styles),
//! the element's used style and its constraints (content width, definite
//! height, the containing block heights, fixed context, relative shift),
//! and is otherwise invariant under translation. A pass records, for every
//! element whose contents it lays out, the order and text ranges the
//! contents produced; the next pass, when the subtree is clean (not in the
//! dirty set of `MeasureCache::invalidate`) and the constraints match,
//! copies those ranges from the previous result, translated and renumbered,
//! instead of laying the subtree out again. Measuring probes use the same
//! entries (and copy nothing when only the height is read).
//!
//! Ancestors change a subtree's boxes after its contents are laid out in
//! three ways, all undone here: uniform shifts (table cell centring,
//! bottom-anchored absolute boxes), which the anchor's position cancels;
//! `clipped_by` and `positioned` set by clipping / positioned ancestors
//! outside the subtree, which are told apart by order (the slots keep each
//! box's own positioning and its nearest positioned ancestor).

use std::rc::Rc;

use rustc_hash::{FxHashMap, FxHashSet};

use super::{Collapse, Ctx, FlowResult, FlowStyle, Margin, NodeId, Pending, Rect, Slot, StyleRef};

/// The entries of the last pass (generation `gen - 1`, read from `prev`)
/// and of the pass in progress (generation `gen`).
#[derive(Default)]
pub(super) struct Reuse {
    gen: u64,
    prev: Option<Rc<FlowResult>>,
    entries: FxHashMap<NodeId, Entry>,
}

impl Reuse {
    pub(super) fn clear(&mut self) {
        self.prev = None;
        self.entries.clear();
    }

    /// A pass ran and its result was kept (nothing cleared since).
    pub(super) fn has_previous(&self) -> bool {
        self.prev.is_some()
    }

    pub(super) fn keep(&mut self, result: Rc<FlowResult>) {
        self.prev = Some(result);
    }

    /// Drops the entries of changed elements and their ancestors.
    pub(super) fn invalidate(&mut self, dirty: &FxHashSet<NodeId>) {
        self.entries.retain(|k, _| !dirty.contains(k));
    }

    pub(super) fn begin_pass(&mut self) {
        if self.prev.is_none() {
            self.entries.clear();
        }
        self.gen += 1;
    }

    /// Keeps the entries recorded (or replayed) by the pass just run; they
    /// describe its result, which the owner hands back with `keep`.
    pub(super) fn end_pass(&mut self, relaid: bool) {
        self.prev = None;
        if relaid {
            self.entries.clear();
        } else {
            let gen = self.gen;
            self.entries.retain(|_, e| e.gen == gen);
        }
    }

    fn usable(&self, el: NodeId) -> Option<(&Rc<FlowResult>, &Entry)> {
        let prev = self.prev.as_ref()?;
        self.entries.get(&el).filter(|e| e.gen + 1 == self.gen).map(|e| (prev, e))
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }
}

/// The constraints an element's contents were laid out under.
#[derive(Clone, Copy, PartialEq)]
struct Key {
    width: u64,
    height: Option<u64>,
    /// The containing block height of the element itself (`own_cb_height`).
    outer_cb: Option<u64>,
    fixed: bool,
    shift: (u64, u64),
    collapse: Collapse,
}

/// A box, run or text of the subtree whose position fixes its translation.
#[derive(Clone, Copy)]
enum Anchor {
    None,
    /// Slot index.
    Box(usize),
    Run(usize),
    Text(usize),
}

/// An absolutely or fixed positioned descendant handed to a containing
/// block outside the subtree, with its static position relative to the
/// contents' origin.
struct Escaped {
    el: NodeId,
    style: StyleRef,
    dx: f64,
    dy: f64,
}

#[derive(Clone)]
struct Entry {
    gen: u64,
    key: Key,
    style: StyleRef,
    /// Orders `order_lo + 1 ..= order_hi` (slots `order_lo..order_hi`);
    /// `order_lo` is the element's own order.
    order_lo: u32,
    order_hi: u32,
    texts: (usize, usize),
    runs: (usize, usize),
    anchor: Anchor,
    /// The anchor's position relative to the contents' (x, y) when laid out.
    anchor_local: (f64, f64),
    height: f64,
    /// Baselines of the first and last line boxes, relative to y.
    first_baseline: Option<f64>,
    last_baseline: Option<f64>,
    /// The margin left below the last child (`Ctx::trailing`).
    trailing: Margin,
    escaped: Rc<[Escaped]>,
}

impl<'a> Ctx<'a> {
    /// `contents_in`, replayed from the previous pass when it laid this
    /// subtree out under the same constraints, and remembered for the next
    /// pass when this is the pass itself (not a probe).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn contents_reused(
        &mut self,
        el: NodeId,
        style: &FlowStyle,
        x: f64,
        y: f64,
        width: f64,
        height: Option<f64>,
        fixed: bool,
        shift: (f64, f64),
        outer_cb: Option<f64>,
        collapse: Collapse,
    ) -> f64 {
        let key = Key {
            width: width.to_bits(),
            height: height.map(f64::to_bits),
            outer_cb: outer_cb.map(f64::to_bits),
            fixed,
            shift: (shift.0.to_bits(), shift.1.to_bits()),
            collapse,
        };
        if let Some(h) = self.replay(el, style, key, x, y) {
            return h;
        }
        if self.probe {
            return self.contents_in(el, style, x, y, width, height, fixed, shift, collapse);
        }
        let order_lo = self.order;
        let (text_lo, run_lo) = (self.out.text_orders.len(), self.out.text_runs.len());
        let depth = self.pending.len();
        let first_lo = self.pending.first().map_or(0, Vec::len);
        let last_lo = self.pending.last().map_or(0, Vec::len);
        let (fb, lb) = (self.first_baseline.take(), self.last_baseline.take());
        let h = self.contents_in(el, style, x, y, width, height, fixed, shift, collapse);
        let trailing = if collapse.bottom { self.trailing } else { Margin::ZERO };
        let (first_baseline, last_baseline) = (self.first_baseline, self.last_baseline);
        self.first_baseline = fb.or(first_baseline);
        self.last_baseline = last_baseline.or(lb);
        let rel = |p: &Pending| Escaped { el: p.el, style: p.style.clone(), dx: p.static_x - x, dy: p.static_y - y };
        let escaped: Rc<[Escaped]> = match self.pending.len() {
            0 => Rc::from([]),
            d if d != depth => return h,
            1 => self.pending[0][first_lo..].iter().map(rel).collect(),
            d => self.pending[0][first_lo..].iter().chain(&self.pending[d - 1][last_lo..]).map(rel).collect(),
        };
        let texts = (text_lo, self.out.text_orders.len());
        let runs = (run_lo, self.out.text_runs.len());
        let (anchor, at) = self.anchor_in(&self.out, order_lo, self.order, texts, runs);
        let entry = Entry {
            gen: 0,
            key,
            style: self.shared_style(el, style),
            order_lo,
            order_hi: self.order,
            texts,
            runs,
            anchor,
            anchor_local: at.map_or((0.0, 0.0), |(ax, ay)| (ax - x, ay - y)),
            height: h,
            first_baseline: first_baseline.map(|b| b - y),
            last_baseline: last_baseline.map(|b| b - y),
            trailing,
            escaped,
        };
        self.remember(el, entry);
        h
    }

    /// The memoized style when `style` is it (the common case), so entries
    /// share it; else a copy (flex items, cells and atomics pass adjusted
    /// sizes).
    fn shared_style(&self, el: NodeId, style: &FlowStyle) -> StyleRef {
        if let Some(Some(s)) = self.memo.styles.borrow().get(&el) {
            if std::ptr::eq(s.as_ref(), style) || **s == *style {
                return s.clone();
            }
        }
        Rc::new(style.clone())
    }

    fn remember(&self, el: NodeId, mut entry: Entry) {
        let mut reuse = self.memo.reuse.borrow_mut();
        entry.gen = reuse.gen;
        if let Some(old) = reuse.entries.insert(el, entry) {
            if old.gen == reuse.gen {
                // Laid out twice in one pass: ranges overlap, keep nothing.
                self.memo.relaid.set(true);
            }
        }
    }

    /// The first box, run or text recorded in a range, and its position.
    fn anchor_in(
        &self,
        out: &FlowResult,
        order_lo: u32,
        order_hi: u32,
        texts: (usize, usize),
        runs: (usize, usize),
    ) -> (Anchor, Option<(f64, f64)>) {
        for i in order_lo as usize..(order_hi as usize).min(out.slots.len()) {
            if let Some(b) = out.slots[i].el.and_then(|el| out.boxes.get(&el)).filter(|b| b.order as usize == i + 1) {
                return (Anchor::Box(i), Some((b.rect.x, b.rect.y)));
            }
        }
        if runs.0 < runs.1 {
            let r = out.text_runs[runs.0].rect;
            return (Anchor::Run(runs.0), Some((r.x, r.y)));
        }
        if texts.0 < texts.1 {
            if let Some((r, _)) = out.texts.get(&out.text_orders[texts.0].1) {
                return (Anchor::Text(texts.0), Some((r.x, r.y)));
            }
        }
        (Anchor::None, None)
    }

    fn anchor_at(out: &FlowResult, anchor: Anchor) -> Option<(f64, f64)> {
        let r = match anchor {
            Anchor::None => return None,
            Anchor::Box(i) => out.boxes.get(&out.slots.get(i)?.el?)?.rect,
            Anchor::Run(i) => out.text_runs.get(i)?.rect,
            Anchor::Text(i) => out.texts.get(&out.text_orders.get(i)?.1)?.0,
        };
        Some((r.x, r.y))
    }

    /// Copies the previous pass's layout of `el`'s contents when it was laid
    /// out under `key` with an equal style. Returns the content height.
    fn replay(&mut self, el: NodeId, style: &FlowStyle, key: Key, x: f64, y: f64) -> Option<f64> {
        let (prev, e) = {
            let reuse = self.memo.reuse.borrow();
            let (prev, e) = reuse.usable(el)?;
            if e.key != key || !(std::ptr::eq(e.style.as_ref(), style) || *e.style == *style) {
                return None;
            }
            (prev.clone(), e.clone())
        };
        let (tx, ty) = match Self::anchor_at(&prev, e.anchor) {
            Some((ax, ay)) => (x + e.anchor_local.0 - ax, y + e.anchor_local.1 - ay),
            None => (0.0, 0.0),
        };
        let order_lo = self.order;
        let delta = order_lo as i64 - e.order_lo as i64;
        let renumber = |o: u32| (o as i64 + delta) as u32;
        let inner = |o: u32| if o > e.order_lo { renumber(o) } else { 0 };
        let moved = |r: Rect| Rect { x: r.x + tx, y: r.y + ty, ..r };
        let (text_lo, run_lo) = (self.out.text_orders.len(), self.out.text_runs.len());
        if !self.measure_only {
            for i in e.order_lo as usize..e.order_hi as usize {
                let slot = prev.slots[i];
                let Some(d) = slot.el else { continue };
                let Some(b) = prev.boxes.get(&d).filter(|b| b.order as usize == i + 1) else { continue };
                let positioned_by = inner(slot.positioned_by);
                let nb = super::FlowBox {
                    rect: moved(b.rect),
                    order: renumber(b.order),
                    clipped_by: inner(b.clipped_by),
                    positioned: slot.own_positioned || positioned_by != 0,
                    ..*b
                };
                self.put_box(d, nb, Slot { el: Some(d), own_positioned: slot.own_positioned, positioned_by });
                if prev.sticky.contains(&d) {
                    self.out.sticky.insert(d);
                }
            }
            for &(o, t) in &prev.text_orders[e.texts.0..e.texts.1] {
                if let Some(&(r, f)) = prev.texts.get(&t) {
                    self.out.texts.insert(t, (moved(r), f));
                }
                self.out.text_orders.push((renumber(o), t));
            }
            for run in &prev.text_runs[e.runs.0..e.runs.1] {
                let mut run = run.clone();
                run.rect = moved(run.rect);
                run.order = renumber(run.order);
                self.out.text_runs.push(run);
            }
        }
        self.order = renumber(e.order_hi);
        if let Some(b) = e.first_baseline {
            self.first_baseline.get_or_insert(y + b);
        }
        if let Some(b) = e.last_baseline {
            self.last_baseline = Some(y + b);
        }
        self.trailing = e.trailing;
        for p in e.escaped.iter() {
            self.defer_absolute(p.el, p.style.clone(), x + p.dx, y + p.dy);
        }
        if !self.probe {
            self.rebase(&prev, &e, delta, text_lo as i64 - e.texts.0 as i64, run_lo as i64 - e.runs.0 as i64);
            let shift_anchor = |a: Anchor| match a {
                Anchor::None => Anchor::None,
                Anchor::Box(i) => Anchor::Box((i as i64 + delta) as usize),
                Anchor::Run(i) => Anchor::Run(run_lo + (i - e.runs.0)),
                Anchor::Text(i) => Anchor::Text(text_lo + (i - e.texts.0)),
            };
            let entry = Entry {
                order_lo,
                order_hi: self.order,
                texts: (text_lo, self.out.text_orders.len()),
                runs: (run_lo, self.out.text_runs.len()),
                anchor: shift_anchor(e.anchor),
                ..e.clone()
            };
            self.remember(el, entry);
        }
        Some(e.height)
    }

    /// Moves the previous pass's entries of the elements inside a replayed
    /// subtree to this pass, renumbered, so a later change inside it only
    /// lays out the changed path again.
    fn rebase(&self, prev: &FlowResult, e: &Entry, delta: i64, text_delta: i64, run_delta: i64) {
        let mut reuse = self.memo.reuse.borrow_mut();
        let gen = reuse.gen;
        let at = |v: usize, d: i64| (v as i64 + d) as usize;
        for slot in &prev.slots[e.order_lo as usize..e.order_hi as usize] {
            let Some(d) = slot.el else { continue };
            let Some(de) = reuse.entries.get_mut(&d) else { continue };
            let inside = de.order_lo >= e.order_lo
                && de.order_hi <= e.order_hi
                && de.texts.0 >= e.texts.0
                && de.texts.1 <= e.texts.1
                && de.runs.0 >= e.runs.0
                && de.runs.1 <= e.runs.1;
            if de.gen + 1 != gen || !inside {
                continue;
            }
            de.gen = gen;
            de.order_lo = (de.order_lo as i64 + delta) as u32;
            de.order_hi = (de.order_hi as i64 + delta) as u32;
            de.texts = (at(de.texts.0, text_delta), at(de.texts.1, text_delta));
            de.runs = (at(de.runs.0, run_delta), at(de.runs.1, run_delta));
            de.anchor = match de.anchor {
                Anchor::None => Anchor::None,
                Anchor::Box(i) => Anchor::Box(at(i, delta)),
                Anchor::Run(i) => Anchor::Run(at(i, run_delta)),
                Anchor::Text(i) => Anchor::Text(at(i, text_delta)),
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn end_pass_keeps_only_this_pass() {
        let mut r = Reuse::default();
        r.begin_pass();
        assert_eq!(r.len(), 0);
        r.end_pass(true);
        assert_eq!(r.len(), 0);
    }
}
