//! `navigator.storage` (StorageManager: `estimate()`, `persisted()`,
//! `persist()`) on windows and dedicated workers. The class is made in JS on a
//! realm's first read of `navigator.storage` or `StorageManager`, so other
//! realms pay one accessor. `estimate()` reports the origin's local storage
//! bytes as usage and a fixed quota; storage is never persistent (a headless
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
const QUOTA: f64 = 10.0 * 1024.0 * 1024.0 * 1024.0;

const SCRIPT: &str = r#"((estimate, window) => {
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
    }
    estimate() {
      try {
        if (!(#brand in this)) throw new TypeErrorC("Failed to execute 'estimate' on 'StorageManager': Illegal invocation");
        const result = estimate();
        // usageDetails as in Chrome (an empty record: nothing is broken down).
        return PromiseC.resolve({ quota: result[1], usage: result[0], usageDetails: {} });
      } catch (e) {
        return PromiseC.reject(e);
      }
    }
    persisted() {
      return this.#check('persisted');
    }
    persist() {
      return this.#check('persist');
    }
    #check(name) {
      try {
        if (!(#brand in this)) throw new TypeErrorC(`Failed to execute '${name}' on 'StorageManager': Illegal invocation`);
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
  for (const name of window ? ['estimate', 'persisted', 'persist'] : ['estimate', 'persisted']) {
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
        let origin = crate::storage::tuple_origin(s, &page)
            .ok_or_else(|| JsError::Type("Storage is unavailable for an opaque origin.".into()))?;
        let usage = page.local_storage.borrow().usage(&origin) as f64;
        Ok(s.array(&[s.number(usage), s.number(QUOTA)])?.value())
    })?;
    let estimate = Persistent::new(s, estimate.value());
    let factory = s.eval(SCRIPT, Some("pyro://internal/storage-manager.js"), 1)?;
    let window = s.boolean(!crate::worker_scope::is_worker(s));
    let module = factory.as_object().ok_or_else(unavailable)?.call(None, &[estimate.get(s), window])?;
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
