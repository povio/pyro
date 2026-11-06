//! Form-control state: value/checkedness/selectedness/disabled, focus and
//! text editing primitives used by input dispatch (HTML §4.10).
//! All offsets are UTF-16 code units, as exposed to JS.

use markup5ever::{local_name, LocalName};

use crate::dom::{utf16_len, utf16_range, Dom};
use crate::error::{DomError, DomErrorKind, DomResult};
use crate::node::{NodeId, NodeKind, NIL};

/// `<input type>` keyword after normalization.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum InputType {
    Hidden,
    Text,
    Search,
    Tel,
    Url,
    Email,
    Password,
    Date,
    Month,
    Week,
    Time,
    DateTimeLocal,
    Number,
    Range,
    Color,
    Checkbox,
    Radio,
    File,
    Submit,
    Image,
    Reset,
    Button,
}

const INPUT_TYPES: &[(&str, InputType)] = &[
    ("hidden", InputType::Hidden),
    ("text", InputType::Text),
    ("search", InputType::Search),
    ("tel", InputType::Tel),
    ("url", InputType::Url),
    ("email", InputType::Email),
    ("password", InputType::Password),
    ("date", InputType::Date),
    ("month", InputType::Month),
    ("week", InputType::Week),
    ("time", InputType::Time),
    ("datetime-local", InputType::DateTimeLocal),
    ("number", InputType::Number),
    ("range", InputType::Range),
    ("color", InputType::Color),
    ("checkbox", InputType::Checkbox),
    ("radio", InputType::Radio),
    ("file", InputType::File),
    ("submit", InputType::Submit),
    ("image", InputType::Image),
    ("reset", InputType::Reset),
    ("button", InputType::Button),
];

impl InputType {
    pub fn parse(attr: Option<&str>) -> Self {
        let Some(v) = attr else { return InputType::Text };
        INPUT_TYPES.iter().find(|(n, _)| n.eq_ignore_ascii_case(v)).map_or(InputType::Text, |(_, t)| *t)
    }
    pub fn keyword(self) -> &'static str {
        INPUT_TYPES.iter().find(|(_, t)| *t == self).map(|(n, _)| *n).unwrap()
    }
    fn value_mode(self) -> ValueMode {
        use InputType::*;
        match self {
            Hidden | Submit | Image | Reset | Button => ValueMode::Default,
            Checkbox | Radio => ValueMode::DefaultOn,
            File => ValueMode::Filename,
            _ => ValueMode::Value,
        }
    }
    /// Types whose value is edited as free text (selection APIs apply).
    pub fn is_textual(self) -> bool {
        use InputType::*;
        matches!(self, Text | Search | Tel | Url | Email | Password)
    }
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum ValueMode {
    Value,
    Default,
    DefaultOn,
    Filename,
}

fn strip_newlines(s: &str) -> std::borrow::Cow<'_, str> {
    if s.contains(['\n', '\r']) {
        s.chars().filter(|&c| c != '\n' && c != '\r').collect::<String>().into()
    } else {
        s.into()
    }
}

/// Value sanitization algorithm for the subset of types we model. Date/time
/// types are not validated (kept verbatim) — documented gap.
fn sanitize(ty: InputType, v: &str) -> String {
    use InputType::*;
    match ty {
        Text | Search | Tel | Password => strip_newlines(v).into_owned(),
        Url | Email => strip_newlines(v).trim_matches(|c: char| c.is_ascii_whitespace()).to_owned(),
        Number => {
            let t = v;
            let valid = !t.is_empty()
                && t.parse::<f64>().is_ok_and(f64::is_finite)
                && !t.starts_with('+')
                && !t.ends_with('.')
                && !t.contains(|c: char| c.is_ascii_whitespace())
                && !t.eq_ignore_ascii_case("inf")
                && !t.to_ascii_lowercase().contains("nan");
            if valid {
                t.to_owned()
            } else {
                String::new()
            }
        }
        Color => {
            let t = v;
            if t.len() == 7 && t.starts_with('#') && t[1..].bytes().all(|b| b.is_ascii_hexdigit()) {
                t.to_ascii_lowercase()
            } else {
                "#000000".to_owned()
            }
        }
        _ => v.to_owned(),
    }
}

impl Dom {
    fn html_local(&self, idx: u32) -> Option<&LocalName> {
        self.n(idx).element().filter(|e| e.is_html()).map(|e| &e.name.local)
    }
    fn is_input(&self, idx: u32) -> bool {
        self.html_local(idx) == Some(&local_name!("input"))
    }
    pub(crate) fn input_type_idx(&self, idx: u32) -> InputType {
        InputType::parse(self.n(idx).element().and_then(|e| e.attr_local(&local_name!("type"))))
    }
    pub fn input_type(&self, id: NodeId) -> DomResult<InputType> {
        let i = self.check(id)?;
        Ok(self.input_type_idx(i))
    }

    pub(crate) fn is_form_control_idx(&self, idx: u32) -> bool {
        matches!(
            self.html_local(idx).map(|l| &**l),
            Some("input" | "button" | "select" | "textarea" | "optgroup" | "option" | "fieldset")
        )
    }

    pub(crate) fn is_disabled_idx(&self, idx: u32) -> bool {
        let Some(local) = self.html_local(idx) else { return false };
        let e = self.n(idx).element().unwrap();
        match &**local {
            "option" => {
                if e.attr_local(&local_name!("disabled")).is_some() {
                    return true;
                }
                let p = self.n(idx).parent;
                p != NIL
                    && self.html_local(p) == Some(&local_name!("optgroup"))
                    && self.n(p).element().unwrap().attr_local(&local_name!("disabled")).is_some()
            }
            "optgroup" => e.attr_local(&local_name!("disabled")).is_some(),
            "input" | "button" | "select" | "textarea" | "fieldset" => {
                if e.attr_local(&local_name!("disabled")).is_some() {
                    return true;
                }
                // Descendant of a disabled fieldset, unless inside its first legend.
                let mut child = idx;
                let mut p = self.n(idx).parent;
                while p != NIL {
                    if self.html_local(p) == Some(&local_name!("fieldset"))
                        && self.n(p).element().unwrap().attr_local(&local_name!("disabled")).is_some()
                    {
                        let first_legend =
                            self.child_idx_iter(p).find(|&c| self.html_local(c) == Some(&local_name!("legend")));
                        if first_legend != Some(child) {
                            return true;
                        }
                    }
                    child = p;
                    p = self.n(p).parent;
                }
                false
            }
            _ => false,
        }
    }
    pub fn is_disabled(&self, id: NodeId) -> DomResult<bool> {
        Ok(self.is_disabled_idx(self.check(id)?))
    }

    /// Editable text control (input text-like or textarea), not readonly/disabled.
    pub(crate) fn is_mutable_text_idx(&self, idx: u32) -> bool {
        let Some(local) = self.html_local(idx) else { return false };
        let e = self.n(idx).element().unwrap();
        let textual = match &**local {
            "textarea" => true,
            "input" => matches!(self.input_type_idx(idx).value_mode(), ValueMode::Value),
            _ => {
                return e.attr_local(&local_name!("contenteditable")).is_some_and(|v| !v.eq_ignore_ascii_case("false"))
            }
        };
        textual && e.attr_local(&local_name!("readonly")).is_none() && !self.is_disabled_idx(idx)
    }

    pub(crate) fn checkedness_idx(&self, idx: u32) -> bool {
        if !self.is_input(idx) || !matches!(self.input_type_idx(idx), InputType::Checkbox | InputType::Radio) {
            return false;
        }
        let e = self.n(idx).element().unwrap();
        match &e.control {
            Some(c) if c.dirty_checked => c.checked.unwrap_or(false),
            _ => e.attr_local(&local_name!("checked")).is_some(),
        }
    }
    pub fn checked(&self, id: NodeId) -> DomResult<bool> {
        Ok(self.checkedness_idx(self.check(id)?))
    }

    /// `input.checked = v`; unchecks other radios in the group when set.
    pub fn set_checked(&mut self, id: NodeId, v: bool) -> DomResult<()> {
        let i = self.check(id)?;
        {
            let c = self.control_state_mut(id)?;
            c.checked = Some(v);
            c.dirty_checked = true;
        }
        if v && self.is_input(i) && self.input_type_idx(i) == InputType::Radio {
            for other in self.radio_group(i) {
                if other != i {
                    let oid = self.id_of(other);
                    let c = self.control_state_mut(oid)?;
                    c.checked = Some(false);
                    c.dirty_checked = true;
                }
            }
        }
        Ok(())
    }

    /// Form owner: `form` attribute target or nearest ancestor form.
    pub(crate) fn form_owner_idx(&self, idx: u32) -> u32 {
        let Some(e) = self.n(idx).element() else { return NIL };
        if let Some(fid) = e.attr_local(&local_name!("form")) {
            // The first element with that id in the tree: the tree scope's
            // id map (an empty id is never indexed: walk for it).
            let root = self.root_idx(idx);
            let found = if fid.is_empty() { self.walk_by_id(root, fid) } else { self.first_by_id(root, fid) };
            return if found != NIL && self.html_local(found) == Some(&local_name!("form")) { found } else { NIL };
        }
        let mut p = self.n(idx).parent;
        while p != NIL {
            if self.html_local(p) == Some(&local_name!("form")) {
                return p;
            }
            p = self.n(p).parent;
        }
        NIL
    }
    pub fn form_owner(&self, id: NodeId) -> DomResult<Option<NodeId>> {
        let i = self.check(id)?;
        Ok(self.opt_id(self.form_owner_idx(i)))
    }

    pub(crate) fn radio_group(&self, idx: u32) -> Vec<u32> {
        let Some(name) =
            self.n(idx).element().and_then(|e| e.attr_local(&local_name!("name"))).filter(|n| !n.is_empty())
        else {
            return vec![idx];
        };
        let form = self.form_owner_idx(idx);
        let root = self.root_idx(idx);
        let mut out = Vec::new();
        let mut cur = root;
        while cur != NIL {
            if self.is_input(cur)
                && self.input_type_idx(cur) == InputType::Radio
                && self.n(cur).element().unwrap().attr_local(&local_name!("name")) == Some(name)
                && self.form_owner_idx(cur) == form
            {
                out.push(cur);
            }
            cur = self.following_in(cur, root);
        }
        out
    }

    pub(crate) fn option_selected_idx(&self, idx: u32) -> bool {
        if self.html_local(idx) != Some(&local_name!("option")) {
            return false;
        }
        let select = self.option_select(idx);
        if select != NIL {
            return self.selected_options_idx(select).contains(&idx);
        }
        self.option_explicit(idx)
    }
    fn option_explicit(&self, idx: u32) -> bool {
        let e = self.n(idx).element().unwrap();
        match e.control.as_ref().and_then(|c| c.selected) {
            Some(s) => s,
            None => e.attr_local(&local_name!("selected")).is_some(),
        }
    }
    /// Before `node` leaves its parent: an option its select shows as
    /// selected only by default (the selectedness setting algorithm picked
    /// it) keeps that selectedness outside the select, as its selectedness
    /// is state of the option. Covers an option and the options of an
    /// optgroup.
    pub(crate) fn options_before_remove(&mut self, node: u32) {
        let (select, options): (u32, Vec<u32>) = match self.html_local(node).map(|l| &**l) {
            Some("option") => (self.option_select(node), vec![node]),
            Some("optgroup") => {
                let parent = self.n(node).parent;
                if parent == NIL || self.html_local(parent) != Some(&local_name!("select")) {
                    return;
                }
                let options =
                    self.child_idx_iter(node).filter(|&c| self.html_local(c) == Some(&local_name!("option"))).collect();
                (parent, options)
            }
            _ => return,
        };
        if select == NIL {
            return;
        }
        let selected = self.selected_options_idx(select);
        for option in options {
            if selected.contains(&option) && !self.option_explicit(option) {
                if let Some(e) = self.n_mut(option).element_mut() {
                    e.control.get_or_insert_with(Default::default).selected = Some(true);
                }
            }
        }
    }
    fn option_select(&self, opt: u32) -> u32 {
        let mut p = self.n(opt).parent;
        if p != NIL && self.html_local(p) == Some(&local_name!("optgroup")) {
            p = self.n(p).parent;
        }
        if p != NIL && self.html_local(p) == Some(&local_name!("select")) {
            p
        } else {
            NIL
        }
    }
    /// Options of a select (children and optgroup children), tree order.
    pub(crate) fn select_options_idx(&self, select: u32) -> Vec<u32> {
        let mut out = Vec::new();
        for c in self.child_idx_iter(select) {
            match self.html_local(c).map(|l| &**l) {
                Some("option") => out.push(c),
                Some("optgroup") => {
                    out.extend(self.child_idx_iter(c).filter(|&g| self.html_local(g) == Some(&local_name!("option"))))
                }
                _ => {}
            }
        }
        out
    }
    fn is_multiple(&self, select: u32) -> bool {
        self.n(select).element().unwrap().attr_local(&local_name!("multiple")).is_some()
    }
    /// Selectedness setting algorithm applied on read: single selects with
    /// display size 1 select their first non-disabled option when none is.
    pub(crate) fn selected_options_idx(&self, select: u32) -> Vec<u32> {
        let opts = self.select_options_idx(select);
        if self.is_multiple(select) {
            return opts.into_iter().filter(|&o| self.option_explicit(o)).collect();
        }
        if let Some(&last) = opts.iter().rev().find(|&&o| self.option_explicit(o)) {
            return vec![last];
        }
        // The selectedness setting algorithm runs when options come and go,
        // not after script deselected them all.
        let deselected = self.n(select).element().unwrap().control.as_ref().and_then(|c| c.no_selection.as_deref());
        if deselected == Some(opts.as_slice()) {
            return Vec::new();
        }
        let size = self
            .n(select)
            .element()
            .unwrap()
            .attr_local(&local_name!("size"))
            .and_then(|s| s.trim().parse::<u32>().ok())
            .unwrap_or(1);
        if size <= 1 {
            if let Some(&first) = opts.iter().find(|&&o| !self.is_disabled_idx(o)) {
                return vec![first];
            }
        }
        Vec::new()
    }
    pub fn select_options(&self, select: NodeId) -> DomResult<Vec<NodeId>> {
        let s = self.check(select)?;
        Ok(self.select_options_idx(s).into_iter().map(|i| self.id_of(i)).collect())
    }
    pub fn selected_options(&self, select: NodeId) -> DomResult<Vec<NodeId>> {
        let s = self.check(select)?;
        Ok(self.selected_options_idx(s).into_iter().map(|i| self.id_of(i)).collect())
    }
    pub fn selected_index(&self, select: NodeId) -> DomResult<i32> {
        let s = self.check(select)?;
        let opts = self.select_options_idx(s);
        Ok(match self.selected_options_idx(s).first() {
            Some(sel) => opts.iter().position(|o| o == sel).map_or(-1, |p| p as i32),
            None => -1,
        })
    }
    pub fn option_selected(&self, opt: NodeId) -> DomResult<bool> {
        Ok(self.option_selected_idx(self.check(opt)?))
    }
    /// `option.selected = v` (single select: deselects the others).
    pub fn set_option_selected(&mut self, opt: NodeId, v: bool) -> DomResult<()> {
        let o = self.check(opt)?;
        let select = self.option_select(o);
        if v && select != NIL && !self.is_multiple(select) {
            for other in self.select_options_idx(select) {
                let oid = self.id_of(other);
                self.control_state_mut(oid)?.selected = Some(false);
            }
        }
        self.control_state_mut(opt)?.selected = Some(v);
        // The setter asks the select for a reset.
        if select != NIL {
            self.set_no_selection(select, None);
        }
        Ok(())
    }
    /// Records that script left the options `opts` of `select` all
    /// deselected (`None`: the default selection applies again).
    fn set_no_selection(&mut self, select: u32, opts: Option<Vec<u32>>) {
        let Some(el) = self.n_mut(select).element_mut() else { return };
        match opts {
            Some(opts) => el.control.get_or_insert_with(Default::default).no_selection = Some(opts.into()),
            None => {
                if let Some(c) = el.control.as_mut() {
                    c.no_selection = None;
                }
            }
        }
    }
    pub fn set_selected_index(&mut self, select: NodeId, index: i32) -> DomResult<()> {
        let s = self.check(select)?;
        let opts = self.select_options_idx(s);
        for (k, &o) in opts.iter().enumerate() {
            let oid = self.id_of(o);
            self.control_state_mut(oid)?.selected = Some(k as i32 == index);
        }
        let none = index < 0 || index as usize >= opts.len();
        self.set_no_selection(s, none.then_some(opts));
        Ok(())
    }

    /// `option.value`: value attribute or stripped/collapsed text.
    pub fn option_value(&self, opt: NodeId) -> DomResult<String> {
        let o = self.check(opt)?;
        if let Some(v) = self.n(o).element().and_then(|e| e.attr_local(&local_name!("value"))) {
            return Ok(v.to_owned());
        }
        Ok(self.option_text_idx(o))
    }
    fn option_text_idx(&self, o: u32) -> String {
        let mut s = String::new();
        self.collect_text(o, &mut s);
        s.split_ascii_whitespace().collect::<Vec<_>>().join(" ")
    }
    pub fn option_text(&self, opt: NodeId) -> DomResult<String> {
        Ok(self.option_text_idx(self.check(opt)?))
    }

    pub(crate) fn control_value_idx(&self, idx: u32) -> String {
        self.value(self.id_of(idx)).unwrap_or_default()
    }

    /// `.value` getter for input/textarea/select/option/button.
    pub fn value(&self, id: NodeId) -> DomResult<String> {
        let i = self.check(id)?;
        let e = self.n(i).element().ok_or(DomError::Type("not an element"))?;
        let local = if e.is_html() { &*e.name.local } else { "" };
        Ok(match local {
            "input" => {
                let ty = self.input_type_idx(i);
                match ty.value_mode() {
                    ValueMode::Value => match e.control.as_ref().and_then(|c| c.value.as_ref()) {
                        Some(v) => v.clone(),
                        None => sanitize(ty, e.attr_local(&local_name!("value")).unwrap_or("")),
                    },
                    ValueMode::Default => e.attr_local(&local_name!("value")).unwrap_or("").to_owned(),
                    ValueMode::DefaultOn => e.attr_local(&local_name!("value")).unwrap_or("on").to_owned(),
                    ValueMode::Filename => e
                        .control
                        .as_ref()
                        .and_then(|c| c.value.clone())
                        .map_or(String::new(), |f| format!("C:\\fakepath\\{f}")),
                }
            }
            "textarea" => match e.control.as_ref().and_then(|c| c.value.as_ref()) {
                Some(v) => v.clone(),
                None => {
                    let mut s = String::new();
                    self.collect_text(i, &mut s);
                    s.replace("\r\n", "\n").replace('\r', "\n")
                }
            },
            "select" => match self.selected_options_idx(i).first() {
                Some(&o) => self.option_value(self.id_of(o))?,
                None => String::new(),
            },
            "option" => self.option_value(id)?,
            _ => e.attr_local(&local_name!("value")).unwrap_or("").to_owned(),
        })
    }

    /// `.value` setter. A changed text value moves selection to the end; an unchanged value preserves it.
    pub fn set_value(&mut self, id: NodeId, v: &str) -> DomResult<()> {
        let i = self.check(id)?;
        let local = self.html_local(i).cloned().ok_or(DomError::Type("not an HTML element"))?;
        match &*local {
            "input" => {
                let ty = self.input_type_idx(i);
                match ty.value_mode() {
                    ValueMode::Value => {
                        let v = sanitize(ty, v);
                        let changed = self.value(id)? != v;
                        let end = utf16_len(&v);
                        let c = self.control_state_mut(id)?;
                        c.value = Some(v);
                        c.dirty_value = true;
                        if changed {
                            c.selection = (end, end);
                            c.selection_set = true;
                        }
                    }
                    ValueMode::Default | ValueMode::DefaultOn => self.set_attribute(id, "value", v)?,
                    ValueMode::Filename => {
                        if !v.is_empty() {
                            return Err(DomError::new(
                                DomErrorKind::InvalidState,
                                "file input value can only be set to the empty string",
                            ));
                        }
                        self.control_state_mut(id)?.value = None;
                    }
                }
            }
            "textarea" => {
                let v = v.replace("\r\n", "\n").replace('\r', "\n");
                let changed = self.value(id)? != v;
                let end = utf16_len(&v);
                let c = self.control_state_mut(id)?;
                c.value = Some(v);
                c.dirty_value = true;
                if changed {
                    c.selection = (end, end);
                    c.selection_set = true;
                }
            }
            "select" => {
                let opts = self.select_options_idx(i);
                let mut found = false;
                for o in opts {
                    let oid = self.id_of(o);
                    let hit = !found && self.option_value(oid)? == v;
                    found |= hit;
                    self.control_state_mut(oid)?.selected = Some(hit);
                }
                let opts = self.select_options_idx(i);
                self.set_no_selection(i, (!found).then_some(opts));
            }
            _ => self.set_attribute(id, "value", v)?,
        }
        Ok(())
    }

    /// File input selection (CDP DOM.setFileInputFiles): records the first
    /// file name for `.value`; File objects/bytes are owned by bindings.
    pub fn set_file_names(&mut self, id: NodeId, names: &[String]) -> DomResult<()> {
        let c = self.control_state_mut(id)?;
        c.value = names.first().cloned();
        Ok(())
    }

    /// `selectionStart/End` of a text control.
    pub fn selection(&self, id: NodeId) -> DomResult<(u32, u32)> {
        let i = self.check(id)?;
        let len = utf16_len(&self.value(id)?);
        let e = self.n(i).element().ok_or(DomError::Type("not an element"))?;
        Ok(match &e.control {
            Some(c) if c.selection_set => (c.selection.0.min(len), c.selection.1.min(len)),
            // A fresh control has its cursor at the end of the default value.
            _ => (len, len),
        })
    }
    pub fn set_selection_range(&mut self, id: NodeId, start: u32, end: u32) -> DomResult<()> {
        let len = utf16_len(&self.value(id)?);
        let end = end.min(len);
        let start = start.min(end);
        let c = self.control_state_mut(id)?;
        c.selection = (start, end);
        c.selection_set = true;
        Ok(())
    }

    /// `selectionDirection`: `Some(true)` backward, `Some(false)` forward,
    /// `None` for "none" (no direction was given for the current selection).
    pub fn selection_direction(&self, id: NodeId) -> DomResult<Option<bool>> {
        let selection = self.selection(id)?;
        let i = self.check(id)?;
        let e = self.n(i).element().ok_or(DomError::Type("not an element"))?;
        Ok(e.control
            .as_ref()
            .and_then(|c| c.selection_direction)
            .filter(|(_, of)| *of == selection)
            .map(|(backward, _)| backward))
    }
    /// Gives the current selection a direction (`None`: "none"). A later
    /// selection change resets it.
    pub fn set_selection_direction(&mut self, id: NodeId, backward: Option<bool>) -> DomResult<()> {
        let selection = self.selection(id)?;
        let i = self.check(id)?;
        let e = self.n_mut(i).element_mut().ok_or(DomError::Type("not an element"))?;
        e.control.get_or_insert_with(Default::default).selection_direction = backward.map(|b| (b, selection));
        Ok(())
    }

    /// Replaces the current selection with `text` (typing, Input.insertText,
    /// or deletion with empty text). Returns true if the value changed.
    pub fn insert_text(&mut self, id: NodeId, text: &str) -> DomResult<bool> {
        let i = self.check(id)?;
        if !self.is_mutable_text_idx(i) || !matches!(self.html_local(i).map(|l| &**l), Some("input" | "textarea")) {
            return Ok(false);
        }
        let mut value = self.value(id)?;
        let (s, e) = self.selection(id)?;
        let is_input = self.html_local(i) == Some(&local_name!("input"));
        let text = if is_input { strip_newlines(text).into_owned() } else { text.to_owned() };
        if s == e && text.is_empty() {
            return Ok(false);
        }
        let (a, b) = utf16_range(&value, s, e - s)?;
        value.replace_range(a..b, &text);
        if let Some(max) = self
            .n(i)
            .element()
            .unwrap()
            .attr_local(&local_name!("maxlength"))
            .and_then(|m| m.trim().parse::<u32>().ok())
        {
            if utf16_len(&value) > max && !text.is_empty() {
                return Ok(false);
            }
        }
        let caret = s + utf16_len(&text);
        let c = self.control_state_mut(id)?;
        c.value = Some(value);
        c.dirty_value = true;
        c.user_edited = true;
        c.selection = (caret, caret);
        c.selection_set = true;
        Ok(true)
    }

    /// Backspace: deletes the selection or the code point before the caret.
    pub fn delete_backward(&mut self, id: NodeId) -> DomResult<bool> {
        let (s, e) = self.selection(id)?;
        if s == e {
            if s == 0 {
                return Ok(false);
            }
            let v = self.value(id)?;
            // Snap to the start of the code point ending at the caret.
            let (a, _) = utf16_range(&v, s - 1, 0)?;
            let start = utf16_len(&v[..a]);
            self.set_selection_range(id, start, s)?;
        }
        self.insert_text(id, "")
    }

    /// Delete key: deletes the selection or the code point after the caret.
    pub fn delete_forward(&mut self, id: NodeId) -> DomResult<bool> {
        let (s, e) = self.selection(id)?;
        if s == e {
            let v = self.value(id)?;
            if s >= utf16_len(&v) {
                return Ok(false);
            }
            let (a, _) = utf16_range(&v, s, 0)?;
            let ch = v[a..].chars().next().map_or(1, |c| c.len_utf16() as u32);
            self.set_selection_range(id, s, s + ch)?;
        }
        self.insert_text(id, "")
    }

    // ----- focus / hover --------------------------------------------------

    /// HTML's focus fixup rule: an element removed from the document is no
    /// longer the focused area (no blur event), so `activeElement` falls back
    /// to the body and the next key goes to the viewport, as in Chrome,
    /// instead of to a detached node.
    pub(crate) fn focused_idx(&self, doc: u32) -> u32 {
        match &self.n(doc).kind {
            NodeKind::Document(d)
                if d.focused != NIL && self.slot_live(d.focused) && self.is_connected_idx(d.focused) =>
            {
                d.focused
            }
            _ => NIL,
        }
    }
    /// The element the pointer is over (`:hover`), if any.
    pub fn hovered(&self, doc: NodeId) -> Option<NodeId> {
        let d = self.check(doc).ok()?;
        let i = self.hovered_idx(d);
        (i != NIL).then(|| self.id_of(i))
    }

    pub(crate) fn hovered_idx(&self, doc: u32) -> u32 {
        match &self.n(doc).kind {
            NodeKind::Document(d) if d.hovered != NIL && self.slot_live(d.hovered) => d.hovered,
            _ => NIL,
        }
    }

    /// Focusable-area check (HTML §6.6.2) without layout: "being rendered"
    /// is not evaluated here; callers combine with computed style.
    pub fn is_focusable(&self, id: NodeId) -> DomResult<bool> {
        let i = self.check(id)?;
        let Some(e) = self.n(i).element() else { return Ok(false) };
        if !self.is_connected(id)? || self.is_disabled_idx(i) {
            return Ok(false);
        }
        if e.attr_local(&local_name!("tabindex")).is_some_and(|t| t.trim().parse::<i32>().is_ok()) {
            return Ok(true);
        }
        if e.attr_local(&local_name!("contenteditable")).is_some_and(|v| !v.eq_ignore_ascii_case("false")) {
            return Ok(true);
        }
        Ok(e.is_html()
            && match &*e.name.local {
                "a" | "area" => e.attr_local(&local_name!("href")).is_some(),
                "input" => self.input_type_idx(i) != InputType::Hidden,
                "button" | "select" | "textarea" | "iframe" | "summary" => true,
                _ => false,
            })
    }

    /// Document's focused element (`activeElement` is body when None).
    pub fn focused(&self, doc: NodeId) -> DomResult<Option<NodeId>> {
        let d = self.check(doc)?;
        Ok(self.opt_id(self.focused_idx(d)))
    }
    /// Sets the focused element. Event dispatch (blur/focusout/focus/focusin)
    /// is the caller's responsibility; returns the previously focused node.
    pub fn set_focused(&mut self, doc: NodeId, el: Option<NodeId>) -> DomResult<Option<NodeId>> {
        let d = self.check(doc)?;
        let new = match el {
            Some(e) => self.check(e)?,
            None => NIL,
        };
        let prev = self.focused_idx(d);
        if prev == new {
            // Refocusing the focused element changes nothing styles see.
            return Ok(self.opt_id(prev));
        }
        if let NodeKind::Document(dd) = &mut self.n_mut(d).kind {
            dd.focused = new;
        }
        // :focus / :focus-within participate in style matching (the style
        // cache compares the focused elements).
        self.bump();
        self.note(crate::dom::Change::Pseudo);
        Ok(self.opt_id(prev))
    }
    pub fn set_hovered(&mut self, doc: NodeId, el: Option<NodeId>) -> DomResult<()> {
        let d = self.check(doc)?;
        let new = match el {
            Some(e) => self.check(e)?,
            None => NIL,
        };
        if self.hovered_idx(d) == new {
            return Ok(());
        }
        if let NodeKind::Document(dd) = &mut self.n_mut(d).kind {
            dd.hovered = new;
        }
        self.bump();
        self.note(crate::dom::Change::Pseudo);
        Ok(())
    }

    /// Whether the element is a candidate for constraint validation.
    pub fn will_validate(&self, id: NodeId) -> DomResult<bool> {
        let i = self.check(id)?;
        let Some(local) = self.html_local(i) else { return Ok(false) };
        let e = self.n(i).element().unwrap();
        let listed = match &**local {
            "input" => !matches!(self.input_type_idx(i), InputType::Hidden | InputType::Reset | InputType::Button),
            "select" | "textarea" => true,
            "button" => !matches!(
                e.attr_local(&local_name!("type")).map(|t| t.to_ascii_lowercase()).as_deref(),
                Some("reset" | "button")
            ),
            _ => false,
        };
        if !listed
            || self.is_disabled_idx(i)
            || (e.attr_local(&local_name!("readonly")).is_some() && &**local != "select")
        {
            return Ok(false);
        }
        // Not inside a <datalist>.
        let mut p = self.n(i).parent;
        while p != NIL {
            if self.html_local(p).is_some_and(|l| &**l == "datalist") {
                return Ok(false);
            }
            p = self.n(p).parent;
        }
        Ok(true)
    }

    /// `ValidityState` flags (constraint validation, HTML §4.10.20.1).
    /// `patternMismatch`, `stepMismatch` and `badInput` are not evaluated.
    pub fn validity(&self, id: NodeId) -> DomResult<Validity> {
        let mut v = Validity::default();
        if !self.will_validate(id)? {
            return Ok(v);
        }
        let i = self.check(id)?;
        let e = self.n(i).element().unwrap();
        let local = self.html_local(i).map(|l| l.to_string()).unwrap_or_default();
        v.custom_error =
            e.control.as_ref().is_some_and(|c| c.custom_validity.as_deref().is_some_and(|m| !m.is_empty()));
        let required = e.attr_local(&local_name!("required")).is_some();
        let value = self.value(id)?;
        match local.as_str() {
            "input" => {
                let ty = self.input_type_idx(i);
                match ty {
                    InputType::Checkbox => v.value_missing = required && !self.checkedness_idx(i),
                    InputType::Radio => {
                        v.value_missing = required && !self.radio_group(i).into_iter().any(|r| self.checkedness_idx(r))
                    }
                    InputType::File => {
                        v.value_missing = required && e.control.as_ref().and_then(|c| c.value.as_ref()).is_none()
                    }
                    _ => v.value_missing = required && value.is_empty(),
                }
                if !value.is_empty() {
                    match ty {
                        InputType::Email => {
                            let check = |a: &str| {
                                let Some((l, d)) = a.split_once('@') else { return false };
                                !l.is_empty()
                                    && !d.is_empty()
                                    && !d.starts_with('.')
                                    && !d.ends_with('.')
                                    && !a.contains(char::is_whitespace)
                            };
                            v.type_mismatch = if e.attr_local(&local_name!("multiple")).is_some() {
                                !value.split(',').all(|a| check(a.trim()))
                            } else {
                                !check(&value)
                            };
                        }
                        InputType::Url => v.type_mismatch = !value.contains(':') || value.starts_with(':'),
                        InputType::Number | InputType::Range => {
                            if let Ok(n) = value.parse::<f64>() {
                                if let Some(min) = e.attr_local(&local_name!("min")).and_then(|m| m.parse::<f64>().ok())
                                {
                                    v.range_underflow = n < min;
                                }
                                if let Some(max) = e.attr_local(&local_name!("max")).and_then(|m| m.parse::<f64>().ok())
                                {
                                    v.range_overflow = n > max;
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            "textarea" => v.value_missing = required && value.is_empty(),
            "select" => {
                v.value_missing = required && {
                    let sel = self.selected_options_idx(i);
                    // Placeholder label option (first, empty value, size 1 single select).
                    sel.is_empty()
                        || (e.attr_local(&local_name!("multiple")).is_none()
                            && sel.first() == self.select_options_idx(i).first()
                            && self.option_value(self.id_of(sel[0]))?.is_empty())
                };
            }
            _ => {}
        }
        // tooLong/tooShort only apply after user edits (dirty value flag).
        let dirty = e.control.as_ref().is_some_and(|c| c.dirty_value && c.user_edited);
        if dirty && !value.is_empty() {
            let len = crate::dom::utf16_len(&value) as i64;
            if let Some(max) = e.attr_local(&local_name!("maxlength")).and_then(|m| m.trim().parse::<i64>().ok()) {
                v.too_long = len > max;
            }
            if let Some(min) = e.attr_local(&local_name!("minlength")).and_then(|m| m.trim().parse::<i64>().ok()) {
                v.too_short = len < min;
            }
        }
        Ok(v)
    }

    /// `setCustomValidity(message)`.
    pub fn set_custom_validity(&mut self, id: NodeId, message: &str) -> DomResult<()> {
        let c = self.control_state_mut(id)?;
        c.custom_validity = (!message.is_empty()).then(|| message.to_owned());
        Ok(())
    }
    pub fn custom_validity(&self, id: NodeId) -> DomResult<Option<&str>> {
        let i = self.check(id)?;
        Ok(self.n(i).element().and_then(|e| e.control.as_ref()).and_then(|c| c.custom_validity.as_deref()))
    }
}

/// Constraint-validation flags.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Validity {
    pub value_missing: bool,
    pub type_mismatch: bool,
    pub pattern_mismatch: bool,
    pub too_long: bool,
    pub too_short: bool,
    pub range_underflow: bool,
    pub range_overflow: bool,
    pub step_mismatch: bool,
    pub bad_input: bool,
    pub custom_error: bool,
}

impl Validity {
    pub fn valid(&self) -> bool {
        *self == Validity::default()
    }
}
