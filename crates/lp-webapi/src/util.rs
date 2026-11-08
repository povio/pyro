//! Conversions between lp-dom/lp-net results and JS values (WebIDL-ish).

use std::cell::Cell;

use lp_dom::{DomError, NodeId};
use lp_js::{Args, HeldValue, IdentityKey, JsError, JsResult, Object, Scope, Value, WeakRealm};

/// A wrapper found again through its realm's identity map. A native payload
/// keeps JS values its wrapper owns in hidden properties on that wrapper,
/// which the GC traces, rather than in a `Persistent`, which is a root the GC
/// cannot see through: a protected value that reaches back to the wrapper
/// would keep both, its realm and its page alive after the page closes.
#[derive(Clone)]
pub(crate) struct WrapperRef {
    realm: WeakRealm,
    identity: IdentityKey,
}

impl WrapperRef {
    pub(crate) fn new(s: &Scope<'_>, identity: IdentityKey) -> WrapperRef {
        WrapperRef { realm: s.realm().downgrade(), identity }
    }

    /// The wrapper, from a call whose receiver (or argument) is that wrapper,
    /// which keeps it alive.
    pub(crate) fn get<'s>(&self, s: &Scope<'s>) -> JsResult<Object<'s>> {
        let realm = self.realm.upgrade().ok_or(JsError::ContextDestroyed)?;
        let owner = realm.scope()?;
        let wrapper = owner.lookup_identity(self.identity).ok_or(JsError::ContextDestroyed)?;
        // SAFETY: the caller's live receiver traces this same-VM wrapper.
        Ok(unsafe { Value::from_raw(s.ctx(), wrapper.value().as_raw()) }.as_object().expect("wrapper"))
    }

    pub(crate) fn hidden<'s>(&self, s: &Scope<'s>, key: &'static str) -> JsResult<Option<Value<'s>>> {
        s.get_hidden(self.get(s)?, key)
    }
}

/// An interned atom as a JavaScript string: the realm's one string for it,
/// made on the first read (see [`Scope::interned_string`]).
pub(crate) trait AtomJs {
    fn js<'s>(&self, s: &Scope<'s>) -> Value<'s>;
}

/// The identity is string_cache's own: an atom's bits, which its equality
/// compares. A static atom's bits are an index into its set, so the second
/// half tells the sets apart (and 4 is `upper_local_name`); a dynamic atom's
/// are its entry's address, reusable once the atom is freed.
macro_rules! atom_js {
    ($($atom:ty => $set:literal),* $(,)?) => {$(
        impl AtomJs for $atom {
            #[inline]
            fn js<'s>(&self, s: &Scope<'s>) -> Value<'s> {
                s.interned_string((self.unsafe_data(), $set), self, self.is_dynamic())
            }
        }
    )*};
}
atom_js!(lp_dom::Atom => 0, markup5ever::LocalName => 1, markup5ever::Namespace => 2, markup5ever::Prefix => 3);

/// A local name uppercased (`tagName` of an HTML element in an HTML
/// document), by the local name's identity.
#[inline]
pub(crate) fn upper_local_name<'s>(s: &Scope<'s>, local: &markup5ever::LocalName) -> Value<'s> {
    s.interned_string_with((local.unsafe_data(), 4), local, local.is_dynamic(), || {
        str::to_ascii_uppercase(local).into()
    })
}

/// The realm's string for an atom-valued attribute of one node (its `id`, a
/// one-token `class`), kept beside the wrapper that reads it. The second read
/// of the same atom finds the string here, in the payload the getter has in
/// hand, instead of in the realm's interned-string table
/// ([`Scope::interned_string`]), whose probe (control bytes, entry, weak
/// handle) is three dependent cache lines once the page has read a few
/// hundred distinct ids. The atom is held, so its bits name its text for as
/// long as the entry lives: a changed attribute (another atom) is told apart
/// exactly, and the entry follows it. The string is held from the second
/// read on (a wrapper read once costs one small allocation and no slot), in
/// the engine's array of held values ([`HeldValue`]: no root per string, no
/// protect-table entry for every collection to walk), and released with the
/// payload at a safe point, or when another atom takes its place.
#[derive(Default)]
pub(crate) struct AtomString(Cell<Option<Box<(lp_dom::Atom, Option<HeldValue>)>>>);

impl AtomString {
    #[inline]
    pub(crate) fn get<'s>(&self, s: &Scope<'s>, atom: &lp_dom::Atom) -> Value<'s> {
        let mut entry = self.0.take();
        let v = match entry.as_mut() {
            Some(e) if e.0 == *atom => match &e.1 {
                Some(string) => string.get(s),
                None => {
                    let v = atom.js(s);
                    e.1 = Some(HeldValue::new(s, v));
                    v
                }
            },
            Some(e) => {
                e.0 = atom.clone();
                e.1 = None;
                atom.js(s)
            }
            None => {
                entry = Some(Box::new((atom.clone(), None)));
                atom.js(s)
            }
        };
        self.0.set(entry);
        v
    }
}

/// An element's id atom as a string: `""` when it has none.
#[inline]
pub(crate) fn atom_or_empty<'s>(s: &Scope<'s>, atom: Option<&lp_dom::Atom>) -> Value<'s> {
    match atom {
        Some(a) => a.js(s),
        None => lp_dom::Atom::default().js(s),
    }
}

/// An attribute's value as a string: the atom the element keeps for it
/// ([`lp_dom::node::ElementData::attr_atom`]), else the realm's string for
/// that text ([`Scope::text_string`]: one per distinct value, by its hash).
/// Nothing is done when the attribute is written.
#[inline]
pub(crate) fn attr_value<'s>(s: &Scope<'s>, value: &str, atom: Option<&lp_dom::Atom>) -> Value<'s> {
    match atom {
        Some(a) => a.js(s),
        None => s.text_string(value),
    }
}

/// `className` (and the class list's `value`): `""` without the attribute.
/// A one-token value is its token's atom, any other its text (interning the
/// value when written made every class write dearer).
#[inline]
pub(crate) fn class_name<'s>(s: &Scope<'s>, el: &lp_dom::node::ElementData) -> Value<'s> {
    match el.class_value() {
        Some((value, atom)) => attr_value(s, value, atom),
        None => lp_dom::Atom::default().js(s),
    }
}

/// Maps a DOM error to the JS exception the bindings throw.
pub fn dom_err(e: DomError) -> JsError {
    match e {
        DomError::Exception { kind, message } => JsError::dom(kind.name(), message),
        DomError::Type(m) => JsError::Type(m.to_owned().into()),
        DomError::StaleNode => JsError::dom("InvalidStateError", "The node is no longer available."),
    }
}

pub trait DomResultExt<T> {
    fn js(self) -> JsResult<T>;
}
impl<T> DomResultExt<T> for Result<T, DomError> {
    fn js(self) -> JsResult<T> {
        self.map_err(dom_err)
    }
}

/// WebIDL `DOMString` conversion (ToString; symbols throw).
pub fn dom_string(v: Value<'_>) -> JsResult<String> {
    v.to_string()
}

/// `DOMString?`: null/undefined -> None.
pub fn opt_dom_string(v: Value<'_>) -> JsResult<Option<String>> {
    if v.is_nullish() {
        Ok(None)
    } else {
        v.to_string().map(Some)
    }
}

/// Required string argument with a WebIDL-style arity error.
pub fn arg_string(a: &Args<'_>, i: usize, what: &str) -> JsResult<String> {
    a.require(i + 1, what)?;
    dom_string(a.get(i))
}

/// Runs `f` with string argument `i` (as [`arg_string`] converts it). An
/// ASCII string argument is borrowed from the JavaScript string (patched
/// JSC), with no copy: attribute names and the like arrive as short ASCII
/// literals, and the copy into a `String` cost more than the lookup they
/// are for.
pub fn with_arg_str<R>(a: &Args<'_>, i: usize, what: &str, f: impl FnOnce(&str) -> JsResult<R>) -> JsResult<R> {
    a.require(i + 1, what)?;
    with_str(a.get(i), f)
}

/// Runs `f` with `v` converted as [`dom_string`] does, borrowing the
/// characters of an ASCII string (see [`with_arg_str`]).
pub fn with_str<R>(v: Value<'_>, f: impl FnOnce(&str) -> JsResult<R>) -> JsResult<R> {
    f(&str_of(&v)?)
}

/// `v` converted as [`dom_string`] does: the characters of an ASCII string
/// borrowed (see [`with_arg_str`]), anything else converted.
pub fn str_of<'v>(v: &'v Value<'_>) -> JsResult<std::borrow::Cow<'v, str>> {
    if let Some(lp_js::StringUnits::Latin1(bytes)) = v.string_units() {
        if bytes.is_ascii() {
            // SAFETY: ASCII bytes are UTF-8.
            return Ok(std::borrow::Cow::Borrowed(unsafe { std::str::from_utf8_unchecked(bytes) }));
        }
    }
    dom_string(*v).map(std::borrow::Cow::Owned)
}

/// `f` on the parsed selector list of argument `i`, from the page's cache
/// (not copied out of it; `f` must not use the cache). A string argument is
/// looked up by its characters, without making a `String` (patched JSC);
/// anything else is converted as `arg_string` does.
pub fn with_selector<R>(
    a: &Args<'_>,
    i: usize,
    what: &str,
    page: &crate::page::PageDom,
    f: impl FnOnce(&lp_dom::select::Query) -> R,
) -> JsResult<R> {
    a.require(i + 1, what)?;
    let v = a.get(i);
    match v.string_units() {
        Some(lp_js::StringUnits::Latin1(chars)) => page.selectors.borrow_mut().with_latin1(chars, f).js(),
        Some(lp_js::StringUnits::Utf16(units)) => page.selectors.borrow_mut().with_utf16(units, f).js(),
        None => {
            // The conversion may run script, which may query selectors.
            let sel = dom_string(v)?;
            let q = page.selectors.borrow_mut().get(&sel).js()?;
            Ok(f(&q))
        }
    }
}

/// `[LegacyNullToEmptyString]` (innerHTML, textContent setter semantics).
pub fn null_to_empty(v: Value<'_>) -> JsResult<String> {
    if v.is_null() {
        Ok(String::new())
    } else {
        v.to_string()
    }
}

pub fn opt_node<'s>(s: &Scope<'s>, page: &crate::PageDom, id: Option<NodeId>) -> JsResult<Value<'s>> {
    match id {
        Some(id) => Ok(crate::node::wrap_node(s, page, id)?.value()),
        None => Ok(s.null()),
    }
}

/// ToNumber -> unsigned long (WebIDL modulo 2^32).
pub fn to_u32(v: Value<'_>) -> JsResult<u32> {
    let n = v.to_number()?;
    if !n.is_finite() {
        return Ok(0);
    }
    Ok((n.trunc() as i64).rem_euclid(1 << 32) as u32)
}

/// ToNumber -> long.
pub fn to_i32(v: Value<'_>) -> JsResult<i32> {
    Ok(to_u32(v)? as i32)
}

/// ToNumber -> a scroll offset in CSS px. CSSOM View takes an unrestricted
/// double; Chromium (device scale 1) rounds to the nearest pixel, half up,
/// treats non-finite values as 0 and clamps huge ones (no WebIDL wrapping).
pub fn to_scroll_offset(v: Value<'_>) -> JsResult<i32> {
    let n = v.to_number()?;
    if !n.is_finite() {
        return Ok(0);
    }
    Ok((n + 0.5).floor().clamp(i32::MIN as f64, i32::MAX as f64) as i32)
}
