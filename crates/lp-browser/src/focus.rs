//! Page-owned focus routing. Registry and DOM borrows never cross event dispatch.
use crate::frames::{FrameHost, FrameKey};
use crate::{Browser, Frame, LoaderId, PageId};
use lp_webapi::{FocusTarget, FocusTransition};

pub(crate) struct FocusState {
    target: FrameKey,
    transition: Option<FocusTransition>,
}
impl FocusState {
    pub fn new(page: PageId) -> Self {
        Self { target: page.into(), transition: None }
    }
}

impl FrameHost {
    /// Root-to-leaf snapshots. A missing ancestor invalidates the entire chain.
    fn focus_chain(&self, leaf: FrameKey) -> Option<Vec<(FrameKey, Frame)>> {
        let mut chain = Vec::new();
        let mut key = leaf;
        loop {
            let frame = self.frame(key)?;
            let parent = frame.parent.clone();
            chain.push((key, frame));
            match parent {
                Some(id) => key = self.key(&id)?,
                None => break,
            }
        }
        chain.reverse();
        Some(chain)
    }

    fn focus_chain_current(&self, chain: &[(FrameKey, Frame)], token: &FocusTransition) -> bool {
        token.is_current()
            && chain.iter().all(|(key, f)| {
                self.matches(*key, &f.loader_id) && f.page_dom.as_ref().is_some_and(|p| !p.is_disposed())
            })
    }

    pub fn is_focused(&self, key: FrameKey, loader: &LoaderId) -> bool {
        self.focus.borrow().target == key
            && self.matches(key, loader)
            && self.frame(key).and_then(|f| f.page_dom).is_some_and(|p| !p.is_disposed())
    }

    /// Called by native bindings before local focusing steps, including no-op
    /// local requests: a document's remembered active element is not the page's
    /// active frame. A false result means handled here, stale, or superseded.
    pub fn request_focus(
        &self,
        source: FrameKey,
        loader: &LoaderId,
        target: FocusTarget,
        token: &FocusTransition,
    ) -> bool {
        if !self.matches(source, loader) || !token.is_current() {
            return false;
        }
        let redirected = match target {
            FocusTarget::Element(node) => self.owner(node),
            _ => None,
        };
        let destination = redirected.unwrap_or(source);
        let Some(chain) = self.focus_chain(destination) else { return false };
        if !self.focus_chain_current(&chain, token) {
            return false;
        }
        {
            let mut state = self.focus.borrow_mut();
            if let Some(previous) = state.transition.replace(token.clone()) {
                if !previous.ptr_eq(token) {
                    previous.cancel();
                }
            }
            if !matches!(target, FocusTarget::Blur(_)) {
                state.target = destination;
            }
        }
        // Explicit blur changes the document's active element, not frame identity.
        if matches!(target, FocusTarget::Blur(_)) {
            return token.is_current();
        }
        // Reentry can interrupt a previous chain halfway through. Reconcile all
        // currently focused documents, deepest first; this scan clears stale
        // flags and never chooses the destination frame.
        let mut old: Vec<_> = self
            .keys()
            .into_iter()
            .filter_map(|key| {
                let depth = self.focus_chain(key)?.len();
                Some((depth, key, self.frame(key)?))
            })
            .collect();
        old.sort_by_key(|(depth, _, _)| std::cmp::Reverse(*depth));
        for (_, key, frame) in old {
            if !self.focus_chain_current(&chain, token) {
                return false;
            }
            let Some(page) = frame.page_dom else { continue };
            let current = page.dom.borrow().focused(page.document()).ok().flatten();
            // Local native steps preserve same-document relatedTarget.
            if current.is_some() && !chain.iter().any(|(k, _)| *k == key) {
                lp_webapi::input::blur_document_with_transition(&page, token);
            }
        }
        for pair in chain.windows(2) {
            if !self.focus_chain_current(&chain, token) {
                return false;
            }
            if let (Some(page), Some(owner)) = (&pair[0].1.page_dom, pair[1].1.owner) {
                lp_webapi::input::focus_element_with_transition(page, owner, token);
            }
        }
        if !self.focus_chain_current(&chain, token) {
            return false;
        }
        if redirected.is_some() {
            if let Some(page) = &chain.last().expect("nonempty chain").1.page_dom {
                lp_webapi::input::focus_window_with_transition(page, token);
            }
            false
        } else {
            true
        }
    }

    /// Called before removing/replacing documents, without dispatching JS. A
    /// detached focused subtree resets the entire chain to the root body, never
    /// its old editing selection; navigation retains the frame identity.
    pub fn invalidate_focus(&self, key: FrameKey, detached: bool) {
        let selected = self.focus.borrow().target;
        let Some(chain) = self.focus_chain(selected) else { return };
        if !chain.iter().any(|(k, _)| *k == key) {
            return;
        }
        let fallback = if detached { self.page.into() } else { key };
        {
            let mut state = self.focus.borrow_mut();
            if let Some(token) = state.transition.take() {
                token.cancel();
            }
            state.target = fallback;
        }
        for (frame_key, frame) in chain {
            if detached || frame_key == key {
                if let Some(page) = frame.page_dom {
                    lp_webapi::input::clear_document_focus(&page);
                }
            }
        }
    }
}

impl Browser {
    /// Fresh, owned snapshot of the page's active frame. Inspector evaluation
    /// does not select a frame. Call again for each input command; no registry
    /// borrow or remembered editable target escapes through this accessor.
    pub fn focused_frame(&self, page: PageId) -> Option<Frame> {
        self.sync_root(page);
        let host = &self.page(page)?.frames;
        let key = host.focus.borrow().target;
        let frame = host.frame(key)?;
        if frame.page_dom.as_ref().is_none_or(|p| p.is_disposed()) || frame.main_realm().is_none_or(|r| r.is_disposed())
        {
            return None;
        }
        Some(frame)
    }
}
