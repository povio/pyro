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
}

#[derive(Debug)]
struct Reservation {
    child: bool,
    quota: Arc<Quota>,
    pixels: u64,
    work: u64,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let mut used = self.quota.used.lock().unwrap();
        used.slots -= 1;
        used.pixels -= self.pixels;
        used.work -= self.work;
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

fn admit(mut page: Rc<PageDom>, local_slots: usize) -> Option<Capture> {
    for depth in 0..64 {
        if page.is_detached() || page.is_disposed() { return None; }
        if page.hooks().is_some() {
            match page.container() {
                crate::page::FrameContainer::Frame(parent, _) => { page = parent; continue; }
                crate::page::FrameContainer::Detached => return None,
                crate::page::FrameContainer::Top => (),
            }
        }
        let quota = page.transition_quota.borrow_mut().get_or_insert_with(Default::default).clone();
        let mut used = quota.used.lock().unwrap();
        let child = depth > 0;
        if used.slots >= 16 || (child && (used.slots >= 12 || local_slots >= 4)) { return None; }
        used.slots += 1;
        drop(used);
        return Some(Capture { reservation: Arc::new(Reservation { child, quota, pixels: 0, work: 0 }), old: Default::default(), new: Default::default(), samples: Default::default(), tree: None });
    }
    None
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
        let Some(record) = captures.as_mut().and_then(|map| map.get_mut(&scope_key)) else { return Ok(s.boolean(false)); };
        let Some(reservation) = Arc::get_mut(&mut record.reservation) else { return Ok(s.boolean(false)); };
        let layout = page.layout();
        let dom = page.dom.borrow();
        if page.is_hidden() || !dom.is_connected(root.id).unwrap_or(false) { return Ok(s.boolean(false)); }
        let Some(source) = layout.synced_geometry_source(&dom) else { return Ok(s.boolean(true)); };
        let mut quota = reservation.quota.used.lock().unwrap();
        let used_pixels = quota.pixels;
        let used_work = quota.work;
        let work_limit: u64 = if reservation.child { 3 << 16 } else { 1 << 18 };
        let pixel_limit: u64 = if reservation.child { 3 << 22 } else { 1 << 24 };
        if used_work >= work_limit || (old && used_pixels >= pixel_limit) { return Ok(s.boolean(false)); }
        let mut work = work_limit - used_work;
        let result = source.capture_transition(key, old, pixel_limit.saturating_sub(used_pixels), &mut work);
        let Some(boxes) = result else { return Ok(s.boolean(false)); };
        let charged = work_limit - used_work - work;
        reservation.work += charged;
        quota.work += charged;
        if old {
            reservation.pixels = boxes.iter().filter_map(|item| item.rgba.as_ref()).map(|img| img.len() as u64 / 4).sum();
            quota.pixels += reservation.pixels;
            record.old = boxes.into();
        } else {
            let cost = 1 + 4 * (record.old.len() + boxes.len()) as u64;
            if cost > work || !source.set_transition_boxes(key, &record.old, &boxes) { return Ok(s.boolean(false)); }
            reservation.work += cost;
            quota.work += cost;
            record.tree = Some((Rc::downgrade(source), key));
            record.new = boxes.into();
        }
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
                runs.push(s.array(&[s.number(index as f64),s.number(run.index as f64),s.string(&pseudo), s.string(&run.name), s.number(run.delay_ms), s.number(run.duration_ms), s.number(run.iterations), s.string(&run.style.direction), s.string(&run.style.fill), s.string(run.style.easing.as_deref().unwrap_or("ease")), s.boolean(run.style.paused)])?.value());
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
    Ok(out)
}

fn unavailable() -> JsError {
    JsError::Type("ViewTransition is unavailable".into())
}

impl PageDom {
    pub fn transition_frames(&self, document: &str, out: &mut Vec<pyro_dom::geometry::TransitionFrame>) {
        let captures = self.transition_captures.borrow();
        let Some(map) = captures.as_ref() else { return };
        for record in map.values() {
            let Some((_, root)) = record.tree.as_ref() else { continue };
            out.push(pyro_dom::geometry::TransitionFrame { document: document.to_owned(), root: *root,
                old: record.old.clone(), new: record.new.clone(), samples: record.samples.clone(), reservation: record.reservation.clone() });
        }
    }
}

pub(crate) fn active<'s>(s: &Scope<'s>, node: &crate::node::NodeData) -> JsResult<Value<'s>> {
    let Some(module) = MODULE.existing(s, node.page())? else { return Ok(s.null()) };
    let scope = crate::node::wrap_node(s, node.page(), node.id)?.value();
    module.get("active")?.as_object().ok_or_else(unavailable)?.call(None, &[scope])
}

/// `startViewTransition(callbackOptions)` on `scope` (a document, or an element for a scoped
/// transition), whose `transitionRoot` is `root`.
pub(crate) fn start<'s>(
    s: &Scope<'s>,
    page: &Rc<PageDom>,
    scope: Value<'s>,
    root: Value<'s>,
    arg: Value<'s>,
) -> JsResult<Value<'s>> {
    MODULE.helper(s, page, "start", &[scope, root, arg, s.boolean(page.is_hidden())])
}
