//! Names script passes as strings (`getAttribute(name)`, `setAttribute(name, ..)`,
//! `createElement(name)`), resolved to atoms by the identity of the JavaScript
//! string instead of by its text.
//!
//! A name in page script is nearly always a literal or a property key: one
//! JavaScript string, passed again on every call. Its characters are read,
//! checked and interned once, and the result is kept under the address of
//! the string cell. A later call with the same string finds the atoms with
//! one probe, makes no call into the engine and reads no character.
//!
//! # What keeps an entry valid
//!
//! An address names a string only while that string is alive: a collected
//! cell's address is given to the next cell of its size. So a resolved entry
//! holds a [`Persistent`] root of the string it was made from. While the
//! entry exists the cell is alive, no other value can have its address, and
//! a string's text never changes: a match on the address is a match on the
//! text. An entry whose root is gone does not exist: replacing or dropping
//! the entry is the only way the root is released.
//!
//! The key is the cell and not its character buffer: JavaScriptCore replaces
//! a string's buffer when the string is used as a property key and an atom
//! with its text already exists (`JSString::swapToAtomString`), and the old
//! buffer's address is then free for another string while the cell lives.
//!
//! A value seen for the first time is only remembered as a candidate, by
//! address, with no root and no atom: the address is compared, and never
//! read through. When the next sighting in that slot has the same address,
//! the value *of that call* is resolved (its own text, whatever the earlier
//! value at that address was) and rooted. Strings built per call
//! (`'data-' + key`) therefore cost one compare and one store over the text
//! path, and never an atom or a root unless their address repeats.

use std::borrow::Cow;
use std::cell::RefCell;

use pyro_dom::ScriptName;
use pyro_js::{JsResult, Persistent, Scope, StringUnits, Value};

/// Slots of a page's table (a power of two). A page uses a few dozen names.
const SLOTS: usize = 256;
/// Longer strings are not names worth an entry.
const MAX_NAME: usize = 64;

#[derive(Default)]
struct Slot {
    /// The value's bits: for a string, the address of its cell (0: empty slot).
    key: usize,
    /// The resolved name and the root that keeps `key` this string's (see
    /// the module docs); `None` for a candidate.
    name: Option<(ScriptName, Persistent)>,
}

/// A page's names by JavaScript string identity.
#[derive(Default)]
pub(crate) struct NameTable {
    /// Empty until the first name argument.
    slots: RefCell<Vec<Slot>>,
}

/// A name argument: its atoms when the table has the string, else its text.
pub(crate) enum NameArg<'v> {
    Name(ScriptName),
    Text(Cow<'v, str>),
}

impl NameArg<'_> {
    /// The name as script wrote it.
    pub(crate) fn text(&self) -> &str {
        match self {
            NameArg::Name(n) => &n.exact,
            NameArg::Text(t) => t,
        }
    }
}

impl NameTable {
    /// `v` converted as `util::str_of` does, as atoms when `v` is a string
    /// this table resolved (or resolves now, on its second sighting).
    pub(crate) fn arg<'v>(&self, s: &Scope<'_>, v: &'v Value<'_>) -> JsResult<NameArg<'v>> {
        let key = v.as_raw() as usize;
        let seen = {
            let mut slots = self.slots.borrow_mut();
            if slots.is_empty() {
                slots.resize_with(SLOTS, Slot::default);
            }
            // Cells are 16 bytes apart at least.
            let slot = &mut slots[((key >> 4) ^ (key >> 12)) & (SLOTS - 1)];
            if slot.key == key {
                if let Some((name, _)) = &slot.name {
                    return Ok(NameArg::Name(name.clone()));
                }
                true
            } else {
                // Another value's slot: it becomes this one's candidate (a
                // resolved entry's root is released with it).
                *slot = Slot { key, name: None };
                false
            }
        };
        let Some(StringUnits::Latin1(bytes)) = v.string_units() else {
            return crate::util::dom_string(*v).map(|t| NameArg::Text(Cow::Owned(t)));
        };
        if !bytes.is_ascii() {
            return crate::util::dom_string(*v).map(|t| NameArg::Text(Cow::Owned(t)));
        }
        // SAFETY: ASCII bytes are UTF-8.
        let text = unsafe { std::str::from_utf8_unchecked(bytes) };
        if seen && bytes.len() <= MAX_NAME {
            let name = ScriptName::new(text);
            // The slot is looked up again: nothing ran in between, but the
            // borrow is not held across the calls into the engine. The root
            // goes in with the name: from here on the address is this
            // string's for as long as the entry is.
            let mut slots = self.slots.borrow_mut();
            let slot = &mut slots[((key >> 4) ^ (key >> 12)) & (SLOTS - 1)];
            if slot.key == key {
                slot.name = Some((name.clone(), Persistent::new(s, *v)));
            }
            return Ok(NameArg::Name(name));
        }
        Ok(NameArg::Text(Cow::Borrowed(text)))
    }

    /// Drops every entry and its root (the page is closing).
    pub(crate) fn clear(&self) {
        self.slots.borrow_mut().clear();
    }
}
