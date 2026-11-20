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

/// pyro: the attribute family render snapshots carry engine data in (`data-pyro-*`, no
/// namespace): never one of an element's attributes in the renderer's document; see
/// [`Attributes::reserved`].
pub const RESERVED_PREFIX: &str = "data-pyro-";

/// pyro: whether `name` is of the [`RESERVED_PREFIX`] family.
pub fn is_reserved(name: &QualName) -> bool {
    name.ns == markup5ever::ns!() && name.local.starts_with(RESERVED_PREFIX)
}

pub struct Attributes {
    inner: ThinVec<Attribute>,
    /// pyro: the [`RESERVED_PREFIX`] attributes the markup or tree carried, kept apart: no
    /// selector, `attr()`, serialisation or iteration of the element's attributes sees them (author
    /// CSS could otherwise probe the snapshot's nonce through layout); only the engine's readers
    /// ([`Attributes::reserved`]) do.
    reserved: ThinVec<Attribute>,
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
            reserved: self.reserved.clone(),
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
        let (reserved, inner): (Vec<Attribute>, Vec<Attribute>) = inner.into_iter().partition(|a| is_reserved(&a.name));
        Self {
            inner: inner.into_iter().collect(),
            reserved: reserved.into_iter().collect(),
            presentational_hints: OnceLock::new(),
        }
    }

    /// pyro: the engine's reserved attribute `local` ([`RESERVED_PREFIX`]), for engine readers only.
    pub fn reserved(&self, local: &str) -> Option<&str> {
        self.reserved.iter().find(|a| &*a.name.local == local).map(|a| a.value.as_str())
    }

    pub fn get(&mut self, name: &QualName) -> Option<&Attribute> {
        self.inner.iter().find(|attr| attr.name == *name)
    }

    pub fn set(&mut self, name: QualName, value: &str) {
        if is_reserved(&name) {
            match self.reserved.iter_mut().find(|a| a.name == name) {
                Some(a) => value.clone_into(&mut a.value),
                None => self.reserved.push(Attribute { name, value: value.to_owned() }),
            }
            return;
        }
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
        if is_reserved(name) {
            let idx = self.reserved.iter().position(|attr| attr.name == *name);
            return idx.map(|idx| self.reserved.remove(idx));
        }
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

#[cfg(test)]
mod reserved_tests {
    use super::*;

    fn name(local: &str) -> QualName {
        QualName::new(None, markup5ever::ns!(), local.into())
    }

    /// The reserved family is never among an element's attributes (iteration, lookup), only the
    /// engine's accessor sees it, through construction, `set` and `remove` alike (security review
    /// cycle 7, 1).
    #[test]
    fn reserved_attributes_are_kept_apart() {
        let mut attrs = Attributes::new(vec![
            Attribute { name: name("id"), value: "a".into() },
            Attribute { name: name("data-pyro-n"), value: "0f".into() },
        ]);
        attrs.set(name("data-pyro-sheet"), "k");
        attrs.set(name("data-x"), "y");
        let visible: Vec<&str> = attrs.iter().map(|a| &*a.name.local).collect();
        assert_eq!(visible, ["id", "data-x"]);
        assert_eq!((attrs.reserved("data-pyro-n"), attrs.reserved("data-pyro-sheet")), (Some("0f"), Some("k")));
        // (Another namespace is no reserved attribute.)
        attrs.set(QualName::new(None, markup5ever::ns!(xlink), "data-pyro-n".into()), "z");
        assert_eq!(attrs.iter().count(), 3);
        assert_eq!(attrs.remove(&name("data-pyro-n")).map(|a| a.value), Some("0f".into()));
        assert_eq!(attrs.reserved("data-pyro-n"), None);
    }
}
