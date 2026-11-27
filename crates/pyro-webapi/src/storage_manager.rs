//! `navigator.storage` (StorageManager: `estimate()`, `persisted()`,
//! `persist()`, `getDirectory()`) on windows and workers. The class is made in JS on a
//! realm's first read of `navigator.storage` or `StorageManager`, so other
//! realms pay one accessor. `estimate()` reports the origin's local storage
//! and committed origin-private file system bytes and a fixed quota; storage is never persistent (a headless
//! context's data dies with it), so `persisted()` and `persist()` are false.
//! Opaque origins reject with TypeError, as in Chrome.
use std::rc::Rc;

use pyro_js::{ClassDef, JsClass, JsError, JsResult, Object, Persistent, Scope, Value};

use crate::page::PageDom;
use crate::window::WindowHost;

/// Kept on the realm's `navigator` (a hidden property), itself kept per realm.
const KEY: &str = "lpStorageManager";
/// What `estimate()` reports as the quota: 10 GiB, the order of what Chrome
/// grants an origin on a typical disk.
const QUOTA: f64 = crate::storage_buckets::catalog::DEFAULT_QUOTA as f64;

const SCRIPT: &str = r#"((estimate, directory, directorySlot, window, secure) => {
  'use strict';
  const token = Symbol('pyro');
  const PromiseC = Promise;
  const TypeErrorC = TypeError;
  const defineProperty = Object.defineProperty;
  class StorageManager {
    #brand;
    // A rest parameter keeps the interface object's length at 0.
    constructor(...t) {
      if (t[0] !== token) throw new TypeErrorC('Illegal constructor');
      this.#brand = true;
      if (secure) directorySlot.set(this, directory);
    }
    estimate() {
      try {
        if (!(#brand in this)) throw new TypeErrorC("Failed to execute 'estimate' on 'StorageManager': Illegal invocation");
        const result = estimate();
        const usageDetails = {};
        if (result[2]) usageDetails.fileSystem = result[2];
        if (result[3]) usageDetails.caches = result[3];
        if (result[4]) usageDetails.indexedDB = result[4];
        return PromiseC.resolve({ quota: result[1], usage: result[0], usageDetails });
      } catch (e) {
        return PromiseC.reject(e);
      }
    }
    persisted() {
      return StorageManager.#check(this, 'persisted');
    }
    getDirectory() {
      try {
        const getRoot = directorySlot.get(this);
        if (!getRoot) throw new TypeErrorC("Illegal invocation");
        return getRoot();
      } catch (e) {
        return PromiseC.reject(e);
      }
    }
    persist() {
      return StorageManager.#check(this, 'persist');
    }
    // Static, so a call on a foreign receiver rejects instead of throwing on `this.#check`.
    static #check(self, name) {
      try {
        if (!(#brand in self)) throw new TypeErrorC(`Failed to execute '${name}' on 'StorageManager': Illegal invocation`);
        estimate();
        return PromiseC.resolve(false);
      } catch (e) {
        return PromiseC.reject(e);
      }
    }
  }
  const proto = StorageManager.prototype;
  // persist() is [Exposed=Window].
  if (!window) delete proto.persist;
  if (!secure) delete proto.getDirectory;
  for (const name of ['estimate', 'persisted', 'persist', 'getDirectory']) {
    if (!(name in proto)) continue;
    const desc = Object.getOwnPropertyDescriptor(proto, name);
    desc.enumerable = true;
    defineProperty(proto, name, desc);
  }
  defineProperty(proto, Symbol.toStringTag, { value: 'StorageManager', configurable: true });
  return { StorageManager, storage: new StorageManager(token) };
})"#;

fn unavailable() -> JsError {
    JsError::Type("StorageManager is unavailable in this realm".into())
}

fn module<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Object<'s>> {
    let holder = crate::worker_scope::module_holder(s, page)?;
    if let Some(module) = s.get_hidden(holder, KEY)?.and_then(|v| v.as_object()) {
        return Ok(module);
    }
    let estimate = s.function("estimate", |s, _, _| {
        let page = crate::target::page_of(s)?;
        let key = crate::storage_buckets::storage_key(s, &page)
            .ok_or_else(|| JsError::Type("Storage is unavailable for an opaque origin.".into()))?;
        let storage = page.local_storage.borrow();
        let catalog = storage.buckets();
        let default_files = storage.opfs_usage(&key.origin);
        let default_idb = storage.indexed_db().usage(&key.origin);
        let default_caches = storage.caches().usage(&key.origin);
        let default_usage = storage.usage(&key.origin) as u64 + default_files + default_idb + default_caches;
        drop(storage);
        let named = catalog.usage(&key);
        Ok(s.array(&[
            s.number((default_usage + named.usage) as f64),
            s.number(QUOTA),
            s.number((default_files + named.file_system) as f64),
            s.number((default_caches + named.caches) as f64),
            s.number((default_idb + named.indexed_db) as f64),
        ])?.value())
    })?;
    let estimate = Persistent::new(s, estimate.value());
    let directory = s.function("getDirectory", |s, _, _| {
        let page = crate::target::page_of(s)?;
        let module = crate::opfs::module(s, &page, dedicated_worker(s))?;
        module.get_atom("getDirectory")?.as_object().ok_or_else(unavailable)?.call(None, &[])
    })?;
    let directory = Persistent::new(s, directory.value());
    let factory = s.eval(SCRIPT, Some("pyro://internal/storage-manager.js"), 1)?;
    let window = s.boolean(!crate::worker_scope::is_worker(s));
    let secure = crate::worker_scope::secure_worker(s).unwrap_or_else(|| crate::secure_context::secure(s, page));
    let slot = if secure {
        crate::streams::shared_slot(s, page, "StorageManager.getDirectory")?
    } else {
        s.null()
    };
    let module = factory.as_object().ok_or_else(unavailable)?.call(None,
        &[estimate.get(s), directory.get(s), slot, window, s.boolean(secure)])?;
    s.set_hidden(holder, KEY, module)?;
    module.as_object().ok_or_else(unavailable)
}

/// `navigator.storage`: one StorageManager per realm.
pub(crate) fn storage<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Value<'s>> {
    module(s, page)?.get_atom("storage")
}

/// Adds the `StorageManager` interface name to a window or worker global.
pub(crate) fn define_globals<W>(def: &mut ClassDef<W>)
where
    W: JsClass,
    W::Data: WindowHost,
{
    def.global_value_accessor(
        "StorageManager",
        |s, w| {
            let value = module(s, w.page())?.get_atom("StorageManager")?;
            replace(s, w.page(), value)?;
            Ok(value)
        },
        |s, w, value| replace(s, w.page(), value),
    );
}

fn replace(s: &Scope<'_>, page: &Rc<PageDom>, value: Value<'_>) -> JsResult<()> {
    let define = page.define_properties.borrow().get(&s.realm_id()).map(|p| p.get(s));
    let define = define.and_then(|v| v.as_object()).ok_or_else(unavailable)?;
    let desc = s.object();
    desc.set_atom("value", value)?;
    desc.set_atom("writable", s.boolean(true))?;
    desc.set_atom("enumerable", s.boolean(false))?;
    desc.set_atom("configurable", s.boolean(true))?;
    define.call(None, &[s.global().value(), s.string("StorageManager"), desc.value()])?;
    Ok(())
}


fn dedicated_worker(s: &Scope<'_>) -> bool {
    s.global_data::<crate::worker_scope::DedicatedWorkerGlobalScope>().is_some()
}

fn file_system_interface<'s>(s: &Scope<'s>, page: &Rc<PageDom>, name: &'static str) -> JsResult<Value<'s>> {
    let module = crate::opfs::module(s, page, dedicated_worker(s))?;
    let value = module.get_atom(name)?;
    crate::lazy_module::replace(s, page, name, value)?;
    Ok(value)
}

macro_rules! file_system_global {
    ($def:ident, $name:literal) => {
        $def.global_value_accessor($name, |s, w| file_system_interface(s, w.page(), $name),
            |s, w, v| crate::lazy_module::replace(s, w.page(), $name, v));
    };
}

pub(crate) fn define_file_system_globals<W: JsClass>(def: &mut ClassDef<W>) where W::Data: WindowHost {
    file_system_global!(def, "FileSystemHandle");
    file_system_global!(def, "FileSystemDirectoryHandle");
    file_system_global!(def, "FileSystemFileHandle");
    file_system_global!(def, "FileSystemWritableFileStream");
}

/// Worker interfaces belong on the global object itself. Registering one as
/// a member of WorkerGlobalScope would also expose it on the interface prototype.
pub(crate) fn install_worker_file_system_globals(s: &Scope<'_>) -> JsResult<()> {
    macro_rules! install {
        ($interface:ident) => {{
            struct $interface;
            impl pyro_js::LazyGlobalInterface for $interface {
                const NAME: &'static str = stringify!($interface);
                fn value<'s>(s: &Scope<'s>) -> JsResult<Value<'s>> {
                    let page = crate::target::page_of(s)?;
                    crate::opfs::module(s, &page, dedicated_worker(s))?.get_atom(Self::NAME)
                }
            }
            s.define_lazy_global_interface::<$interface>()?;
        }};
    }
    install!(FileSystemHandle);
    install!(FileSystemDirectoryHandle);
    install!(FileSystemFileHandle);
    install!(FileSystemWritableFileStream);
    // StorageManager is an own property of a worker global (not an accessor of its prototype).
    struct StorageManagerInterface;
    impl pyro_js::LazyGlobalInterface for StorageManagerInterface {
        const NAME: &'static str = "StorageManager";
        fn value<'s>(s: &Scope<'s>) -> JsResult<Value<'s>> {
            let page = crate::target::page_of(s)?;
            module(s, &page)?.get_atom("StorageManager")
        }
    }
    s.define_lazy_global_interface::<StorageManagerInterface>()?;
    if dedicated_worker(s) {
        install!(FileSystemSyncAccessHandle);
    }
    Ok(())
}

pub(crate) fn hide_file_system_globals(s: &Scope<'_>) -> JsResult<()> {
    for name in ["FileSystemHandle", "FileSystemDirectoryHandle", "FileSystemFileHandle",
                 "FileSystemWritableFileStream", "FileSystemSyncAccessHandle"] {
        s.global().delete(name)?;
    }
    Ok(())
}
