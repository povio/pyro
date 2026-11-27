//! Live SVG lists. The DOM holds a weak observer, so a retained list or
//! attached item keeps the element alive through its writer without a cycle.

use crate::{Dom, DomError, DomErrorKind, DomResult, NodeId};
use std::cell::{Cell, RefCell};
use std::fmt::Write;
use std::rc::{Rc, Weak};

type Pair = (f32, f32);
type Writer = Box<dyn Fn(&str) -> DomResult<()>>;

pub(crate) fn parse_points(text: &str) -> impl Iterator<Item = Pair> + '_ {
    svgtypes::PointsParser::from(text).map_while(|(x, y)| {
        let (x, y) = (x as f32, y as f32);
        (x.is_finite() && y.is_finite()).then_some((x, y))
    })
}

fn out_of_bounds() -> DomError {
    DomError::new(DomErrorKind::IndexSize, "The index is outside the SVG list.")
}

pub fn read_only() -> DomError {
    DomError::new(DomErrorKind::NoModificationAllowed, "The animated list is read only.")
}

#[derive(Clone)]
struct Attachment<T: Copy> {
    owner: Rc<ListStorage<T>>,
    index: usize,
    animated: bool,
}

pub struct ListItem<T: Copy> {
    value: Cell<T>,
    attached: RefCell<Option<Attachment<T>>>,
}

impl<T: Copy> ListItem<T> {
    pub fn detached(value: T) -> Rc<Self> {
        Rc::new(Self { value: Cell::new(value), attached: RefCell::new(None) })
    }

    pub fn value(&self) -> T {
        self.value.get()
    }

    pub fn set_value(&self, value: T) -> DomResult<()> {
        let attached = self.attached.borrow().clone();
        if let Some(a) = attached {
            if a.animated {
                return Err(read_only());
            }
            a.owner.base.borrow_mut()[a.index].value = value;
            self.value.set(value);
            a.owner.commit()
        } else {
            self.value.set(value);
            Ok(())
        }
    }

    fn insertion_value(self: &Rc<Self>) -> Rc<Self> {
        if self.attached.borrow().is_some() { Self::detached(self.value()) } else { self.clone() }
    }
}

struct Slot<T: Copy> {
    value: T,
    item: Weak<ListItem<T>>,
}

impl<T: Copy> Slot<T> {
    fn new(value: T) -> Self {
        Self { value, item: Weak::new() }
    }

    fn detach(&self) {
        if let Some(item) = self.item.upgrade() {
            item.attached.replace(None);
        }
    }
}

pub struct ListStorage<T: Copy> {
    base: RefCell<Vec<Slot<T>>>,
    animated: RefCell<Vec<Slot<T>>>,
    animation: RefCell<Option<Box<dyn Fn(&[T]) -> Option<Vec<T>>>>>,
    writing: Cell<bool>,
    length_unit_factor: Option<Box<dyn Fn(u16) -> f64>>,
    write: Writer,
    write_item: fn(&mut String, T),
}

impl<T: Copy> ListStorage<T> {
    fn new(write: Writer, write_item: fn(&mut String, T), length_unit_factor: Option<Box<dyn Fn(u16) -> f64>>) -> Rc<Self> {
        Rc::new(Self {
            base: RefCell::default(),
            animated: RefCell::default(),
            animation: RefCell::new(None),
            writing: Cell::new(false),
            length_unit_factor,
            write,
            write_item,
        })
    }

    fn view(&self, animated: bool) -> &RefCell<Vec<Slot<T>>> {
        if animated { &self.animated } else { &self.base }
    }

    pub fn len(&self, animated: bool) -> usize {
        if animated {
            self.refresh();
        }
        self.view(animated).borrow().len()
    }

    /// Attribute synchronization preserves surviving objects by index.
    fn synchronize(list: &mut Vec<Slot<T>>, values: impl Iterator<Item = T>) {
        let mut count = 0;
        for value in values {
            if let Some(slot) = list.get_mut(count) {
                slot.value = value;
                if let Some(item) = slot.item.upgrade() {
                    item.value.set(value);
                }
            } else {
                list.push(Slot::new(value));
            }
            count += 1;
        }
        for slot in list.drain(count..) {
            slot.detach();
        }
    }

    /// Where the animated list's values come from while an animation is in effect: the source takes
    /// the base list's values and returns the animated ones, or `None` when no animation is in effect.
    pub fn set_animation(&self, source: Box<dyn Fn(&[T]) -> Option<Vec<T>>>) {
        *self.animation.borrow_mut() = Some(source);
    }

    /// Brings the animated list up to the animations' current values.
    fn refresh(&self) {
        if self.animation.borrow().is_none() {
            return;
        }
        let base: Vec<T> = self.base.borrow().iter().map(|s| s.value).collect();
        let sampled = match &*self.animation.borrow() {
            Some(source) => source(&base),
            None => return,
        };
        Self::synchronize(&mut self.animated.borrow_mut(), sampled.unwrap_or(base).into_iter());
    }

    fn sync_attribute(&self, values: impl Iterator<Item = T>) {
        if self.writing.get() {
            return;
        }
        let mut base = self.base.borrow_mut();
        Self::synchronize(&mut base, values);
        Self::synchronize(&mut self.animated.borrow_mut(), base.iter().map(|s| s.value));
    }

    fn commit(&self) -> DomResult<()> {
        let mut text = String::new();
        let base = self.base.borrow();
        for (i, s) in base.iter().enumerate() {
            if i != 0 {
                text.push(' ');
            }
            (self.write_item)(&mut text, s.value);
        }
        Self::synchronize(&mut self.animated.borrow_mut(), base.iter().map(|s| s.value));
        drop(base);
        // The ordinary attribute write queues mutation records and dirties
        // geometry. Its observer must not renumber the just-edited objects.
        self.writing.set(true);
        let result = (self.write)(&text);
        self.writing.set(false);
        result
    }

    pub fn item(self: &Rc<Self>, index: usize, animated: bool) -> Option<Rc<ListItem<T>>> {
        if animated {
            self.refresh();
        }
        let mut view = self.view(animated).borrow_mut();
        let slot = view.get_mut(index)?;
        if let Some(item) = slot.item.upgrade() {
            return Some(item);
        }
        let item = ListItem::detached(slot.value);
        item.attached.replace(Some(Attachment { owner: self.clone(), index, animated }));
        slot.item = Rc::downgrade(&item);
        Some(item)
    }

    fn reindex(&self, from: usize) {
        for (index, slot) in self.base.borrow().iter().enumerate().skip(from) {
            if let Some(item) = slot.item.upgrade() {
                if let Some(a) = item.attached.borrow_mut().as_mut() {
                    a.index = index;
                }
            }
        }
    }

    pub fn clear(&self) -> DomResult<()> {
        for slot in self.base.borrow_mut().drain(..) {
            slot.detach();
        }
        self.commit()
    }

    pub fn initialize(self: &Rc<Self>, value: &Rc<ListItem<T>>) -> DomResult<Rc<ListItem<T>>> {
        for slot in self.base.borrow_mut().drain(..) {
            slot.detach();
        }
        self.insert(0, value, false)
    }

    pub fn insert(self: &Rc<Self>, index: usize, value: &Rc<ListItem<T>>, replace: bool) -> DomResult<Rc<ListItem<T>>> {
        let mut base = self.base.borrow_mut();
        if replace && index >= base.len() {
            return Err(out_of_bounds());
        }
        let index = index.min(base.len());
        let value = value.insertion_value();
        let slot = Slot { value: value.value(), item: Rc::downgrade(&value) };
        if replace {
            base[index].detach();
            base[index] = slot;
        } else {
            base.insert(index, slot);
        }
        value.attached.replace(Some(Attachment { owner: self.clone(), index, animated: false }));
        drop(base);
        if !replace {
            self.reindex(index + 1);
        }
        self.commit()?;
        Ok(value)
    }

    pub fn remove(self: &Rc<Self>, index: usize) -> DomResult<Rc<ListItem<T>>> {
        let value = self.item(index, false).ok_or_else(out_of_bounds)?;
        self.base.borrow_mut().remove(index).detach();
        self.reindex(index);
        self.commit()?;
        Ok(value)
    }
}

pub type PointItem = ListItem<Pair>;
pub type NumberItem = ListItem<f32>;
pub type PointLists = ListStorage<Pair>;
pub type NumberLists = ListStorage<f32>;
pub type LengthItem = ListItem<(f64, u16)>;
pub type LengthLists = ListStorage<(f64, u16)>;
pub type TransformItem = ListItem<TransformValue>;
pub type TransformLists = ListStorage<TransformValue>;

/// One function of an SVG `transform` list (an `SVGTransform`): its type (1 matrix, 2 translate,
/// 3 scale, 4 rotate, 5 skewX, 6 skewY), its angle (degrees; rotations and skews) and its matrix
/// `[a b c d e f]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransformValue {
    pub kind: u16,
    pub angle: f64,
    pub matrix: [f64; 6],
}

impl TransformValue {
    pub const IDENTITY: [f64; 6] = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

    pub fn from_matrix(matrix: [f64; 6]) -> Self {
        Self { kind: 1, angle: 0.0, matrix }
    }

    pub fn translate(x: f64, y: f64) -> Self {
        Self { kind: 2, angle: 0.0, matrix: [1.0, 0.0, 0.0, 1.0, x, y] }
    }

    pub fn scale(x: f64, y: f64) -> Self {
        Self { kind: 3, angle: 0.0, matrix: [x, 0.0, 0.0, y, 0.0, 0.0] }
    }

    /// A rotation of `angle` degrees about (`cx`, `cy`).
    pub fn rotate(angle: f64, cx: f64, cy: f64) -> Self {
        let (sin, cos) = angle.to_radians().sin_cos();
        Self { kind: 4, angle, matrix: [cos, sin, -sin, cos, cx - cos * cx + sin * cy, cy - sin * cx - cos * cy] }
    }

    pub fn skew_x(angle: f64) -> Self {
        Self { kind: 5, angle, matrix: [1.0, 0.0, angle.to_radians().tan(), 1.0, 0.0, 0.0] }
    }

    pub fn skew_y(angle: f64) -> Self {
        Self { kind: 6, angle, matrix: [1.0, angle.to_radians().tan(), 0.0, 1.0, 0.0, 0.0] }
    }
}

/// The functions of a `transform` list attribute (SVG 1.1 syntax: commas or spaces between
/// numbers); an attribute that does not parse is an empty list, as in Chromium.
pub fn parse_transform_list(text: &str) -> Vec<TransformValue> {
    let mut items = Vec::new();
    let mut rest = text.trim_start_matches(|c: char| c.is_ascii_whitespace() || c == ',');
    while !rest.is_empty() {
        let Some(open) = rest.find('(') else { return Vec::new() };
        let name = rest[..open].trim();
        let Some(close) = rest[open..].find(')') else { return Vec::new() };
        let args: Option<Vec<f64>> = rest[open + 1..open + close]
            .split(|c: char| c.is_ascii_whitespace() || c == ',')
            .filter(|v| !v.is_empty())
            .map(|v| v.parse::<f64>().ok())
            .collect();
        let Some(n) = args else { return Vec::new() };
        let item = match (name, n.as_slice()) {
            ("matrix", &[a, b, c, d, e, f]) => TransformValue::from_matrix([a, b, c, d, e, f]),
            ("translate", &[x]) => TransformValue::translate(x, 0.0),
            ("translate", &[x, y]) => TransformValue::translate(x, y),
            ("scale", &[x]) => TransformValue::scale(x, x),
            ("scale", &[x, y]) => TransformValue::scale(x, y),
            ("rotate", &[a]) => TransformValue::rotate(a, 0.0, 0.0),
            ("rotate", &[a, cx, cy]) => TransformValue::rotate(a, cx, cy),
            ("skewX", &[a]) => TransformValue::skew_x(a),
            ("skewY", &[a]) => TransformValue::skew_y(a),
            _ => return Vec::new(),
        };
        items.push(item);
        rest = rest[open + close + 1..].trim_start_matches(|c: char| c.is_ascii_whitespace() || c == ',');
    }
    items
}

/// One function of a transform list as the attribute writes it (each type in its own function,
/// so a list edited by script keeps the types a script set).
fn write_transform(text: &mut String, t: TransformValue) {
    let m = t.matrix;
    let written = match t.kind {
        2 => write!(text, "translate({} {})", m[4] as f32, m[5] as f32),
        3 => write!(text, "scale({} {})", m[0] as f32, m[3] as f32),
        4 => {
            // The center is what the matrix's translation says: (1 - cos) cx + sin cy = e,
            // -sin cx + (1 - cos) cy = f.
            let (sin, cos) = t.angle.to_radians().sin_cos();
            let det = (1.0 - cos) * (1.0 - cos) + sin * sin;
            if (m[4] == 0.0 && m[5] == 0.0) || det < 1e-12 {
                write!(text, "rotate({})", t.angle as f32)
            } else {
                let cx = ((1.0 - cos) * m[4] - sin * m[5]) / det;
                let cy = (sin * m[4] + (1.0 - cos) * m[5]) / det;
                write!(text, "rotate({} {} {})", t.angle as f32, cx as f32, cy as f32)
            }
        }
        5 => write!(text, "skewX({})", t.angle as f32),
        6 => write!(text, "skewY({})", t.angle as f32),
        _ => write!(text, "matrix({} {} {} {} {} {})", m[0] as f32, m[1] as f32, m[2] as f32, m[3] as f32, m[4] as f32, m[5] as f32),
    };
    written.expect("writing to String is infallible");
}

impl TransformLists {
    pub(crate) fn attribute_changed(&self, text: &str) {
        if !self.writing.get() {
            self.sync_attribute(parse_transform_list(text).into_iter());
        }
    }
}

/// Unit suffixes indexed by the internal kind. Kinds 1 through 10 are
/// SVGLength's public constants; the remaining units report UNKNOWN.
pub const LENGTH_UNITS: &[&str] = &[
    "", "", "%", "em", "ex", "px", "cm", "mm", "in", "pt", "pc", "rem", "vw", "vh", "vmin", "vmax", "q",
    "ch", "cap", "ic", "lh", "rlh", "rex", "rch", "rcap", "ric", "vi", "vb",
    "svw", "svh", "svmin", "svmax", "svi", "svb", "lvw", "lvh", "lvmin", "lvmax", "lvi", "lvb",
    "dvw", "dvh", "dvmin", "dvmax", "dvi", "dvb",
];

impl LengthItem {
    /// The current list supplies the viewport, direction and font for unit
    /// conversion. Detached lengths have no associated element.
    pub fn unit_factor(&self, kind: u16) -> Option<f64> {
        let attachment = self.attached.borrow().clone()?;
        attachment.owner.length_unit_factor.as_ref().map(|factor| factor(kind))
    }

    pub fn is_read_only(&self) -> bool {
        self.attached.borrow().as_ref().is_some_and(|a| a.animated)
    }
}

impl PointItem {
    pub fn coordinates(&self) -> Pair {
        self.value()
    }

    pub fn set_coordinates(&self, value: Pair) -> DomResult<()> {
        self.set_value(value)
    }
}

impl PointLists {
    pub(crate) fn attribute_changed(&self, text: &str) {
        self.sync_attribute(parse_points(text));
    }
}

// Filter Effects requires whitespace or a comma between numbers. A missing item or one
// malformed token invalidates the whole attribute, including its valid prefix.
fn parse_numbers(text: &str) -> Option<Vec<f32>> {
    let mut values = Vec::new();
    for group in text.split(',') {
        let start = values.len();
        for token in group.split([' ', '\t', '\r', '\n', '\x0c']).filter(|s| !s.is_empty()) {
            let value = token.parse::<svgtypes::Number>().ok()?.0 as f32;
            if !value.is_finite() {
                return None;
            }
            values.push(value);
        }
        if start == values.len() {
            return None;
        }
    }
    Some(values)
}

// SVG text's rotate attribute uses CSS value syntax: [ <number>+ ]#.
// Adjacent number tokens need no whitespace when tokenization is unambiguous.
fn parse_rotate(text: &str) -> Option<Vec<f32>> {
    use cssparser::{Parser, Token};
    let mut parser = Parser::new(text);
    let mut values = Vec::new();
    let mut need_number = true;
    while !parser.is_exhausted() {
        match parser.next().ok()? {
            Token::Number { value, .. } if value.is_finite() => {
                values.push(*value);
                need_number = false;
            }
            Token::Comma if !need_number => need_number = true,
            _ => return None,
        }
    }
    (!need_number).then_some(values)
}

impl NumberLists {
    pub(crate) fn attribute_changed(&self, attr: &str, text: &str) {
        if !self.writing.get() {
            let parsed = if attr == "rotate" { parse_rotate(text) } else { parse_numbers(text) };
            self.sync_attribute(parsed.into_iter().flatten());
        }
    }
}

// For x, y, dx, and dy, SVG 2 Text specifies comma- or space-separated
// lengths, percentages, or unitless numbers. Reject the whole list when
// any member is invalid, preserving neither a numeric nor a list prefix.
fn parse_lengths(text: &str) -> Option<Vec<(f64, u16)>> {
    use cssparser::{Parser, Token};
    let mut values = Vec::new();
    for group in text.split(',') {
        let start = values.len();
        for word in group.split_ascii_whitespace() {
            let mut p = Parser::new(word);
            let (value, kind) = match p.next_including_whitespace_and_comments().ok()? {
                Token::Number { value, .. } => (*value, 1),
                Token::Percentage { .. } => (word.strip_suffix('%')?.parse::<f32>().ok()?, 2),
                Token::Dimension { value, unit, .. } => {
                    let kind = LENGTH_UNITS[3..].iter().position(|s| s.eq_ignore_ascii_case(unit))? + 3;
                    (*value, kind as u16)
                }
                _ => return None,
            };
            if p.position().byte_index() != word.len() || !value.is_finite() {
                return None;
            }
            values.push((f64::from(value), kind));
        }
        if values.len() == start {
            return None;
        }
    }
    Some(values)
}

impl LengthLists {
    pub(crate) fn attribute_changed(&self, text: &str) {
        if !self.writing.get() {
            self.sync_attribute(parse_lengths(text).into_iter().flatten());
        }
    }
}

impl Dom {
    pub fn svg_point_lists(&mut self, id: NodeId, write: Box<dyn Fn(Option<&str>) -> DomResult<()>>) -> DomResult<Rc<PointLists>> {
        let index = self.check(id)?;
        if let Some(lists) = self.svg_points.get(&index).and_then(Weak::upgrade) {
            return Ok(lists);
        }
        let point_writer = Box::new(move |text: &str| write((!text.is_empty()).then_some(text)));
        let lists = PointLists::new(point_writer, |text, (x, y)| {
            write!(text, "{x} {y}").expect("writing to String is infallible");
        }, None);
        lists.attribute_changed(self.get_attribute_ns(id, None, "points")?.unwrap_or_default());
        self.svg_points.insert(index, Rc::downgrade(&lists));
        Ok(lists)
    }

    pub fn svg_length_lists(
        &mut self,
        id: NodeId,
        attr: &'static str,
        write: Writer,
        unit_factor: Box<dyn Fn(u16) -> f64>,
    ) -> DomResult<Rc<LengthLists>> {
        let index = self.check(id)?;
        if let Some(lists) = self.svg_lengths.get(&index).and_then(|attrs| attrs.get(attr)).and_then(Weak::upgrade) {
            return Ok(lists);
        }
        let lists = LengthLists::new(write, |text, (value, unit)| {
            write!(text, "{}{}", value as f32, LENGTH_UNITS[usize::from(unit)]).expect("writing to String is infallible");
        }, Some(unit_factor));
        lists.attribute_changed(self.get_attribute_ns(id, None, attr)?.unwrap_or_default());
        self.svg_lengths.entry(index).or_default().insert(attr, Rc::downgrade(&lists));
        Ok(lists)
    }

    /// The `SVGTransformList`s of the transform list attribute `attr` of `id` (`transform`,
    /// `gradientTransform`, `patternTransform`).
    pub fn svg_transform_lists(&mut self, id: NodeId, attr: &'static str, write: Writer) -> DomResult<Rc<TransformLists>> {
        let index = self.check(id)?;
        if let Some(lists) = self.svg_transforms.get(&index).and_then(|attrs| attrs.get(attr)).and_then(Weak::upgrade) {
            return Ok(lists);
        }
        let lists = TransformLists::new(write, write_transform, None);
        lists.attribute_changed(self.get_attribute_ns(id, None, attr)?.unwrap_or_default());
        self.svg_transforms.entry(index).or_default().insert(attr, Rc::downgrade(&lists));
        Ok(lists)
    }

    pub fn svg_number_lists(&mut self, id: NodeId, attr: &'static str, write: Writer) -> DomResult<Rc<NumberLists>> {
        let index = self.check(id)?;
        if let Some(lists) = self.svg_numbers.get(&index).and_then(|attrs| attrs.get(attr)).and_then(Weak::upgrade) {
            return Ok(lists);
        }
        let lists = NumberLists::new(write, |text, n| write!(text, "{n}").expect("writing to String is infallible"), None);
        lists.attribute_changed(attr, self.get_attribute_ns(id, None, attr)?.unwrap_or_default());
        self.svg_numbers.entry(index).or_default().insert(attr, Rc::downgrade(&lists));
        Ok(lists)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DocumentKind;

    #[test]
    fn rotate_uses_css_number_tokens() {
        assert_eq!(parse_rotate("1-2,1.2.3/**/4\x0c5"), Some(vec![1., -2., 1.2, 0.3, 4., 5.]));
        for invalid in ["1,", ",2", "1,,2", "2px", "3%"] {
            assert_eq!(parse_rotate(invalid), None);
        }
        assert_eq!(parse_numbers("1-2"), None);
        assert_eq!(parse_numbers("1\x0c2"), Some(vec![1., 2.]));
    }

    #[test]
    fn number_attributes_observe_independent_live_lists() {
        let mut dom = Dom::new();
        let doc = dom.create_document(DocumentKind::Html, "about:blank");
        let node = dom.create_element_ns(doc, Some("http://www.w3.org/2000/svg"), "feConvolveMatrix").unwrap();
        dom.set_attribute(node, "kernelMatrix", "1, 2e1 -3.5").unwrap();
        let kernel = dom.svg_number_lists(node, "kernelMatrix", Box::new(|_| Ok(()))).unwrap();
        let other = dom.svg_number_lists(node, "other", Box::new(|_| Ok(()))).unwrap();
        assert_eq!(kernel.len(false), 3);
        let held = kernel.item(1, true).unwrap();
        assert_eq!(held.value(), 20.);
        dom.set_attribute(node, "other", "100").unwrap();
        assert_eq!(other.len(false), 1);
        dom.set_attribute(node, "kernelMatrix", "1 2").unwrap();
        assert_eq!(held.value(), 2.);
        assert!(held.set_value(9.).is_err());
        for invalid in ["1,2,", "1 2-3", "2,,3", "1 1e90", "1 2 INVALID"] {
            dom.set_attribute(node, "kernelMatrix", invalid).unwrap();
            assert_eq!(kernel.len(false), 0, "{invalid}");
        }
        held.set_value(9.).unwrap();
        assert_eq!(other.item(0, false).unwrap().value(), 100.);
        dom.remove_attribute(node, "other").unwrap();
        assert_eq!(other.len(true), 0);
    }

    #[test]
    fn length_lists_parse_units_and_reject_invalid_members() {
        assert_eq!(parse_lengths(" 1e-2px,+2.5% 3EM, 4Q "),
            Some(vec![(f64::from(0.01f32), 5), (2.5, 2), (3., 3), (4., 16)]));
        assert_eq!(parse_lengths("1ch 2cap,3ic 4lh"), Some(vec![(1., 17), (2., 18), (3., 19), (4., 20)]));
        for text in ["1-2", "1px-2px", "1/**/2", ",1", "1,,2", "1,2,", "1px 2bad", "1e99pc"] {
            assert_eq!(parse_lengths(text), None, "{text}");
        }
    }

    #[test]
    fn length_items_follow_their_owner_across_mutation_and_removal() {
        let dom = Rc::new(RefCell::new(Dom::new()));
        let doc = dom.borrow_mut().create_document(DocumentKind::Html, "about:blank");
        let node = dom.borrow_mut().create_element_ns(doc, Some("http://www.w3.org/2000/svg"), "text").unwrap();
        dom.borrow_mut().set_attribute(node, "x", "1cm 25%").unwrap();
        let writer = dom.clone();
        let x = dom.borrow_mut().svg_length_lists(
            node, "x", Box::new(move |s| writer.borrow_mut().set_attribute(node, "x", s)), Box::new(|_| 2.),
        ).unwrap();
        let y = dom.borrow_mut().svg_length_lists(node, "y", Box::new(|_| Ok(())), Box::new(|_| 3.)).unwrap();
        let first = x.item(0, false).unwrap();
        let last = x.item(1, true).unwrap();
        assert_eq!(first.unit_factor(2), Some(2.));
        assert!(!first.is_read_only());
        assert!(last.is_read_only());
        assert!(last.set_value((2., 5)).is_err());
        first.set_value((2., 6)).unwrap();
        assert_eq!(dom.borrow().get_attribute(node, "x").unwrap(), Some("2cm 25%"));
        dom.borrow_mut().set_attribute(node, "x", "10em").unwrap();
        assert_eq!(first.value(), (10., 3));
        assert_eq!(last.unit_factor(2), None);
        assert!(!last.is_read_only());
        last.set_value((3., 5)).unwrap();
        assert!(Rc::ptr_eq(&last, &y.insert(0, &last, false).unwrap()));
        assert_eq!(last.unit_factor(2), Some(3.));
        dom.borrow_mut().set_attribute(node, "x", "5 broken").unwrap();
        assert_eq!(x.len(true), 0);
        assert_eq!(first.unit_factor(2), None);
        assert_eq!(first.value(), (10., 3));
    }

    #[test]
    fn retained_points_survive_rewrites_and_detach_on_shrink() {
        let dom = Rc::new(RefCell::new(Dom::new()));
        let doc = dom.borrow_mut().create_document(DocumentKind::Html, "about:blank");
        let node = dom.borrow_mut().create_element_ns(doc, Some("http://www.w3.org/2000/svg"), "polygon").unwrap();
        dom.borrow_mut().set_attribute(node, "points", "10,20 30,40").unwrap();
        let target = dom.clone();
        let lists = dom
            .borrow_mut()
            .svg_point_lists(
                node,
                Box::new(move |text| match text {
                    Some(s) => target.borrow_mut().set_attribute(node, "points", s),
                    None => target.borrow_mut().remove_attribute(node, "points").map(|_| ()),
                }),
            )
            .unwrap();
        let first = lists.item(0, false).unwrap();
        let last = lists.item(1, false).unwrap();
        let animated = lists.item(1, true).unwrap();
        assert!(animated.set_coordinates((1., 1.)).is_err());
        dom.borrow_mut().set_attribute(node, "points", "5 6").unwrap();
        dom.borrow_mut().set_attribute(node, "points", "7 8 9 10").unwrap();
        assert_eq!(first.coordinates(), (7., 8.));
        assert_eq!(last.coordinates(), (30., 40.));
        animated.set_coordinates((50., 60.)).unwrap();
        assert_eq!(lists.item(1, true).unwrap().coordinates(), (9., 10.));
        assert!(Rc::ptr_eq(&last, &lists.insert(0, &last, false).unwrap()));
        assert!(Rc::ptr_eq(&first, &lists.item(1, false).unwrap()));
        let copied = lists.insert(0, &first, true).unwrap();
        assert!(!Rc::ptr_eq(&copied, &first));
        first.set_coordinates((11., 12.)).unwrap();
        assert_eq!(copied.coordinates(), (7., 8.));
        assert_eq!(dom.borrow().get_attribute(node, "points").unwrap(), Some("7 8 11 12 9 10"));
        assert!(Rc::ptr_eq(&first, &lists.remove(1).unwrap()));
        let initialized = lists.initialize(&copied).unwrap();
        assert!(Rc::ptr_eq(&copied, &initialized));
        initialized.set_coordinates((13., 14.)).unwrap();
        assert_eq!(dom.borrow().get_attribute(node, "points").unwrap(), Some("13 14"));
        drop(initialized);
        let weak = Rc::downgrade(&lists);
        drop(lists);
        assert!(weak.upgrade().is_some(), "the attached copy retains its element writer");
        drop(copied);
        assert!(weak.upgrade().is_none(), "detached items do not retain the element writer");
    }
}
