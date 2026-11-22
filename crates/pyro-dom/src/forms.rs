//! Form-control state: value/checkedness/selectedness/disabled, focus and
//! text editing primitives used by input dispatch (HTML §4.10).
//! All offsets are UTF-16 code units, as exposed to JS.

use std::cell::Cell;
use std::rc::{Rc, Weak};

use markup5ever::{local_name, LocalName};

use crate::dom::{utf16_len, utf16_range, Dom};
use crate::error::{DomError, DomErrorKind, DomResult};
use crate::node::{NodeId, NodeKind, NIL};

/// Selection offsets and direction used by text-control editing history.
#[derive(Clone, Copy)]
pub struct ControlSelection {
    pub range: (u32, u32),
    pub direction: Option<bool>,
}

/// Live UTF-16 boundaries in a form control's value space. The public range
/// does not expose the control's UA-internal text nodes.
pub struct OpaqueRange {
    element: Cell<Option<NodeId>>,
    offsets: Cell<(u32, u32)>,
    release_control: Cell<Option<Box<dyn FnOnce()>>>,
}

impl OpaqueRange {
    pub fn element(&self) -> Option<NodeId> {
        self.element.get()
    }

    pub fn offsets(&self) -> (u32, u32) {
        self.offsets.get()
    }

    /// The binding holds the associated control, including a detached one,
    /// until the range is disconnected or its last wrapper is collected.
    pub fn hold_control(&self, release: Box<dyn FnOnce()>) {
        self.release_control.set(Some(release));
    }

    pub fn disconnect(&self) {
        self.element.set(None);
        self.offsets.set((0, 0));
        if let Some(release) = self.release_control.take() {
            release();
        }
    }
}

impl Drop for OpaqueRange {
    fn drop(&mut self) {
        self.disconnect();
    }
}

impl Dom {
    pub fn create_value_range(&mut self, id: NodeId, start: u32, end: u32) -> DomResult<Rc<OpaqueRange>> {
        let i = self.check(id)?;
        let supported = self.html_local(i) == Some(&local_name!("textarea"))
            || self.is_input(i) && self.input_type_idx(i).supports_value_ranges();
        if !supported {
            return Err(DomError::new(DomErrorKind::NotSupported, "control does not support value ranges"));
        }
        let length = utf16_len(&self.value(id)?);
        if start > length || end > length {
            return Err(DomError::new(DomErrorKind::IndexSize, "offset exceeds the control value length"));
        }
        let range = Rc::new(OpaqueRange {
            element: Cell::new(Some(id)),
            offsets: Cell::new((start, end.max(start))),
            release_control: Cell::new(None),
        });
        let ranges = self.value_ranges.entry(i).or_default();
        if ranges.len() == ranges.capacity() {
            ranges.retain(|weak| weak.upgrade().is_some_and(|r| r.element().is_some()));
        }
        ranges.push(Rc::downgrade(&range));
        Ok(range)
    }

    fn change_value_ranges(&mut self, i: u32, adjust: impl Fn(u32) -> u32) {
        let Some(ranges) = self.value_ranges.get_mut(&i) else { return };
        ranges.retain(|weak| {
            let Some(r) = weak.upgrade().filter(|r| r.element().is_some()) else { return false };
            let (start, end) = r.offsets();
            r.offsets.set((adjust(start), adjust(end)));
            true
        });
        if ranges.is_empty() {
            self.value_ranges.remove(&i);
        }
    }

    pub(crate) fn replace_all_value_ranges(&mut self, i: u32) {
        self.change_value_ranges(i, |_| 0);
    }

    fn edit_value_ranges(&mut self, i: u32, value: &str, bytes: std::ops::Range<usize>, replacement: &str) {
        if !self.value_ranges.contains_key(&i) || &value[bytes.clone()] == replacement {
            return;
        }
        // Use the actual edited span (the UTF-8-backed control editor may
        // snap an offset within a surrogate pair to a scalar boundary).
        let start = utf16_len(&value[..bytes.start]);
        let end = start + utf16_len(&value[bytes]);
        let added = utf16_len(replacement);
        self.change_value_ranges(i, |offset| {
            if offset <= start { offset }
            else if offset < end { start }
            else { (offset - (end - start)).saturating_add(added) }
        });
    }

    pub(crate) fn disconnect_value_ranges(&mut self, i: u32) {
        if let Some(ranges) = self.value_ranges.remove(&i) {
            for range in ranges.iter().filter_map(Weak::upgrade) {
                range.disconnect();
            }
        }
    }

    pub(crate) fn disconnect_subtree_value_ranges(&mut self, root: u32) {
        let controls: Vec<_> = self.value_ranges.keys().copied()
            .filter(|&i| self.is_host_including_inclusive_ancestor(root, i)).collect();
        for i in controls {
            self.disconnect_value_ranges(i);
        }
    }

    /// Only direct text children supply a clean textarea's default value.
    pub(crate) fn tracked_textarea_value(&self, parent: u32) -> Option<(NodeId, String)> {
        if !self.value_ranges.contains_key(&parent) || self.html_local(parent) != Some(&local_name!("textarea")) {
            return None;
        }
        let el = self.n(parent).element()?;
        if el.control.as_ref().is_some_and(|control| control.value.is_some()) {
            return None;
        }
        let id = self.id_of(parent);
        Some((id, self.value(id).ok()?))
    }

    pub(crate) fn update_tracked_textarea(&mut self, old: Option<(NodeId, String)>) {
        if let Some((id, old)) = old {
            if self.value(id).is_ok_and(|value| value != old) {
                self.replace_all_value_ranges(id.idx);
            }
        }
    }

    /// `<output>`'s `defaultValue`: its default value override, else its descendant text content.
    pub fn output_default_value(&self, id: NodeId) -> DomResult<String> {
        match self.control_state(id)?.and_then(|c| c.output_default_override.clone()) {
            Some(value) => Ok(value),
            None => Ok(self.text_content(id)?.unwrap_or_default()),
        }
    }

    /// Setting `<output>`'s `defaultValue`: without an override it replaces the element's
    /// children (string replace all), else it becomes the override.
    pub fn set_output_default_value(&mut self, id: NodeId, value: &str) -> DomResult<()> {
        if self.control_state(id)?.is_some_and(|c| c.output_default_override.is_some()) {
            self.control_state_mut(id)?.output_default_override = Some(value.to_owned());
            return Ok(());
        }
        self.set_text_content(id, value)
    }

    /// Setting `<output>`'s `value`: the default value is kept as the override first, then
    /// the value replaces the element's children.
    pub fn set_output_value(&mut self, id: NodeId, value: &str) -> DomResult<()> {
        if !self.control_state(id)?.is_some_and(|c| c.output_default_override.is_some()) {
            let current = self.text_content(id)?.unwrap_or_default();
            self.control_state_mut(id)?.output_default_override = Some(current);
        }
        self.set_text_content(id, value)
    }

    /// `<output>`'s reset algorithm: its children become its default value and the override
    /// is cleared.
    pub fn reset_output(&mut self, id: NodeId) -> DomResult<()> {
        let default = self.output_default_value(id)?;
        self.set_text_content(id, &default)?;
        if self.control_state(id)?.is_some_and(|c| c.output_default_override.is_some()) {
            self.control_state_mut(id)?.output_default_override = None;
        }
        Ok(())
    }

    /// Restore default value, checkedness and selectedness while retaining
    /// custom validity. A changed relevant value is a wholesale replacement.
    pub fn reset_control(&mut self, id: NodeId) -> DomResult<()> {
        let i = self.check(id)?;
        if self.control_state(id)?.is_none() { return Ok(()) }
        let before = self.value_ranges.contains_key(&i).then(|| self.value(id)).transpose()?;
        let state = self.control_state_mut(id)?;
        let validity = state.custom_validity.take();
        *state = Default::default();
        state.custom_validity = validity;
        if let Some(before) = before {
            if self.value(id)? != before { self.replace_all_value_ranges(i); }
        }
        Ok(())
    }
}

/// HTML "rules for parsing integers". Saturates: callers range-check against
/// values far inside `i64`.
pub fn parse_html_int(input: &str) -> Option<i64> {
    let bytes = input.trim_start_matches([' ', '\t', '\n', '\x0c', '\r']).as_bytes();
    let (negative, digits) = match bytes.first()? {
        b'-' => (true, &bytes[1..]),
        b'+' => (false, &bytes[1..]),
        _ => (false, bytes),
    };
    let len = digits.iter().take_while(|c| c.is_ascii_digit()).count();
    if len == 0 {
        return None;
    }
    let value = digits[..len].iter().fold(0i64, |v, d| v.saturating_mul(10).saturating_add((d - b'0') as i64));
    Some(if negative { -value } else { value })
}

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
    pub(crate) fn supports_value_ranges(self) -> bool {
        matches!(self, Self::Text | Self::Search | Self::Tel | Self::Url | Self::Password)
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

/// HTML "valid floating-point number", parsed: no whitespace, no leading
/// `+`, no trailing `.`, finite.
pub fn parse_float(t: &str) -> Option<f64> {
    let b = t.as_bytes();
    let first = *b.first()?;
    if !(first == b'-' || first == b'.' || first.is_ascii_digit())
        || t.ends_with('.')
        || !b.iter().all(|c| c.is_ascii_digit() || matches!(c, b'-' | b'+' | b'.' | b'e' | b'E'))
    {
        return None;
    }
    t.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// Value sanitization algorithm of every state but Range, whose result
/// depends on the element's attributes (`Dom::range_value`).
fn sanitize(ty: InputType, v: &str) -> String {
    use InputType::*;
    let keep_if = |ok: bool| if ok { v.to_owned() } else { String::new() };
    match ty {
        Text | Search | Tel | Password => strip_newlines(v).into_owned(),
        Url | Email => strip_newlines(v).trim_matches(|c: char| c.is_ascii_whitespace()).to_owned(),
        Number => keep_if(parse_float(v).is_some()),
        Date => keep_if(datetime::parse_date(v).is_some()),
        Month => keep_if(datetime::parse_month(v).is_some()),
        Week => keep_if(datetime::parse_week(v).is_some()),
        Time => keep_if(datetime::parse_time(v).is_some()),
        // Normalized: `T`, and the shortest time.
        DateTimeLocal => {
            datetime::parse_local_datetime(v).and_then(datetime::format_local_datetime).unwrap_or_default()
        }
        Color => crate::color_well::sanitize(v, false, false),
        _ => v.to_owned(),
    }
}

impl InputType {
    /// States whose value converts to a number: `valueAsNumber`, `min`/`max`,
    /// `step` and `stepUp()`/`stepDown()` apply.
    pub fn is_numeric(self) -> bool {
        use InputType::*;
        matches!(self, Date | Month | Week | Time | DateTimeLocal | Number | Range)
    }
    /// `valueAsDate` applies.
    pub fn has_date_value(self) -> bool {
        use InputType::*;
        matches!(self, Date | Month | Week | Time)
    }
    /// `maxlength`, `minlength` and `pattern` apply (HTML's input attribute
    /// table); so do the first two on a textarea.
    pub fn has_length_limits(self) -> bool {
        use InputType::*;
        matches!(self, Text | Search | Url | Tel | Email | Password)
    }
    /// "Convert a string to a number" of the state (None: an error).
    pub fn string_to_number(self, s: &str) -> Option<f64> {
        use InputType::*;
        match self {
            Number | Range => parse_float(s),
            Date => datetime::parse_date(s),
            Month => datetime::parse_month(s),
            Week => datetime::parse_week(s),
            Time => datetime::parse_time(s),
            DateTimeLocal => datetime::parse_local_datetime(s),
            _ => None,
        }
    }
    /// "Convert a number to a string" of the state (None: not representable).
    pub fn number_to_string(self, x: f64) -> Option<String> {
        use InputType::*;
        if !x.is_finite() {
            return None;
        }
        match self {
            Number | Range => Some(format_float(x, None)),
            Date => datetime::format_date(x),
            Month => datetime::format_month(x),
            Week => datetime::format_week(x),
            Time => datetime::format_time(x),
            DateTimeLocal => datetime::format_local_datetime(x),
            _ => None,
        }
    }
    /// (default step, step scale factor, default step base).
    fn step_description(self) -> (f64, f64, f64) {
        use InputType::*;
        match self {
            Date => (1.0, datetime::MS_PER_DAY, 0.0),
            Week => (1.0, 7.0 * datetime::MS_PER_DAY, -259_200_000.0),
            Time | DateTimeLocal => (60.0, 1000.0, 0.0),
            _ => (1.0, 1.0, 0.0),
        }
    }
}

/// `x` as a valid floating-point number, rounded to `decimals` places when
/// given (stepping in binary floating point: 0.2 + 0.1 gives "0.3", as
/// Chrome's decimal arithmetic does).
fn format_float(x: f64, decimals: Option<usize>) -> String {
    let s = match decimals {
        Some(d) => {
            let s = format!("{x:.d$}");
            if s.contains('.') {
                s.trim_end_matches('0').trim_end_matches('.').to_owned()
            } else {
                s
            }
        }
        None => js_number(x),
    };
    if s == "-0" {
        "0".into()
    } else {
        s
    }
}

/// ECMAScript Number::toString of a finite `x`, which "best representation
/// as a floating-point number" means: exponent notation below 1e-6 and
/// from 1e21 ("1e+21", as Chrome's `valueAsNumber` setter writes).
fn js_number(x: f64) -> String {
    if x == 0.0 {
        return "0".into();
    }
    // Shortest round-trip digits, in exponent form: "1.5e-7".
    let sci = format!("{:e}", x.abs());
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let k = digits.len() as i32;
    let n = exp.parse::<i32>().unwrap_or(0) + 1;
    let body = if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 21 {
        format!("{}.{}", &digits[..n as usize], &digits[n as usize..])
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat((-n) as usize))
    } else {
        let rest = if k > 1 { format!(".{}", &digits[1..]) } else { String::new() };
        format!("{}{rest}e{}{}", &digits[..1], if n > 0 { '+' } else { '-' }, (n - 1).abs())
    };
    if x < 0.0 {
        format!("-{body}")
    } else {
        body
    }
}

/// Digits after the decimal point of `x`'s shortest representation.
fn decimals_of(x: f64) -> usize {
    let s = format!("{x}");
    s.find('.').map_or(0, |p| s.len() - p - 1)
}

/// What `stepUp()`/`stepDown()`, step mismatch and range sanitization work
/// with, in the state's number space.
struct StepRange {
    min: Option<f64>,
    max: Option<f64>,
    /// None: `step=any`, no allowed value step.
    step: Option<f64>,
    base: f64,
}

impl StepRange {
    fn mismatch(&self, v: f64) -> bool {
        let Some(step) = self.step else { return false };
        let d = v - self.base;
        let q = d / step;
        // A step too small (or too large) to count steps in binary floating
        // point: no value can be shown to mismatch it.
        if !q.is_finite() || !(step * q.round()).is_finite() {
            return false;
        }
        // Chrome's tolerance for binary rounding (StepRange::StepMismatch).
        (d - step * q.round()).abs() > step / 2f64.powi(46)
    }
    /// The aligned value at or below `v`.
    fn floor(&self, v: f64, step: f64) -> f64 {
        self.base + ((v - self.base) / step + 1e-9).floor() * step
    }
    /// The aligned value at or above `v`.
    fn ceil(&self, v: f64, step: f64) -> f64 {
        self.base + ((v - self.base) / step - 1e-9).ceil() * step
    }
}

/// Whether `value` does not match the `pattern` attribute `p`: the whole
/// value against `p` compiled with the `v` flag. A pattern that does not
/// compile is ignored, as in browsers. Compiled per check: only elements
/// with a pattern pay for it.
fn pattern_mismatch(p: &str, value: &str) -> bool {
    match regress::Regex::with_flags(&format!("^(?:{p})$"), "v") {
        Ok(re) => re.find(value).is_none(),
        Err(_) => false,
    }
}

/// A valid e-mail address (HTML's definition, which Chrome checks).
fn is_valid_email(a: &str) -> bool {
    let Some((local, domain)) = a.split_once('@') else { return false };
    let local_ok =
        !local.is_empty() && local.bytes().all(|b| b.is_ascii_alphanumeric() || b".!#$%&'*+/=?^_`{|}~-".contains(&b));
    let label_ok = |l: &str| {
        let b = l.as_bytes();
        !b.is_empty()
            && b.len() <= 63
            && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'-')
            && b[0] != b'-'
            && b[b.len() - 1] != b'-'
    };
    local_ok && domain.split('.').all(label_ok)
}

mod datetime;

/// An `<input>`'s state before its type attribute is written.
pub(crate) struct InputTypeBefore {
    ty: InputType,
    /// The element's value in value mode (else unused).
    value: String,
}

impl Dom {
    pub(crate) fn input_type_before_change(&self, i: u32) -> InputTypeBefore {
        let ty = self.input_type_idx(i);
        let value = if ty.value_mode() == ValueMode::Value {
            self.value(self.id_of(i)).unwrap_or_default()
        } else {
            String::new()
        };
        InputTypeBefore { ty, value }
    }

    /// HTML "type attribute state change" value steps: move the value
    /// between the value modes, then the new state's value sanitization
    /// (stored sanitized only for free text; other states sanitize on
    /// read). Setting the value content attribute waits for the type
    /// mutation to be queued (`apply_pending_input_value_attr`).
    pub(crate) fn input_type_changed(&mut self, i: u32, before: InputTypeBefore) {
        let new = self.input_type_idx(i);
        if new == before.ty {
            return;
        }
        let (old_mode, new_mode) = (before.ty.value_mode(), new.value_mode());
        if old_mode == ValueMode::Value && matches!(new_mode, ValueMode::Default | ValueMode::DefaultOn) {
            if !before.value.is_empty() {
                self.pending_input_value_attr = Some((i, before.value));
            }
            return;
        }
        let has_control = self.n(i).element().is_some_and(|e| e.control.is_some());
        if !has_control {
            return;
        }
        let id = self.id_of(i);
        if old_mode != new_mode {
            // Into value mode: the value content attribute, not dirty.
            // Into or out of filename mode: no value (no selected files).
            let Ok(c) = self.control_state_mut(id) else { return };
            c.value = None;
            if new_mode == ValueMode::Value {
                c.dirty_value = false;
            }
            return;
        }
        if new_mode != ValueMode::Value || !new.has_length_limits() {
            return;
        }
        let Some(stored) = self.n(i).element().and_then(|e| e.control.as_ref()).and_then(|c| c.value.clone()) else {
            return;
        };
        let sanitized = sanitize(new, &stored);
        if sanitized != stored {
            // A changed value moves the cursor to the end, direction "none".
            let end = utf16_len(&sanitized);
            let Ok(c) = self.control_state_mut(id) else { return };
            c.value = Some(sanitized);
            c.selection = (end, end);
            c.selection_set = true;
        }
    }

    /// The value content attribute an `<input>`'s type change left to set.
    pub(crate) fn apply_pending_input_value_attr(&mut self, i: u32) {
        if let Some((_, value)) = self.pending_input_value_attr.take_if(|(idx, _)| *idx == i) {
            let _ = self.set_attribute(self.id_of(i), "value", &value);
        }
    }
}

impl Dom {
    pub(crate) fn html_local(&self, idx: u32) -> Option<&LocalName> {
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

    /// The `button` element's `type` state when it is Submit Button: the
    /// keyword `submit`, or the Auto state (missing or invalid `type`)
    /// without a `command` or `commandfor` attribute (HTML "submit button").
    fn button_type_is_submit_idx(&self, idx: u32) -> bool {
        let Some(e) = self.n(idx).element() else { return false };
        match e.attr_local(&local_name!("type")) {
            Some(t) if t.eq_ignore_ascii_case("submit") => true,
            Some(t) if t.eq_ignore_ascii_case("reset") || t.eq_ignore_ascii_case("button") => false,
            _ => {
                e.attr_local(&local_name!("command")).is_none()
                    && e.attr_local(&LocalName::from("commandfor")).is_none()
            }
        }
    }

    /// Whether `<button>` `id` is a submit button (its `type` state is
    /// Submit Button, or Auto without `command`/`commandfor`).
    pub fn button_is_submit(&self, id: NodeId) -> bool {
        self.check(id).is_ok_and(|i| self.button_type_is_submit_idx(i))
    }

    pub(crate) fn is_form_control_idx(&self, idx: u32) -> bool {
        matches!(
            self.html_local(idx).map(|l| &**l),
            Some("input" | "button" | "select" | "textarea" | "optgroup" | "option" | "fieldset")
        )
    }

    /// Whether `required` applies to element `idx`, which `:required` and `:optional` match
    /// (HTML §4.16.3): a `select`, a `textarea`, or an `input` of a type it applies to (not
    /// Hidden, Range, Color or the buttons).
    pub(crate) fn required_applies_idx(&self, idx: u32) -> bool {
        match self.html_local(idx).map(|l| &**l) {
            Some("select" | "textarea") => true,
            Some("input") => !matches!(
                self.input_type_idx(idx),
                InputType::Hidden
                    | InputType::Range
                    | InputType::Color
                    | InputType::Submit
                    | InputType::Image
                    | InputType::Reset
                    | InputType::Button
            ),
            _ => false,
        }
    }

    pub(crate) fn is_disabled_idx(&self, idx: u32) -> bool {
        let Some(local) = self.html_local(idx) else { return false };
        let e = self.n(idx).element().unwrap();
        match &**local {
            // As Chromium: a disabled select disables its options and optgroups too.
            "option" => {
                if self.option_disabled_idx(idx) {
                    return true;
                }
                let select = self.option_nearest_ancestor_select(idx);
                select != NIL && self.is_disabled_idx(select)
            }
            "optgroup" => {
                if e.attr_local(&local_name!("disabled")).is_some() {
                    return true;
                }
                let select = self.subtree_ancestor_select(idx);
                select != NIL && self.is_disabled_idx(select)
            }
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
            _ => self.face_disabled_idx(idx),
        }
    }
    /// An option's own disabledness (HTML): its `disabled` attribute, or
    /// its nearest optgroup's, past wrapper elements but not past a select,
    /// datalist, hr or option. The selectedness setting algorithm skips
    /// these options.
    pub(crate) fn option_disabled_idx(&self, idx: u32) -> bool {
        if self.n(idx).element().is_some_and(|e| e.attr_local(&local_name!("disabled")).is_some()) {
            return true;
        }
        let mut p = self.n(idx).parent;
        while p != NIL {
            match self.html_local(p).map(|l| &**l) {
                Some("optgroup") => return self.n(p).element().unwrap().attr_local(&local_name!("disabled")).is_some(),
                Some("select" | "datalist" | "hr" | "option") => return false,
                _ => {}
            }
            p = self.n(p).parent;
        }
        false
    }
    pub fn is_disabled(&self, id: NodeId) -> DomResult<bool> {
        Ok(self.is_disabled_idx(self.check(id)?))
    }

    /// Whether a text edit at this control is permitted by its current state.
    pub fn is_mutable_text(&self, id: NodeId) -> DomResult<bool> {
        Ok(self.is_mutable_text_idx(self.check(id)?))
    }

    /// Editable text control (input text-like or textarea), not readonly/disabled.
    pub(crate) fn is_mutable_text_idx(&self, idx: u32) -> bool {
        let Some(local) = self.html_local(idx) else { return false };
        let e = self.n(idx).element().unwrap();
        let textual = match &**local {
            "textarea" => true,
            "input" => matches!(self.input_type_idx(idx).value_mode(), ValueMode::Value),
            _ => {
                return e.attr_local(&local_name!("contenteditable")).is_some_and(|v| !v.eq_ignore_ascii_case("false"));
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
            if !self.is_connected_idx(idx) {
                return NIL;
            }
            // The first element with that id in the tree: the tree scope's
            // id map (an empty id is never indexed: walk for it).
            let root = self.root_idx(idx);
            let found = if fid.is_empty() { self.walk_by_id(root, fid) } else { self.first_by_id(root, fid) };
            let found = self.resolve_reference_target_idx(found);
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

    /// Controls with this owner, including external references to an exposed
    /// shadow form, in shadow-including tree order.
    pub fn owned_form_elements(&self, form: NodeId) -> DomResult<Vec<NodeId>> {
        let f = self.check(form)?;
        let root = self.reference_scope_idx(f);
        let cross_scope = root != self.root_idx(f);
        let mut out = Vec::new();
        let mut node = root;
        while node != NIL {
            if (self.html_local(node).is_some_and(|l| matches!(&**l,
                "button" | "fieldset" | "input" | "object" | "output" | "select" | "textarea" | "img"))
                || self.is_face_idx(node))
                && self.form_owner_idx(node) == f
            {
                out.push(self.id_of(node));
            }
            node = if cross_scope { self.next_shadow_including(node, root) } else { self.following_in(node, root) };
        }
        Ok(out)
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
    /// is state of the option. Covers an option and every option of the
    /// select's list of options inside `node` (an optgroup, a wrapper div).
    pub(crate) fn options_before_remove(&mut self, node: u32) {
        if self.options_batch_removal {
            return;
        }
        // 0: nothing, 1: an option, 2: an element that can hold options.
        let kind = match self.html_local(node).map(|l| &**l) {
            Some("option") => 1,
            // Elements whose options are never their ancestor select's.
            Some("select" | "datalist" | "hr") => 0,
            _ if self.n(node).element().is_none() || self.n(node).first_child == NIL => 0,
            _ => 2,
        };
        match kind {
            1 => {
                let select = self.option_select(node);
                if select != NIL {
                    self.keep_default_selectedness(select, &[node]);
                    self.queue_selectedcontent_update(select, false);
                }
            }
            2 => self.subtree_options_before_remove(node),
            _ => {}
        }
    }
    /// [`Self::options_before_remove`] for an element that can hold options
    /// of a select around it.
    fn subtree_options_before_remove(&mut self, node: u32) {
        let select = self.subtree_ancestor_select(node);
        if select == NIL {
            return;
        }
        let options: Vec<u32> =
            self.select_options_idx(select).into_iter().filter(|&o| self.is_inclusive_ancestor_idx(node, o)).collect();
        if !options.is_empty() {
            self.keep_default_selectedness(select, &options);
            self.queue_selectedcontent_update(select, false);
        }
    }
    /// [`Self::options_before_remove`] for every child of `parent` at once
    /// (`replace_all`), with the selectedness from before any removal.
    /// Returns whether the children's removals can skip it.
    pub(crate) fn options_before_remove_children(&mut self, parent: u32) -> bool {
        let select = if self.html_local(parent) == Some(&local_name!("select")) {
            parent
        } else {
            match self.html_local(parent).map(|l| &**l) {
                Some("datalist" | "hr" | "option") => return true,
                _ => self.subtree_ancestor_select(parent),
            }
        };
        if select == NIL {
            return true;
        }
        let options: Vec<u32> = self
            .select_options_idx(select)
            .into_iter()
            .filter(|&o| o != parent && self.is_inclusive_ancestor_idx(parent, o))
            .collect();
        if !options.is_empty() {
            self.keep_default_selectedness(select, &options);
            self.queue_selectedcontent_update(select, false);
        }
        true
    }
    /// After `nodes` were inserted into `parent`: when the last of their
    /// options that is selected joins a single select, it stays its selected
    /// option (as in browsers, an inserted selected option wins), so a selected
    /// option after it in tree order is deselected.
    pub(crate) fn options_after_insert(&mut self, nodes: &[u32], parent: u32) {
        let select = if self.html_local(parent) == Some(&local_name!("select")) {
            parent
        } else {
            match self.html_local(parent).map(|l| &**l) {
                Some("datalist" | "hr" | "option") => return,
                _ => self.subtree_ancestor_select(parent),
            }
        };
        if select == NIL || self.is_multiple(select) {
            return;
        }
        // Only an inserted explicitly selected option can deselect others: without one, skip
        // the walk over the select's whole list (the parser inserts its options one by one).
        let explicit_in = |root: u32| {
            let mut cur = root;
            while cur != NIL {
                if self.html_local(cur) == Some(&local_name!("option")) && self.option_explicit(cur) {
                    return true;
                }
                cur = self.following_in(cur, root);
            }
            false
        };
        if !nodes.iter().any(|&k| explicit_in(k)) {
            return;
        }
        let options = self.select_options_idx(select);
        let inserted = |o: u32| nodes.iter().any(|&k| self.is_inclusive_ancestor_idx(k, o));
        let Some(winner) = options.iter().rposition(|&o| inserted(o) && self.option_explicit(o)) else { return };
        let later: Vec<u32> =
            options[winner + 1..].iter().copied().filter(|&o| !inserted(o) && self.option_explicit(o)).collect();
        for o in later {
            if let Some(e) = self.n_mut(o).element_mut() {
                e.control.get_or_insert_with(Default::default).selected = Some(false);
            }
            // Selectedness is form-control state: `:checked` restyles.
            self.bump();
            self.note(crate::dom::Change::State(o));
        }
    }
    /// After a select's `multiple` attribute was added or removed: once it is
    /// single, only its first selected option in tree order stays selected.
    pub(crate) fn select_multiple_changed(&mut self, select: u32, multiple: bool) {
        if multiple || self.html_local(select) != Some(&local_name!("select")) {
            return;
        }
        let selected: Vec<u32> =
            self.select_options_idx(select).into_iter().filter(|&o| self.option_explicit(o)).skip(1).collect();
        for o in selected {
            if let Some(e) = self.n_mut(o).element_mut() {
                e.control.get_or_insert_with(Default::default).selected = Some(false);
            }
            // Selectedness is form-control state: `:checked` restyles.
            self.bump();
            self.note(crate::dom::Change::State(o));
        }
    }
    fn keep_default_selectedness(&mut self, select: u32, options: &[u32]) {
        let selected = self.selected_options_idx(select);
        for &option in options {
            if selected.contains(&option) && !self.option_explicit(option) {
                if let Some(e) = self.n_mut(option).element_mut() {
                    e.control.get_or_insert_with(Default::default).selected = Some(true);
                }
            }
        }
    }
    /// The select whose list of options the options inside `node` can be
    /// in: the nearest select ancestor, unless a datalist, hr or option (or
    /// a second optgroup, `node` included) comes first.
    fn subtree_ancestor_select(&self, node: u32) -> u32 {
        let mut optgroup = self.html_local(node) == Some(&local_name!("optgroup"));
        let mut p = self.n(node).parent;
        while p != NIL {
            match self.html_local(p).map(|l| &**l) {
                Some("datalist" | "hr" | "option") => return NIL,
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
    fn is_inclusive_ancestor_idx(&self, ancestor: u32, mut node: u32) -> bool {
        while node != NIL {
            if node == ancestor {
                return true;
            }
            node = self.n(node).parent;
        }
        false
    }
    /// The select whose list of options holds `opt`: its "option element
    /// nearest ancestor select" (HTML).
    fn option_select(&self, opt: u32) -> u32 {
        self.option_nearest_ancestor_select(opt)
    }
    /// An option's "option element nearest ancestor select" (HTML): the
    /// closest select ancestor, past at most one optgroup and no datalist,
    /// hr or option.
    pub(crate) fn option_nearest_ancestor_select(&self, opt: u32) -> u32 {
        let mut optgroup = false;
        let mut p = self.n(opt).parent;
        while p != NIL {
            match self.html_local(p).map(|l| &**l) {
                Some("datalist" | "hr" | "option") => return NIL,
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
    /// A select's "enabled selectedcontent" (HTML): none for a multiple
    /// select, else its first selectedcontent descendant unless that one is
    /// disabled, which it is inside a nested select.
    fn enabled_selectedcontent(&self, select: u32) -> u32 {
        if self.is_multiple(select) {
            return NIL;
        }
        let mut cur = self.following_in(select, select);
        while cur != NIL && self.html_local(cur).map(|l| &**l) != Some("selectedcontent") {
            cur = self.following_in(cur, select);
        }
        if cur == NIL {
            return NIL;
        }
        let mut p = self.n(cur).parent;
        while p != select {
            if p == NIL || self.html_local(p) == Some(&local_name!("select")) {
                return NIL;
            }
            p = self.n(p).parent;
        }
        cur
    }
    /// "Maybe clone an option into selectedcontent" (HTML), which the HTML
    /// parser runs whenever it pops an option off its stack of open
    /// elements: a selected option of a select with an enabled
    /// selectedcontent has its children cloned into that selectedcontent.
    pub(crate) fn maybe_clone_option_into_selectedcontent(&mut self, opt: u32) {
        if self.html_local(opt) != Some(&local_name!("option")) {
            return;
        }
        let select = self.option_nearest_ancestor_select(opt);
        if select == NIL || !self.option_selected_idx(opt) {
            return;
        }
        let selectedcontent = self.enabled_selectedcontent(select);
        if selectedcontent != NIL {
            let _ = self.clone_selected_option_into(opt, selectedcontent);
            self.note_selectedcontent_option(select, opt);
        }
    }
    /// "Clone selected option into select content": a fragment of deep
    /// clones of the option's children replaces the selectedcontent's.
    pub(crate) fn clone_selected_option_into(&mut self, opt: u32, selectedcontent: u32) -> DomResult<()> {
        let doc = self.id_of(self.n(opt).owner_doc);
        let fragment = self.create_fragment(doc)?;
        let children: Vec<NodeId> = self.child_idx_iter(opt).map(|c| self.id_of(c)).collect();
        for child in children {
            let clone = self.clone_node(child, true, None)?;
            self.append_child(fragment, clone)?;
        }
        let selectedcontent = self.id_of(selectedcontent);
        self.replace_all(selectedcontent, Some(fragment))
    }
    /// A select's list of options (HTML "get the list of options"): its
    /// option descendants in tree order whose option element nearest
    /// ancestor select is this select. A walk skips what holds none: the
    /// insides of options, hrs, datalists, nested selects, and of an
    /// optgroup inside another optgroup.
    pub(crate) fn select_options_idx(&self, select: u32) -> Vec<u32> {
        let mut out = Vec::new();
        // The optgroup the walk is inside (NIL: none).
        let mut optgroup = NIL;
        let mut cur = self.n(select).first_child;
        while cur != NIL {
            if optgroup != NIL && !self.is_inclusive_ancestor_idx(optgroup, cur) {
                optgroup = NIL;
            }
            let descend = match self.html_local(cur).map(|l| &**l) {
                Some("option") => {
                    out.push(cur);
                    false
                }
                Some("hr" | "datalist" | "select") => false,
                Some("optgroup") => {
                    if optgroup != NIL {
                        false
                    } else {
                        optgroup = cur;
                        true
                    }
                }
                _ => self.n(cur).element().is_some(),
            };
            cur = if descend { self.following_in(cur, select) } else { self.following_skipping(cur, select) };
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
            if let Some(&first) = opts.iter().find(|&&o| !self.option_disabled_idx(o)) {
                return vec![first];
            }
        }
        Vec::new()
    }
    /// A select's "placeholder label option" (HTML), NIL when none: for a
    /// single select of display size 1, the first option of its list of
    /// options when that option's value is empty and no optgroup groups it
    /// (a wrapper element does not stop it from being one).
    fn select_placeholder_idx(&self, select: u32) -> u32 {
        let e = self.n(select).element().unwrap();
        if e.attr_local(&local_name!("multiple")).is_some() {
            return NIL;
        }
        let size = e.attr_local(&local_name!("size")).and_then(|s| s.trim().parse::<u32>().ok()).unwrap_or(1);
        if size > 1 {
            return NIL;
        }
        let Some(&first) = self.select_options_idx(select).first() else { return NIL };
        let mut p = self.n(first).parent;
        while p != select {
            if self.html_local(p) == Some(&local_name!("optgroup")) {
                return NIL;
            }
            p = self.n(p).parent;
        }
        if self.option_value(self.id_of(first)).is_ok_and(|v| v.is_empty()) {
            first
        } else {
            NIL
        }
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
    /// An option's "option element nearest ancestor select" (HTML): the
    /// select whose list of options holds it.
    pub fn option_nearest_select(&self, opt: NodeId) -> DomResult<Option<NodeId>> {
        let o = self.check(opt)?;
        Ok(self.opt_id(self.option_nearest_ancestor_select(o)))
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
            self.update_select_selectedcontent(select, false);
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
        self.update_select_selectedcontent(s, true);
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
    /// HTML `option.text`: the option's descendant text, leaving out the contents of HTML and
    /// SVG `script` elements, with ASCII whitespace stripped and collapsed.
    fn option_text_idx(&self, o: u32) -> String {
        let mut s = String::new();
        let mut cur = self.n(o).first_child;
        while cur != NIL {
            let node = self.n(cur);
            if let NodeKind::Text(t) | NodeKind::CData(t) = &node.kind {
                s.push_str(t);
            }
            let script = node.element().is_some_and(|e| {
                &*e.name.local == "script" && (e.is_html() || e.name.ns == markup5ever::ns!(svg))
            });
            cur = if script { self.following_skipping(cur, o) } else { self.following_in(cur, o) };
        }
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
                        // Free text is stored sanitized. Other states keep
                        // what the user typed (a number input's bad input)
                        // or what an earlier type allowed, and range
                        // depends on min/max/step: sanitized on read.
                        Some(v) if ty.has_length_limits() => v.clone(),
                        Some(v) if ty == InputType::Range => self.range_value(i, v),
                        Some(v) if ty == InputType::Color => self.color_value(i, v),
                        Some(v) => sanitize(ty, v),
                        None if ty == InputType::Range => {
                            self.range_value(i, e.attr_local(&local_name!("value")).unwrap_or(""))
                        }
                        None if ty == InputType::Color => {
                            self.color_value(i, e.attr_local(&local_name!("value")).unwrap_or(""))
                        }
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
                None => self.child_text_content(id).replace("\r\n", "\n").replace('\r', "\n"),
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
                        let v = match ty {
                            InputType::Range => self.range_value(i, v),
                            InputType::Color => self.color_value(i, v),
                            _ => sanitize(ty, v),
                        };
                        let changed = self.value(id)? != v;
                        if changed { self.replace_all_value_ranges(i); }
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
                if changed { self.replace_all_value_ranges(i); }
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
                self.update_select_selectedcontent(i, true);
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

    fn attr_number(&self, i: u32, ty: InputType, name: &LocalName) -> Option<f64> {
        self.n(i).element()?.attr_local(name).and_then(|v| ty.string_to_number(v))
    }

    /// The minimum, maximum, allowed value step and step base of a numeric
    /// input, as Chrome derives them: date, month and week round the step
    /// to whole units, time and datetime-local to whole milliseconds; range
    /// has a default minimum (0) and maximum (100, never below the minimum)
    /// and steps from its minimum.
    fn step_range(&self, i: u32, ty: InputType) -> StepRange {
        use InputType::*;
        let (default_step, scale, default_base) = ty.step_description();
        let mut min = self.attr_number(i, ty, &local_name!("min"));
        let mut max = self.attr_number(i, ty, &local_name!("max"));
        if ty == Range {
            let lo = min.unwrap_or(0.0);
            min = Some(lo);
            max = Some(max.unwrap_or(100.0).max(lo));
        }
        let step = match self.n(i).element().and_then(|e| e.attr_local(&local_name!("step"))) {
            Some(s) if s.eq_ignore_ascii_case("any") => None,
            Some(s) => Some(match parse_float(s).filter(|&v| v > 0.0) {
                None => default_step * scale,
                Some(v) => match ty {
                    Date | Month | Week => v.round().max(1.0) * scale,
                    Time | DateTimeLocal => (v * scale).round().max(1.0),
                    _ => v * scale,
                },
            }),
            None => Some(default_step * scale),
        };
        let base = match ty {
            Range => min.unwrap_or(0.0),
            _ => min.or_else(|| self.attr_number(i, ty, &local_name!("value"))).unwrap_or(default_base),
        };
        StepRange { min, max, step, base }
    }

    /// Color's value sanitization: in the element's `colorspace`, with
    /// its alpha channel when it has an `alpha` attribute. Sanitized on read
    /// too, so changing either attribute changes the value.
    fn color_value(&self, i: u32, raw: &str) -> String {
        let Some(e) = self.n(i).element() else { return sanitize(InputType::Color, raw) };
        let alpha = e.attr_local(&LocalName::from("alpha")).is_some();
        let p3 = e.attr_local(&LocalName::from("colorspace")).is_some_and(|c| c.eq_ignore_ascii_case("display-p3"));
        crate::color_well::sanitize(raw, alpha, p3)
    }

    /// Range's value sanitization: a valid floating-point number (else the
    /// midpoint of min and max), clamped to them and snapped to the nearest
    /// allowed step (the upper one on a tie).
    fn range_value(&self, i: u32, raw: &str) -> String {
        let r = self.step_range(i, InputType::Range);
        let (min, max) = (r.min.unwrap_or(0.0), r.max.unwrap_or(100.0));
        let parsed = parse_float(raw);
        let mut v = parsed.unwrap_or(min + (max - min) / 2.0).clamp(min, max);
        let Some(step) = r.step else { return format_float(v, None) };
        let decimals = decimals_of(step).max(decimals_of(r.base)).max(decimals_of(v)).min(15);
        if r.mismatch(v) {
            let lo = r.floor(v, step);
            let hi = lo + step;
            v = if hi > max || (v - lo) < (hi - v) { lo } else { hi };
        }
        format_float(v, Some(decimals))
    }

    /// `stepUp(n)` (`up`) / `stepDown(n)` (HTML §4.10.5.4).
    pub fn step_value(&mut self, id: NodeId, n: f64, up: bool) -> DomResult<()> {
        let i = self.check(id)?;
        let ty = self.input_type_idx(i);
        if !self.is_input(i) || !ty.is_numeric() {
            return Err(DomError::new(DomErrorKind::InvalidState, "This form element is not steppable."));
        }
        let r = self.step_range(i, ty);
        let Some(step) = r.step else {
            return Err(DomError::new(
                DomErrorKind::InvalidState,
                "This form element does not have an allowed value step.",
            ));
        };
        // A time is within one day (Chrome's implicit limits).
        let (min, max) = match ty {
            InputType::Time => (r.min.or(Some(0.0)), r.max.or(Some(datetime::MS_PER_DAY - 1.0))),
            _ => (r.min, r.max),
        };
        if let (Some(lo), Some(hi)) = (min, max) {
            if lo > hi || r.ceil(lo, step) > hi {
                return Ok(());
            }
        }
        let before = ty.string_to_number(&self.value(id)?).unwrap_or(0.0);
        let mut v = if r.mismatch(before) {
            if up {
                r.ceil(before, step)
            } else {
                r.floor(before, step)
            }
        } else {
            before + if up { step * n } else { -step * n }
        };
        if let Some(lo) = min.filter(|&lo| v < lo) {
            v = r.ceil(lo, step);
        }
        if let Some(hi) = max.filter(|&hi| v > hi) {
            v = r.floor(hi, step);
        }
        if (up && v < before) || (!up && v > before) {
            return Ok(());
        }
        let text = match ty {
            InputType::Number | InputType::Range => {
                let decimals = decimals_of(step).max(decimals_of(r.base)).max(decimals_of(before)).min(15);
                Some(format_float(v, Some(decimals)))
            }
            _ => ty.number_to_string(v),
        };
        match text {
            Some(t) => self.set_value(id, &t),
            None => Ok(()),
        }
    }

    /// `valueAsNumber` (NaN where it does not apply or the value is not a number).
    pub fn value_as_number(&self, id: NodeId) -> DomResult<f64> {
        let i = self.check(id)?;
        let ty = self.input_type_idx(i);
        if !self.is_input(i) || !ty.is_numeric() {
            return Ok(f64::NAN);
        }
        Ok(ty.string_to_number(&self.value(id)?).unwrap_or(f64::NAN))
    }
    /// `valueAsNumber = x` for a finite or NaN `x` (NaN empties the value).
    pub fn set_value_as_number(&mut self, id: NodeId, x: f64) -> DomResult<()> {
        let i = self.check(id)?;
        let ty = self.input_type_idx(i);
        if !self.is_input(i) || !ty.is_numeric() {
            return Err(DomError::new(
                DomErrorKind::InvalidState,
                "This input element does not support Number values.",
            ));
        }
        let text = ty.number_to_string(x).unwrap_or_default();
        self.set_value(id, &text)
    }
    /// `valueAsDate` as a time value (ms since the epoch, UTC); None for
    /// null (no valid value, or a state it does not apply to).
    pub fn value_as_date(&self, id: NodeId) -> DomResult<Option<f64>> {
        let i = self.check(id)?;
        let ty = self.input_type_idx(i);
        if !self.is_input(i) || !ty.has_date_value() {
            return Ok(None);
        }
        let n = ty.string_to_number(&self.value(id)?);
        Ok(n.map(|n| if ty == InputType::Month { datetime::date_of_months(n) } else { n }))
    }
    /// `valueAsDate = date` (None: null; a NaN time value empties the value).
    pub fn set_value_as_date(&mut self, id: NodeId, ms: Option<f64>) -> DomResult<()> {
        let i = self.check(id)?;
        let ty = self.input_type_idx(i);
        if !self.is_input(i) || !ty.has_date_value() {
            return Err(DomError::new(DomErrorKind::InvalidState, "This input element does not support Date values."));
        }
        let text = ms
            .filter(|m| m.is_finite())
            .and_then(|ms| match ty {
                InputType::Month => datetime::months_of_date(ms).and_then(datetime::format_month),
                _ => ty.number_to_string(ms),
            })
            .unwrap_or_default();
        self.set_value(id, &text)
    }

    /// The text the user edits in a control: its value, except that a
    /// number input keeps what was typed (unparsable text makes its value
    /// empty: it suffers from bad input).
    fn edit_text(&self, i: u32) -> DomResult<String> {
        if self.is_input(i) && self.input_type_idx(i) == InputType::Number {
            if let Some(v) = self.n(i).element().and_then(|e| e.control.as_ref()).and_then(|c| c.value.as_ref()) {
                return Ok(v.clone());
            }
        }
        self.value(self.id_of(i))
    }

    fn selection_for_length(&self, id: NodeId, len: u32) -> DomResult<ControlSelection> {
        let control = self.element(id)?.control.as_ref();
        let range = control.filter(|c| c.selection_set)
            .map_or((0, 0), |c| (c.selection.0.min(len), c.selection.1.min(len)));
        let direction = control.and_then(|c| c.selection_direction)
            .filter(|(_, of)| *of == range)
            .map(|(backward, _)| backward);
        Ok(ControlSelection { range, direction })
    }

    /// Read both the bounds and direction against the current editable value.
    pub fn selection_snapshot(&self, id: NodeId) -> DomResult<ControlSelection> {
        let len = utf16_len(&self.edit_text(self.check(id)?)?);
        self.selection_for_length(id, len)
    }

    /// Editing has just updated the value and bounds together. Copy them
    /// directly for the inverse history step, without rereading the value.
    pub fn selection_after_edit(&self, id: NodeId) -> DomResult<ControlSelection> {
        self.selection_for_length(id, u32::MAX)
    }

    /// `selectionStart/End` of a text control.
    pub fn selection(&self, id: NodeId) -> DomResult<(u32, u32)> {
        Ok(self.selection_snapshot(id)?.range)
    }

    pub fn set_selection_range(&mut self, id: NodeId, start: u32, end: u32) -> DomResult<()> {
        let len = utf16_len(&self.edit_text(self.check(id)?)?);
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
        Ok(self.selection_snapshot(id)?.direction)
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

    /// Use a computed deletion span without changing the direction of the
    /// user's original selection before replacing it.
    pub fn replace_text_in_range(&mut self, id: NodeId, range: (u32, u32), text: &str) -> DomResult<bool> {
        self.replace_control_text(id, Some(range), text)
    }

    /// Replaces the current selection with `text` (typing, Input.insertText,
    /// or deletion with empty text). Returns true if the value changed.
    pub fn insert_text(&mut self, id: NodeId, text: &str) -> DomResult<bool> {
        self.replace_control_text(id, None, text)
    }

    fn replace_control_text(&mut self, id: NodeId, range: Option<(u32, u32)>, text: &str) -> DomResult<bool> {
        let i = self.check(id)?;
        if !self.is_mutable_text_idx(i) || !matches!(self.html_local(i).map(|l| &**l), Some("input" | "textarea")) {
            return Ok(false);
        }
        let mut value = self.edit_text(i)?;
        let value_len = utf16_len(&value);
        let original = self.selection_for_length(id, value_len)?;
        let (s, e) = range.map_or(original.range, |(start, end)| {
            let end = end.min(value_len);
            (start.min(end), end)
        });
        let is_input = self.html_local(i) == Some(&local_name!("input"));
        let mut text = if is_input { strip_newlines(text).into_owned() } else { text.to_owned() };
        if s == e && text.is_empty() {
            return Ok(false);
        }
        // A user's insertion stops at the maximum allowed value length: the
        // text is cut to the room left (never splitting a surrogate pair).
        let limited = !is_input || self.input_type_idx(i).has_length_limits();
        if let Some(max) = self
            .n(i)
            .element()
            .unwrap()
            .attr_local(&local_name!("maxlength"))
            .filter(|_| limited && !text.is_empty())
            .and_then(|m| m.trim().parse::<u32>().ok())
        {
            let room = max.saturating_sub(value_len.saturating_sub(e - s));
            if utf16_len(&text) > room {
                let mut units = 0;
                let cut = text
                    .char_indices()
                    .find(|(_, c)| {
                        units += c.len_utf16() as u32;
                        units > room
                    })
                    .map_or(text.len(), |(at, _)| at);
                text.truncate(cut);
                if text.is_empty() && s == e {
                    return Ok(false);
                }
            }
        }
        let (a, b) = utf16_range(&value, s, e - s)?;
        self.edit_value_ranges(i, &value, a..b, &text);
        value.replace_range(a..b, &text);
        let caret = s + utf16_len(&text);
        let c = self.control_state_mut(id)?;
        c.value = Some(value);
        c.dirty_value = true;
        c.user_edited = true;
        c.selection = (caret, caret);
        c.selection_direction = original.direction.map(|_| (false, c.selection));
        c.selection_set = true;
        Ok(true)
    }

    /// `setRangeText(text, start, end, mode)` (HTML §4.10.18): replaces
    /// `start..end`, clamped to the value, without the maxlength limit and
    /// the user-edit flag of typing, then selects per `mode`. The caller
    /// throws IndexSizeError for `start > end`.
    pub fn set_range_text(&mut self, id: NodeId, text: &str, start: u32, end: u32, mode: RangeMode) -> DomResult<()> {
        let i = self.check(id)?;
        let is_input = self.is_input(i);
        let text =
            if is_input { strip_newlines(text).into_owned() } else { text.replace("\r\n", "\n").replace('\r', "\n") };
        let mut value = self.edit_text(i)?;
        let len = utf16_len(&value);
        let (start, end) = (start.min(len), end.min(len));
        let (mut sel_start, mut sel_end) = self.selection(id)?;
        let (a, b) = utf16_range(&value, start, end.saturating_sub(start))?;
        let text_len = utf16_len(&text);
        self.edit_value_ranges(i, &value, a..b, &text);
        value.replace_range(a..b, &text);
        let new_end = start + text_len;
        match mode {
            RangeMode::Select => (sel_start, sel_end) = (start, new_end),
            RangeMode::Start => (sel_start, sel_end) = (start, start),
            RangeMode::End => (sel_start, sel_end) = (new_end, new_end),
            RangeMode::Preserve => {
                let delta = text_len as i64 - (end - start) as i64;
                let shift = |p: u32, collapsed: u32| {
                    if p > end {
                        (p as i64 + delta) as u32
                    } else if p > start {
                        collapsed
                    } else {
                        p
                    }
                };
                sel_start = shift(sel_start, start);
                sel_end = shift(sel_end, new_end);
            }
        }
        let c = self.control_state_mut(id)?;
        c.value = Some(value);
        c.dirty_value = true;
        c.selection = (sel_start, sel_end.max(sel_start));
        c.selection_set = true;
        Ok(())
    }

    /// Backspace: deletes the selection or the code point before the caret.
    pub fn delete_backward(&mut self, id: NodeId) -> DomResult<bool> {
        let (s, e) = self.selection(id)?;
        if s == e {
            if s == 0 {
                return Ok(false);
            }
            let v = self.edit_text(self.check(id)?)?;
            // Snap to the start of the code point ending at the caret.
            let (a, _) = utf16_range(&v, s - 1, 0)?;
            let start = utf16_len(&v[..a]);
            return self.replace_text_in_range(id, (start, s), "");
        }
        self.insert_text(id, "")
    }

    /// Delete key: deletes the selection or the code point after the caret.
    pub fn delete_forward(&mut self, id: NodeId) -> DomResult<bool> {
        let (s, e) = self.selection(id)?;
        if s == e {
            let v = self.edit_text(self.check(id)?)?;
            if s >= utf16_len(&v) {
                return Ok(false);
            }
            let (a, _) = utf16_range(&v, s, 0)?;
            let ch = v[a..].chars().next().map_or(1, |c| c.len_utf16() as u32);
            return self.replace_text_in_range(id, (s, s + ch), "");
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

    pub(crate) fn activated_idx(&self, doc: u32) -> u32 {
        match &self.n(doc).kind {
            NodeKind::Document(d) if d.activated != NIL && self.slot_live(d.activated) => d.activated,
            _ => NIL,
        }
    }

    pub(crate) fn focus_visible_flag(&self, doc: u32) -> bool {
        matches!(&self.n(doc).kind, NodeKind::Document(d) if d.focus_visible)
    }

    /// Whether the focused element of `doc` matches `:focus-visible`: the flag its focusing set
    /// (keyboard modality, script focus without a pointer focus), or, as Chrome decides it when it
    /// matches, the element is a field a virtual keyboard would type into (a textarea, a text-like
    /// input), whatever focused it. A button that becomes `type=text` while focused shows the ring
    /// (WPT css/selectors `focus-visible-027`).
    pub(crate) fn focus_visible_now(&self, doc: u32) -> bool {
        if self.focus_visible_flag(doc) {
            return true;
        }
        let f = self.focused_idx(doc);
        f != NIL
            && match self.html_local(f).map(|l| &**l) {
                Some("textarea") => true,
                Some("input") => matches!(
                    self.input_type_idx(f),
                    InputType::Text
                        | InputType::Search
                        | InputType::Tel
                        | InputType::Url
                        | InputType::Email
                        | InputType::Password
                        | InputType::Number
                ),
                _ => false,
            }
    }

    /// The element being activated (`:active`), if any.
    pub fn activated(&self, doc: NodeId) -> Option<NodeId> {
        let d = self.check(doc).ok()?;
        self.opt_id(self.activated_idx(d))
    }

    /// Sets the element being activated (`:active`). With `restyle` (some
    /// style can depend on `:active`, [`crate::style::StyleEngine::active_styled`])
    /// a change restyles like a hover change; without, only the state
    /// changes: nothing computed from the document is invalidated, and the
    /// layout engine reads the state at its next sync.
    pub fn set_activated(&mut self, doc: NodeId, el: Option<NodeId>, restyle: bool) -> DomResult<()> {
        let d = self.check(doc)?;
        let new = match el {
            Some(e) => self.check(e)?,
            None => NIL,
        };
        if self.activated_idx(d) == new {
            return Ok(());
        }
        if let NodeKind::Document(dd) = &mut self.n_mut(d).kind {
            dd.activated = new;
        }
        if restyle {
            self.bump();
            self.note(crate::dom::Change::Pseudo);
        }
        Ok(())
    }

    /// Whether the focused element matches `:focus-visible`.
    pub fn focus_visible(&self, doc: NodeId) -> bool {
        self.check(doc).is_ok_and(|d| self.focus_visible_now(d))
    }

    /// Sets whether the focused element matches `:focus-visible`.
    pub fn set_focus_visible(&mut self, doc: NodeId, visible: bool) -> DomResult<()> {
        let d = self.check(doc)?;
        if self.focus_visible_flag(d) == visible {
            return Ok(());
        }
        if let NodeKind::Document(dd) = &mut self.n_mut(d).kind {
            dd.focus_visible = visible;
        }
        if self.focused_idx(d) != NIL {
            self.bump();
            self.note(crate::dom::Change::Pseudo);
        }
        Ok(())
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
        if self.tabindex_attr(i).is_some() {
            return Ok(true);
        }
        if e.attr_local(&local_name!("contenteditable")).is_some_and(|v| !v.eq_ignore_ascii_case("false")) {
            return Ok(true);
        }
        if e.namespace() == &markup5ever::ns!(mathml) && e.name.local == local_name!("a") {
            return Ok(e.attr_local(&local_name!("href")).is_some());
        }
        Ok(e.is_html()
            && match &*e.name.local {
                "a" | "area" => e.attr_local(&local_name!("href")).is_some(),
                "input" => self.input_type_idx(i) != InputType::Hidden,
                "summary" => self.is_details_summary(id),
                // A select's button (its first child button) is part of the select, which takes
                // the focus (customizable select).
                "button" => !self.is_select_button_idx(i),
                "select" | "textarea" | "iframe" => true,
                // An option is focusable while its select's picker is open.
                "option" => {
                    let select = self.option_nearest_ancestor_select(i);
                    select != NIL && self.is_select_picker_open_idx(select)
                }
                _ => false,
            })
    }
    /// Whether `i` is a select's button: a button that is the select's first
    /// element child (HTML customizable select).
    pub(crate) fn is_select_button_idx(&self, i: u32) -> bool {
        let parent = self.n(i).parent;
        if parent == NIL || self.html_local(parent) != Some(&local_name!("select")) {
            return false;
        }
        let mut c = self.n(parent).first_child;
        while c != NIL && self.n(c).element().is_none() {
            c = self.n(c).next;
        }
        c == i
    }

    /// Document's focused element (`activeElement` is body when None).
    pub fn focused(&self, doc: NodeId) -> DomResult<Option<NodeId>> {
        let d = self.check(doc)?;
        Ok(self.opt_id(self.focused_idx(d)))
    }
    /// DOM "retarget" of `node` against the tree rooted at `root`: the node
    /// itself when its root is `root` (or is not a shadow root), else the
    /// nearest shadow host whose root is. `None` for a dead node.
    pub fn retarget_node(&self, node: NodeId, root: NodeId) -> Option<NodeId> {
        let mut cur = node;
        loop {
            let r = self.root_node(cur, false).ok()?;
            if r == root || !self.is_shadow_root(r).unwrap_or(false) {
                return Some(cur);
            }
            cur = self.fragment_host(r).ok()??;
        }
    }

    /// `DocumentOrShadowRoot.activeElement` for `root` (a document or a
    /// shadow root): the focused element retargeted against `root`, null
    /// when that lands outside `root`'s tree; a document falls back to its
    /// body when nothing in it is focused, else its document element.
    pub fn active_element(&self, root: NodeId) -> DomResult<Option<NodeId>> {
        let doc = self.node_document(root)?;
        let candidate = self.focused(doc)?.and_then(|f| self.retarget_node(f, root));
        let in_tree = candidate.filter(|&c| self.root_node(c, false).is_ok_and(|r| r == root));
        if root == doc {
            return Ok(in_tree.or(self.body(doc)?).or(self.document_element(doc)?));
        }
        Ok(in_tree)
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
            "button" => self.button_type_is_submit_idx(i),
            _ if self.is_face_idx(i) => return Ok(self.face_will_validate_idx(i)),
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

    /// `:in-range` (`Some(true)`) or `:out-of-range` (`Some(false)`): an
    /// input that is a candidate for constraint validation and has range
    /// limitations (a minimum or maximum), by whether its value underflows or
    /// overflows them; `None` for other elements.
    pub(crate) fn range_state_idx(&self, i: u32) -> Option<bool> {
        if !self.is_input(i) {
            return None;
        }
        let ty = self.input_type_idx(i);
        if !ty.is_numeric() {
            return None;
        }
        let r = self.step_range(i, ty);
        if r.min.is_none() && r.max.is_none() {
            return None;
        }
        let id = self.id_of(i);
        if !self.will_validate(id).ok()? {
            return None;
        }
        let v = self.validity(id).ok()?;
        Some(!(v.range_underflow || v.range_overflow))
    }

    /// Sets a control's user validity (HTML §4.10.18.6, `:user-valid` /
    /// `:user-invalid`); nothing changes (and nothing restyles) when it
    /// already has that value.
    pub fn set_user_validity(&mut self, id: NodeId, v: bool) -> DomResult<()> {
        let current = self.control_state(id)?.is_some_and(|c| c.user_validity);
        if current != v {
            self.control_state_mut(id)?.user_validity = v;
        }
        Ok(())
    }

    /// A submission validates `form`: each of its listed `input`, `select`
    /// and `textarea` elements gets user validity (HTML form submission,
    /// "interactively validate the constraints").
    pub fn set_form_user_validity(&mut self, form: NodeId) -> DomResult<()> {
        for c in self.owned_form_elements(form)? {
            if self.html_local(self.check(c)?).is_some_and(|l| matches!(&**l, "input" | "select" | "textarea")) {
                self.set_user_validity(c, true)?;
            }
        }
        Ok(())
    }

    /// `ValidityState` flags (constraint validation, HTML §4.10.20.1) of a
    /// candidate for constraint validation: none for an element that is
    /// barred from it (`:invalid`, `checkValidity()`).
    pub fn validity(&self, id: NodeId) -> DomResult<Validity> {
        if !self.will_validate(id)? {
            return Ok(Validity::default());
        }
        self.validity_flags(id)
    }

    /// The flags a `ValidityState` object reports: computed for a disabled,
    /// read-only or `<datalist>` control too (it suffers from a mismatch
    /// whether or not it is barred), except that only a mutable control
    /// suffers from being missing.
    pub fn validity_flags(&self, id: NodeId) -> DomResult<Validity> {
        let mut v = Validity::default();
        let i = self.check(id)?;
        if self.is_face_idx(i) {
            return Ok(if self.will_validate(id)? { self.face_validity_idx(i) } else { v });
        }
        let Some(local) = self.html_local(i) else { return Ok(v) };
        let e = self.n(i).element().unwrap();
        // Any element with `setCustomValidity` suffers from a custom error, also one that is
        // never a candidate (`<output>`, `<object>`, `<fieldset>`, a hidden input).
        v.custom_error =
            e.control.as_ref().is_some_and(|c| c.custom_validity.as_deref().is_some_and(|m| !m.is_empty()));
        let listed = match &**local {
            "input" => !matches!(self.input_type_idx(i), InputType::Hidden | InputType::Reset | InputType::Button),
            "select" | "textarea" => true,
            "button" => self.button_type_is_submit_idx(i),
            _ => false,
        };
        if !listed {
            return Ok(v);
        }
        let mutable = !self.is_disabled_idx(i) && !(e.attr_local(&local_name!("readonly")).is_some() && &**local != "select");
        let local = self.html_local(i).map(|l| l.to_string()).unwrap_or_default();
        let required = mutable && e.attr_local(&local_name!("required")).is_some();
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
                let multiple = ty == InputType::Email && e.attr_local(&local_name!("multiple")).is_some();
                let addresses = || value.split(',').map(|a| a.trim_matches(|c: char| c.is_ascii_whitespace()));
                if !value.is_empty() {
                    match ty {
                        InputType::Email if multiple => v.type_mismatch = !addresses().all(is_valid_email),
                        InputType::Email => v.type_mismatch = !is_valid_email(&value),
                        InputType::Url => v.type_mismatch = url::Url::parse(&value).is_err(),
                        _ if ty.is_numeric() => {
                            if let Some(n) = ty.string_to_number(&value) {
                                let r = self.step_range(i, ty);
                                match (r.min, r.max) {
                                    // A reversed time range spans midnight.
                                    (Some(lo), Some(hi)) if ty == InputType::Time && lo > hi => {
                                        let outside = n < lo && n > hi;
                                        v.range_underflow = outside;
                                        v.range_overflow = outside;
                                    }
                                    (lo, hi) => {
                                        v.range_underflow = lo.is_some_and(|lo| n < lo);
                                        v.range_overflow = hi.is_some_and(|hi| n > hi);
                                    }
                                }
                                v.step_mismatch = r.mismatch(n);
                            }
                        }
                        _ => {}
                    }
                    if ty.has_length_limits() {
                        if let Some(p) = e.attr_local(&local_name!("pattern")) {
                            v.pattern_mismatch = if multiple {
                                addresses().any(|a| pattern_mismatch(p, a))
                            } else {
                                pattern_mismatch(p, &value)
                            };
                        }
                    }
                }
                // What was typed into a number input is not a number.
                if ty == InputType::Number && value.is_empty() {
                    v.bad_input = e.control.as_ref().and_then(|c| c.value.as_ref()).is_some_and(|raw| !raw.is_empty());
                }
            }
            "textarea" => v.value_missing = required && value.is_empty(),
            "select" => {
                v.value_missing = required && {
                    let sel = self.selected_options_idx(i);
                    sel.is_empty() || sel.first().is_some_and(|&o| self.select_placeholder_idx(i) == o)
                };
            }
            _ => {}
        }
        // tooLong/tooShort only apply after user edits (dirty value flag),
        // and only where maxlength/minlength do.
        let limited = local == "textarea" || (local == "input" && self.input_type_idx(i).has_length_limits());
        let dirty = limited && e.control.as_ref().is_some_and(|c| c.dirty_value && c.user_edited);
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

/// `SelectionMode` of `setRangeText`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RangeMode {
    Select,
    Start,
    End,
    Preserve,
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

/// A field name's autofill category and the most tokens an `autocomplete`
/// value ending in it may hold (HTML "determine a field's category").
#[derive(Clone, Copy, PartialEq, Eq)]
enum AutofillCategory {
    Off,
    Automatic,
    Normal,
    Contact,
    Credential,
}

fn autofill_category(field: &str) -> Option<(AutofillCategory, usize)> {
    use AutofillCategory::*;
    Some(match field {
        "off" => (Off, 1),
        "on" => (Automatic, 1),
        "name" | "honorific-prefix" | "given-name" | "additional-name" | "family-name" | "honorific-suffix"
        | "nickname" | "organization-title" | "username" | "new-password" | "current-password" | "one-time-code"
        | "organization" | "street-address" | "address-line1" | "address-line2" | "address-line3"
        | "address-level4" | "address-level3" | "address-level2" | "address-level1" | "country"
        | "country-name" | "postal-code" | "cc-name" | "cc-given-name" | "cc-additional-name" | "cc-family-name"
        | "cc-number" | "cc-exp" | "cc-exp-month" | "cc-exp-year" | "cc-csc" | "cc-type"
        | "transaction-currency" | "transaction-amount" | "language" | "bday" | "bday-day" | "bday-month"
        | "bday-year" | "sex" | "url" | "photo" => (Normal, 3),
        "tel" | "tel-country-code" | "tel-national" | "tel-area-code" | "tel-local" | "tel-local-prefix"
        | "tel-local-suffix" | "tel-extension" | "email" | "impp" => (Contact, 4),
        "webauthn" => (Credential, 5),
        _ => return None,
    })
}

/// The IDL-exposed autofill value of an `autocomplete` attribute (what the
/// `autocomplete` IDL attribute of `input`, `select` and `textarea` returns):
/// its tokens lowercased and validated by the autofill processing model, or
/// "" when they are not a valid autofill detail. `anchor` is true for an
/// `<input type=hidden>`, whose attribute wears the autofill anchor mantle
/// (`on` and `off` are not allowed).
pub fn autofill_idl_value(attr: Option<&str>, anchor: bool) -> String {
    use AutofillCategory::*;
    let tokens: Vec<String> =
        attr.unwrap_or("").split(|c: char| c.is_ascii_whitespace()).filter(|t| !t.is_empty()).map(str::to_ascii_lowercase).collect();
    let Some(mut index) = tokens.len().checked_sub(1) else { return String::new() };
    let Some((mut category, max)) = autofill_category(&tokens[index]) else { return String::new() };
    if tokens.len() > max {
        return String::new();
    }
    match category {
        Off | Automatic if anchor => return String::new(),
        Off => return "off".into(),
        Automatic => return "on".into(),
        _ => {}
    }
    let mut credential = false;
    if category == Credential {
        credential = true;
        if index == 0 {
            return String::new();
        }
        index -= 1;
        match autofill_category(&tokens[index]) {
            Some((c @ (Normal | Contact), _)) => category = c,
            _ => return String::new(),
        }
    }
    let mut parts = vec![tokens[index].as_str()];
    let mut rest = &tokens[..index];
    if category == Contact {
        if let Some((last, before)) = rest.split_last() {
            if matches!(last.as_str(), "home" | "work" | "mobile" | "fax" | "pager") {
                parts.push(last);
                rest = before;
            }
        }
    }
    if let Some((last, before)) = rest.split_last() {
        if matches!(last.as_str(), "shipping" | "billing") {
            parts.push(last);
            rest = before;
        }
    }
    if let Some((last, before)) = rest.split_last() {
        if last.starts_with("section-") {
            parts.push(last);
            rest = before;
        }
    }
    if !rest.is_empty() {
        return String::new();
    }
    parts.reverse();
    if credential {
        parts.push("webauthn");
    }
    parts.join(" ")
}

#[cfg(test)]
mod autofill_tests {
    use super::autofill_idl_value as v;

    #[test]
    fn autofill_idl_value_follows_the_processing_model() {
        assert_eq!(v(None, false), "");
        assert_eq!(v(Some("  \n"), false), "");
        assert_eq!(v(Some(" ON\t"), false), "on");
        assert_eq!(v(Some("off"), true), "");
        assert_eq!(v(Some("foobar"), false), "");
        assert_eq!(v(Some(" HOME\ntel"), false), "home tel");
        assert_eq!(v(Some("billing  work  email"), false), "billing work email");
        assert_eq!(v(Some("  section-FOO  bday"), false), "section-foo bday");
        assert_eq!(v(Some("work name"), false), "");
        assert_eq!(v(Some("foo section-foo billing name"), false), "");
        assert_eq!(v(Some("\tusername webauthn"), false), "username webauthn");
        assert_eq!(v(Some("  section-LOGIN  shipping work tel webauthn "), false), "section-login shipping work tel webauthn");
        assert_eq!(v(Some("foo section-bar billing work tel webauthn"), false), "");
        assert_eq!(v(Some("webauthn"), false), "");
        assert_eq!(v(Some("on webauthn"), false), "");
    }
}
