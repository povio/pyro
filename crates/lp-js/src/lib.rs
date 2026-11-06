//! Safe JavaScriptCore runtime layer for lightningpanda.
//!
//! * [`Engine`]: one per thread (a JSC context group / VM / heap).
//! * [`Realm`]: a global environment (page main world, isolated world).
//! * [`Scope`], [`Value`], [`Object`]: stack-scoped access (see `value` docs
//!   for the GC rooting rule); [`Persistent`] for heap-stored references.
//! * [`JsClass`]/[`ClassDef`]: WebIDL-shaped host classes backed by Rust data.
//!
//! Thread affinity: every type except [`TerminateHandle`] is `!Send`/`!Sync`.

mod accessors;
mod autorelease;
mod class;
mod engine;
mod error;
mod interned;
mod persistent;
mod realm;
mod string;
mod value;

pub use accessors::slot_usage as accessor_slot_usage;
pub use class::{ClassDef, ClassId, GetterResult, GetterThisFn, JsClass, MethodThisFn, NoParent};
pub use engine::{
    ImpureNames,
    configure_process, entry_realm, prepare_process_for_fork, Capabilities, ConsoleLevel, Engine, EngineHooks, TerminateHandle,
    WrapperPropertyHook,
};
pub use engine::set_allocator_release;
pub use error::{JsError, JsException, JsResult};
pub use interned::InternedId;
pub use persistent::{Persistent, RootedVec};
pub use realm::{
    Deferred, IdentityKey, LazyGlobalConstant, ModuleFetcher, ModuleRequest, NameUnits, NamedProperties,
    NativeCallbackHook, Realm, RealmId, RealmOptions, Scope, UnhandledRejectionHandler, ValueDescription, WeakRealm,
    WrapperRef,
};
pub use string::{atom, JsString};
pub use value::{Args, JsType, Object, StringUnits, Value};

/// Raw FFI, for code that needs an entry point not wrapped here.
pub use lp_jsc_sys as sys;
