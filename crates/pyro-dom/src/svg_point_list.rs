//! Live SVG point and number lists. The DOM holds a weak observer, so a retained
//! list or point keeps the element alive through its writer without a cycle.

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
    writing: Cell<bool>,
    write: Writer,
    write_item: fn(&mut String, T),
}

impl<T: Copy> ListStorage<T> {
    fn new(write: Writer, write_item: fn(&mut String, T)) -> Rc<Self> {
        Rc::new(Self {
            base: RefCell::default(),
            animated: RefCell::default(),
            writing: Cell::new(false),
            write,
            write_item,
        })
    }

    fn view(&self, animated: bool) -> &RefCell<Vec<Slot<T>>> {
        if animated { &self.animated } else { &self.base }
    }

    pub fn len(&self, animated: bool) -> usize {
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

// SVG number lists use comma-wsp between numbers. A missing item or one
// malformed token invalidates the whole attribute, including its valid prefix.
fn parse_numbers(text: &str) -> Option<Vec<f32>> {
    let mut values = Vec::new();
    for group in text.split(',') {
        let start = values.len();
        for token in group.split([' ', '\t', '\r', '\n']).filter(|s| !s.is_empty()) {
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

impl NumberLists {
    pub(crate) fn attribute_changed(&self, text: &str) {
        if !self.writing.get() {
            self.sync_attribute(parse_numbers(text).into_iter().flatten());
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
        });
        lists.attribute_changed(self.get_attribute_ns(id, None, "points")?.unwrap_or_default());
        self.svg_points.insert(index, Rc::downgrade(&lists));
        Ok(lists)
    }

    pub fn svg_number_lists(&mut self, id: NodeId, attr: &'static str, write: Writer) -> DomResult<Rc<NumberLists>> {
        let index = self.check(id)?;
        if let Some(lists) = self.svg_numbers.get(&index).and_then(|attrs| attrs.get(attr)).and_then(Weak::upgrade) {
            return Ok(lists);
        }
        let lists = NumberLists::new(write, |text, n| write!(text, "{n}").expect("writing to String is infallible"));
        lists.attribute_changed(self.get_attribute_ns(id, None, attr)?.unwrap_or_default());
        self.svg_numbers.entry(index).or_default().insert(attr, Rc::downgrade(&lists));
        Ok(lists)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DocumentKind;

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
