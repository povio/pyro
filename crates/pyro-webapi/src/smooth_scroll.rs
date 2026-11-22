//! pyro: smooth scrolls (CSSOM View § 4.1 "perform a smooth scroll"; `behavior: "smooth"` and
//! `scroll-behavior: smooth`).
//!
//! A smooth scroll is a scroll write that has already run: the box was put back where it was,
//! and moves to where the write put it (snapped and clamped there, in the write's direction) over
//! [`FRAMES`] rendering opportunities, one step per animation frame of its page's frame tree
//! (the clock CSS animations step on; never the wall clock). Each step queues a `scroll` event;
//! the last lands on the destination and queues `scrollend`. Another scroll of the same box
//! interrupts it. The promise a scroll method returns settles when all the smooth scrolls it
//! started end (Chrome's `Promise<ScrollResult>`), `interrupted` when one was cut short.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use pyro_dom::NodeId;
use pyro_js::{Deferred, WeakRealm};

use crate::page::PageDom;

/// Rendering opportunities a smooth scroll takes from start to end.
const FRAMES: u32 = 15;

/// The promise of a scroll method that started smooth scrolls, settled when they all end.
#[derive(Default)]
pub(crate) struct Waiter {
    /// Smooth scrolls still running.
    remaining: Cell<usize>,
    /// One of them was interrupted.
    interrupted: Cell<bool>,
    /// They all ended before the promise was attached.
    done: Cell<bool>,
    promise: RefCell<Option<(WeakRealm, Deferred)>>,
}

impl Waiter {
    pub(crate) fn new() -> Rc<Self> {
        Rc::new(Self::default())
    }
}

struct Animation {
    page: Weak<PageDom>,
    /// The document the box is in (a navigation ends the scroll).
    doc: NodeId,
    /// The box (`None`: the viewport).
    target: Option<NodeId>,
    from: (i32, i32),
    to: (i32, i32),
    /// Frames run so far.
    frame: u32,
    waiters: Vec<Rc<Waiter>>,
}

impl Animation {
    fn is(&self, page: &Rc<PageDom>, target: Option<NodeId>) -> bool {
        self.target == target && std::ptr::eq(self.page.as_ptr(), Rc::as_ptr(page))
    }

    /// The offsets at the current frame: ease-in-out from `from` to `to`.
    fn position(&self) -> (i32, i32) {
        if self.frame >= FRAMES {
            return self.to;
        }
        let p = self.frame as f64 / FRAMES as f64;
        let eased = if p < 0.5 { 2.0 * p * p } else { 1.0 - (2.0 - 2.0 * p).powi(2) / 2.0 };
        let lerp = |a: i32, b: i32| (a as f64 + (b as f64 - a as f64) * eased).round() as i32;
        (lerp(self.from.0, self.to.0), lerp(self.from.1, self.to.1))
    }
}

thread_local! {
    static RUNNING: RefCell<Vec<Animation>> = const { RefCell::new(Vec::new()) };
}

/// Whether a scroll of `target` (`None`: the viewport) of `page` with behavior `auto` is smooth:
/// the box's computed `scroll-behavior`, the root element's for the viewport (the `<body>`'s is
/// not propagated; WPT cssom-view `scroll-behavior-smooth`).
pub(crate) fn css_smooth(page: &Rc<PageDom>, target: Option<NodeId>) -> bool {
    let el = match target {
        Some(el) => el,
        None => match page.dom.borrow().document_element(page.document()) {
            Ok(Some(root)) => root,
            _ => return false,
        },
    };
    crate::animation_events::style_flush(page, Some(el));
    let (Ok(dom), Ok(layout)) = (page.dom.try_borrow(), page.layout.try_borrow()) else { return false };
    layout.computed_style_value(&dom, el, "scroll-behavior") == "smooth"
}

/// Starts a smooth scroll of `target` (`None`: the viewport) of `page` from `from` (where the box
/// is) to `to`; one already running on the box is interrupted. `waiter` waits for it.
pub(crate) fn start(page: &Rc<PageDom>, target: Option<NodeId>, from: (i32, i32), to: (i32, i32), waiter: &Rc<Waiter>) {
    cancel(page, target);
    waiter.remaining.set(waiter.remaining.get() + 1);
    let animation = Animation {
        page: Rc::downgrade(page),
        doc: page.document(),
        target,
        from,
        to,
        frame: 0,
        waiters: vec![waiter.clone()],
    };
    RUNNING.with(|r| r.borrow_mut().push(animation));
    set_smooth_scrolling(page, target, true);
    request_frame(page);
}

/// Tells `page`'s layout whether a smooth scroll is moving `target` (it neither snaps nor re-snaps
/// the box meanwhile; `Layout::set_smooth_scrolling`).
fn set_smooth_scrolling(page: &PageDom, target: Option<NodeId>, running: bool) {
    if let Ok(mut layout) = page.layout.try_borrow_mut() {
        layout.set_smooth_scrolling(target, running);
    }
}

/// Another scroll of `target` (`None`: the viewport) of `page`: a smooth scroll running on it
/// stops where it is, interrupted.
pub(crate) fn cancel(page: &Rc<PageDom>, target: Option<NodeId>) {
    let stopped = RUNNING.with(|r| {
        let mut r = r.borrow_mut();
        let i = r.iter().position(|a| a.is(page, target))?;
        Some(r.remove(i))
    });
    if let Some(animation) = stopped {
        set_smooth_scrolling(page, target, false);
        for waiter in animation.waiters {
            settle(page, &waiter, true);
        }
    }
}

/// The smooth-scroll step of an animation frame of `page`, before its scroll events are
/// dispatched: each smooth scroll of `page`, or of a page sharing its DOM (a child frame stepped
/// by its top-level page's frames, as the frame tree renders together), advances a frame, and its
/// `scroll` event (with `scrollend` at its end) is queued.
pub fn step(page: &Rc<PageDom>) {
    // (A child frame's smooth scrolls step with its top-level page's frames only: stepping them
    // on its own frames too would run them at twice the speed.)
    if page.is_nested_frame() {
        return;
    }
    let mut steps: Vec<(Rc<PageDom>, Option<NodeId>, (i32, i32), bool)> = Vec::new();
    let mut ended: Vec<(Weak<PageDom>, Vec<Rc<Waiter>>, bool)> = Vec::new();
    // The boxes whose smooth scrolls stopped without a last step (removed from the document).
    let mut stopped: Vec<(Rc<PageDom>, Option<NodeId>)> = Vec::new();
    RUNNING.with(|r| {
        let Ok(mut r) = r.try_borrow_mut() else { return };
        if r.is_empty() {
            return;
        }
        r.retain_mut(|a| {
            let Some(p) = a.page.upgrade().filter(|p| !p.is_disposed() && p.document() == a.doc) else {
                ended.push((a.page.clone(), std::mem::take(&mut a.waiters), true));
                return false;
            };
            if !Rc::ptr_eq(&p, page) && !Rc::ptr_eq(&p.dom, &page.dom) {
                return true;
            }
            // A box removed from the document stops, with no scroll events (its promise settles;
            // WPT cssom-view `element-scroll-promise-after-removal`).
            if let Some(el) = a.target {
                if !p.dom.try_borrow().map_or(true, |dom| dom.is_connected(el).unwrap_or(false)) {
                    ended.push((a.page.clone(), std::mem::take(&mut a.waiters), true));
                    stopped.push((p, a.target));
                    return false;
                }
            }
            a.frame += 1;
            let done = a.frame >= FRAMES;
            steps.push((p, a.target, a.position(), done));
            if done {
                ended.push((a.page.clone(), std::mem::take(&mut a.waiters), false));
            }
            !done
        });
    });
    for (p, target, pos, done) in steps {
        let moved = match (p.dom.try_borrow(), p.layout.try_borrow_mut()) {
            (Ok(dom), Ok(mut layout)) => {
                // (Within the range the box has now: its content may have shrunk since it started.)
                let pos = layout.clamp_scroll_position(&dom, target, pos);
                let moved = layout.set_scroll_position(&dom, target, pos);
                if done {
                    layout.set_smooth_scrolling(target, false);
                }
                moved
            }
            _ => {
                if done {
                    set_smooth_scrolling(&p, target, false);
                }
                false
            }
        };
        if moved || done {
            crate::layout_api::queue_scroll(&p, target, done);
        }
    }
    for (p, target) in stopped {
        set_smooth_scrolling(&p, target, false);
    }
    for (p, waiters, interrupted) in ended {
        let Some(p) = p.upgrade() else { continue };
        for waiter in waiters {
            settle(&p, &waiter, interrupted);
        }
    }
}

/// At the end of an animation frame of `page`: another frame is requested while a smooth scroll
/// of its frame tree runs.
pub fn request_frames(page: &Rc<PageDom>) {
    // (A child frame's are stepped, and their frames requested, by its top-level page: [`step`].)
    if page.is_nested_frame() {
        return;
    }
    if running(page) {
        request_frame(page);
    }
}

/// Whether a smooth scroll of `page`'s frame tree (`page`, or a page sharing its DOM) runs.
pub fn running(page: &Rc<PageDom>) -> bool {
    RUNNING.with(|r| {
        r.try_borrow().is_ok_and(|r| {
            r.iter().any(|a| a.page.upgrade().is_some_and(|p| Rc::ptr_eq(&p, page) || Rc::ptr_eq(&p.dom, &page.dom)))
        })
    })
}

/// Requests an animation frame that steps `page`'s smooth scrolls: its own, or for a child frame
/// its top-level page's (whose frames step the frame tree's, which shares its DOM).
fn request_frame(page: &Rc<PageDom>) {
    crate::lifecycle::request_top_level_frames(page);
}

/// One of the smooth scrolls `waiter` waits for ended (`interrupted`: cut short).
fn settle(page: &Rc<PageDom>, waiter: &Rc<Waiter>, interrupted: bool) {
    if interrupted {
        waiter.interrupted.set(true);
    }
    let left = waiter.remaining.get().saturating_sub(1);
    waiter.remaining.set(left);
    if left > 0 {
        return;
    }
    match waiter.promise.borrow_mut().take() {
        Some((realm, deferred)) => resolve(page, realm, deferred, waiter.interrupted.get()),
        None => waiter.done.set(true),
    }
}

/// Gives `waiter` the promise of the scroll method that started its smooth scrolls.
pub(crate) fn attach(page: &Rc<PageDom>, waiter: &Rc<Waiter>, realm: WeakRealm, deferred: Deferred) {
    if waiter.done.get() {
        resolve(page, realm, deferred, waiter.interrupted.get());
    } else {
        *waiter.promise.borrow_mut() = Some((realm, deferred));
    }
}

/// Fulfills a scroll promise with `{interrupted}` in a task of its own (the scroll that ended it
/// may be running script, or a rendering update).
fn resolve(page: &Rc<PageDom>, realm: WeakRealm, deferred: Deferred, interrupted: bool) {
    page.post_task(Box::new(move || {
        let Some(realm) = realm.upgrade() else { return };
        let Ok(s) = realm.scope() else { return };
        let o = s.object();
        if o.set_atom("interrupted", s.boolean(interrupted)).is_ok() {
            let _ = deferred.resolve(&s, o.value());
        }
    }));
}
