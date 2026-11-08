//! Host classes: WebIDL-shaped interfaces backed by Rust data.
//!
//! Shape per realm: an interface object (constructor function) whose
//! `prototype` holds methods (real `Function`s), accessors (getter/setter
//! pairs, as `Object.getOwnPropertyDescriptor(Proto, name)` expects), constants
//! and `Symbol.toStringTag`; prototypes chain to the parent interface.
//! Instances are JSC callback objects of a per-class `JSClassRef` whose private
//! pointer is a `Boxed<C::Data>`; brand checks use the class id stored in the
//! header, not the (mutable) JS prototype chain.
//!
//! Native callbacks are monomorphized `extern "C"` trampolines generated from
//! zero-sized closures / fn items, so calls need no per-function allocation or
//! lookup. Panics are caught at the boundary and rethrown as JS errors.
//!
//! GC finalizers never run Rust payload destructors: they enqueue the payload,
//! and [`drain_finalized`] drops it at a safe point.

use crate::engine::EngineInner;
use crate::error::{throw_into, throw_message, JsError, JsResult};
use crate::realm::{IdentityKey, Scope};
use crate::string::{utf16_of, JsString};
use crate::value::{Args, Object, Value};
use lp_jsc_sys as sys;
use std::any::TypeId;
use std::ffi::CString;
use std::marker::PhantomData;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;
use std::rc::Rc;

pub type ClassId = u32;

/// Marker for classes without a parent interface.
pub enum NoParent {}

/// A host interface. Implement on a marker type; `Data` is the Rust state
/// stored in each instance (use interior mutability for mutation). A subclass
/// either uses its parent's `Data` type or declares [`ClassDef::upcast`] to
/// project its data to the parent's, so inherited members can borrow it.
pub trait JsClass: 'static {
    const NAME: &'static str;
    type Data: 'static;
    /// Parent interface, or [`NoParent`].
    type Parent: JsClass;
    /// Interfaces above this one (0 for a root): where the class sits in
    /// every subclass's brand display ([`ClassInfo::display`]). Not to be
    /// overridden.
    const DEPTH: u32 = <Self::Parent as JsClass>::DEPTH.wrapping_add(1);
    fn define(def: &mut ClassDef<Self>)
    where
        Self: Sized;
}

impl JsClass for NoParent {
    const NAME: &'static str = "";
    type Data = ();
    type Parent = NoParent;
    const DEPTH: u32 = u32::MAX;
    fn define(_: &mut ClassDef<Self>) {}
}

/// Raw `JSObjectCallAsFunctionCallback`.
type IndexedFn = unsafe fn(&Scope<'_>, *mut Header, u32) -> JsResult<Option<sys::JSValueRef>>;
/// Named hooks get the property name as JavaScript has it (UTF-16, lone
/// surrogates intact); the `&str` registration variants convert lossily.
type NamedFn = unsafe fn(&Scope<'_>, *mut Header, &[u16]) -> JsResult<Option<sys::JSValueRef>>;
type NamedSetFn = unsafe fn(&Scope<'_>, *mut Header, &[u16], sys::JSValueRef) -> JsResult<bool>;
type NamedDeleteFn = unsafe fn(&Scope<'_>, *mut Header, &[u16]) -> JsResult<bool>;
type NamesFn = unsafe fn(&Scope<'_>, *mut Header) -> JsResult<Vec<JsString>>;

pub(crate) struct MethodSpec {
    pub name: &'static str,
    pub length: u32,
    pub call: MemberCall,
    /// See [`ClassDef::pure_method`].
    pub pure: bool,
}

pub(crate) struct AccessorSpec {
    pub name: &'static str,
    pub get: MemberCall,
    pub set: Option<MemberCall>,
    /// The getter runs no script and has no observable side effect (see
    /// [`ClassDef::pure_getter`]): it takes the lean dispatch.
    pub pure: bool,
    /// What a pure getter returns, for the JIT (`GetterResult::Any` unless
    /// every result fits).
    pub result: GetterResult,
    /// A pure getter's invoker from the receiver's payload header.
    pub get_h: Option<InvokeHFn>,
    /// The setter's invoker from the receiver's payload header, for a
    /// setter of the class's own data (`ClassDef::accessor`).
    pub set_h: Option<SetHFn>,
    /// Custom accessor slot ([`crate::accessors`]); `None` installs an
    /// ordinary accessor (no patched JSC, or the slots are used up).
    pub slot: Option<u16>,
    /// Listed by `for..in` (WebIDL attributes are); see
    /// [`ClassDef::unenumerable_last_accessor`].
    pub enumerable: bool,
}

/// A member's Rust callback, type-erased, plus the invoker that knows its
/// real type. One invoker exists per class and member kind (not per member),
/// which keeps the number of monomorphized functions small: lp-webapi has
/// ~1,000 members, and one trampoline each made its codegen serial.
#[derive(Clone, Copy)]
pub(crate) struct MemberCall {
    pub invoke: InvokeFn,
    pub f: *const (),
}

/// Calls `f` (a member callback, cast back to its real fn-pointer type) for
/// `this` with `args`.
pub(crate) type InvokeFn = unsafe fn(&Scope<'_>, sys::JSObjectRef, Args<'_>, *const ()) -> JsResult<sys::JSValueRef>;
/// A getter invoked with its receiver (a wrapper instance) and the
/// receiver's payload header, in a scope of the receiver's realm.
pub(crate) type InvokeHFn =
    unsafe fn(&Scope<'_>, *mut Header, sys::JSObjectRef, *const ()) -> JsResult<sys::JSValueRef>;

/// A setter invoked with its receiver's payload header and the new value,
/// in a scope of the receiver's realm.
/// `Ok(false)`: the payload is not one of the setter's class (the full
/// dispatch then decides).
pub(crate) type SetHFn = unsafe fn(&Scope<'_>, *mut Header, sys::JSValueRef, *const ()) -> JsResult<bool>;

/// Member callback types: plain fn pointers. Non-capturing closures coerce.
pub type MethodFn<C> = for<'s> fn(&Scope<'s>, &<C as JsClass>::Data, Args<'s>) -> JsResult<Value<'s>>;
pub type GetterFn<C> = for<'s> fn(&Scope<'s>, &<C as JsClass>::Data) -> JsResult<Value<'s>>;
/// [`MethodFn`] that also gets the receiver: the wrapper whose data it is
/// (see [`ClassDef::method_this`]).
pub type MethodThisFn<C> = for<'s> fn(&Scope<'s>, &<C as JsClass>::Data, Object<'s>, Args<'s>) -> JsResult<Value<'s>>;
/// [`GetterFn`] that also gets the receiver (see [`ClassDef::getter_this`]).
pub type GetterThisFn<C> = for<'s> fn(&Scope<'s>, &<C as JsClass>::Data, Object<'s>) -> JsResult<Value<'s>>;
pub type SetterFn<C> = for<'s> fn(&Scope<'s>, &<C as JsClass>::Data, Value<'s>) -> JsResult<()>;
pub type StaticFn = for<'s> fn(&Scope<'s>, Args<'s>) -> JsResult<Value<'s>>;
pub type CtorFn = for<'s> fn(&Scope<'s>, Object<'s>, Args<'s>) -> JsResult<Object<'s>>;

#[derive(Default, Clone, Copy)]
pub(crate) struct PropertyHooks {
    pub indexed: Option<IndexedFn>,
    pub named: Option<NamedFn>,
    /// `[LegacyOverrideBuiltIns]`: named properties shadow prototype members.
    pub override_builtins: bool,
    pub named_set: Option<NamedSetFn>,
    /// The named setter runs for every string name, prototype members
    /// included (WebIDL `[[Set]]` on a true named-setter interface such as
    /// `Storage`). Off, it emulates per-name IDL attributes and stays out of
    /// the way of prototype accessors (`style.cssText = ..`).
    pub set_all_names: bool,
    pub named_delete: Option<NamedDeleteFn>,
    pub names: Option<NamesFn>,
    /// [`ClassDef::unenumerable_property_names`].
    pub unenumerable_names: Option<NamesFn>,
    /// [`ClassDef::legacy_platform_object`].
    pub legacy: bool,
}

impl PropertyHooks {
    fn any(&self) -> bool {
        self.indexed.is_some() || self.named.is_some() || self.named_set.is_some() || self.named_delete.is_some()
    }

    /// Own hooks override inherited ones per kind.
    fn inherit(self, parent: &PropertyHooks) -> PropertyHooks {
        let named_own = self.named.is_some();
        PropertyHooks {
            indexed: self.indexed.or(parent.indexed),
            named: self.named.or(parent.named),
            override_builtins: if named_own { self.override_builtins } else { parent.override_builtins },
            named_set: self.named_set.or(parent.named_set),
            set_all_names: if self.named_set.is_some() { self.set_all_names } else { parent.set_all_names },
            named_delete: self.named_delete.or(parent.named_delete),
            names: self.names.or(parent.names),
            unenumerable_names: self.unenumerable_names.or(parent.unenumerable_names),
            legacy: self.legacy || parent.legacy,
        }
    }
}

/// What a pure getter ([`ClassDef::pure_getter_typed`]) returns: a promise
/// the compiled code relies on, so a kind must cover every value the getter
/// can return (an exception is not a value).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum GetterResult {
    #[default]
    Any,
    /// A number (an integer or a double).
    Number,
    Boolean,
    String,
    /// An object, or null.
    ObjectOrNull,
}

/// Builder passed to [`JsClass::define`].
pub struct ClassDef<C: JsClass> {
    pub(crate) methods: Vec<MethodSpec>,
    pub(crate) statics: Vec<MethodSpec>,
    pub(crate) accessors: Vec<AccessorSpec>,
    pub(crate) constants: Vec<(&'static str, f64)>,
    pub(crate) constructor: Option<MemberCall>,
    pub(crate) ctor_length: u32,
    /// `[LegacyFactoryFunction]`s: global constructors of this class's
    /// instances (`Image`, `Option`, `Audio`).
    pub(crate) legacy_factories: Vec<MethodSpec>,
    /// A legacy callback interface (`NodeFilter`): not a constructor, no
    /// `prototype` property.
    pub(crate) callback_interface: bool,
    /// See [`ClassDef::named_properties_are_announced`].
    pub(crate) announced_names: bool,
    /// See [`ClassDef::attributes_without_jit_getters`].
    pub(crate) jit_getters: bool,
    pub(crate) hooks: PropertyHooks,
    pub(crate) exposed: bool,
    pub(crate) indexed_iterable: bool,
    pub(crate) indexed_iterator: bool,
    pub(crate) upcast: Option<UpcastFn>,
    pub(crate) global_receiver: Option<GlobalReceiverFn>,
    _c: PhantomData<C>,
}

/// The identifiers of a `[PutForwards]` setter (see [`ClassDef::put_forwards`]).
struct PutForwards {
    name: &'static str,
    target: &'static str,
}

impl PutForwards {
    /// The program-lifetime pair for `name`/`target`, which a setter's
    /// [`MemberCall`] points at. Interned: classes are defined once per
    /// engine (one per browser thread), so leaking a pair per definition
    /// would grow with every engine created.
    fn interned(name: &'static str, target: &'static str) -> &'static PutForwards {
        static INTERNED: std::sync::Mutex<Vec<&'static PutForwards>> = std::sync::Mutex::new(Vec::new());
        let mut interned = INTERNED.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(found) = interned.iter().copied().find(|p| p.name == name && p.target == target) {
            return found;
        }
        let pair: &'static PutForwards = Box::leak(Box::new(PutForwards { name, target }));
        interned.push(pair);
        pair
    }
}

/// Resolves the data an operation uses when called with an undefined, null or
/// global receiver (see [`ClassDef::global_receiver`]).
pub(crate) type GlobalReceiverFn = unsafe fn(&Scope<'_>) -> Option<*const u8>;

/// Projects a pointer to a class's data to a pointer to its parent's data.
pub(crate) type UpcastFn = unsafe fn(*const u8) -> *const u8;

/// Produces the value of a zero-sized callable from nothing.
///
/// # Safety
/// `F` must be zero-sized and have no validity invariants (closures and fn
/// items); enforced by the `assert` at registration time and here.
unsafe fn conjure<F: Copy>() -> F {
    assert!(std::mem::size_of::<F>() == 0, "callback must be a non-capturing closure or fn item");
    // SAFETY: zero-sized type, no bytes to initialize.
    // Zero-sized reads are valid from any aligned non-null pointer.
    unsafe { std::ptr::NonNull::<F>::dangling().as_ptr().read() }
}

fn assert_zst<F>() {
    assert!(
        std::mem::size_of::<F>() == 0,
        "host class callbacks must be non-capturing closures or fn items (use the Data type for state)"
    );
}

impl<C: JsClass> ClassDef<C> {
    fn new() -> Self {
        ClassDef {
            methods: Vec::new(),
            statics: Vec::new(),
            accessors: Vec::new(),
            constants: Vec::new(),
            constructor: None,
            ctor_length: 0,
            legacy_factories: Vec::new(),
            callback_interface: false,
            announced_names: false,
            jit_getters: true,
            hooks: PropertyHooks::default(),
            exposed: true,
            indexed_iterable: false,
            indexed_iterator: false,
            upcast: None,
            global_receiver: None,
            _c: PhantomData,
        }
    }

    /// Adds an operation on the prototype.
    pub fn method(&mut self, name: &'static str, length: u32, f: MethodFn<C>) -> &mut Self {
        self.methods.push(MethodSpec {
            name,
            length,
            call: MemberCall { invoke: method_invoke::<C>, f: f as *const () },
            pure: false,
        });
        self
    }

    /// Adds an operation that runs no JavaScript, changes nothing script can
    /// observe and may be run again after an error (`matches`,
    /// `getAttribute`, `querySelector`): it takes the lean dispatch, as a
    /// [`ClassDef::pure_getter`] does.
    pub fn pure_method(&mut self, name: &'static str, length: u32, f: MethodFn<C>) -> &mut Self {
        self.methods.push(MethodSpec {
            name,
            length,
            call: MemberCall { invoke: method_invoke::<C>, f: f as *const () },
            pure: true,
        });
        self
    }

    /// Adds a read-only attribute.
    pub fn getter(&mut self, name: &'static str, g: GetterFn<C>) -> &mut Self {
        let get = MemberCall { invoke: getter_invoke::<C>, f: g as *const () };
        self.accessors.push(AccessorSpec {
            name,
            get,
            set: None,
            slot: None,
            pure: false,
            result: GetterResult::Any,
            get_h: None,
            set_h: None,
            enumerable: true,
        });
        self
    }

    /// Adds a read-only attribute whose getter runs no JavaScript, changes
    /// nothing script can observe, and may be run again after an error (the
    /// usual tree-navigation and state reads). Such a getter skips the
    /// per-callback bookkeeping (nesting depth, termination state, the
    /// post-callback hook) that exists for callbacks that can run script or
    /// mutate: a selector engine reads these tens of thousands of times per
    /// query. An error or a panic reruns it under the full dispatch, which
    /// then reports it.
    pub fn pure_getter(&mut self, name: &'static str, g: GetterFn<C>) -> &mut Self {
        self.pure_getter_typed(name, GetterResult::Any, g)
    }

    /// [`ClassDef::pure_getter`] with what the getter returns, which lets the
    /// JIT type what follows the read (see [`GetterResult`]).
    pub fn pure_getter_typed(&mut self, name: &'static str, result: GetterResult, g: GetterFn<C>) -> &mut Self {
        let get = MemberCall { invoke: getter_invoke::<C>, f: g as *const () };
        self.accessors.push(AccessorSpec {
            name,
            get,
            set: None,
            slot: None,
            pure: true,
            result,
            get_h: Some(getter_invoke_h::<C>),
            set_h: None,
            enumerable: true,
        });
        self
    }

    /// [`ClassDef::method`] whose callback also gets the receiver: the
    /// wrapper whose data it is (in the receiver's realm), so a member that
    /// needs its own wrapper (hidden properties, events it fires at itself)
    /// does not look it up again. A class with a
    /// [`ClassDef::global_receiver`] that resolved an `undefined`/`null`/global
    /// receiver gets the global object.
    pub fn method_this(&mut self, name: &'static str, length: u32, f: MethodThisFn<C>) -> &mut Self {
        self.methods.push(MethodSpec {
            name,
            length,
            call: MemberCall { invoke: method_this_invoke::<C>, f: f as *const () },
            pure: false,
        });
        self
    }

    /// [`ClassDef::pure_method`] with the receiver (see [`ClassDef::method_this`]).
    pub fn pure_method_this(&mut self, name: &'static str, length: u32, f: MethodThisFn<C>) -> &mut Self {
        self.methods.push(MethodSpec {
            name,
            length,
            call: MemberCall { invoke: method_this_invoke::<C>, f: f as *const () },
            pure: true,
        });
        self
    }

    /// [`ClassDef::getter`] with the receiver (see [`ClassDef::method_this`]).
    pub fn getter_this(&mut self, name: &'static str, g: GetterThisFn<C>) -> &mut Self {
        let get = MemberCall { invoke: getter_this_invoke::<C>, f: g as *const () };
        self.accessors.push(AccessorSpec {
            name,
            get,
            set: None,
            slot: None,
            pure: false,
            result: GetterResult::Any,
            get_h: None,
            set_h: None,
            enumerable: true,
        });
        self
    }

    /// [`ClassDef::pure_getter`] with the receiver (see [`ClassDef::method_this`]).
    pub fn pure_getter_this(&mut self, name: &'static str, g: GetterThisFn<C>) -> &mut Self {
        self.pure_getter_this_typed(name, GetterResult::Any, g)
    }

    /// [`ClassDef::pure_getter_typed`] with the receiver (see [`ClassDef::method_this`]).
    pub fn pure_getter_this_typed(
        &mut self,
        name: &'static str,
        result: GetterResult,
        g: GetterThisFn<C>,
    ) -> &mut Self {
        let get = MemberCall { invoke: getter_this_invoke::<C>, f: g as *const () };
        self.accessors.push(AccessorSpec {
            name,
            get,
            set: None,
            slot: None,
            pure: true,
            result,
            get_h: Some(getter_this_invoke_h::<C>),
            set_h: None,
            enumerable: true,
        });
        self
    }

    /// Adds a read-write attribute.
    pub fn accessor(&mut self, name: &'static str, g: GetterFn<C>, s: SetterFn<C>) -> &mut Self {
        let get = MemberCall { invoke: getter_invoke::<C>, f: g as *const () };
        let set = MemberCall { invoke: setter_invoke::<C>, f: s as *const () };
        self.accessors.push(AccessorSpec {
            name,
            get,
            set: Some(set),
            slot: None,
            pure: false,
            result: GetterResult::Any,
            get_h: None,
            set_h: Some(setter_invoke_h::<C>),
            enumerable: true,
        });
        self
    }

    /// [`ClassDef::accessor`] for a global's value that browsers keep in a
    /// data property (an interface object made on first use): it reads and
    /// writes the current realm's global whatever the receiver, as a data
    /// property's value is had regardless of `this`. Code that inspects the
    /// property (`Object.getOwnPropertyDescriptor(self, 'X').get()`, which
    /// calls the getter on the descriptor) then sees the value, not a
    /// TypeError.
    pub fn global_value_accessor(&mut self, name: &'static str, g: GetterFn<C>, s: SetterFn<C>) -> &mut Self {
        let get = MemberCall { invoke: global_value_getter_invoke::<C>, f: g as *const () };
        let set = MemberCall { invoke: global_value_setter_invoke::<C>, f: s as *const () };
        self.accessors.push(AccessorSpec {
            name,
            get,
            set: Some(set),
            slot: None,
            pure: false,
            result: GetterResult::Any,
            get_h: None,
            set_h: Some(setter_invoke_h::<C>),
            enumerable: true,
        });
        self
    }

    /// [`ClassDef::accessor`] whose getter is pure (see
    /// [`ClassDef::pure_getter_typed`]): reads take the lean dispatch, writes
    /// the full one.
    pub fn pure_accessor_typed(
        &mut self,
        name: &'static str,
        result: GetterResult,
        g: GetterFn<C>,
        s: SetterFn<C>,
    ) -> &mut Self {
        let get = MemberCall { invoke: getter_invoke::<C>, f: g as *const () };
        let set = MemberCall { invoke: setter_invoke::<C>, f: s as *const () };
        self.accessors.push(AccessorSpec {
            name,
            get,
            set: Some(set),
            slot: None,
            pure: true,
            result,
            get_h: Some(getter_invoke_h::<C>),
            set_h: Some(setter_invoke_h::<C>),
            enumerable: true,
        });
        self
    }

    /// Leaves the attribute added last out of `for..in` and `Object.keys` of
    /// the prototype (the property is still found by name): for aliases a
    /// browser answers without listing them, such as the dashed and
    /// `Webkit`-cased names of a CSS property in `CSSStyleDeclaration`.
    pub fn unenumerable_last_accessor(&mut self) -> &mut Self {
        if let Some(a) = self.accessors.last_mut() {
            a.enumerable = false;
        }
        self
    }

    /// Adds a read-write attribute with `[PutForwards=target]`: assignment
    /// brand-checks the receiver, reads the attribute (an observable `Get`,
    /// so an overridden getter runs and its exception propagates; a
    /// non-object result is a TypeError) and performs `[[Set]]` of `target`
    /// on the result (https://webidl.spec.whatwg.org/#PutForwards).
    pub fn put_forwards(&mut self, name: &'static str, g: GetterFn<C>, target: &'static str) -> &mut Self {
        let get = MemberCall { invoke: getter_invoke::<C>, f: g as *const () };
        let forwards = PutForwards::interned(name, target);
        let set = MemberCall { invoke: put_forwards_invoke::<C>, f: forwards as *const PutForwards as *const () };
        self.accessors.push(AccessorSpec {
            name,
            get,
            set: Some(set),
            slot: None,
            pure: false,
            result: GetterResult::Any,
            get_h: None,
            set_h: None,
            enumerable: true,
        });
        self
    }

    /// Adds a `[LegacyFactoryFunction]`: a global constructor `name` (such as
    /// `Image` for `HTMLImageElement`) whose `prototype` is this interface's
    /// prototype object. The callback is a [`ClassDef::constructor`] one and
    /// receives `new.target`, so `class X extends Image {}` works.
    pub fn legacy_factory(&mut self, name: &'static str, length: u32, f: CtorFn) -> &mut Self {
        self.legacy_factories.push(MethodSpec {
            name,
            length,
            call: MemberCall { invoke: ctor_invoke, f: f as *const () },
            pure: false,
        });
        self
    }

    /// Makes this a legacy callback interface (`NodeFilter`): the interface
    /// object is a plain function (no `[[Construct]]`, no `prototype`
    /// property) carrying the constants; `x instanceof NodeFilter` throws.
    /// The named getter answers only names the embedder announced with
    /// [`crate::ImpureNames`] before they can first be answered, and never a name of
    /// the prototype chain (`named(false, ..)`). The wrappers' property lookups
    /// then stay cacheable by the JITs (patched JavaScriptCore), instead of
    /// running the hook on every read, as WebCore does for HTMLDocument.
    pub fn named_properties_are_announced(&mut self) -> &mut Self {
        self.announced_names = true;
        self
    }

    /// Installs this class's pure attributes as plain custom accessors, not
    /// as DOM attributes the JITs call directly (`JSObjectSetFastGetter`).
    /// Reads stay inline-cached and dispatch the same way; compiled code
    /// calls the getter through the custom accessor instead. For a class
    /// with hundreds of attributes whose getters cost far more than the call
    /// (`CSSStyleDeclaration` of `getComputedStyle`): JavaScriptCore interns
    /// each fast getter by a linear search of every one the process made, so
    /// defining hundreds of them on each realm's first use cost milliseconds.
    pub fn attributes_without_jit_getters(&mut self) -> &mut Self {
        self.jit_getters = false;
        self
    }

    pub fn legacy_callback_interface(&mut self) -> &mut Self {
        self.callback_interface = true;
        self
    }

    /// Adds a constant on both the interface object and the prototype.
    pub fn constant(&mut self, name: &'static str, value: f64) -> &mut Self {
        self.constants.push((name, value));
        self
    }

    /// Adds a static operation on the interface object.
    pub fn static_method(&mut self, name: &'static str, length: u32, f: StaticFn) -> &mut Self {
        self.statics.push(MethodSpec {
            name,
            length,
            call: MemberCall { invoke: static_invoke, f: f as *const () },
            pure: false,
        });
        self
    }

    /// Makes the interface constructible. The callback receives `new.target`
    /// (for subclassing, e.g. custom elements) and returns the new object,
    /// typically via [`Scope::wrap_new`]. Without this, `new X()` throws
    /// "Illegal constructor".
    pub fn constructor(&mut self, length: u32, f: CtorFn) -> &mut Self {
        self.constructor = Some(MemberCall { invoke: ctor_invoke, f: f as *const () });
        self.ctor_length = length;
        self
    }

    /// Indexed property getter (`obj[0]`); return `None` for absent indices.
    pub fn indexed<F>(&mut self, _f: F) -> &mut Self
    where
        F: for<'s> Fn(&Scope<'s>, &C::Data, u32) -> JsResult<Option<Value<'s>>> + Copy + 'static,
    {
        assert_zst::<F>();
        self.hooks.indexed = Some(indexed_erased::<C, F>);
        self
    }

    /// Named property getter (`collection.someId`, `storage.key`). Unless
    /// `override_builtins`, prototype members win over named properties.
    pub fn named<F>(&mut self, override_builtins: bool, _f: F) -> &mut Self
    where
        F: for<'s> Fn(&Scope<'s>, &C::Data, &str) -> JsResult<Option<Value<'s>>> + Copy + 'static,
    {
        assert_zst::<F>();
        self.hooks.named = Some(named_erased::<C, F>);
        self.hooks.override_builtins = override_builtins;
        self
    }

    /// [`ClassDef::named`] with the name in UTF-16 code units, for classes
    /// whose names may hold lone surrogates (Web Storage keys).
    pub fn named_utf16<F>(&mut self, override_builtins: bool, _f: F) -> &mut Self
    where
        F: for<'s> Fn(&Scope<'s>, &C::Data, &[u16]) -> JsResult<Option<Value<'s>>> + Copy + 'static,
    {
        assert_zst::<F>();
        self.hooks.named = Some(named_utf16_erased::<C, F>);
        self.hooks.override_builtins = override_builtins;
        self
    }

    /// Named property setter; return `false` to fall back to an ordinary own
    /// property. Per WebIDL `[[Set]]` it runs for every string name on the
    /// receiver itself, prototype members included (`storage.length = 1`
    /// stores an item), so a setter that only supports some names must return
    /// `false` for the others.
    pub fn named_setter<F>(&mut self, _f: F) -> &mut Self
    where
        F: for<'s> Fn(&Scope<'s>, &C::Data, &str, Value<'s>) -> JsResult<bool> + Copy + 'static,
    {
        assert_zst::<F>();
        self.hooks.named_set = Some(named_set_erased::<C, F>);
        self
    }

    /// [`ClassDef::named_setter`] with the name in UTF-16 code units.
    pub fn named_setter_utf16<F>(&mut self, _f: F) -> &mut Self
    where
        F: for<'s> Fn(&Scope<'s>, &C::Data, &[u16], Value<'s>) -> JsResult<bool> + Copy + 'static,
    {
        assert_zst::<F>();
        self.hooks.named_set = Some(named_set_utf16_erased::<C, F>);
        self
    }

    /// Runs the named setter for every string name, prototype members
    /// included, as WebIDL `[[Set]]` does on an interface with a named
    /// property setter (`storage.length = 1` stores an item). Without it the
    /// setter emulates per-name IDL attributes and prototype members win.
    pub fn named_setter_for_all_names(&mut self) -> &mut Self {
        self.hooks.set_all_names = true;
        self
    }

    /// Named property deleter; return `false` when the name is not supported,
    /// so the ordinary `[[Delete]]` runs (own properties such as symbol-keyed
    /// expandos keep their configurability). Not consulted for names visible
    /// on the prototype chain unless `[LegacyOverrideBuiltIns]`.
    pub fn named_deleter<F>(&mut self, _f: F) -> &mut Self
    where
        F: for<'s> Fn(&Scope<'s>, &C::Data, &str) -> JsResult<bool> + Copy + 'static,
    {
        assert_zst::<F>();
        self.hooks.named_delete = Some(named_delete_erased::<C, F>);
        self
    }

    /// [`ClassDef::named_deleter`] with the name in UTF-16 code units.
    pub fn named_deleter_utf16<F>(&mut self, _f: F) -> &mut Self
    where
        F: for<'s> Fn(&Scope<'s>, &C::Data, &[u16]) -> JsResult<bool> + Copy + 'static,
    {
        assert_zst::<F>();
        self.hooks.named_delete = Some(named_delete_utf16_erased::<C, F>);
        self
    }

    /// Supported property names for enumeration (`Object.keys`, `for..in`).
    pub fn property_names<F>(&mut self, _f: F) -> &mut Self
    where
        F: for<'s> Fn(&Scope<'s>, &C::Data) -> JsResult<Vec<String>> + Copy + 'static,
    {
        assert_zst::<F>();
        self.hooks.names = Some(names_erased::<C, F>);
        self
    }

    /// Supported property names of a `[LegacyUnenumerableNamedProperties]`
    /// interface (`HTMLCollection`, `NamedNodeMap`): own property keys that
    /// `Object.getOwnPropertyNames` and `Reflect.ownKeys` list after the
    /// indices, and `Object.keys` and `for..in` leave out. Names that are not
    /// visible (an array index on a class with indexed properties, a name the
    /// prototype chain has) are dropped by the caller.
    pub fn unenumerable_property_names<F>(&mut self, _f: F) -> &mut Self
    where
        F: for<'s> Fn(&Scope<'s>, &C::Data) -> JsResult<Vec<String>> + Copy + 'static,
    {
        assert_zst::<F>();
        self.hooks.unenumerable_names = Some(names_erased::<C, F>);
        self
    }

    /// [`ClassDef::property_names`] with the names in UTF-16 code units.
    pub fn property_names_utf16<F>(&mut self, _f: F) -> &mut Self
    where
        F: for<'s> Fn(&Scope<'s>, &C::Data) -> JsResult<Vec<Vec<u16>>> + Copy + 'static,
    {
        assert_zst::<F>();
        self.hooks.names = Some(names_utf16_erased::<C, F>);
        self
    }

    /// Value iterator over an indexed object with `length` (WebIDL
    /// `iterable<V>` on indexed properties): installs `Symbol.iterator`,
    /// `entries`, `keys`, `values` and `forEach` from `Array.prototype`.
    pub fn indexed_iterable(&mut self) -> &mut Self {
        self.indexed_iterable = true;
        self
    }

    /// `Symbol.iterator` alone (`Array.prototype.values`): what WebIDL gives
    /// an interface that supports indexed properties and has `length` but no
    /// `iterable<V>` declaration (`HTMLCollection`, `NamedNodeMap`), which
    /// therefore has no `entries`, `keys`, `values` or `forEach`.
    pub fn indexed_iterator(&mut self) -> &mut Self {
        self.indexed_iterator = true;
        self
    }

    /// WebIDL legacy platform object rules for the indexed properties and the
    /// setter-less named properties of this class (`HTMLCollection`,
    /// `NodeList`, `NamedNodeMap`), over the default "prototype members first,
    /// then the getter":
    /// - an own property of the wrapper (an expando script set while the name
    ///   was not supported) shadows the named property of that name (named
    ///   property visibility algorithm);
    /// - assigning to an array index or to a visible named property is
    ///   refused like a write to a read-only property: a `TypeError` in
    ///   strict mode code, ignored in sloppy mode code, and no own property is
    ///   created.
    pub fn legacy_platform_object(&mut self) -> &mut Self {
        self.hooks.legacy = true;
        self
    }

    /// Resolves the instance used when an operation or attribute of this class
    /// is invoked with an `undefined`/`null`/global receiver, e.g.
    /// `(cond ? history.pushState : history.replaceState)(...)`, which
    /// browsers accept and real libraries (TanStack History) rely on. `F`
    /// returns the realm's singleton (typically from the global's data).
    pub fn global_receiver<F>(&mut self, _f: F) -> &mut Self
    where
        F: for<'s> Fn(&Scope<'s>) -> Option<&'s C::Data> + Copy + 'static,
    {
        assert_zst::<F>();
        self.global_receiver = Some(global_receiver_erased::<C, F>);
        self
    }

    /// Declares how to borrow the parent interface's data from this class's
    /// data (required when `Data` differs from the parent's `Data`).
    pub fn upcast<F>(&mut self, _f: F) -> &mut Self
    where
        F: Fn(&C::Data) -> &<C::Parent as JsClass>::Data + Copy + 'static,
    {
        assert_zst::<F>();
        self.upcast = Some(upcast_erased::<C, F>);
        self
    }

    /// Do not expose the interface object on the global (`[LegacyNoInterfaceObject]`-like).
    pub fn not_exposed(&mut self) -> &mut Self {
        self.exposed = false;
        self
    }
}

pub(crate) struct ClassInfo {
    pub upcast: Option<UpcastFn>,
    pub global_receiver: Option<GlobalReceiverFn>,
    pub name: &'static str,
    pub data_type: TypeId,
    /// The `JsClass` type: the brand a member checks its receiver against.
    pub class_type: TypeId,
    pub parent: Option<ClassId>,
    /// The parent's entry (null at the root). Entries are boxed, so the
    /// pointer stays valid while the registry lives: payload headers keep
    /// one, and a member finds its receiver's data by walking these without
    /// touching the registry.
    pub parent_info: *const ClassInfo,
    /// The brand display: entry `d` is this class's ancestor at depth `d`
    /// (`JsClass::DEPTH`), the last one the class itself. "Is a `C`" is one
    /// compare at a constant index, whatever the depth (a range over
    /// registration-order ids cannot express it: classes register in use
    /// order, per engine, and a member knows its class only as a type).
    pub display: Box<[Brand]>,
    pub instance_class: sys::JSClassRef,
    pub def: ClassDefErased,
    /// The engine's [`HookState`] (it outlives the registry's entries): an
    /// indexed read finds it from the receiver's payload, with no
    /// thread-local lookup.
    pub hook_state: *const HookState,
}

/// An entry of [`ClassInfo::display`].
#[derive(Clone, Copy)]
pub(crate) struct Brand {
    /// The ancestor's `JsClass` type.
    pub class_type: TypeId,
    /// No `upcast` between the class and this ancestor: the ancestor's data
    /// is the payload's own data, at the payload's fixed data offset.
    pub direct: bool,
}

/// `ClassDef` without the marker type parameter.
pub(crate) struct ClassDefErased {
    pub methods: Vec<MethodSpec>,
    pub statics: Vec<MethodSpec>,
    pub accessors: Vec<AccessorSpec>,
    pub constants: Vec<(&'static str, f64)>,
    pub constructor: Option<MemberCall>,
    pub ctor_length: u32,
    pub legacy_factories: Vec<MethodSpec>,
    pub announced_names: bool,
    pub jit_getters: bool,
    pub hooks: PropertyHooks,
    pub exposed: bool,
    pub indexed_iterable: bool,
    pub indexed_iterator: bool,
}

pub(crate) struct ClassRegistry {
    pub classes: Vec<Box<ClassInfo>>,
    by_type: rustc_hash::FxHashMap<TypeId, ClassId>,
    /// Placeholder properties awaiting removal ([`leave_placeholder`]): the
    /// protected wrapper and the property name.
    placeholders: Vec<(sys::JSObjectRef, JsString)>,
    /// Parent JSClass of every wrapper class: carries the finalizer and marks
    /// objects whose private pointer is a `Header`.
    pub base: sys::JSClassRef,
    /// Callable objects backed by Rust closures.
    pub closure: sys::JSClassRef,
    /// Interface member functions (methods, accessors, statics, constructors):
    /// callable objects whose private data is a `MemberCall`. Not a `base`
    /// subclass: their private pointer is not a `Header`.
    pub member: sys::JSClassRef,
    /// `classes` as the table `installInterfaces` takes (helpers.js),
    /// appended to as each registers, so every realm creation passes it as
    /// is: the names, newline-separated (no class name contains one), and
    /// per class `[parent or -1, constructor length, flags]` with
    /// [`SPEC_EXPOSED`] and [`SPEC_CALLBACK`] the flags. Typed data costs a
    /// realm one string and one Int32Array; the JSON text it replaced was
    /// parsed by every realm (6% of its creation).
    spec_names: String,
    spec_data: Vec<i32>,
}

/// `installInterfaces` flags of a class: an exposed one (a property of the
/// global); a callback interface (not a constructor, no `prototype`).
pub(crate) const SPEC_EXPOSED: i32 = 1;
pub(crate) const SPEC_CALLBACK: i32 = 2;
/// Entries of `ClassRegistry::spec_data` per class.
pub(crate) const SPEC_STRIDE: usize = 3;

impl ClassRegistry {
    pub fn new() -> ClassRegistry {
        let base_name = c"LightpandaWrapper";
        let mut def = sys::JSClassDefinition::EMPTY;
        def.className = base_name.as_ptr();
        def.attributes = sys::kJSClassAttributeNoAutomaticPrototype;
        def.finalize = Some(finalize_wrapper);
        // SAFETY: static class name, valid definition.
        let base = unsafe { sys::JSClassCreate(&def) };
        let mut cdef = sys::JSClassDefinition::EMPTY;
        cdef.className = c"Function".as_ptr();
        cdef.attributes = sys::kJSClassAttributeNoAutomaticPrototype;
        cdef.parentClass = base;
        cdef.callAsFunction = Some(crate::realm::closure_call);
        let closure = unsafe { sys::JSClassCreate(&cdef) };
        let mut mdef = sys::JSClassDefinition::EMPTY;
        mdef.className = c"Function".as_ptr();
        mdef.attributes = sys::kJSClassAttributeNoAutomaticPrototype;
        mdef.callAsFunction = Some(member_call);
        mdef.finalize = Some(member_finalize);
        let member = unsafe { sys::JSClassCreate(&mdef) };
        ClassRegistry {
            classes: Vec::new(),
            by_type: rustc_hash::FxHashMap::default(),
            placeholders: Vec::new(),
            base,
            closure,
            member,
            spec_names: String::new(),
            spec_data: Vec::new(),
        }
    }

    /// See [`ClassRegistry::spec_names`]: (names, data).
    pub fn spec(&self) -> (&str, &[i32]) {
        (&self.spec_names, &self.spec_data)
    }

    fn push_spec(&mut self, name: &str, parent: Option<ClassId>, ctor_length: u32, exposed: bool, callback: bool) {
        debug_assert!(!name.contains('\n'), "class name {name:?} contains the spec separator");
        if !self.classes.is_empty() {
            self.spec_names.push('\n');
        }
        self.spec_names.push_str(name);
        let flags = if exposed { SPEC_EXPOSED } else { 0 } | if callback { SPEC_CALLBACK } else { 0 };
        self.spec_data.extend([parent.map_or(-1, |p| p as i32), ctor_length as i32, flags]);
        debug_assert_eq!(self.spec_data.len(), (self.classes.len() + 1) * SPEC_STRIDE);
    }

    pub fn id_of<C: JsClass>(&self) -> Option<ClassId> {
        self.id_of_type(TypeId::of::<C>())
    }

    pub fn id_of_type(&self, class: TypeId) -> Option<ClassId> {
        self.by_type.get(&class).copied()
    }

    pub fn release_all(&mut self) {
        unsafe {
            for c in &self.classes {
                sys::JSClassRelease(c.instance_class);
            }
            sys::JSClassRelease(self.closure);
            sys::JSClassRelease(self.member);
            sys::JSClassRelease(self.base);
        }
        self.classes.clear();
        self.spec_names.clear();
        self.spec_data.clear();
        // The VM is gone, and the placeholder properties with it (the
        // engine resets its `HookState::placeholders`).
        self.placeholders.clear();
    }
}

pub(crate) fn register<C: JsClass>(engine: &EngineInner) -> ClassId {
    if let Some(id) = engine.classes.borrow().id_of::<C>() {
        return id;
    }
    let parent = if TypeId::of::<C::Parent>() == TypeId::of::<NoParent>() {
        None
    } else {
        let pid = register::<C::Parent>(engine);
        let reg = engine.classes.borrow();
        let same = reg.classes[pid as usize].data_type == TypeId::of::<C::Data>();
        drop(reg);
        Some((pid, same))
    };
    let mut def = ClassDef::<C>::new();
    C::define(&mut def);
    let parent = match parent {
        Some((pid, same)) => {
            assert!(
                same || def.upcast.is_some(),
                "{}: Data differs from parent {} Data; declare ClassDef::upcast",
                C::NAME,
                C::Parent::NAME
            );
            Some(pid)
        }
        None => None,
    };
    let mut reg = engine.classes.borrow_mut();
    let id = reg.classes.len() as ClassId;
    let cname = CString::new(C::NAME).expect("class name without NUL");
    if let Some(p) = parent {
        let inherited = reg.classes[p as usize].def.hooks;
        // A subclass answers the same names unless it adds a named hook of its own.
        let own_named = def.hooks.named.is_some_and(|own| !inherited.named.is_some_and(|p| std::ptr::fn_addr_eq(own, p)));
        def.hooks = def.hooks.inherit(&inherited);
        if reg.classes[p as usize].def.announced_names && !own_named {
            def.announced_names = true;
        }
    }
    let has_hooks = def.hooks.any();
    let has_named_hooks =
        def.hooks.named.is_some() || def.hooks.named_set.is_some() || def.hooks.named_delete.is_some();
    // The patched JSC takes indexed properties through their own callbacks,
    // which leaves a class with only those (NodeList) cacheable: its named
    // lookups never run a hook. Without it, the named hooks parse index names.
    let indexed_callbacks = sys::spi::get().class_set_indexed_callbacks.filter(|_| def.hooks.indexed.is_some());
    let mut jsdef = sys::JSClassDefinition::EMPTY;
    jsdef.className = cname.as_ptr();
    jsdef.attributes = sys::kJSClassAttributeNoAutomaticPrototype;
    if def.announced_names && sys::spi::get().add_impure_property.is_some() {
        debug_assert!(!def.hooks.override_builtins && def.hooks.indexed.is_none(), "announced names need named(false)");
        jsdef.attributes |= sys::kJSClassAttributeImpureNamedProperties;
    }
    jsdef.parentClass = reg.base;
    if has_named_hooks || (def.hooks.indexed.is_some() && indexed_callbacks.is_none()) {
        jsdef.getProperty = Some(get_property_hook);
        jsdef.setProperty = Some(set_property_hook);
        jsdef.deleteProperty = Some(delete_property_hook);
    }
    if has_hooks {
        jsdef.getPropertyNames = Some(property_names_hook);
    }
    // SAFETY: valid definition; JSClassCreate copies the class name.
    let instance_class = unsafe { sys::JSClassCreate(&jsdef) };
    if let Some(set) = indexed_callbacks {
        // SAFETY: the class was just created and has no objects yet.
        unsafe { set(instance_class, Some(get_by_index_hook), Some(set_by_index_hook)) };
    }
    // Last first: a class that defines an attribute twice (shared members,
    // then its own) gets the later definition in the slot both share, as the
    // prototype property it installs last.
    for a in def.accessors.iter_mut().rev() {
        a.slot =
            crate::accessors::slot_for(TypeId::of::<C>(), a.name, a.get, a.set, a.pure, a.result, a.get_h, a.set_h);
    }
    let parent_info: *const ClassInfo = match parent {
        Some(p) => &*reg.classes[p as usize],
        None => ptr::null(),
    };
    let own_upcast = def.upcast.is_some();
    let mut display: Vec<Brand> = match parent {
        Some(p) => reg.classes[p as usize]
            .display
            .iter()
            .map(|b| Brand { class_type: b.class_type, direct: b.direct && !own_upcast })
            .collect(),
        None => Vec::new(),
    };
    display.push(Brand { class_type: TypeId::of::<C>(), direct: true });
    debug_assert_eq!(display.len() - 1, C::DEPTH as usize, "{}: depth from the registry", C::NAME);
    reg.push_spec(C::NAME, parent, def.ctor_length, def.exposed, def.callback_interface);
    reg.classes.push(Box::new(ClassInfo {
        upcast: def.upcast,
        global_receiver: def.global_receiver,
        name: C::NAME,
        data_type: TypeId::of::<C::Data>(),
        class_type: TypeId::of::<C>(),
        parent,
        parent_info,
        display: display.into_boxed_slice(),
        instance_class,
        hook_state: &engine.hook_state,
        def: ClassDefErased {
            methods: def.methods,
            statics: def.statics,
            accessors: def.accessors,
            constants: def.constants,
            constructor: def.constructor,
            ctor_length: def.ctor_length,
            legacy_factories: def.legacy_factories,
            announced_names: def.announced_names,
            jit_getters: def.jit_getters,
            hooks: def.hooks,
            exposed: def.exposed,
            indexed_iterable: def.indexed_iterable,
            indexed_iterator: def.indexed_iterator,
        },
    }));
    reg.by_type.insert(TypeId::of::<C>(), id);
    id
}

// ---------------------------------------------------------------------------
// Wrapper payloads

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    Instance,
    Closure,
}

/// First word of every object private payload in the VM: wrapper payloads
/// ([`Header`]) and member functions ([`MemberPayload`]). Lets
/// `Scope::header_of_object` tell a wrapper from a member function by reading
/// its private pointer, without the class check (which takes the JSC lock).
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u64)]
pub(crate) enum PayloadTag {
    Wrapper = 0x4c50_5752_4150_5052, // "LPWRAPPR"
    Member = 0x4c50_4d45_4d42_4552,  // "LPMEMBER"
}

/// Private payload of a member function object.
#[repr(C)]
pub(crate) struct MemberPayload {
    pub tag: PayloadTag,
    pub call: MemberCall,
    /// See [`ClassDef::pure_method`].
    pub pure: bool,
    /// The realm the function was made in, whose context JSC passes to its
    /// calls: the full dispatch's candidate realm. `None` once that realm is
    /// disposed (`RealmInner::member_fns` lists the payloads to clear, so the
    /// engine-lifetime payload does not keep the realm's allocation), and for
    /// functions made without the patched JSC, whose payload is freed by a GC
    /// finalizer, which may run on another thread and so must not touch the
    /// `Rc` counts. Written only on the engine thread, by `dispose`, which a
    /// call of the function may run: readers never hold a reference into it.
    pub realm: std::cell::Cell<Option<std::rc::Weak<crate::realm::RealmInner>>>,
}

/// Header at the start of every wrapper payload (`Boxed<T>`).
#[repr(C)]
pub(crate) struct Header {
    pub tag: PayloadTag,
    pub class: ClassId,
    /// The class's registry entry (null for closures); see `ClassInfo::parent_info`.
    pub info: *const ClassInfo,
    /// The realm the object was made in: the lean member dispatch gets its
    /// scope from the receiver instead of from thread-local state, the full
    /// dispatch its candidate realm (`guard_in`), cross-realm calls their
    /// receiver's realm and finalization the identity map to prune.
    pub realm_ref: std::rc::Weak<crate::realm::RealmInner>,
    /// The same realm for the lean member dispatch: its context and the
    /// realm itself in two loads, with no reference count touched.
    pub core: Rc<crate::realm::RealmCore>,
    /// An owned wrapper (`IdentityKey::OWNED`): nonzero while its owner wants
    /// the object kept. The collector reads it while marking, possibly from
    /// another thread (`JSWeakCreateKeptWhile`).
    pub keep_alive: std::sync::atomic::AtomicU8,
    /// An owned wrapper's weak handle (null otherwise, and once released).
    pub weak: std::cell::Cell<sys::JSWeakRef>,
    /// The realm's list of owned wrappers (`RealmInner::owned_head`), so it
    /// can release their handles when it is disposed.
    pub owned_prev: std::cell::Cell<*mut Header>,
    pub owned_next: std::cell::Cell<*mut Header>,
    pub kind: Kind,
    pub key: Option<IdentityKey>,
    pub data_type: TypeId,
    pub drop_fn: unsafe fn(*mut Header),
    pub data_ptr: unsafe fn(*mut Header) -> *const u8,
    /// Owning engine's finalization queue (outlives every payload: the VM is
    /// destroyed, finalizing all objects, before the queue is dropped).
    pub queue: *const FinalizeQueue,
}

/// Payload headers awaiting destruction on the engine thread. Finalizers may
/// run on any thread, hence the mutex.
#[derive(Default)]
pub(crate) struct FinalizeQueue {
    headers: std::sync::Mutex<Vec<usize>>,
    /// How many are queued, readable without the lock (an embedder decides
    /// from it whether to drain at a callback safe point).
    len: std::sync::atomic::AtomicUsize,
}

impl FinalizeQueue {
    fn push(&self, h: *mut Header) {
        let mut headers = self.headers.lock().unwrap_or_else(|e| e.into_inner());
        headers.push(h as usize);
        self.len.store(headers.len(), std::sync::atomic::Ordering::Relaxed);
    }
    fn take(&self) -> Vec<usize> {
        let mut headers = self.headers.lock().unwrap_or_else(|e| e.into_inner());
        self.len.store(0, std::sync::atomic::Ordering::Relaxed);
        std::mem::take(&mut *headers)
    }
    /// Payloads queued for [`drain_finalized`].
    pub(crate) fn len(&self) -> usize {
        self.len.load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[repr(C)]
pub(crate) struct Boxed<T> {
    pub header: Header,
    pub data: T,
}

unsafe fn data_ptr_boxed<T>(h: *mut Header) -> *const u8 {
    // SAFETY: h points to a live Boxed<T>.
    unsafe { &(*(h as *mut Boxed<T>)).data as *const T as *const u8 }
}

unsafe fn drop_boxed<T>(h: *mut Header) {
    // SAFETY: h was produced by Box::into_raw(Box<Boxed<T>>) (see new_payload).
    drop(unsafe { Box::from_raw(h as *mut Boxed<T>) });
}

pub(crate) fn new_payload<T: 'static>(
    engine: &EngineInner,
    class: ClassId,
    kind: Kind,
    realm: &Rc<crate::realm::RealmInner>,
    key: Option<IdentityKey>,
    data: T,
) -> *mut Header {
    let realm_ref = Rc::downgrade(realm);
    let info: *const ClassInfo = match engine.classes.borrow().classes.get(class as usize) {
        Some(i) => &**i,
        None => ptr::null(),
    };
    let b = Box::new(Boxed {
        header: Header {
            tag: PayloadTag::Wrapper,
            class,
            info,
            realm_ref,
            core: realm.core.clone(),
            keep_alive: std::sync::atomic::AtomicU8::new(0),
            weak: std::cell::Cell::new(ptr::null_mut()),
            owned_prev: std::cell::Cell::new(ptr::null_mut()),
            owned_next: std::cell::Cell::new(ptr::null_mut()),
            kind,
            key,
            data_type: TypeId::of::<T>(),
            drop_fn: drop_boxed::<T>,
            data_ptr: data_ptr_boxed::<T>,
            queue: std::sync::Arc::as_ptr(&engine.finalized),
        },
        data,
    });
    Box::into_raw(b) as *mut Header
}

unsafe fn upcast_erased<C, F>(p: *const u8) -> *const u8
where
    C: JsClass,
    F: Fn(&C::Data) -> &<C::Parent as JsClass>::Data + Copy + 'static,
{
    let f = unsafe { conjure::<F>() };
    // SAFETY: p points to a live C::Data (see data_of_header).
    f(unsafe { &*(p as *const C::Data) }) as *const _ as *const u8
}

/// # Safety
/// `h` must be a live payload header.
pub(crate) unsafe fn payload_data<'a, T: 'static>(h: *mut Header) -> Option<&'a T> {
    unsafe {
        if (*h).data_type != TypeId::of::<T>() {
            return None;
        }
        Some(&(*(h as *mut Boxed<T>)).data)
    }
}

unsafe extern "C" fn finalize_wrapper(object: sys::JSObjectRef) {
    // Runs inside GC, possibly on a JSC helper thread: no JSC calls besides
    // GetPrivate, no Rust destructors. The header is immutable after creation.
    let h = unsafe { sys::JSObjectGetPrivate(object) } as *mut Header;
    if h.is_null() {
        return;
    }
    unsafe { (*(*h).queue).push(h) };
}

/// Drops payloads of finalized wrappers and prunes dead identity-map entries.
/// Engine thread only, outside GC.
/// Takes a dead owned wrapper's payload out of its realm's list and gives
/// its weak handle to release (none for other payloads, and once the realm
/// disposed).
///
/// # Safety
/// `h` is a payload header whose object is dead, on the engine's thread.
unsafe fn release_owned(h: *mut Header) -> Option<sys::JSWeakRef> {
    unsafe {
        let weak = (*h).weak.replace(ptr::null_mut());
        if weak.is_null() {
            return None;
        }
        let prev = (*h).owned_prev.replace(ptr::null_mut());
        let next = (*h).owned_next.replace(ptr::null_mut());
        if !prev.is_null() {
            (*prev).owned_next.set(next);
        } else if let Some(realm) = (*h).realm_ref.upgrade() {
            if realm.owned_head.get() == h {
                realm.owned_head.set(next);
            }
        }
        if !next.is_null() {
            (*next).owned_prev.set(prev);
        }
        Some(weak)
    }
}

/// Releases the weak handles of dead owned wrappers: under one lock with the
/// patched JSC (`JSWeakReleaseMany`), one by one otherwise.
fn release_weaks(engine: &EngineInner, weaks: &[sys::JSWeakRef]) {
    if weaks.is_empty() {
        return;
    }
    let spi = sys::spi::get();
    // SAFETY: handles this engine made and still owns, released once.
    unsafe {
        if let Some(many) = spi.weak_release_many {
            many(engine.group(), weaks.as_ptr(), weaks.len());
        } else if let Some((_, release, _)) = spi.weak() {
            for &w in weaks {
                release(engine.group(), w);
            }
        }
    }
}

pub(crate) fn drain_finalized(engine: &EngineInner) -> usize {
    let mut total = 0;
    let mut weaks = Vec::new();
    loop {
        let batch = engine.finalized.take();
        if batch.is_empty() {
            return total;
        }
        total += batch.len();
        for &h in &batch {
            let h = h as *mut Header;
            // SAFETY: each pointer was enqueued exactly once by finalize_wrapper
            // and its JS object is dead, so we are the sole owner.
            unsafe {
                if let Some(key) = (*h).key {
                    // The payload's own realm, while it is live (a disposed
                    // realm has released its identity map already).
                    if let Some(realm) = (*h).realm_ref.upgrade().filter(|r| !r.ctx.get().is_null()) {
                        realm.prune_identity(key);
                    }
                }
                weaks.extend(release_owned(h));
            }
        }
        // The handles go before the payloads that hold their flag bytes.
        release_weaks(engine, &weaks);
        weaks.clear();
        for h in batch {
            let h = h as *mut Header;
            // SAFETY: as above; dropped once.
            unsafe { ((*h).drop_fn)(h) };
        }
    }
}

// ---------------------------------------------------------------------------
// Trampolines

/// Result shapes used by JSC native callbacks. A raw value needs an explicit
/// temporary root before the scheduling hook can allocate or collect garbage.
pub(crate) trait CallbackResult {
    fn root(&self, _scope: &Scope<'_>) -> Option<crate::Persistent> {
        None
    }
}
impl CallbackResult for bool {}
impl CallbackResult for () {}
impl CallbackResult for sys::JSValueRef {
    fn root(&self, scope: &Scope<'_>) -> Option<crate::Persistent> {
        if self.is_null() {
            None
        } else {
            // SAFETY: this is the value just returned by the native callback.
            Some(crate::Persistent::new(scope, unsafe { Value::from_raw(scope.ctx, *self) }))
        }
    }
}

/// Runs a callback body, converting errors and panics into JS exceptions.
/// Realm lookup, the body, error conversion and the release of the realm
/// handle (which may dispose it and call hooks) all happen inside the unwind
/// boundary.
///
/// Every trampoline instantiates this once per callback (thousands of times),
/// so it only adapts `body` to [`guard_erased`], which holds the logic once.
#[inline]
pub(crate) unsafe fn guard<R: CallbackResult>(
    ctx: sys::JSContextRef,
    exc: *mut sys::JSValueRef,
    fallback: R,
    body: impl FnOnce(&Scope<'_>) -> JsResult<R>,
) -> R {
    unsafe { guard_in(ctx, None, exc, fallback, body) }
}

/// [`guard`] with a candidate for the realm the callback runs in: the realm
/// of the member function or closure being called (whose context JSC passes
/// as `ctx`), or of the receiver of a property hook or custom accessor
/// (which usually is the caller's). The candidate is used when its global
/// context is `ctx`; otherwise the realm is looked up by `ctx` as `guard`
/// does, so the realm is the same either way.
#[inline]
pub(crate) unsafe fn guard_in<R: CallbackResult>(
    ctx: sys::JSContextRef,
    candidate: Option<&std::rc::Weak<crate::realm::RealmInner>>,
    exc: *mut sys::JSValueRef,
    fallback: R,
    body: impl FnOnce(&Scope<'_>) -> JsResult<R>,
) -> R {
    let realm = candidate.and_then(|c| crate::realm::realm_if_ctx(c, ctx));
    unsafe { guard_resolved(ctx, realm, exc, fallback, body) }
}

/// [`guard_in`] with the candidate already upgraded and checked by
/// [`crate::realm::realm_if_ctx`] (`None`: look the realm up by `ctx`).
#[inline]
pub(crate) unsafe fn guard_resolved<R: CallbackResult>(
    ctx: sys::JSContextRef,
    realm: Option<Rc<crate::realm::RealmInner>>,
    exc: *mut sys::JSValueRef,
    fallback: R,
    body: impl FnOnce(&Scope<'_>) -> JsResult<R>,
) -> R {
    let mut body = Some(body);
    let mut out = None;
    let mut step = |scope: &Scope<'_>, step: Step| match step {
        Step::Run => {
            out = Some(body.take().expect("callback body runs once")(scope)?);
            Ok(None)
        }
        Step::Root => Ok(out.as_ref().and_then(|v| v.root(scope))),
    };
    match unsafe { guard_erased(ctx, realm, exc, &mut step) } {
        true => out.expect("successful callback result"),
        false => fallback,
    }
}

/// What [`guard_erased`] asks of the typed callback.
enum Step {
    /// Run the body, keeping its result.
    Run,
    /// Root the kept result (see [`CallbackResult::root`]).
    Root,
}

/// The body of [`guard`]; returns whether the callback's result stands.
/// Dev builds keep one copy. Release builds inline it so the `dyn` call
/// disappears from the JS-to-native hot path.
#[cfg_attr(not(debug_assertions), inline(always))]
unsafe fn guard_erased(
    ctx: sys::JSContextRef,
    candidate: Option<Rc<crate::realm::RealmInner>>,
    exc: *mut sys::JSValueRef,
    step: &mut dyn FnMut(&Scope<'_>, Step) -> JsResult<Option<crate::Persistent>>,
) -> bool {
    // The realm handle is released inside the unwind boundary (its drop may
    // dispose the realm and call hooks).
    let mut candidate = candidate;
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let tls = crate::engine::callback_tls();
        let terminating = &tls.terminating;
        let _depth = DepthGuard::enter_with(&tls.depth);
        let realm = match candidate.take() {
            Some(realm) => Some(realm),
            None => crate::realm::realm_for_ctx(ctx),
        };
        debug_assert!(
            realm.as_ref().map(Rc::as_ptr) == crate::realm::realm_for_ctx(ctx).as_ref().map(Rc::as_ptr),
            "the candidate realm is the one the context belongs to"
        );
        let Some(realm) = realm else {
            unsafe { throw_message(ctx, "Execution context was destroyed.", exc) };
            return false;
        };
        let scope = Scope::new(ctx, &realm);
        let result = step(&scope, Step::Run);
        let hook = if matches!(result, Err(JsError::Terminated)) || terminating.get() {
            None
        } else {
            realm.native_callback_hook()
        };
        // Error::Thrown already owns a Persistent. Other errors materialize only
        // after the hook. Keep successful JS results protected until returning.
        let _return_root = match (&hook, &result) {
            (Some(_), Ok(_)) => step(&scope, Step::Root).ok().flatten(),
            _ => None,
        };
        if let Some(hook) = hook {
            realm.run_native_callback_hook(&scope, hook);
        }
        // Native code that ignored an inner termination must not let the
        // calling JS continue: treat the callback as terminated as well.
        let result = match result {
            Ok(_) if terminating.get() => Err(JsError::Terminated),
            r => r,
        };
        match result {
            Ok(_) => true,
            Err(JsError::Terminated) => {
                // Keep terminating: page code may catch the rethrown value.
                crate::engine::rearm_termination(ctx);
                unsafe { throw_message_value(ctx, "JavaScript execution terminated.", exc) };
                false
            }
            Err(e) => {
                unsafe { throw_into(&scope, &e, exc) };
                false
            }
        }
    }));
    match outcome {
        Ok(stands) => stands,
        Err(panic) => {
            let msg = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panic".into());
            // Creating the error cannot unwind (no Rust callbacks involved).
            unsafe { throw_message(ctx, &format!("Internal error: {msg}"), exc) };
            false
        }
    }
}

/// Debug check of the invariant the lean dispatch rests on: a lean callback
/// (pure getter, pure method, lean indexed getter) never runs page script.
/// `RealmCore::live` lends the receiver's realm without a reference count on
/// that premise; script could drop the realm's last handle mid-call. So
/// `pure_member_call` sends any call with an object argument (whose
/// conversion may call `toString`/`valueOf`/`Symbol.toPrimitive`) to the full
/// dispatch, and primitive conversions run no script (a Symbol or BigInt that
/// cannot convert throws a TypeError without calling out).
///
/// Debug builds mark the thread while a lean callback runs, and
/// `RealmInner::dispose` (which every drop of a realm runs first) aborts if
/// the mark is set: that is the event the invariant exists to exclude. The
/// check sits there rather than on every entry into script because a lean
/// getter may legitimately run the engine's own helper script (lazy
/// prototype population on a class's first wrapper in a realm), which
/// drops no realm. Release builds compile it away.
pub(crate) struct LeanCall {
    #[cfg(debug_assertions)]
    prev: bool,
}

#[cfg(debug_assertions)]
thread_local! {
    static IN_LEAN_CALL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

impl LeanCall {
    #[inline(always)]
    pub(crate) fn enter() -> LeanCall {
        #[cfg(debug_assertions)]
        {
            LeanCall { prev: IN_LEAN_CALL.with(|c| c.replace(true)) }
        }
        #[cfg(not(debug_assertions))]
        LeanCall {}
    }
}

impl Drop for LeanCall {
    #[inline(always)]
    fn drop(&mut self) {
        #[cfg(debug_assertions)]
        IN_LEAN_CALL.with(|c| c.set(self.prev));
    }
}

/// Asserts (debug builds) that no lean callback is on the stack: called
/// where a realm is disposed (see [`LeanCall`]).
#[inline(always)]
pub(crate) fn assert_not_in_lean_call() {
    #[cfg(debug_assertions)]
    if IN_LEAN_CALL.with(|c| c.get()) {
        // Not unwound into JS (where page code could catch it): abort loudly.
        eprintln!("lp-js invariant violated: a realm was disposed inside a lean (refcount-free) callback");
        std::process::abort();
    }
}

/// Tracks native-callback nesting (see `engine::DEPTH`).
pub(crate) struct DepthGuard(&'static std::cell::Cell<u32>);

impl DepthGuard {
    pub(crate) fn enter() -> DepthGuard {
        Self::enter_with(&crate::engine::callback_tls().depth)
    }

    /// With the thread's depth cell already looked up.
    pub(crate) fn enter_with(depth: &'static std::cell::Cell<u32>) -> DepthGuard {
        depth.set(depth.get() + 1);
        DepthGuard(depth)
    }
}

impl Drop for DepthGuard {
    fn drop(&mut self) {
        self.0.set(self.0.get().saturating_sub(1));
    }
}

/// Throws a primitive string (used to rethrow termination unchanged).
unsafe fn throw_message_value(ctx: sys::JSContextRef, msg: &str, exc: *mut sys::JSValueRef) {
    if !exc.is_null() {
        let s = JsString::new(msg);
        unsafe { *exc = sys::JSValueMakeString(ctx, s.as_raw()) };
    }
}

/// Runs a member or property hook in `realm`, the realm of its receiver, when
/// that is not the realm JSC called it in: property hooks get the caller's
/// global, and `Node.prototype.x.call(otherRealmNode)` the callee's. A closed
/// receiver realm throws as a direct call into it would. Errors become values
/// here so a DOMException comes from the receiver's realm.
#[cold]
#[inline(never)]
fn in_receiver_realm<R: CallbackResult>(
    realm: &ReceiverRealm,
    body: impl FnOnce(&Scope<'_>) -> JsResult<R>,
) -> JsResult<R> {
    // The receiver's payload holds its realm: a disposed one has no
    // context, a dropped one does not upgrade.
    let inner = realm.upgrade().ok_or(JsError::ContextDestroyed)?;
    let ctx = inner.global_ctx()?;
    let receiver = Scope::new(ctx, &inner);
    let result = body(&receiver);
    let hook = if crate::engine::callback_tls().terminating.get() { None } else { inner.native_callback_hook() };
    let _return_root = match (&hook, &result) {
        (Some(_), Ok(v)) => v.root(&receiver),
        _ => None,
    };
    if let Some(hook) = hook {
        inner.run_native_callback_hook(&receiver, hook);
    }
    result.map_err(|e| match e {
        JsError::Thrown(_) | JsError::Terminated | JsError::ContextDestroyed => e,
        e => JsError::thrown(crate::Persistent::new(&receiver, e.to_value(&receiver))),
    })
}

/// [`in_receiver_realm`] for members, whose function is alive in `scope`'s realm:
/// a live realm's method keeps working on a closed realm's object where it can.
#[cold]
#[inline(never)]
fn member_in_receiver_realm<R: CallbackResult>(
    scope: &Scope<'_>,
    realm: &ReceiverRealm,
    body: impl FnOnce(&Scope<'_>) -> JsResult<R>,
) -> JsResult<R> {
    match realm.upgrade() {
        Some(inner) if inner.global_ctx().is_ok() => in_receiver_realm(realm, body),
        _ => body(scope),
    }
}

/// A receiver's realm, as its payload header holds it.
type ReceiverRealm = std::rc::Weak<crate::realm::RealmInner>;

/// Resolves `this` to class `C`'s data and, when it is not `scope`'s, the
/// realm it belongs to: wrappers of `C` or a subclass, or the realm's global
/// object (also for `undefined`/`null` receivers).
unsafe fn this_data<'a, C: JsClass>(
    scope: &Scope<'_>,
    this: sys::JSObjectRef,
) -> JsResult<(&'a C::Data, Option<&'a ReceiverRealm>)> {
    // Walk the receiver's class chain (stable boxed entries) up to the brand,
    // projecting the data as it goes: no registry.
    // SAFETY: h is a live payload of a reachable object (receiver on stack);
    // `info` entries live as long as the registry, which outlives every
    // callback.
    if let Some(h) = unsafe { scope.header_of_receiver(this) } {
        if let Some(d) = unsafe { data_of_header::<C>(h) } {
            let realm = unsafe { &(*h).realm_ref };
            return Ok((d, (!crate::realm::is_scope_realm(realm, scope)).then_some(realm)));
        }
    }
    // Not a `C`: only the global receiver fallback is left, which needs the
    // class's registry entry (a cold path, one copy for every class).
    let p = this_data_global(scope, this, TypeId::of::<C>())?;
    // SAFETY: the pointer refers to C::Data owned by the realm's global
    // payload, which outlives this callback.
    Ok((unsafe { &*(p as *const C::Data) }, None))
}

/// The global receiver fallback of [`this_data`]: the data of `class` the
/// realm's global holds when `this` is that global (or `undefined`/`null`),
/// else "Illegal invocation".
#[cold]
#[inline(never)]
fn this_data_global(scope: &Scope<'_>, this: sys::JSObjectRef, class: TypeId) -> JsResult<*const u8> {
    let fallback = {
        let reg = scope.engine_inner().classes.borrow();
        let want = reg.id_of_type(class).expect("class registered");
        reg.classes[want as usize].global_receiver
    };
    if let Some(f) = fallback {
        // SAFETY: only for receivers that resolve to the realm's global.
        let global_receiver = this.is_null() || scope.is_global_object(this);
        if global_receiver {
            if let Some(p) = unsafe { f(scope) } {
                return Ok(p);
            }
        }
    }
    Err(JsError::Type("Illegal invocation".into()))
}

unsafe fn global_receiver_erased<C, F>(scope: &Scope<'_>) -> Option<*const u8>
where
    C: JsClass,
    F: for<'s> Fn(&Scope<'s>) -> Option<&'s C::Data> + Copy + 'static,
{
    let f = unsafe { conjure::<F>() };
    f(scope).map(|d| d as *const C::Data as *const u8)
}

/// `callAsFunction` of every member function object: the object's private
/// data is its `MemberCall`.
pub(crate) unsafe extern "C" fn member_call(
    ctx: sys::JSContextRef,
    function: sys::JSObjectRef,
    this: sys::JSObjectRef,
    argc: usize,
    argv: *const sys::JSValueRef,
    exc: *mut sys::JSValueRef,
) -> sys::JSValueRef {
    unsafe {
        // No realm in the payload here (see `MemberPayload::realm`): the
        // receiver's realm is the usual one, checked against `ctx`.
        guard_in(ctx, wrapper_realm(this), exc, ptr::null(), |scope| {
            let payload = sys::JSObjectGetPrivate(function) as *const MemberPayload;
            if payload.is_null() {
                return Err(JsError::Type("Illegal invocation".into()));
            }
            let call = (*payload).call;
            (call.invoke)(scope, this, Args::from_raw(ctx, argc, argv), call.f)
        })
    }
}

/// Frees a member function object's `MemberCall` (runs inside GC: no JSC
/// calls, no Rust destructors beyond the deallocation).
pub(crate) unsafe extern "C" fn member_finalize(object: sys::JSObjectRef) {
    let p = unsafe { sys::JSObjectGetPrivate(object) } as *mut MemberPayload;
    if !p.is_null() {
        unsafe { drop(Box::from_raw(p)) };
    }
}

unsafe fn method_invoke<C: JsClass>(
    scope: &Scope<'_>,
    this: sys::JSObjectRef,
    args: Args<'_>,
    f: *const (),
) -> JsResult<sys::JSValueRef> {
    let (data, realm) = unsafe { this_data::<C>(scope, this) }?;
    // SAFETY: stored by `ClassDef::method` from exactly this fn-pointer type.
    let f: MethodFn<C> = unsafe { std::mem::transmute(f) };
    if let Some(realm) = realm {
        let raw = args.raw;
        return member_in_receiver_realm(scope, realm, |s| f(s, data, Args { ctx: s.ctx, raw }).map(|v| v.raw));
    }
    f(scope, data, args).map(|v| v.raw)
}

/// The data of `C` in the instance payload `h` (the payload's class or a
/// subclass of `C`), by the class chain: no registry, no JavaScriptCore.
///
/// # Safety
/// `h` is a live wrapper payload header.
#[inline]
pub(crate) unsafe fn data_of_header<'a, C: JsClass>(h: *mut Header) -> Option<&'a C::Data> {
    unsafe {
        if (*h).kind != Kind::Instance || (*h).info.is_null() {
            return None;
        }
        // The brand: one compare at `C`'s depth in the receiver's display.
        let info: &ClassInfo = &*(*h).info;
        let brand = info.display.get(C::DEPTH as usize)?;
        if brand.class_type != TypeId::of::<C>() {
            return None;
        }
        if brand.direct {
            // `C`'s data is the payload's own (no upcast on the way, so the
            // same type): a `Boxed<C::Data>`, at its constant offset.
            let p = (h as *const u8).add(std::mem::offset_of!(Boxed<C::Data>, data));
            debug_assert_eq!(p, ((*h).data_ptr)(h));
            debug_assert_eq!((*h).data_type, TypeId::of::<C::Data>(), "a direct brand's payload holds `C::Data`");
            return Some(&*(p as *const C::Data));
        }
        // Upcasts on the way: project the data class by class.
        let mut info = (*h).info;
        let mut ptr = ((*h).data_ptr)(h);
        loop {
            if (*info).class_type == TypeId::of::<C>() {
                return Some(&*(ptr as *const C::Data));
            }
            if let Some(up) = (*info).upcast {
                ptr = up(ptr);
            }
            info = (*info).parent_info;
            if info.is_null() {
                return None;
            }
        }
    }
}

unsafe fn getter_invoke_h<C: JsClass>(
    scope: &Scope<'_>,
    h: *mut Header,
    _this: sys::JSObjectRef,
    f: *const (),
) -> JsResult<sys::JSValueRef> {
    let data = unsafe { data_of_header::<C>(h) }.ok_or_else(|| JsError::Type("Illegal invocation".into()))?;
    // SAFETY: stored by `ClassDef::pure_getter_typed` from this fn-pointer type.
    let f: GetterFn<C> = unsafe { std::mem::transmute(f) };
    f(scope, data).map(|v| v.raw)
}

/// The object a receiver-aware member gets: the receiver itself, or the
/// realm's global object (what script sees) for an `undefined`/`null`
/// receiver and for the global behind its proxy.
fn receiver_object(scope: &Scope<'_>, this: sys::JSObjectRef) -> sys::JSObjectRef {
    if this.is_null() || scope.is_global_object(this) {
        scope.global_object_raw()
    } else {
        this
    }
}

unsafe fn method_this_invoke<C: JsClass>(
    scope: &Scope<'_>,
    this: sys::JSObjectRef,
    args: Args<'_>,
    f: *const (),
) -> JsResult<sys::JSValueRef> {
    let (data, realm) = unsafe { this_data::<C>(scope, this) }?;
    // SAFETY: stored by `ClassDef::method_this` from exactly this fn-pointer type.
    let f: MethodThisFn<C> = unsafe { std::mem::transmute(f) };
    let receiver = receiver_object(scope, this);
    // SAFETY (both calls): the receiver is the callback's `this` (rooted by
    // the calling frame) or the realm's global; values of one VM may be
    // used with any of its contexts.
    if let Some(realm) = realm {
        let raw = args.raw;
        return member_in_receiver_realm(scope, realm, |s| {
            f(s, data, unsafe { Object::from_raw(s.ctx, receiver) }, Args { ctx: s.ctx, raw }).map(|v| v.raw)
        });
    }
    f(scope, data, unsafe { Object::from_raw(scope.ctx, receiver) }, args).map(|v| v.raw)
}

unsafe fn getter_this_invoke<C: JsClass>(
    scope: &Scope<'_>,
    this: sys::JSObjectRef,
    _args: Args<'_>,
    f: *const (),
) -> JsResult<sys::JSValueRef> {
    let (data, realm) = unsafe { this_data::<C>(scope, this) }?;
    // SAFETY: stored by `ClassDef::getter_this` from exactly this fn-pointer type.
    let f: GetterThisFn<C> = unsafe { std::mem::transmute(f) };
    let receiver = receiver_object(scope, this);
    // SAFETY: as in `method_this_invoke`.
    if let Some(realm) = realm {
        return member_in_receiver_realm(scope, realm, |s| {
            f(s, data, unsafe { Object::from_raw(s.ctx, receiver) }).map(|v| v.raw)
        });
    }
    f(scope, data, unsafe { Object::from_raw(scope.ctx, receiver) }).map(|v| v.raw)
}

unsafe fn getter_this_invoke_h<C: JsClass>(
    scope: &Scope<'_>,
    h: *mut Header,
    this: sys::JSObjectRef,
    f: *const (),
) -> JsResult<sys::JSValueRef> {
    let data = unsafe { data_of_header::<C>(h) }.ok_or_else(|| JsError::Type("Illegal invocation".into()))?;
    // SAFETY: stored by `ClassDef::pure_getter_this_typed` from this fn-pointer type.
    let f: GetterThisFn<C> = unsafe { std::mem::transmute(f) };
    // SAFETY: `this` is the wrapper whose payload `h` is, rooted by the caller.
    f(scope, data, unsafe { Object::from_raw(scope.ctx, this) }).map(|v| v.raw)
}

unsafe fn getter_invoke<C: JsClass>(
    scope: &Scope<'_>,
    this: sys::JSObjectRef,
    _args: Args<'_>,
    f: *const (),
) -> JsResult<sys::JSValueRef> {
    let (data, realm) = unsafe { this_data::<C>(scope, this) }?;
    // SAFETY: stored by `ClassDef::getter`/`accessor` from this fn-pointer type.
    let f: GetterFn<C> = unsafe { std::mem::transmute(f) };
    if let Some(realm) = realm {
        return member_in_receiver_realm(scope, realm, |s| f(s, data).map(|v| v.raw));
    }
    f(scope, data).map(|v| v.raw)
}

/// [`getter_invoke`] that falls back to the realm's global for a receiver
/// that is not a `C` (see [`ClassDef::global_value_accessor`]).
unsafe fn global_value_getter_invoke<C: JsClass>(
    scope: &Scope<'_>,
    this: sys::JSObjectRef,
    args: Args<'_>,
    f: *const (),
) -> JsResult<sys::JSValueRef> {
    let this = if !this.is_null() && unsafe { this_data::<C>(scope, this) }.is_err() { ptr::null_mut() } else { this };
    unsafe { getter_invoke::<C>(scope, this, args, f) }
}

/// [`setter_invoke`] with the fallback of [`global_value_getter_invoke`].
unsafe fn global_value_setter_invoke<C: JsClass>(
    scope: &Scope<'_>,
    this: sys::JSObjectRef,
    args: Args<'_>,
    f: *const (),
) -> JsResult<sys::JSValueRef> {
    let this = if !this.is_null() && unsafe { this_data::<C>(scope, this) }.is_err() { ptr::null_mut() } else { this };
    unsafe { setter_invoke::<C>(scope, this, args, f) }
}

/// [`setter_invoke`] from the receiver's payload header `h`, in the
/// receiver's realm: the custom setter dispatch found `h` already.
unsafe fn setter_invoke_h<C: JsClass>(
    scope: &Scope<'_>,
    h: *mut Header,
    value: sys::JSValueRef,
    f: *const (),
) -> JsResult<bool> {
    let Some(data) = (unsafe { data_of_header::<C>(h) }) else { return Ok(false) };
    // SAFETY: stored by `ClassDef::accessor` from this fn-pointer type.
    let f: SetterFn<C> = unsafe { std::mem::transmute(f) };
    // SAFETY: the setter's argument, rooted by the calling frame.
    f(scope, data, unsafe { Value::from_raw(scope.ctx, value) })?;
    Ok(true)
}

unsafe fn setter_invoke<C: JsClass>(
    scope: &Scope<'_>,
    this: sys::JSObjectRef,
    args: Args<'_>,
    f: *const (),
) -> JsResult<sys::JSValueRef> {
    let (data, realm) = unsafe { this_data::<C>(scope, this) }?;
    // SAFETY: stored by `ClassDef::accessor` from this fn-pointer type.
    let f: SetterFn<C> = unsafe { std::mem::transmute(f) };
    if let Some(realm) = realm {
        let value = args.get(0).raw;
        return member_in_receiver_realm(scope, realm, |s| {
            f(s, data, unsafe { Value::from_raw(s.ctx, value) })?;
            Ok(unsafe { sys::JSValueMakeUndefined(s.ctx) })
        });
    }
    f(scope, data, args.get(0))?;
    Ok(unsafe { sys::JSValueMakeUndefined(scope.ctx) })
}

/// Setter of a `[PutForwards]` attribute (see [`ClassDef::put_forwards`]).
/// Only the brand check touches the instance; the forwarding is ordinary
/// JavaScript property access on the receiver, so it runs in the caller's
/// realm like any other `Get`/`Set` the caller performs.
unsafe fn put_forwards_invoke<C: JsClass>(
    scope: &Scope<'_>,
    this: sys::JSObjectRef,
    args: Args<'_>,
    f: *const (),
) -> JsResult<sys::JSValueRef> {
    unsafe { this_data::<C>(scope, this) }?;
    // SAFETY: stored by `ClassDef::put_forwards` from an interned (leaked) `PutForwards`.
    let forwards = unsafe { &*(f as *const PutForwards) };
    let receiver = if this.is_null() { scope.global() } else { unsafe { Object::from_raw(scope.ctx, this) } };
    let value = receiver.get(forwards.name)?;
    let Some(target) = value.as_object() else {
        return Err(JsError::Type(format!(
            "Failed to set the '{}' property on '{}': The '{}' attribute's value is not an object.",
            forwards.name,
            C::NAME,
            forwards.name
        ).into()));
    };
    // A missing argument is `undefined` (Args::get), like any attribute setter.
    target.set(forwards.target, args.get(0))?;
    Ok(unsafe { sys::JSValueMakeUndefined(scope.ctx) })
}

unsafe fn static_invoke(
    scope: &Scope<'_>,
    _this: sys::JSObjectRef,
    args: Args<'_>,
    f: *const (),
) -> JsResult<sys::JSValueRef> {
    // SAFETY: stored by `ClassDef::static_method` from this fn-pointer type.
    let f: StaticFn = unsafe { std::mem::transmute(f) };
    f(scope, args).map(|v| v.raw)
}

/// Called by the realm's JS constructor shim as `native(newTarget, argsArray)`.
unsafe fn ctor_invoke(
    scope: &Scope<'_>,
    _this: sys::JSObjectRef,
    a: Args<'_>,
    f: *const (),
) -> JsResult<sys::JSValueRef> {
    // SAFETY: stored by `ClassDef::constructor` from this fn-pointer type.
    let f: CtorFn = unsafe { std::mem::transmute(f) };
    let ctx = scope.ctx;
    let new_target = a.get(0).as_object().ok_or_else(|| JsError::Type("Illegal constructor".into()))?;
    let list = a.get(1).as_object().ok_or_else(|| JsError::Type("Illegal constructor".into()))?;
    let len = list.get("length")?.to_number()? as usize;
    // Arguments stay reachable through `list` (a live argument array).
    let mut raw = Vec::with_capacity(len);
    for i in 0..len {
        raw.push(list.get_index(i as u32)?.raw);
    }
    let args = Args { ctx, raw: &raw };
    f(scope, new_target, args).map(|o| o.raw as sys::JSValueRef)
}

unsafe fn indexed_erased<C, F>(scope: &Scope<'_>, h: *mut Header, index: u32) -> JsResult<Option<sys::JSValueRef>>
where
    C: JsClass,
    F: for<'s> Fn(&Scope<'s>, &C::Data, u32) -> JsResult<Option<Value<'s>>> + Copy + 'static,
{
    let data = unsafe { hook_data::<C>(scope, h) }?;
    let f = unsafe { conjure::<F>() };
    Ok(f(scope, data, index)?.map(|v| v.raw))
}

unsafe fn named_erased<C, F>(scope: &Scope<'_>, h: *mut Header, name: &[u16]) -> JsResult<Option<sys::JSValueRef>>
where
    C: JsClass,
    F: for<'s> Fn(&Scope<'s>, &C::Data, &str) -> JsResult<Option<Value<'s>>> + Copy + 'static,
{
    let data = unsafe { hook_data::<C>(scope, h) }?;
    let f = unsafe { conjure::<F>() };
    Ok(f(scope, data, &String::from_utf16_lossy(name))?.map(|v| v.raw))
}

unsafe fn named_utf16_erased<C, F>(scope: &Scope<'_>, h: *mut Header, name: &[u16]) -> JsResult<Option<sys::JSValueRef>>
where
    C: JsClass,
    F: for<'s> Fn(&Scope<'s>, &C::Data, &[u16]) -> JsResult<Option<Value<'s>>> + Copy + 'static,
{
    let data = unsafe { hook_data::<C>(scope, h) }?;
    let f = unsafe { conjure::<F>() };
    Ok(f(scope, data, name)?.map(|v| v.raw))
}

unsafe fn named_set_erased<C, F>(scope: &Scope<'_>, h: *mut Header, name: &[u16], v: sys::JSValueRef) -> JsResult<bool>
where
    C: JsClass,
    F: for<'s> Fn(&Scope<'s>, &C::Data, &str, Value<'s>) -> JsResult<bool> + Copy + 'static,
{
    let data = unsafe { hook_data::<C>(scope, h) }?;
    unsafe { conjure::<F>()(scope, data, &String::from_utf16_lossy(name), Value::from_raw(scope.ctx, v)) }
}

unsafe fn named_set_utf16_erased<C, F>(
    scope: &Scope<'_>,
    h: *mut Header,
    name: &[u16],
    v: sys::JSValueRef,
) -> JsResult<bool>
where
    C: JsClass,
    F: for<'s> Fn(&Scope<'s>, &C::Data, &[u16], Value<'s>) -> JsResult<bool> + Copy + 'static,
{
    let data = unsafe { hook_data::<C>(scope, h) }?;
    unsafe { conjure::<F>()(scope, data, name, Value::from_raw(scope.ctx, v)) }
}

unsafe fn named_delete_erased<C, F>(scope: &Scope<'_>, h: *mut Header, name: &[u16]) -> JsResult<bool>
where
    C: JsClass,
    F: for<'s> Fn(&Scope<'s>, &C::Data, &str) -> JsResult<bool> + Copy + 'static,
{
    let data = unsafe { hook_data::<C>(scope, h) }?;
    let f = unsafe { conjure::<F>() };
    f(scope, data, &String::from_utf16_lossy(name))
}

unsafe fn named_delete_utf16_erased<C, F>(scope: &Scope<'_>, h: *mut Header, name: &[u16]) -> JsResult<bool>
where
    C: JsClass,
    F: for<'s> Fn(&Scope<'s>, &C::Data, &[u16]) -> JsResult<bool> + Copy + 'static,
{
    let data = unsafe { hook_data::<C>(scope, h) }?;
    let f = unsafe { conjure::<F>() };
    f(scope, data, name)
}

unsafe fn names_erased<C, F>(scope: &Scope<'_>, h: *mut Header) -> JsResult<Vec<JsString>>
where
    C: JsClass,
    F: for<'s> Fn(&Scope<'s>, &C::Data) -> JsResult<Vec<String>> + Copy + 'static,
{
    let data = unsafe { hook_data::<C>(scope, h) }?;
    let f = unsafe { conjure::<F>() };
    Ok(f(scope, data)?.iter().map(|n| JsString::new(n)).collect())
}

unsafe fn names_utf16_erased<C, F>(scope: &Scope<'_>, h: *mut Header) -> JsResult<Vec<JsString>>
where
    C: JsClass,
    F: for<'s> Fn(&Scope<'s>, &C::Data) -> JsResult<Vec<Vec<u16>>> + Copy + 'static,
{
    let data = unsafe { hook_data::<C>(scope, h) }?;
    let f = unsafe { conjure::<F>() };
    Ok(f(scope, data)?.iter().map(|n| JsString::from_utf16(n)).collect())
}

/// Data of class `C` for a property-hook receiver (hooks may be inherited).
unsafe fn hook_data<'a, C: JsClass>(_scope: &Scope<'_>, h: *mut Header) -> JsResult<&'a C::Data> {
    // The class chain walk of `this_data`, without the registry.
    // SAFETY: h is a live payload header (the hook's object); `info` entries
    // live as long as the registry.
    unsafe { data_of_header::<C>(h) }.ok_or_else(|| JsError::Type("Illegal invocation".into()))
}

/// Parses a canonical array index ("0", "17"; not "01", "-1", "4294967295").
fn array_index(name: &[u16]) -> Option<u32> {
    if name.is_empty() || name.len() > 10 || (name.len() > 1 && name[0] == b'0' as u16) {
        return None;
    }
    let mut n: u64 = 0;
    for &c in name {
        if !(b'0' as u16..=b'9' as u16).contains(&c) {
            return None;
        }
        n = n * 10 + (c - b'0' as u16) as u64;
    }
    (n < u32::MAX as u64).then_some(n as u32)
}

/// Shared prologue of the property hooks: realm scope, payload header and the
/// class's hooks (copied out so no registry borrow is held during callbacks).
struct HookCtx {
    header: *mut Header,
    indexed: Option<IndexedFn>,
    named: Option<NamedFn>,
    override_builtins: bool,
    named_set: Option<NamedSetFn>,
    set_all_names: bool,
    named_delete: Option<NamedDeleteFn>,
    names: Option<NamesFn>,
    unenumerable_names: Option<NamesFn>,
    legacy: bool,
}

impl HookCtx {
    /// The receiver's realm, when it is not the one `scope` works in (the
    /// hooks get the caller's context).
    fn foreign_realm(&self, scope: &Scope<'_>) -> Option<&std::rc::Weak<crate::realm::RealmInner>> {
        // SAFETY: the hook's object, and so its payload, is alive while the
        // hook runs.
        let realm = unsafe { &(*self.header).realm_ref };
        (!crate::realm::is_scope_realm(realm, scope)).then_some(realm)
    }
}

/// The realm a wrapper was made in, as the candidate for a callback's realm
/// ([`guard_in`]); `None` for any other object.
///
/// # Safety
/// `object` is null or a live object on the VM's thread; the reference is
/// valid while the object is.
#[inline]
pub(crate) unsafe fn wrapper_realm<'a>(
    object: sys::JSObjectRef,
) -> Option<&'a std::rc::Weak<crate::realm::RealmInner>> {
    if object.is_null() {
        return None;
    }
    unsafe {
        let h = sys::JSObjectGetPrivate(object) as *mut Header;
        (!h.is_null() && (*h).tag == PayloadTag::Wrapper).then(|| &(*h).realm_ref)
    }
}

unsafe fn hook_ctx(scope: &Scope<'_>, object: sys::JSObjectRef) -> Option<HookCtx> {
    let h = unsafe { sys::JSObjectGetPrivate(object) } as *mut Header;
    if h.is_null() {
        return None;
    }
    if scope.engine_inner().hook_state.placeholders.get() != 0
        && !scope.engine_inner().classes.borrow().placeholders.is_empty()
    {
        unsafe { sweep_placeholders(scope) };
    }
    // The hooks from the payload's own class entry (inherited ones are
    // merged in at registration): no registry borrow, no class-id lookup.
    // SAFETY: only wrapper classes install the hooks, so `h` is the live
    // payload header of `object`; `info` lives as long as the registry.
    let info = unsafe { (*h).info };
    if info.is_null() {
        return None;
    }
    let hooks = unsafe { &(*info).def.hooks };
    if !hooks.any() {
        return None;
    }
    Some(HookCtx {
        header: h,
        indexed: hooks.indexed,
        named: hooks.named,
        override_builtins: hooks.override_builtins,
        named_set: hooks.named_set,
        set_all_names: hooks.set_all_names,
        named_delete: hooks.named_delete,
        names: hooks.names,
        unenumerable_names: hooks.unenumerable_names,
        legacy: hooks.legacy,
    })
}

/// The property hooks' switches, one per engine (`EngineInner::hook_state`).
/// Every object whose hooks read them is a wrapper of that engine, whose
/// payload leads here through its class entry (`ClassInfo::hook_state`):
/// the hot indexed read gets them with the loads it makes anyway, not with
/// thread-local lookups.
#[derive(Default)]
pub(crate) struct HookState {
    /// Set while a hook asks JavaScriptCore about a wrapper's ordinary
    /// properties: the property hooks then answer nothing, so the operation
    /// sees only the wrapper's real own properties and its prototype chain.
    pub ordinary_only: std::cell::Cell<bool>,
    /// Placeholders awaiting removal ([`leave_placeholder`]): the lean
    /// dispatch paths, which do not sweep, step aside for the full one while
    /// it is nonzero, and [`hook_ctx`] looks at the list only then.
    pub placeholders: std::cell::Cell<usize>,
}

/// The hook switches of the engine `object` (a wrapper with property hooks)
/// belongs to; `None` for an object without a wrapper payload, whose hooks
/// answer nothing anyway.
#[inline]
unsafe fn hook_state_of<'a>(h: *mut Header) -> Option<&'a HookState> {
    unsafe {
        if h.is_null() || (*h).tag != PayloadTag::Wrapper || (*h).info.is_null() {
            return None;
        }
        Some(&*(*(*h).info).hook_state)
    }
}

/// Whether the property hooks of `object` are switched off (see
/// [`HookState::ordinary_only`]).
#[inline]
unsafe fn ordinary_only_on(object: sys::JSObjectRef) -> bool {
    unsafe { hook_state_of(sys::JSObjectGetPrivate(object) as *mut Header) }.is_some_and(|s| s.ordinary_only.get())
}

/// Runs `f` with the property hooks switched off (see
/// [`HookState::ordinary_only`]).
fn ordinary_only<R>(scope: &Scope<'_>, f: impl FnOnce() -> R) -> R {
    let state = &scope.engine_inner().hook_state;
    let was = state.ordinary_only.replace(true);
    let out = f();
    state.ordinary_only.set(was);
    out
}

/// True if `name` resolves without the hooks: a real own property of the
/// wrapper (an expando) or a property on its prototype chain.
unsafe fn has_ordinary_property(scope: &Scope<'_>, object: sys::JSObjectRef, name: sys::JSStringRef) -> bool {
    ordinary_only(scope, || unsafe { sys::JSObjectHasProperty(scope.ctx, object, name) })
}

/// Puts a placeholder on the wrapper: a real own property `name` that is
/// read-only and not enumerable, for the ordinary object code JavaScriptCore
/// runs right after a hook returns. The C API hooks cannot express two things
/// a WebIDL legacy platform object needs, and that code does both for such a
/// property:
/// - a write that fails like one to a read-only property, i.e. with a
///   `TypeError` in strict mode code only. The set hook can claim a write or
///   throw whatever the caller's strictness; when it returns `false` the
///   ordinary put runs and stops at the placeholder.
/// - an own property key that `Object.getOwnPropertyNames` lists and
///   `Object.keys` and `for..in` do not. The names hook is not told which is
///   asking; the real properties are listed after it by that rule.
///
/// Script does not see the placeholder itself: every property operation on a
/// wrapper of a class with hooks enters a hook before it looks at the real
/// properties, and [`hook_ctx`] removes the pending placeholders first. The
/// one exception is the list of real index keys, read before the names hook:
/// an own-keys listing that directly follows a refused write to an index past
/// the end still has that index. The wrapper stays protected until the
/// placeholder is removed. Call only for a name [`has_ordinary_property`]
/// lacks.
unsafe fn leave_placeholder(scope: &Scope<'_>, object: sys::JSObjectRef, name: sys::JSStringRef) {
    unsafe {
        let mut exc: sys::JSValueRef = ptr::null();
        ordinary_only(scope, || {
            sys::JSObjectSetProperty(
                scope.ctx,
                object,
                name,
                sys::JSValueMakeUndefined(scope.ctx),
                sys::kJSPropertyAttributeReadOnly | sys::kJSPropertyAttributeDontEnum,
                &mut exc,
            )
        });
        sys::JSValueProtect(scope.ctx, object);
        let name = JsString::from_utf16(utf16_of(name));
        scope.engine_inner().classes.borrow_mut().placeholders.push((object, name));
        let pending = &scope.engine_inner().hook_state.placeholders;
        pending.set(pending.get() + 1);
    }
}

/// Removes the pending placeholders ([`leave_placeholder`]).
unsafe fn sweep_placeholders(scope: &Scope<'_>) {
    let pending = std::mem::take(&mut scope.engine_inner().classes.borrow_mut().placeholders);
    let count = &scope.engine_inner().hook_state.placeholders;
    count.set(count.get().saturating_sub(pending.len()));
    for (object, name) in pending {
        unsafe {
            ordinary_only(scope, || sys::JSObjectDeleteProperty(scope.ctx, object, name.as_raw(), ptr::null_mut()));
            sys::JSValueUnprotect(scope.ctx, object);
        }
    }
}

/// True if `name` resolves on the object's prototype chain (named properties
/// must not shadow prototype members without `[LegacyOverrideBuiltIns]`).
unsafe fn on_prototype_chain(ctx: sys::JSContextRef, object: sys::JSObjectRef, name: sys::JSStringRef) -> bool {
    unsafe {
        let proto = sys::JSObjectGetPrototype(ctx, object);
        sys::JSValueIsObject(ctx, proto) && sys::JSObjectHasProperty(ctx, proto as sys::JSObjectRef, name)
    }
}

unsafe extern "C" fn get_property_hook(
    ctx: sys::JSContextRef,
    object: sys::JSObjectRef,
    name: sys::JSStringRef,
    exc: *mut sys::JSValueRef,
) -> sys::JSValueRef {
    if unsafe { ordinary_only_on(object) } {
        return ptr::null();
    }
    unsafe {
        guard_in(ctx, wrapper_realm(object), exc, ptr::null(), |scope| {
            let Some(hc) = hook_ctx(scope, object) else { return Ok(ptr::null()) };
            let units = utf16_of(name);
            let get = |scope: &Scope<'_>| -> JsResult<sys::JSValueRef> {
                if let Some(idx) = array_index(units) {
                    if let Some(f) = hc.indexed {
                        return Ok(f(scope, hc.header, idx)?.unwrap_or(ptr::null()));
                    }
                }
                if let Some(f) = hc.named {
                    if !hc.override_builtins && on_prototype_chain(scope.ctx, object, name) {
                        return Ok(ptr::null());
                    }
                    return Ok(match f(scope, hc.header, units)? {
                        // An own property shadows the named property.
                        Some(_)
                            if hc.legacy && !hc.override_builtins && has_ordinary_property(scope, object, name) =>
                        {
                            ptr::null()
                        }
                        Some(value) => value,
                        None => ptr::null(),
                    });
                }
                Ok(ptr::null())
            };
            if let Some(realm) = hc.foreign_realm(scope) {
                return match in_receiver_realm(realm, get) {
                    // A closed realm's members stay readable; calling them throws.
                    Err(JsError::ContextDestroyed) if on_prototype_chain(ctx, object, name) => Ok(ptr::null()),
                    result => result,
                };
            }
            get(scope)
        })
    }
}

/// `getPropertyByIndex` of a class with an indexed getter (patched JSC): the
/// index arrives as a number, never as a string the named hooks would parse.
unsafe extern "C" fn get_by_index_hook(
    ctx: sys::JSContextRef,
    object: sys::JSObjectRef,
    index: std::os::raw::c_uint,
    exc: *mut sys::JSValueRef,
) -> sys::JSValueRef {
    // Indexed getters run no script and have no observable side effect
    // (collections wrap their items): the lean dispatch first, the full one
    // for a receiver of another realm, an error, a panic, or while a
    // termination propagates (see `ClassDef::pure_getter`).
    // The switches come from the receiver's payload, which the lean read
    // needs anyway.
    let h = unsafe { sys::JSObjectGetPrivate(object) } as *mut Header;
    if let Some(state) = unsafe { hook_state_of(h) } {
        if state.ordinary_only.get() {
            return ptr::null();
        }
        if state.placeholders.get() == 0 {
            if let Some(v) = unsafe { get_by_index_lean(h, index) } {
                return v;
            }
        }
    }
    unsafe {
        guard_in(ctx, wrapper_realm(object), exc, ptr::null(), |scope| {
            let Some(hc) = hook_ctx(scope, object) else { return Ok(ptr::null()) };
            let Some(f) = hc.indexed else { return Ok(ptr::null()) };
            let get = |scope: &Scope<'_>| -> JsResult<sys::JSValueRef> {
                Ok(f(scope, hc.header, index)?.unwrap_or(ptr::null()))
            };
            if let Some(realm) = hc.foreign_realm(scope) {
                return in_receiver_realm(realm, get);
            }
            get(scope)
        })
    }
}

/// The lean half of [`get_by_index_hook`]: `Some` when it answered.
/// `h` is the receiver's wrapper payload, its class entry set
/// ([`hook_state_of`] checked both).
unsafe fn get_by_index_lean(h: *mut Header, index: u32) -> Option<sys::JSValueRef> {
    unsafe {
        let f = (*(*h).info).def.hooks.indexed?;
        // Lent without a count: an indexed getter runs no script (`LeanCall`).
        let (ctx, realm) = (*h).core.live()?;
        let scope = Scope::new(ctx, realm);
        let _lean = LeanCall::enter();
        match catch_unwind(AssertUnwindSafe(|| f(&scope, h, index))) {
            Ok(Ok(v)) => Some(v.unwrap_or(ptr::null())),
            _ => None,
        }
    }
}

/// `setPropertyByIndex`: a class with a named setter takes the index as a
/// name (`select[0] = option`); one without ignores (sloppy) writes to its
/// read-only indexed properties, as `set_property_hook` does.
unsafe extern "C" fn set_by_index_hook(
    ctx: sys::JSContextRef,
    object: sys::JSObjectRef,
    index: std::os::raw::c_uint,
    value: sys::JSValueRef,
    exc: *mut sys::JSValueRef,
) -> bool {
    if unsafe { ordinary_only_on(object) } {
        return false;
    }
    unsafe {
        guard_in(ctx, wrapper_realm(object), exc, false, |scope| {
            let Some(hc) = hook_ctx(scope, object) else { return Ok(false) };
            let Some(f) = hc.named_set else {
                // A legacy platform object refuses the write the way a
                // read-only property does (see `set_property_hook`): the
                // ordinary put that follows stops at the placeholder.
                if hc.legacy && hc.indexed.is_some() {
                    let name = JsString::new(&index.to_string());
                    if !has_ordinary_property(scope, object, name.as_raw()) {
                        leave_placeholder(scope, object, name.as_raw());
                        return Ok(false);
                    }
                }
                return Ok(true);
            };
            let name: Vec<u16> = index.to_string().encode_utf16().collect();
            let set = |scope: &Scope<'_>| f(scope, hc.header, &name, value);
            if let Some(realm) = hc.foreign_realm(scope) {
                return in_receiver_realm(realm, set);
            }
            set(scope)
        })
    }
}

/// `callAsFunction` of a member function made with
/// `JSObjectMakeFunctionWithCallbackAndData`: `data` is its `MemberPayload`.
pub(crate) unsafe extern "C" fn member_call_data(
    ctx: sys::JSContextRef,
    _function: sys::JSObjectRef,
    this: sys::JSObjectRef,
    argc: usize,
    argv: *const sys::JSValueRef,
    data: *mut std::ffi::c_void,
    exc: *mut sys::JSValueRef,
) -> sys::JSValueRef {
    unsafe {
        let payload = data as *const MemberPayload;
        if !payload.is_null() && (*payload).pure {
            if let Some(v) = pure_member_call(&*payload, ctx, this, argc, argv, exc) {
                return v;
            }
        }
        // The function's own realm is the one JSC calls it in. Upgraded here,
        // with no reference into the cell kept: the call may dispose the realm,
        // which clears it.
        let realm = if payload.is_null() {
            None
        } else {
            (*(*payload).realm.as_ptr()).as_ref().and_then(|c| crate::realm::realm_if_ctx(c, ctx))
        };
        guard_resolved(ctx, realm, exc, ptr::null(), |scope| {
            if payload.is_null() {
                return Err(JsError::Type("Illegal invocation".into()));
            }
            let call = (*payload).call;
            (call.invoke)(scope, this, Args::from_raw(ctx, argc, argv), call.f)
        })
    }
}

/// The lean dispatch of a pure operation (`ClassDef::pure_method`), in the
/// receiver's realm: `None` sends the call through the full dispatch (a
/// receiver that is not a wrapper, a disposed realm, an object argument, a
/// panic, termination, or an error in a realm other than the one JSC calls
/// the function in).
unsafe fn pure_member_call(
    payload: &MemberPayload,
    caller_ctx: sys::JSContextRef,
    this: sys::JSObjectRef,
    argc: usize,
    argv: *const sys::JSValueRef,
    exc: *mut sys::JSValueRef,
) -> Option<sys::JSValueRef> {
    unsafe {
        if this.is_null() {
            return None;
        }
        let h = sys::JSObjectGetPrivate(this) as *mut Header;
        if h.is_null() || (*h).tag != PayloadTag::Wrapper || (*h).kind != Kind::Instance {
            return None;
        }
        // `live` lends the realm without a count: sound only because nothing
        // below runs page script (see `LeanCall`).
        let (ctx, realm) = (*h).core.live()?;
        // An object argument's conversion (`toString`, `valueOf`,
        // `Symbol.toPrimitive`, a getter) runs page script, which may drop
        // the receiver's realm (borrowed here without a count) and must not
        // run twice when the call then falls back to the full dispatch.
        if argc > 0 && std::slice::from_raw_parts(argv, argc).iter().any(|&v| sys::JSValueIsObject(ctx, v)) {
            return None;
        }
        let scope = Scope::new(ctx, realm);
        let call = payload.call;
        let _lean = LeanCall::enter();
        catch_unwind(AssertUnwindSafe(|| match (call.invoke)(&scope, this, Args::from_raw(ctx, argc, argv), call.f) {
            Ok(v) if !v.is_null() => Some(v),
            // The full dispatch would run the same body in the same realm
            // (the one of `caller_ctx`) and throw the same error: throw it
            // here instead of running the operation a second time. Its
            // post-callback steps have nothing to do after a pure operation.
            Err(e) if ctx == caller_ctx && !matches!(e, JsError::Terminated | JsError::ContextDestroyed) => {
                throw_into(&scope, &e, exc);
                Some(ptr::null())
            }
            _ => None,
        }))
        .ok()
        .flatten()
    }
}

/// Whether a window's named properties object may have a name. Runs for every
/// name the global lacks, so it stays out of `guard`.
pub(crate) unsafe extern "C" fn named_properties_filter(
    ctx: sys::JSContextRef,
    characters: *const std::ffi::c_void,
    length: usize,
    is_8bit: bool,
) -> bool {
    let name = unsafe {
        if is_8bit {
            crate::realm::NameUnits::Latin1(std::slice::from_raw_parts(characters.cast(), length))
        } else {
            crate::realm::NameUnits::Utf16(std::slice::from_raw_parts(characters.cast(), length))
        }
    };
    catch_unwind(AssertUnwindSafe(|| {
        let realm = crate::realm::realm_for_ctx(ctx)?;
        let named = realm.named_properties()?;
        Some(named.may_resolve(name))
    }))
    .ok()
    .flatten()
    .unwrap_or(false)
}

/// `[[Get]]` of a window's named properties object, for names the filter let
/// through. `ctx` is the window's own, whichever realm is asking.
pub(crate) unsafe extern "C" fn named_properties_get(
    ctx: sys::JSContextRef,
    object: sys::JSObjectRef,
    name: sys::JSStringRef,
    exc: *mut sys::JSValueRef,
) -> sys::JSValueRef {
    unsafe {
        guard(ctx, exc, ptr::null(), |scope| {
            // The named property visibility algorithm: Object.prototype wins.
            let Some(named) = scope.realm.named_properties() else { return Ok(ptr::null()) };
            if on_prototype_chain(ctx, object, name) {
                return Ok(ptr::null());
            }
            let value = named.resolve(scope, &String::from_utf16_lossy(utf16_of(name)))?;
            Ok(value.map_or(ptr::null(), |v| v.raw))
        })
    }
}

unsafe extern "C" fn set_property_hook(
    ctx: sys::JSContextRef,
    object: sys::JSObjectRef,
    name: sys::JSStringRef,
    value: sys::JSValueRef,
    exc: *mut sys::JSValueRef,
) -> bool {
    if unsafe { ordinary_only_on(object) } {
        return false;
    }
    unsafe {
        guard_in(ctx, wrapper_realm(object), exc, false, |scope| {
            let Some(hc) = hook_ctx(scope, object) else { return Ok(false) };
            let units = utf16_of(name);
            if hc.indexed.is_some() && array_index(units).is_some() && hc.named_set.is_none() {
                // Indexed properties without a setter are read-only, and no
                // index past the end can be defined either.
                if hc.legacy && !has_ordinary_property(scope, object, name) {
                    leave_placeholder(scope, object, name);
                    return Ok(false);
                }
                // Ignore (sloppy) writes.
                return Ok(true);
            }
            if hc.legacy && hc.named_set.is_none() && !hc.override_builtins {
                if let Some(f) = hc.named {
                    // WebIDL [[DefineOwnProperty]]: no own property may be
                    // created for a supported property name, so the write to a
                    // visible named property fails.
                    let visible = |scope: &Scope<'_>| -> JsResult<bool> {
                        Ok(!on_prototype_chain(scope.ctx, object, name) && f(scope, hc.header, units)?.is_some())
                    };
                    let visible = if let Some(realm) = hc.foreign_realm(scope) {
                        in_receiver_realm(realm, visible)?
                    } else {
                        visible(scope)?
                    };
                    if visible && !has_ordinary_property(scope, object, name) {
                        leave_placeholder(scope, object, name);
                    }
                    return Ok(false);
                }
            }
            // WebIDL [[Set]] on the receiver itself invokes the named setter for
            // every string name when the class opted in (`storage.length = 1`
            // stores an item that only `getItem` reads back); otherwise names
            // the prototype chain has go to the prototype member.
            let set = |scope: &Scope<'_>| -> JsResult<bool> {
                if let Some(f) = hc.named_set {
                    if !hc.override_builtins && !hc.set_all_names && on_prototype_chain(scope.ctx, object, name) {
                        return Ok(false);
                    }
                    return f(scope, hc.header, units, value);
                }
                Ok(false)
            };
            if let Some(realm) = hc.foreign_realm(scope) {
                return in_receiver_realm(realm, set);
            }
            set(scope)
        })
    }
}

unsafe extern "C" fn delete_property_hook(
    ctx: sys::JSContextRef,
    object: sys::JSObjectRef,
    name: sys::JSStringRef,
    exc: *mut sys::JSValueRef,
) -> bool {
    if unsafe { ordinary_only_on(object) } {
        return false;
    }
    unsafe {
        guard_in(ctx, wrapper_realm(object), exc, false, |scope| {
            let Some(hc) = hook_ctx(scope, object) else { return Ok(false) };
            let Some(f) = hc.named_delete else { return Ok(false) };

            // The named property visibility algorithm: a name the prototype
            // chain has is not a named property, so the ordinary [[Delete]] runs.
            if !hc.override_builtins && on_prototype_chain(ctx, object, name) {
                return Ok(false);
            }
            let units = utf16_of(name);
            if let Some(realm) = hc.foreign_realm(scope) {
                return in_receiver_realm(realm, |s| f(s, hc.header, units));
            }
            f(scope, hc.header, units)
        })
    }
}

unsafe extern "C" fn property_names_hook(
    ctx: sys::JSContextRef,
    object: sys::JSObjectRef,
    acc: sys::JSPropertyNameAccumulatorRef,
) {
    unsafe {
        guard_in(ctx, wrapper_realm(object), ptr::null_mut(), (), |scope| {
            let Some(hc) = hook_ctx(scope, object) else { return Ok(()) };
            if hc.indexed.is_some() {
                let len = Object::from_raw(ctx, object).get("length")?;
                if len.is_number() {
                    for i in 0..len.to_number()? as u32 {
                        sys::JSPropertyNameAccumulatorAddName(acc, JsString::new(&i.to_string()).as_raw());
                    }
                }
            }
            if let Some(f) = hc.names {
                let names = if let Some(realm) = hc.foreign_realm(scope) {
                    let mut names = Vec::new();
                    in_receiver_realm(realm, |s| {
                        names = f(s, hc.header)?;
                        Ok(())
                    })?;
                    names
                } else {
                    f(scope, hc.header)?
                };
                for n in &names {
                    sys::JSPropertyNameAccumulatorAddName(acc, n.as_raw());
                }
            }
            if let Some(f) = hc.unenumerable_names {
                let names = if let Some(realm) = hc.foreign_realm(scope) {
                    let mut names = Vec::new();
                    in_receiver_realm(realm, |s| {
                        names = f(s, hc.header)?;
                        Ok(())
                    })?;
                    names
                } else {
                    f(scope, hc.header)?
                };
                // Listed by JavaScriptCore itself, from the wrapper's real
                // properties, when the caller wants non-enumerable keys too.
                for n in &names {
                    let visible = !(hc.indexed.is_some() && array_index(n.utf16()).is_some())
                        && !has_ordinary_property(scope, object, n.as_raw());
                    if visible {
                        leave_placeholder(scope, object, n.as_raw());
                    }
                }
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::array_index;

    fn u(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn array_index_parsing() {
        assert_eq!(array_index(&u("0")), Some(0));
        assert_eq!(array_index(&u("42")), Some(42));
        assert_eq!(array_index(&u("4294967294")), Some(4294967294));
        assert_eq!(array_index(&u("4294967295")), None);
        assert_eq!(array_index(&u("01")), None);
        assert_eq!(array_index(&u("-1")), None);
        assert_eq!(array_index(&u("length")), None);
        assert_eq!(array_index(&u("")), None);
    }
}
