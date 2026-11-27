//! Safe JavaScriptCore runtime layer for pyro.
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
pub mod bytecode_cache;
mod class;
mod engine;
mod error;
mod interned;
mod persistent;
mod realm;
mod rejections;
mod string;
mod value;

pub use accessors::slot_usage as accessor_slot_usage;
pub use bytecode_cache::BytecodeCache;
pub use class::{ClassDef, ClassId, GetterResult, GetterThisFn, JsClass, MethodThisFn, NoParent, OwnGetter, OwnName};
pub use engine::{
    configure_process, entry_realm, is_process_default, prepare_process_for_fork, prepare_time_zones, Capabilities,
    BytecodeCacheStats, ConsoleLevel, Engine, EngineHooks, FunctionCoverage, HeapCounts, ImpureNames, ModulePrecompileStats,
    ScriptCoverage, TerminateHandle, WrapperPropertyHook, RECYCLE_HEAP_SLACK,
};
pub use engine::{entered_any, process_footprint, release_thread_memory, reset_entered, script_entry_turn, set_allocator_release, set_allocator_trim};
pub use error::{DomError, JsError, JsException, JsResult, Message};
pub use interned::InternedId;
pub use persistent::{HeldValue, Persistent, RootedVec};
pub use realm::{
    Deferred, DeferredSetup, IdentityKey, LazyGlobalConstant, LazyGlobalInterface, ModuleFetcher, ModuleRequest, ModuleType, WorkletModule, WorkletModuleMap, NameUnits, NamedProperties,
    NativeCallbackHook, Realm, RealmId, RealmOptions, Scope, UnhandledRejectionHandler, ValueDescription, WeakRealm,
    WrapperRef,
};
pub use string::{atom, JsString};
pub use value::{Args, JsType, Object, StringUnits, Value};

/// Raw FFI, for code that needs an entry point not wrapped here.
pub use pyro_jsc_sys as sys;

pub use rejections::{available as rejection_tracking_available, RejectionHooks};

/// Whether this JavaScriptCore has the Trusted Types hooks for eval and Function
/// (patch 0035, [`Scope::set_compile_strings_hooks`]).
pub fn compile_strings_hooks_available() -> bool {
    pyro_jsc_sys::spi::get().set_compile_strings_callbacks.is_some()
}
