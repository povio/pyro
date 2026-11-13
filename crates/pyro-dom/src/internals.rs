//! `ElementInternals` state the tree keeps per element: whether internals
//! were attached, the custom states `:state()` matches, and for
//! form-associated custom elements their form owner, disabled state,
//! validity and submission value.
//!
//! An element that never called `attachInternals` and is not
//! form-associated has no entry. The form owner and disabled state of
//! form-associated custom elements are maintained as the tree changes so
//! `formAssociatedCallback` and `formDisabledCallback` are queued at the
//! mutation; while no such element exists every mutation pays one integer
//! test.

use std::any::Any;

use markup5ever::local_name;
use rustc_hash::FxHashMap;

use crate::dom::{Change, Dom};
use crate::error::{DomError, DomResult};
use crate::forms::Validity;
use crate::node::{element_flags, Atom, NodeId, NIL};

#[derive(Default)]
pub(crate) struct InternalsStore {
    /// By element slot; removed when the element is freed.
    map: FxHashMap<u32, Box<ElementInternalsData>>,
    /// How many entries are form-associated custom elements.
    pub(crate) faces: u32,
}

#[derive(Default)]
struct ElementInternalsData {
    attached: bool,
    /// `CustomStateSet`, in insertion order.
    states: Vec<Atom>,
    face: Option<FaceState>,
}

/// A form-associated custom element's form state.
#[derive(Default)]
struct FaceState {
    owner: Option<NodeId>,
    disabled: bool,
    validity: Validity,
    message: String,
    anchor: Option<NodeId>,
    /// The submission value and state the bindings set (`setFormValue`).
    value: Option<Box<dyn Any>>,
}

impl InternalsStore {
    pub(crate) fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

impl Dom {
    fn internals_entry(&mut self, el: NodeId) -> DomResult<(u32, &mut ElementInternalsData)> {
        let i = self.check(el)?;
        if self.n(i).element().is_none() {
            return Err(DomError::Type("not an element"));
        }
        Ok((i, self.internals.map.entry(i).or_default()))
    }

    /// The element was freed.
    pub(crate) fn forget_internals(&mut self, i: u32) {
        if let Some(data) = self.internals.map.remove(&i) {
            if data.face.is_some() {
                self.internals.faces -= 1;
            }
        }
    }

    /// `attachInternals()` bookkeeping: false when internals were attached
    /// to `el` before.
    pub fn attach_internals(&mut self, el: NodeId) -> DomResult<bool> {
        let (_, data) = self.internals_entry(el)?;
        Ok(!std::mem::replace(&mut data.attached, true))
    }

    // ----- custom states ------------------------------------------------

    /// The states of `el`'s `CustomStateSet`, in insertion order.
    pub fn custom_states(&self, el: NodeId) -> Vec<String> {
        let Ok(i) = self.check(el) else { return Vec::new() };
        self.internals.map.get(&i).map(|d| d.states.iter().map(|s| s.to_string()).collect()).unwrap_or_default()
    }

    /// Whether `el`'s `CustomStateSet` has `state`.
    pub fn has_custom_state(&self, el: NodeId, state: &str) -> bool {
        let Ok(i) = self.check(el) else { return false };
        self.internals.map.get(&i).is_some_and(|d| d.states.iter().any(|s| &**s == state))
    }

    /// `:state(name)`.
    pub(crate) fn has_custom_state_idx(&self, i: u32, state: &Atom) -> bool {
        !self.internals.map.is_empty() && self.internals.map.get(&i).is_some_and(|d| d.states.contains(state))
    }

    /// The custom states of element `i` for a renderer (`:state()`),
    /// separated by U+001F (a state may contain whitespace); `None` without.
    pub(crate) fn render_custom_states(&self, i: u32) -> Option<String> {
        if self.internals.map.is_empty() {
            return None;
        }
        let states = &self.internals.map.get(&i)?.states;
        (!states.is_empty()).then(|| states.iter().map(|s| &**s).collect::<Vec<_>>().join("\u{1f}"))
    }

    /// Adds (`on`) or deletes `state`; returns whether the set changed.
    /// A change restyles the element (`:state()`).
    pub fn set_custom_state(&mut self, el: NodeId, state: &str, on: bool) -> DomResult<bool> {
        let (i, data) = self.internals_entry(el)?;
        let pos = data.states.iter().position(|s| &**s == state);
        let changed = match (pos, on) {
            (None, true) => {
                data.states.push(Atom::from(state));
                true
            }
            (Some(p), false) => {
                data.states.remove(p);
                true
            }
            _ => false,
        };
        if changed {
            self.bump();
            self.note(Change::State(i));
        }
        Ok(changed)
    }

    /// `CustomStateSet.clear()`.
    pub fn clear_custom_states(&mut self, el: NodeId) -> DomResult<()> {
        let (i, data) = self.internals_entry(el)?;
        if !data.states.is_empty() {
            data.states.clear();
            self.bump();
            self.note(Change::State(i));
        }
        Ok(())
    }

    // ----- form-associated custom elements ------------------------------

    pub(crate) fn is_face_idx(&self, i: u32) -> bool {
        self.n(i).element().is_some_and(|e| e.flags & element_flags::FORM_ASSOCIATED != 0)
    }

    /// Whether `el` is a form-associated custom element.
    pub fn is_form_associated(&self, el: NodeId) -> bool {
        self.check(el).is_ok_and(|i| self.is_face_idx(i))
    }

    /// `el` was upgraded or constructed with a form-associated definition
    /// (custom element upgrade, "reset the form owner"): queues
    /// `formAssociatedCallback` when it has a form owner and
    /// `formDisabledCallback(true)` when it is disabled.
    pub fn set_form_associated(&mut self, el: NodeId) -> DomResult<()> {
        let (i, data) = self.internals_entry(el)?;
        if data.face.is_some() {
            return Ok(());
        }
        data.face = Some(FaceState::default());
        self.internals.faces += 1;
        self.refresh_face(i);
        Ok(())
    }

    /// The custom element failed: it is no form control.
    pub(crate) fn forget_form_associated(&mut self, i: u32) {
        if let Some(data) = self.internals.map.get_mut(&i) {
            if data.face.take().is_some() {
                self.internals.faces -= 1;
            }
        }
    }

    fn face(&self, el: NodeId) -> Option<&FaceState> {
        let i = self.check(el).ok()?;
        self.internals.map.get(&i)?.face.as_ref()
    }

    fn face_mut(&mut self, el: NodeId) -> DomResult<&mut FaceState> {
        let i = self.check(el)?;
        self.internals.map.get_mut(&i).and_then(|d| d.face.as_mut()).ok_or(DomError::Type("not form-associated"))
    }

    /// Recomputes `i`'s form owner and disabled state, queueing the
    /// callbacks of what changed.
    fn refresh_face(&mut self, i: u32) {
        let owner = self.opt_id(self.form_owner_idx(i));
        let disabled = self.face_disabled_idx(i);
        let Some(face) = self.internals.map.get_mut(&i).and_then(|d| d.face.as_mut()) else { return };
        let owner_changed = std::mem::replace(&mut face.owner, owner) != owner;
        let disabled_changed = std::mem::replace(&mut face.disabled, disabled) != disabled;
        if owner_changed {
            self.push_custom_reaction(i, crate::custom::CustomReaction::FormAssociated(owner));
        }
        if disabled_changed {
            self.push_custom_reaction(i, crate::custom::CustomReaction::FormDisabled(disabled));
        }
    }

    fn refresh_all_faces(&mut self) {
        let mut faces: Vec<u32> =
            self.internals.map.iter().filter(|(_, d)| d.face.is_some()).map(|(&i, _)| i).collect();
        // Tree order is not known here; slot order keeps runs deterministic.
        faces.sort_unstable();
        for i in faces {
            self.refresh_face(i);
        }
    }

    /// `root` was inserted or removed (form-associated custom elements
    /// exist): resets the form owner and disabled state of those inside it,
    /// or of all of them when it holds a form, fieldset or legend.
    pub(crate) fn faces_after_tree_change(&mut self, root: u32) {
        let mut found = Vec::new();
        let mut i = root;
        while i != NIL {
            if let Some(e) = self.n(i).element() {
                if e.is_html() && matches!(&*e.name.local, "form" | "fieldset" | "legend") {
                    self.refresh_all_faces();
                    return;
                }
                if e.flags & element_flags::FORM_ASSOCIATED != 0 {
                    found.push(i);
                }
            }
            i = self.following_in(i, root);
        }
        for i in found {
            self.refresh_face(i);
        }
    }

    /// An attribute of `i` changed (form-associated custom elements exist):
    /// `form`, `id` and `disabled` can change form owners and disabled states.
    pub(crate) fn faces_after_attr(&mut self, name: &markup5ever::QualName) {
        if name.ns == markup5ever::ns!()
            && matches!(name.local, local_name!("form") | local_name!("id") | local_name!("disabled"))
        {
            self.refresh_all_faces();
        }
    }

    /// Disabled: its own `disabled` attribute, or a disabled fieldset
    /// ancestor it is not in the first legend of.
    pub(crate) fn face_disabled_idx(&self, i: u32) -> bool {
        let Some(e) = self.n(i).element() else { return false };
        if e.flags & element_flags::FORM_ASSOCIATED == 0 {
            return false;
        }
        if e.attr_local(&local_name!("disabled")).is_some() {
            return true;
        }
        let html = |k: u32, name: &str| self.n(k).element().is_some_and(|e| e.is_html() && &*e.name.local == name);
        let mut child = i;
        let mut p = self.n(i).parent;
        while p != NIL {
            if html(p, "fieldset") && self.n(p).element().unwrap().attr_local(&local_name!("disabled")).is_some() {
                let legend = self.child_idx_iter(p).find(|&c| html(c, "legend"));
                if legend != Some(child) {
                    return true;
                }
            }
            child = p;
            p = self.n(p).parent;
        }
        false
    }

    /// Candidate for constraint validation: not disabled, no `readonly`,
    /// not in a `<datalist>`.
    pub(crate) fn face_will_validate_idx(&self, i: u32) -> bool {
        let e = self.n(i).element().expect("element");
        if self.face_disabled_idx(i) || e.attr_local(&local_name!("readonly")).is_some() {
            return false;
        }
        let mut p = self.n(i).parent;
        while p != NIL {
            if self.n(p).element().is_some_and(|e| e.is_html() && &*e.name.local == "datalist") {
                return false;
            }
            p = self.n(p).parent;
        }
        true
    }

    /// The validity flags `setValidity` set (all clear before).
    pub(crate) fn face_validity_idx(&self, i: u32) -> Validity {
        self.internals.map.get(&i).and_then(|d| d.face.as_ref()).map(|f| f.validity).unwrap_or_default()
    }

    /// `ElementInternals.setValidity(flags, message, anchor)`.
    pub fn set_face_validity(
        &mut self,
        el: NodeId,
        validity: Validity,
        message: String,
        anchor: Option<NodeId>,
    ) -> DomResult<()> {
        let face = self.face_mut(el)?;
        let changed = face.validity != validity;
        face.validity = validity;
        face.message = message;
        face.anchor = anchor;
        if changed {
            // :valid/:invalid.
            let i = self.check(el)?;
            self.bump();
            self.note(Change::State(i));
        }
        Ok(())
    }

    /// The validation message `setValidity` set.
    pub fn face_validation_message(&self, el: NodeId) -> Option<&str> {
        self.face(el).map(|f| f.message.as_str())
    }

    /// Whether `node` is a shadow-including descendant of `ancestor`
    /// (`setValidity`'s anchor check).
    pub fn is_shadow_including_descendant(&self, ancestor: NodeId, node: NodeId) -> bool {
        match (self.check(ancestor), self.check(node)) {
            (Ok(a), Ok(n)) => a != n && self.shadow_including_inclusive_ancestor(a, n),
            _ => false,
        }
    }

    /// The validation anchor `setValidity` set.
    pub fn face_validation_anchor(&self, el: NodeId) -> Option<NodeId> {
        self.face(el).and_then(|f| f.anchor).filter(|&a| self.check(a).is_ok())
    }

    /// The form owner, as last reset.
    pub fn face_form(&self, el: NodeId) -> Option<NodeId> {
        self.face(el).and_then(|f| f.owner).filter(|&f| self.check(f).is_ok())
    }

    /// The submission value the bindings stored (`setFormValue`).
    pub fn face_value(&self, el: NodeId) -> Option<&dyn Any> {
        self.face(el).and_then(|f| f.value.as_deref())
    }

    /// Stores the bindings' submission value.
    pub fn set_face_value(&mut self, el: NodeId, value: Option<Box<dyn Any>>) -> DomResult<()> {
        self.face_mut(el)?.value = value;
        Ok(())
    }

    /// HTML "reset" of `form`: `formResetCallback` for the form-associated
    /// custom elements it owns, in tree order.
    pub fn enqueue_form_reset(&mut self, form: NodeId) -> DomResult<()> {
        if self.internals.faces == 0 {
            return Ok(());
        }
        let f = self.check(form)?;
        let root = self.root_idx(f);
        let mut i = root;
        let mut owned = Vec::new();
        while i != NIL {
            if self.is_face_idx(i) && self.face_form(self.id_of(i)) == Some(form) {
                owned.push(i);
            }
            i = self.following_in(i, root);
        }
        for i in owned {
            self.push_custom_reaction(i, crate::custom::CustomReaction::FormReset);
        }
        Ok(())
    }
}
