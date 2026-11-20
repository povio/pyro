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
    /// pyro: the engine's attributes (by canonical name, `data-pyro-sheet`, ...): those the markup
    /// or tree carried under this document's nonce name (`crate::engine_field`), kept apart from
    /// the element's own: no selector, `attr()`, serialisation or iteration sees them, and the
    /// page's own `data-pyro-*` attributes are ordinary attributes. Only the engine's readers
    /// ([`Attributes::reserved`]) see them.
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
        Self {
            inner: inner.into_iter().collect(),
            reserved: ThinVec::new(),
            presentational_hints: OnceLock::new(),
        }
    }

    /// pyro: the engine's attribute `canonical` (`data-pyro-sheet`, ...), for engine readers only.
    pub fn reserved(&self, canonical: &str) -> Option<&str> {
        self.reserved.iter().find(|a| &*a.name.local == canonical).map(|a| a.value.as_str())
    }

    /// pyro: sets the engine's attribute `canonical` (never one of the element's own).
    pub fn set_reserved(&mut self, canonical: &str, value: &str) {
        match self.reserved.iter_mut().find(|a| &*a.name.local == canonical) {
            Some(a) => value.clone_into(&mut a.value),
            None => self.reserved.push(Attribute {
                name: QualName::new(None, markup5ever::ns!(), canonical.into()),
                value: value.to_owned(),
            }),
        }
    }

    /// pyro: removes the engine's attribute `canonical`.
    pub fn remove_reserved(&mut self, canonical: &str) -> Option<Attribute> {
        let idx = self.reserved.iter().position(|a| &*a.name.local == canonical)?;
        Some(self.reserved.remove(idx))
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

#[cfg(test)]
mod reserved_tests {
    use super::*;

    fn name(local: &str) -> QualName {
        QualName::new(None, markup5ever::ns!(), local.into())
    }

    /// The engine's attributes are never among an element's own (iteration, lookup), and the
    /// element's own `data-pyro-*` attributes are ordinary ones (security review cycles 7 and 8).
    #[test]
    fn reserved_attributes_are_kept_apart() {
        let mut attrs = Attributes::new(vec![
            Attribute { name: name("id"), value: "a".into() },
            Attribute { name: name("data-pyro-theme"), value: "dark".into() },
        ]);
        attrs.set_reserved("data-pyro-sheet", "k");
        attrs.set(name("data-pyro-n"), "forged");
        let visible: Vec<&str> = attrs.iter().map(|a| &*a.name.local).collect();
        assert_eq!(visible, ["id", "data-pyro-theme", "data-pyro-n"]);
        assert_eq!((attrs.reserved("data-pyro-sheet"), attrs.reserved("data-pyro-n")), (Some("k"), None));
        assert_eq!(attrs.remove_reserved("data-pyro-sheet").map(|a| a.value), Some("k".into()));
        assert_eq!(attrs.reserved("data-pyro-sheet"), None);
    }
}
