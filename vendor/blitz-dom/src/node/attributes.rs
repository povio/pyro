use std::ops::{Deref, DerefMut};
use std::sync::OnceLock;

use markup5ever::{QualName, local_name};
use style::Atom;
use thin_vec::ThinVec;

/// A tag attribute, e.g. `class="test"` in `<div class="test" ...>`.
#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Debug)]
pub struct Attribute {
    /// The name of the attribute (e.g. the `class` in `<div class="test">`)
    pub name: QualName,
    /// The value of the attribute (e.g. the `"test"` in `<div class="test">`)
    pub value: String,
}

#[derive(Clone, Debug)]
pub struct Attributes {
    inner: ThinVec<Attribute>,
    /// The `class` attribute's tokens, interned on first use and dropped on
    /// every mutation, so selector matching does not re-intern them per check.
    classes: OnceLock<Box<[Atom]>>,
}

impl Attributes {
    pub fn new(inner: Vec<Attribute>) -> Self {
        Self {
            inner: inner.into_iter().collect(),
            classes: OnceLock::new(),
        }
    }

    /// The tokens of the `class` attribute, as atoms.
    pub fn class_atoms(&self) -> &[Atom] {
        self.classes.get_or_init(|| {
            self.inner
                .iter()
                .find(|a| a.name.local == local_name!("class"))
                .map(|a| a.value.split_ascii_whitespace().map(Atom::from).collect())
                .unwrap_or_default()
        })
    }

    pub fn get(&mut self, name: &QualName) -> Option<&Attribute> {
        self.inner.iter().find(|attr| attr.name == *name)
    }

    pub fn set(&mut self, name: QualName, value: &str) {
        self.classes.take();
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
        self.classes.take();
        let idx = self.inner.iter().position(|attr| attr.name == *name);
        idx.map(|idx| self.inner.remove(idx))
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
        self.classes.take();
        &mut self.inner
    }
}
