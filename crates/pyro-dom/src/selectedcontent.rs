//! `<selectedcontent>` (HTML customizable select): the element shows a copy
//! of its select's selected option, kept up to date as the selectedness
//! changes.
//!
//! As in Chromium, every selectedcontent of a select is kept up to date (not
//! one inside another selectedcontent or a nested select), and not while the
//! select is `multiple`. The steps:
//!
//! - a selectedcontent's post-connection steps fill it (insertion while
//!   connected);
//! - an option's insertion, an attribute that can change the selectedness
//!   (`selected`, `disabled`, `multiple`, `size`) and `option.selected` update
//!   the select when its selected option changed ("ask for a reset");
//! - `select.value`, `select.selectedIndex` and a form reset update it always;
//! - an option's removal or move, and a selectedcontent's move into another
//!   select, update in a microtask (`run_selectedcontent_updates`, run by the
//!   bindings with the mutation observer microtask).
//!
//! The parser's own step (clone an option into selectedcontent when popped)
//! is `Dom::maybe_clone_option_into_selectedcontent`. Nothing here runs
//! before a document has created a selectedcontent (`selectedcontent_seen`).

use markup5ever::local_name;

use crate::dom::Dom;
use crate::error::DomResult;
use crate::node::{NodeId, NIL};

impl Dom {
    fn is_selectedcontent(&self, i: u32) -> bool {
        self.html_local(i).is_some_and(|l| &**l == "selectedcontent")
    }

    /// The select a selectedcontent shows the option of: its nearest select
    /// ancestor, none when another selectedcontent comes first (a nested one
    /// is disabled).
    fn selectedcontent_select(&self, sc: u32) -> u32 {
        let mut p = self.n(sc).parent;
        while p != NIL {
            match self.html_local(p).map(|l| &**l) {
                Some("select") => return p,
                Some("selectedcontent") => return NIL,
                _ => {}
            }
            p = self.n(p).parent;
        }
        NIL
    }

    /// The selectedcontent elements `select` keeps up to date, tree order:
    /// its descendants outside nested selects and other selectedcontents.
    fn select_selectedcontents(&self, select: u32) -> Vec<u32> {
        let mut out = Vec::new();
        let mut cur = self.n(select).first_child;
        while cur != NIL {
            let descend = match self.html_local(cur).map(|l| &**l) {
                Some("selectedcontent") => {
                    out.push(cur);
                    false
                }
                Some("select") => false,
                _ => self.n(cur).element().is_some(),
            };
            cur = if descend { self.following_in(cur, select) } else { self.following_skipping(cur, select) };
        }
        out
    }

    /// The option `select`'s selectedcontents show: its first selected
    /// option, `Some(NIL)` when none is, `None` for a multiple select (whose
    /// selectedcontents are not kept up to date).
    fn selectedcontent_shown_option(&self, select: u32) -> Option<u32> {
        let e = self.n(select).element()?;
        if e.attr_local(&local_name!("multiple")).is_some() {
            return None;
        }
        Some(self.selected_options_idx(select).first().copied().unwrap_or(NIL))
    }

    /// Fills `sc` with clones of `option`'s children (empties it for NIL).
    fn fill_selectedcontent(&mut self, option: u32, sc: u32) {
        let was = std::mem::replace(&mut self.selectedcontent_updating, true);
        let id = self.id_of(sc);
        let _: DomResult<()> = if option == NIL {
            if self.n(sc).first_child == NIL {
                Ok(())
            } else {
                self.replace_all(id, None)
            }
        } else {
            self.clone_selected_option_into(option, sc)
        };
        self.selectedcontent_updating = was;
    }

    /// Fills `sc`, one of `select`'s selectedcontents, from `option`, the selected one. When the
    /// select's selectedcontents show another option (or none is recorded), the select's update
    /// fills them all and records it: a later change that leaves the selected option as it is
    /// then updates nothing (a list box without a selected option keeps its selectedcontent when
    /// an unselected option is removed), and none is left showing an option no longer selected.
    fn fill_one_selectedcontent(&mut self, select: u32, option: u32, sc: u32) {
        let shown = self.n(select).element().and_then(|e| e.control.as_ref()).and_then(|c| c.selectedcontent_option);
        if shown == Some(option) {
            self.fill_selectedcontent(option, sc);
        } else {
            self.update_select_selectedcontent(select, true);
        }
    }

    /// The parser inserted selectedcontent `sc`: it shows no option yet, which its select records
    /// unless it has recorded one (the parser runs no post-connection steps; its own step clones
    /// a selected option into it later, `Dom::maybe_clone_option_into_selectedcontent`).
    pub(crate) fn selectedcontent_parsed(&mut self, sc: u32) {
        if !self.is_selectedcontent(sc) {
            return;
        }
        let select = self.selectedcontent_select(sc);
        if select == NIL {
            return;
        }
        let control = self.n(select).element().and_then(|e| e.control.as_ref());
        if !control.is_some_and(|c| c.selectedcontent_option.is_some()) {
            self.note_selectedcontent_option(select, NIL);
        }
    }

    /// Records the option `select`'s selectedcontents were filled from.
    pub(crate) fn note_selectedcontent_option(&mut self, select: u32, option: u32) {
        if let Some(e) = self.n_mut(select).element_mut() {
            e.control.get_or_insert_with(Default::default).selectedcontent_option = Some(option);
        }
    }

    /// Updates `select`'s selectedcontents to its selected option: always
    /// when `force`, else only when that option is not the one they show.
    pub(crate) fn update_select_selectedcontent(&mut self, select: u32, force: bool) {
        if !self.selectedcontent_seen || self.selectedcontent_updating || select == NIL {
            return;
        }
        let Some(option) = self.selectedcontent_shown_option(select) else { return };
        let shown = self.n(select).element().and_then(|e| e.control.as_ref()).and_then(|c| c.selectedcontent_option);
        if !force && shown == Some(option) {
            return;
        }
        self.note_selectedcontent_option(select, option);
        for sc in self.select_selectedcontents(select) {
            self.fill_selectedcontent(option, sc);
        }
    }

    /// `update_select_selectedcontent(select, true)` for the bindings: a
    /// form reset changed the select's selectedness.
    pub fn selectedcontent_after_reset(&mut self, select: NodeId) -> DomResult<()> {
        let s = self.check(select)?;
        self.update_select_selectedcontent(s, true);
        Ok(())
    }

    /// Queues a microtask update (see the module documentation).
    pub(crate) fn queue_selectedcontent_update(&mut self, node: u32, selectedcontent: bool) {
        if !self.selectedcontent_seen || self.selectedcontent_updating || node == NIL {
            return;
        }
        let id = self.id_of(node);
        if self.selectedcontent_pending.contains(&(id, selectedcontent)) {
            return;
        }
        if self.add_ref(id).is_ok() {
            self.selectedcontent_pending.push((id, selectedcontent));
            self.touch_hook();
        }
    }

    /// Whether `run_selectedcontent_updates` has anything to do.
    pub fn has_selectedcontent_updates(&self) -> bool {
        !self.selectedcontent_pending.is_empty()
    }

    /// Runs the updates queued for a microtask: a moved selectedcontent is
    /// filled from the select it is in now, a select whose options changed
    /// is updated when its selected option changed.
    pub fn run_selectedcontent_updates(&mut self) {
        for (id, selectedcontent) in std::mem::take(&mut self.selectedcontent_pending) {
            if let Ok(i) = self.check(id) {
                if selectedcontent {
                    let select = self.selectedcontent_select(i);
                    if select != NIL {
                        if let Some(option) = self.selectedcontent_shown_option(select) {
                            self.fill_one_selectedcontent(select, option, i);
                        }
                    }
                } else {
                    self.update_select_selectedcontent(i, false);
                }
            }
            self.release(id);
        }
    }

    /// The select whose list of options an option inserted into `parent`
    /// joins (the option element nearest ancestor select rules, from
    /// `parent`).
    fn select_for_options_under(&self, parent: u32) -> u32 {
        let mut optgroup = false;
        let mut p = parent;
        while p != NIL {
            match self.html_local(p).map(|l| &**l) {
                Some("datalist" | "hr" | "option" | "selectedcontent") => return NIL,
                Some("optgroup") => {
                    if optgroup {
                        return NIL;
                    }
                    optgroup = true;
                }
                Some("select") => return p,
                _ => {}
            }
            p = self.n(p).parent;
        }
        NIL
    }

    /// Whether `node`'s inclusive subtree holds an option.
    fn subtree_has_option(&self, node: u32) -> bool {
        let mut cur = node;
        while cur != NIL {
            if self.html_local(cur) == Some(&local_name!("option")) {
                return true;
            }
            cur = self.following_in(cur, node);
        }
        false
    }

    /// After `nodes` were inserted into `parent`: options joining a select
    /// ask it for a reset, and the selectedcontents that became connected run
    /// their post-connection steps.
    pub(crate) fn selectedcontent_after_insert(&mut self, nodes: &[u32], parent: u32, connected: bool) {
        if self.selectedcontent_updating {
            return;
        }
        let select = self.select_for_options_under(parent);
        if select != NIL && nodes.iter().any(|&k| self.subtree_has_option(k)) {
            self.update_select_selectedcontent(select, false);
        }
        if !connected {
            return;
        }
        let mut inserted = Vec::new();
        for &k in nodes {
            let mut cur = k;
            while cur != NIL {
                if self.is_selectedcontent(cur) {
                    inserted.push(self.id_of(cur));
                }
                cur = self.following_in(cur, k);
            }
        }
        for id in inserted {
            // An earlier one's filling can have taken this one out.
            let Ok(sc) = self.check(id) else { continue };
            if !self.is_connected_idx(sc) {
                continue;
            }
            let select = self.selectedcontent_select(sc);
            if select == NIL {
                continue;
            }
            if let Some(option) = self.selectedcontent_shown_option(select) {
                self.fill_one_selectedcontent(select, option, sc);
            }
        }
    }

    /// Before `node` moves: its selectedcontents with the select each shows.
    pub(crate) fn selectedcontents_before_move(&self, node: u32) -> Vec<(u32, u32)> {
        let mut out = Vec::new();
        let mut cur = node;
        while cur != NIL {
            if self.is_selectedcontent(cur) {
                out.push((cur, self.selectedcontent_select(cur)));
            }
            cur = self.following_in(cur, node);
        }
        out
    }

    /// After `node` moved: options that joined a select queue its update
    /// (the select they left queued its own on leaving), and a selectedcontent
    /// now in another select queues filling.
    pub(crate) fn selectedcontent_after_move(&mut self, node: u32, before: Vec<(u32, u32)>) {
        if self.selectedcontent_updating {
            return;
        }
        let select = self.select_for_options_under(self.n(node).parent);
        if select != NIL && self.subtree_has_option(node) {
            self.queue_selectedcontent_update(select, false);
        }
        for (sc, old) in before {
            if self.selectedcontent_select(sc) != old {
                self.queue_selectedcontent_update(sc, true);
            }
        }
    }

    /// After attribute `local` of `i` changed: one that can change a
    /// select's selectedness asks it for a reset.
    pub(crate) fn selectedcontent_after_attr(&mut self, i: u32, local: &str) {
        let select = match (self.html_local(i).map(|l| &**l), local) {
            (Some("select"), "multiple" | "size") => i,
            (Some("option"), "selected" | "disabled") => self.option_nearest_ancestor_select(i),
            (Some("optgroup"), "disabled") => self.select_for_options_under(i),
            _ => return,
        };
        self.update_select_selectedcontent(select, false);
    }
}
