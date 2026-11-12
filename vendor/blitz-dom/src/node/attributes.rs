use std::ops::{Deref, DerefMut};
use std::sync::OnceLock;

use markup5ever::QualName;
use style::properties::PropertyDeclarationBlock;
use style::servo_arc::Arc as ServoArc;
use style::shared_lock::Locked;
use thin_vec::ThinVec;

/// A tag attribute, e.g. `class="test"` in `<div class="test" ...>`.
#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Debug)]
pub struct Attribute {
    /// The name of the attribute (e.g. the `class` in `<div class="test">`)
    pub name: QualName,
    /// The value of the attribute (e.g. the `"test"` in `<div class="test">`)
    pub value: String,
}

/// pyro: the declaration blocks of an element's presentational
/// hints, which Stylo 0.24 borrows from the element for the length of a
/// cascade (`TElement::synthesize_presentational_hints_for_legacy_attributes`).
pub struct PresentationalHints {
    /// What the element's own attributes map to, one block per declaration.
    pub own: Box<[ServoArc<Locked<PropertyDeclarationBlock>>]>,
    /// A table's `cellpadding`: the padding of its cells, which borrow it.
    pub cells: Box<[ServoArc<Locked<PropertyDeclarationBlock>>]>,
}

pub struct Attributes {
    inner: ThinVec<Attribute>,
    /// pyro: the hints these attributes map to, built at the
    /// first cascade that asks and dropped by every write (`set`, `remove`,
    /// `DerefMut`). They depend on the attributes and the element's name
    /// only.
    presentational_hints: OnceLock<Box<PresentationalHints>>,
}

impl Clone for Attributes {
    /// The copy may belong to another element: it builds its own hints.
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            presentational_hints: OnceLock::new(),
        }
    }
}

impl std::fmt::Debug for Attributes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Attributes").field("inner", &self.inner).finish()
    }
}

impl Attributes {
    pub fn new(inner: Vec<Attribute>) -> Self {
        Self {
            inner: inner.into_iter().collect(),
            presentational_hints: OnceLock::new(),
        }
    }

    pub fn get(&mut self, name: &QualName) -> Option<&Attribute> {
        self.inner.iter().find(|attr| attr.name == *name)
    }

    pub fn set(&mut self, name: QualName, value: &str) {
        self.presentational_hints.take();
        let existing_attr = self.inner.iter_mut().find(|a| a.name == name);
        if let Some(existing_attr) = existing_attr {
            existing_attr.value.clear();
            existing_attr.value.push_str(value);
        } else {
            self.push(Attribute {
                name: name.clone(),
                value: value.to_string(),
            });
        }
    }

    pub fn remove(&mut self, name: &QualName) -> Option<Attribute> {
        self.presentational_hints.take();
        let idx = self.inner.iter().position(|attr| attr.name == *name);
        idx.map(|idx| self.inner.remove(idx))
    }

    /// pyro: the cached presentational hints, built by `build`
    /// when no cascade asked since the last write.
    pub fn presentational_hints(&self, build: impl FnOnce() -> PresentationalHints) -> &PresentationalHints {
        self.presentational_hints.get_or_init(|| Box::new(build()))
    }
}

impl Deref for Attributes {
    type Target = ThinVec<Attribute>;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
impl DerefMut for Attributes {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.presentational_hints.take();
        &mut self.inner
    }
}
