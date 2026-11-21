//! Polygon and polyline points. The DOM holds a weak observer, so a retained
//! list or point keeps the element alive through its writer without a cycle.

use crate::{Dom, DomError, DomErrorKind, DomResult, NodeId};
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

type Pair = (f32, f32);
type Writer = Box<dyn Fn(Option<&str>) -> DomResult<()>>;

pub(crate) fn parse_points(text: &str) -> impl Iterator<Item = Pair> + '_ {
    svgtypes::PointsParser::from(text).map_while(|(x, y)| {
        let (x, y) = (x as f32, y as f32);
        (x.is_finite() && y.is_finite()).then_some((x, y))
    })
}

fn out_of_bounds() -> DomError {
    DomError::new(DomErrorKind::IndexSize, "The index is outside the SVG point list.")
}

pub fn read_only() -> DomError {
    DomError::new(DomErrorKind::NoModificationAllowed, "The animated point list is read only.")
}

#[derive(Clone)]
struct Attachment {
    owner: Rc<PointLists>,
    index: usize,
    animated: bool,
}

pub struct PointItem {
    value: Cell<Pair>,
    attached: RefCell<Option<Attachment>>,
}

impl PointItem {
    pub fn detached(value: Pair) -> Rc<Self> {
        Rc::new(Self { value: Cell::new(value), attached: RefCell::new(None) })
    }

    pub fn coordinates(&self) -> Pair {
        self.value.get()
    }

    pub fn set_coordinates(&self, value: Pair) -> DomResult<()> {
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
        if self.attached.borrow().is_some() {
            Self::detached(self.coordinates())
        } else {
            self.clone()
        }
    }
}

struct Slot {
    value: Pair,
    item: Weak<PointItem>,
}

impl Slot {
    fn new(value: Pair) -> Self {
        Self { value, item: Weak::new() }
    }

    fn detach(&self) {
        if let Some(item) = self.item.upgrade() {
            item.attached.replace(None);
        }
    }
}

pub struct PointLists {
    base: RefCell<Vec<Slot>>,
    animated: RefCell<Vec<Slot>>,
    writing: Cell<bool>,
    write: Writer,
}

impl PointLists {
    fn view(&self, animated: bool) -> &RefCell<Vec<Slot>> {
        if animated {
            &self.animated
        } else {
            &self.base
        }
    }

    pub fn len(&self, animated: bool) -> usize {
        self.view(animated).borrow().len()
    }

    /// Attribute synchronization preserves surviving objects by index.
    fn synchronize(list: &mut Vec<Slot>, values: impl Iterator<Item = Pair>) {
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

    pub(crate) fn attribute_changed(&self, text: &str) {
        if self.writing.get() {
            return;
        }
        let mut base = self.base.borrow_mut();
        Self::synchronize(&mut base, parse_points(text));
        Self::synchronize(&mut self.animated.borrow_mut(), base.iter().map(|s| s.value));
    }

    fn commit(&self) -> DomResult<()> {
        use std::fmt::Write;
        let mut text = String::new();
        let base = self.base.borrow();
        for (i, s) in base.iter().enumerate() {
            if i != 0 {
                text.push(' ');
            }
            write!(text, "{} {}", s.value.0, s.value.1).expect("writing to String is infallible");
        }
        Self::synchronize(&mut self.animated.borrow_mut(), base.iter().map(|s| s.value));
        drop(base);
        // The ordinary attribute write queues mutation records and dirties
        // geometry. Its observer must not renumber the just-edited objects.
        self.writing.set(true);
        let result = (self.write)((!text.is_empty()).then_some(&text));
        self.writing.set(false);
        result
    }

    pub fn item(self: &Rc<Self>, index: usize, animated: bool) -> Option<Rc<PointItem>> {
        let mut view = self.view(animated).borrow_mut();
        let slot = view.get_mut(index)?;
        if let Some(item) = slot.item.upgrade() {
            return Some(item);
        }
        let item = PointItem::detached(slot.value);
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

    pub fn initialize(self: &Rc<Self>, value: &Rc<PointItem>) -> DomResult<Rc<PointItem>> {
        for slot in self.base.borrow_mut().drain(..) {
            slot.detach();
        }
        self.insert(0, value, false)
    }

    pub fn insert(self: &Rc<Self>, index: usize, value: &Rc<PointItem>, replace: bool) -> DomResult<Rc<PointItem>> {
        let mut base = self.base.borrow_mut();
        if replace && index >= base.len() {
            return Err(out_of_bounds());
        }
        let index = index.min(base.len());
        let value = value.insertion_value();
        let slot = Slot { value: value.coordinates(), item: Rc::downgrade(&value) };
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

    pub fn remove(self: &Rc<Self>, index: usize) -> DomResult<Rc<PointItem>> {
        let value = self.item(index, false).ok_or_else(out_of_bounds)?;
        self.base.borrow_mut().remove(index).detach();
        self.reindex(index);
        self.commit()?;
        Ok(value)
    }
}

impl Dom {
    pub fn svg_point_lists(&mut self, id: NodeId, write: Writer) -> DomResult<Rc<PointLists>> {
        let index = self.check(id)?;
        if let Some(lists) = self.svg_points.get(&index).and_then(Weak::upgrade) {
            return Ok(lists);
        }
        let lists = Rc::new(PointLists {
            base: RefCell::default(),
            animated: RefCell::default(),
            writing: Cell::new(false),
            write,
        });
        lists.attribute_changed(self.get_attribute_ns(id, None, "points")?.unwrap_or_default());
        self.svg_points.insert(index, Rc::downgrade(&lists));
        Ok(lists)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DocumentKind;

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
