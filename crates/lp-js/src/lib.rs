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
pub use class::{ClassDef, ClassId, GetterResult, GetterThisFn, JsClass, MethodThisFn, NoParent, OwnName};
pub use engine::{
    configure_process, entry_realm, is_process_default, prepare_process_for_fork, Capabilities, ConsoleLevel, Engine,
    EngineHooks, FunctionCoverage, ImpureNames, ModulePrecompileStats, ScriptCoverage, TerminateHandle,
    WrapperPropertyHook,
};
pub use engine::{process_footprint, release_thread_memory, set_allocator_release, set_allocator_trim};
pub use error::{DomError, JsError, JsException, JsResult, Message};
pub use interned::InternedId;
pub use persistent::{HeldValue, Persistent, RootedVec};
pub use realm::{
    Deferred, DeferredSetup, IdentityKey, LazyGlobalConstant, ModuleFetcher, ModuleRequest, ModuleType, NameUnits, NamedProperties,
    NativeCallbackHook, Realm, RealmId, RealmOptions, Scope, UnhandledRejectionHandler, ValueDescription, WeakRealm,
    WrapperRef,
};
pub use string::{atom, JsString};
pub use value::{Args, JsType, Object, StringUnits, Value};

/// Raw FFI, for code that needs an entry point not wrapped here.
pub use lp_jsc_sys as sys;
