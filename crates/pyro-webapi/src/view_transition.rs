//! CSS View Transitions (`document.startViewTransition`, `Element.startViewTransition`): the
//! per-scope lifecycle and captured rendering state, created on first use.

use std::{rc::Rc, sync::{Arc, Mutex}};

use pyro_js::{JsError, JsResult, Scope, Value};

use crate::page::PageDom;

type Boxes = Arc<[pyro_dom::geometry::TransitionBox]>;
pub(crate) type Captures = rustc_hash::FxHashMap<u64, Capture>;

#[derive(Default, Debug)]
pub(crate) struct Quota {
    used: Mutex<Usage>,
}

#[derive(Default, Debug)]
struct Usage {
    slots: usize,
    pixels: u64,
    work: u64,
    failures: [u64; 2],
    window: Option<std::time::Instant>,
}

#[derive(Debug)]
struct Reservation {
    scope: bool,
    child: bool,
    quota: Arc<Quota>,
    pixels: u64,
    work: u64,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let mut used = self.quota.used.lock().unwrap();
        used.slots -= usize::from(self.scope);
        used.pixels -= self.pixels;
        used.work -= self.work;
    }
}

impl Usage {
    fn refresh(&mut self) {
        let now = std::time::Instant::now();
        if self.window.is_none_or(|start| now.duration_since(start).as_secs() >= 1) {
            self.window = Some(now);
            self.failures = [0; 2];
        }
    }
}

impl pyro_dom::geometry::TransitionLease for Reservation {
    fn debit_failure(&self, spent: u64) {
        let mut used = self.quota.used.lock().unwrap();
        used.refresh();
        let debt = &mut used.failures[usize::from(self.child)];
        *debt = debt.saturating_add(spent).min(if self.child { 3 << 16 } else { 1 << 18 });
    }
}

impl Reservation {
    fn attempt(&self, prep: u64, old: bool) -> Option<Self> {
        let mut used = self.quota.used.lock().unwrap();
        used.refresh();
        let work_limit: u64 = if self.child { 3 << 16 } else { 1 << 18 };
        let pixel_limit: u64 = if self.child { 3 << 22 } else { 1 << 24 };
        let work = work_limit.checked_sub(used.work + used.failures.iter().sum::<u64>())?;
        let pixels = pixel_limit.saturating_sub(used.pixels);
        if work <= prep || (old && pixels == 0) { return None; }
        used.work += work;
        used.pixels += pixels;
        Some(Self { scope: false, child: self.child, quota: self.quota.clone(), pixels, work })
    }

    fn failed(&self, remaining: u64) {
        pyro_dom::geometry::TransitionLease::debit_failure(self, self.work.saturating_sub(remaining));
    }
}

pub(crate) struct Capture {
    reservation: Arc<Reservation>,
    old: Boxes,
    new: Boxes,
    samples: Arc<[(usize, usize, f64)]>,
    tree: Option<(std::rc::Weak<dyn pyro_dom::geometry::GeometrySource>, u64)>,
}

impl Drop for Capture {
    fn drop(&mut self) {
        if let Some((source, root)) = self.tree.take() { if let Some(source) = source.upgrade() { source.clear_transition_boxes(root); } }
    }
}

/// The top-level page above `page` and how many frames up it is, or `None` when a
/// document on the way is detached or disposed (not fully active).
fn top_page(mut page: Rc<PageDom>) -> Option<(Rc<PageDom>, usize)> {
    for depth in 0..64 {
        if page.is_detached() || page.is_disposed() { return None; }
        if page.hooks().is_some() {
            match page.container() {
                crate::page::FrameContainer::Frame(parent, _) => { page = parent; continue; }
                crate::page::FrameContainer::Detached => return None,
                crate::page::FrameContainer::Top => (),
            }
        }
        return Some((page, depth));
    }
    None
}

fn admit(page: Rc<PageDom>, local_slots: usize) -> Option<Capture> {
    let (page, depth) = top_page(page)?;
    let quota = page.transition_quota.borrow_mut().get_or_insert_with(Default::default).clone();
    let mut used = quota.used.lock().unwrap();
    let child = depth > 0;
    if used.slots >= 16 || (child && (used.slots >= 12 || local_slots >= 4)) { return None; }
    used.slots += 1;
    drop(used);
    Some(Capture { reservation: Arc::new(Reservation { scope:true, child, quota, pixels: 0, work: 0 }), old: Default::default(), new: Default::default(), samples: Default::default(), tree: None })
}

pub(crate) static MODULE: crate::lazy_module::LazyModule = crate::lazy_module::LazyModule {
    key: "lpViewTransition",
    source: include_str!("view_transition.js"),
    url: "pyro://internal/view_transition.js",
    natives: Some(natives),
};

fn natives<'s>(s: &Scope<'s>) -> JsResult<pyro_js::Object<'s>> {
    let capture = s.function("captureValid", |s, _, args| {
        let Some(root) = s.unwrap::<crate::node::Node>(args.get(0)) else { return Ok(s.boolean(false)) };
        let page = root.page();
        let key = pyro_dom::snapshot::render_key(root.id);
        let Some(scope) = s.unwrap::<crate::node::Node>(args.get(3)) else { return Ok(s.boolean(false)) };
        let scope_key = pyro_dom::snapshot::render_key(scope.id);
        let mut captures = page.transition_captures.borrow_mut();
        if args.get(2).to_bool() {
            if let Some(map) = captures.as_mut() { map.remove(&scope_key); if map.is_empty() { *captures = None; } }
            return Ok(s.boolean(true));
        }
        let old = args.get(1).to_bool();
        if args.get(4).to_bool() {
            if captures.as_ref().is_some_and(|map| map.contains_key(&scope_key)) { return Ok(s.boolean(false)); }
            let Some(record) = admit(page.clone(), captures.as_ref().map_or(0, |map| map.len())) else { return Ok(s.boolean(false)); };
            captures.get_or_insert_with(Default::default).insert(scope_key, record);
            return Ok(s.boolean(true));
        }
        // The elements the other transitions captured: an element takes part in one transition at a
        // time, so a scope's transition whose old state would capture one of them is skipped, with
        // InvalidStateError (Chrome; WPT css-view-transitions `scoped/collision`).
        let others: rustc_hash::FxHashSet<u64> = if old {
            captures.as_ref().map(|map| map.iter().filter(|(k, _)| **k != scope_key).flat_map(|(_, r)| r.old.iter().chain(r.new.iter()).map(|b| b.key)).collect()).unwrap_or_default()
        } else {
            Default::default()
        };
        let Some(record) = captures.as_mut().and_then(|map| map.get_mut(&scope_key)) else { return Ok(s.boolean(false)); };
        let Some(reservation) = Arc::get_mut(&mut record.reservation) else { return Ok(s.boolean(false)); };
        let prep = u64::from(page.dom.borrow().slot_count()) + 1;
        let Some(mut attempt) = reservation.attempt(prep, old) else { return Ok(s.boolean(false)); };
        let mut work = attempt.work - prep;
        let dom = page.dom.borrow();
        if page.is_hidden() || !dom.is_connected(root.id).unwrap_or(false) { attempt.failed(work); return Ok(s.boolean(false)); }
        drop(dom);
        let Some(source) = pyro_raster::capture::scope(&mut work, |_| {
            let layout = page.layout();
            layout.synced_geometry_source(&page.dom.borrow()).cloned()
        }) else { attempt.failed(work); return Ok(s.boolean(false)); };
        let Some(source) = source else { return Ok(s.boolean(true)); };
        // An element scope (its own transition root) that cannot take layout containment (a
        // non-atomic inline, `display: contents`, an internal table box but a cell) fails both of
        // its captures (Chrome; WPT `scoped/element-scoped-containment-invalid-state`).
        if scope.id == root.id && !source.layout_containment_eligible(key) { attempt.failed(work); return Ok(s.boolean(false)); }
        let result = source.capture_transition(key, old, attempt.pixels, &mut work);
        let Some(boxes) = result else { attempt.failed(work); return Ok(s.boolean(false)); };
        if boxes.iter().any(|item| others.contains(&item.key)) { attempt.failed(work); return Ok(s.boolean(false)); }
        if old {
            reservation.pixels = boxes.iter().filter_map(|item| item.rgba.as_ref()).map(|img| img.len() as u64 / 4).sum();
            attempt.pixels -= reservation.pixels;
            record.old = boxes.into();
        } else {
            let cost = 1 + 4 * (record.old.len() + boxes.len()) as u64;
            let Some(left) = work.checked_sub(cost) else { attempt.failed(0); return Ok(s.boolean(false)); };
            work = left;
            if !source.set_transition_boxes(key, &record.old, &boxes) { attempt.failed(work); return Ok(s.boolean(false)); }
            record.tree = Some((Rc::downgrade(&source), key));
            record.new = boxes.into();
        }
        reservation.work += attempt.work - work;
        attempt.work = work;
        Ok(s.boolean(true))
    })?;
    let animations = s.function("transitionAnimations", |s, _, args| {
        let Some(root) = s.unwrap::<crate::node::Node>(args.get(0)) else { return Ok(s.null()); };
        let page = root.page();
        let key = pyro_dom::snapshot::render_key(root.id);
        let styles = {
            let dom = page.dom.borrow();
            let layout = page.layout();
            layout.synced_geometry_source(&dom).map(|source| source.transition_animation_styles(key)).unwrap_or(Some(Vec::new()))
        };
        let Some(styles) = styles else { return Ok(s.null()); };
        let mut runs = Vec::new();
        for (index,pseudo, style) in styles {
            for run in pyro_dom::timing::AnimationTiming::from_transition_box(&style) {
                runs.push(s.array(&[s.number(index as f64),s.number(run.index as f64),s.string(&pseudo), s.string(&run.name), s.number(run.delay_ms), s.number(run.duration_ms), s.number(run.iterations), s.string(&run.style.direction), s.string(&run.style.fill), s.string(&run.style.easing.as_deref().map_or_else(|| "ease".to_owned(), pyro_dom::timing::shown_easing)), s.boolean(run.style.paused)])?.value());
            }
        }
        let entries = s.array(&runs)?.value();
        if args.get(1).to_bool() { Ok(entries) } else { crate::animations::view_runs(s, &page, args.get(0), entries) }
    })?;
    let out = s.object();
    out.set_atom("captureValid", capture.value())?;
    out.set_atom("transitionAnimations", animations.value())?;
    let sample = s.function("sample",|s,_,args| {
        let Some(root) = s.unwrap::<crate::node::Node>(args.get(0)) else { return Ok(s.boolean(false)); };
        let Some(list) = args.get(1).as_object() else { return Ok(s.boolean(false)); };
        let len = list.get("length")?.to_number()? as u32;
        if len > 3072 || len % 3 != 0 { return Ok(s.boolean(false)); }
        let mut values = Vec::with_capacity((len/3) as usize);
        for offset in (0..len).step_by(3) { values.push((list.get_index(offset)?.to_number()? as usize,list.get_index(offset+1)?.to_number()? as usize,list.get_index(offset+2)?.to_number()?)); }
        let page = root.page();
        let dom = page.dom.borrow();
        let layout = page.layout();
        let key = pyro_dom::snapshot::render_key(root.id);
        let success = layout.synced_geometry_source(&dom).is_none_or(|source|source.set_transition_samples(key,&values));
        if success {
            if let Some(record) = page.transition_captures.borrow_mut().as_mut().and_then(|captures| captures.values_mut().find(|r| r.tree.as_ref().is_some_and(|(_,root)| *root == key))) {
                record.samples = values.into();
            }
        }
        Ok(s.boolean(success))
    })?;
    out.set_atom("sample",sample.value())?;
    // Whether an update callback comes from a realm that is gone (a removed
    // iframe's): it is not run and the transition aborts, as in Chrome.
    let gone = s.function("callbackGone", |s, _, args| {
        Ok(s.boolean(args.get(0).as_object().is_some_and(|callback| s.object_realm_gone(callback))))
    })?;
    out.set_atom("callbackGone", gone.value())?;
    // The state of the scope of an element-scoped view transition (`Dom::set_view_transition_scope`):
    // 0 none, 1 active, 2 active and keeping its size (until its update callback is done). A
    // document scope has none.
    let scope_state = s.function("scopeState", |s, _, args| {
        let Some(scope) = s.unwrap::<crate::node::Node>(args.get(0)) else { return Ok(s.undefined()) };
        let state = match args.get(1).to_number()? as u8 { 1 => Some(false), 2 => Some(true), _ => None };
        let page = scope.page();
        let mut dom = page.dom.borrow_mut();
        if dom.node(scope.id).ok().is_some_and(|node| node.element().is_some()) {
            dom.set_view_transition_scope(scope.id, state);
        }
        Ok(s.undefined())
    })?;
    out.set_atom("scopeState", scope_state.value())?;
    Ok(out)
}

fn unavailable() -> JsError {
    JsError::Type("ViewTransition is unavailable".into())
}

impl PageDom {
    pub fn reserve_transition_job(&self, top_document: &str, frames: &mut [pyro_dom::geometry::TransitionFrame]) {
        if frames.is_empty() || frames.len() > 16 { return; }
        let quota = self.transition_quota.borrow();
        let Some(quota) = quota.as_ref() else { return; };
        for child in [true, false] {
            let mut matching = frames.iter_mut().filter(|frame| (frame.document != top_document) == child).peekable();
            if matching.peek().is_none() { continue; }
            let job = quota.reserve_job(child);
            for frame in matching { frame.job = job.clone(); }
        }
    }
    pub fn transition_frames(&self, document: &str, out: &mut Vec<pyro_dom::geometry::TransitionFrame>) {
        let captures = self.transition_captures.borrow();
        let Some(map) = captures.as_ref() else { return };
        for record in map.values() {
            let Some((_, root)) = record.tree.as_ref() else { continue };
            out.push(pyro_dom::geometry::TransitionFrame { document: document.to_owned(), root: *root,
                old: record.old.clone(), new: record.new.clone(), samples: record.samples.clone(), reservation: record.reservation.clone(), job: None });
        }
    }
}

impl Quota {
    fn reserve_job(self: &Arc<Self>, child: bool) -> Option<Arc<pyro_dom::geometry::TransitionJob>> {
        use pyro_dom::geometry::{TransitionAllowance, TransitionJob};
        let source = Reservation { scope:false, child, quota: self.clone(), pixels:0, work:0 };
        let lease = Arc::new(source.attempt(0, true)?);
        let (work, pixels) = (lease.work, lease.pixels);
        Some(Arc::new(TransitionJob { remaining: Mutex::new(TransitionAllowance { pixels, work }), lease }))
    }
}

#[test]
fn view_transition_overlapping_job_admission_charges_once_until_last_frame_drops() {
    let quota = Arc::new(Quota::default());
    quota.used.lock().unwrap().pixels = 123;
    let frame = quota.reserve_job(false).unwrap();
    assert_eq!(frame.remaining.lock().unwrap().pixels, (1 << 24) - 123);
    let child = frame.clone();
    drop(frame);
    assert_eq!((0..1000).filter_map(|_| quota.reserve_job(false)).count(), 0);
    drop(child);
    assert_eq!(quota.used.lock().unwrap().pixels, 123);
    assert!(quota.reserve_job(false).is_some());
}

#[test]
fn view_transition_failed_child_jobs_cannot_retry_or_deny_top() {
    let quota = Arc::new(Quota::default());
    let child = quota.reserve_job(true).unwrap();
    let cost = child.remaining.lock().unwrap().work;
    child.lease.debit_failure(cost);
    drop(child);
    assert_eq!((0..100).filter_map(|_| quota.reserve_job(true)).count(), 0);
    assert_eq!(quota.reserve_job(false).unwrap().remaining.lock().unwrap().work, (1 << 18) - cost);
}

pub(crate) fn scope_id(node: &crate::node::NodeData) -> pyro_dom::NodeId {
    let dom = node.page().dom.borrow();
    dom.owner_document(node.id).ok().flatten().filter(|&doc| dom.document_element(doc).ok().flatten() == Some(node.id)).unwrap_or(node.id)
}

pub(crate) fn active<'s>(s: &Scope<'s>, node: &crate::node::NodeData) -> JsResult<Value<'s>> {
    let Some(module) = MODULE.existing(s, node.page())? else { return Ok(s.null()) };
    let scope = crate::node::wrap_node(s, node.page(), scope_id(node))?.value();
    module.get("active")?.as_object().ok_or_else(unavailable)?.call(None, &[scope])
}

/// `startViewTransition(callbackOptions)` on `scope` (a document, or an element for a scoped
/// transition), whose `transitionRoot` is `root`. A document that is not fully active (a
/// removed iframe's) returns a transition skipped with AbortError.
pub(crate) fn start<'s>(
    s: &Scope<'s>,
    page: &Rc<PageDom>,
    scope: Value<'s>,
    root: Value<'s>,
    arg: Value<'s>,
) -> JsResult<Value<'s>> {
    let inactive = top_page(page.clone()).is_none();
    MODULE.helper(s, page, "start", &[scope, root, arg, s.boolean(page.is_hidden()), s.boolean(inactive)])
}
