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
mod persistent;
mod realm;
mod string;
mod value;

pub use class::{ClassDef, ClassId, JsClass, NoParent};
pub use engine::{
    configure_process, prepare_process_for_fork, Capabilities, ConsoleLevel, Engine, EngineHooks, TerminateHandle,
    WrapperPropertyHook,
};
pub use error::{JsError, JsException, JsResult};
pub use persistent::{Persistent, RootedVec};
pub use realm::{
    Deferred, IdentityKey, ModuleFetcher, ModuleRequest, NameUnits, NamedProperties, NativeCallbackHook, Realm,
    RealmId, RealmOptions, Scope, UnhandledRejectionHandler, ValueDescription, WeakRealm,
};
pub use string::{atom, JsString};
pub use value::{Args, JsType, Object, StringUnits, Value};

/// Raw FFI, for code that needs an entry point not wrapped here.
pub use lp_jsc_sys as sys;
