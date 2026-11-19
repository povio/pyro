//! CSS Painting API: `CSS.paintWorklet` and `Worklet` (made in JS, paint_worklet.js), and the page's
//! paint worklet global: a realm of its own (no window, no DOM: the JS built-ins, `registerPaint`
//! and `devicePixelRatio`), made on the first `addModule()`, where each module is evaluated as a
//! module script. `registerPaint` validates and records a paint definition as the spec's
//! "registerPaint(name, paintCtor)" steps do. pyro does not draw `paint()` images (the style engine
//! parses them; blitz-paint leaves them empty), so a registered painter is never invoked.
//! `[SecureContext]` (`crate::secure_context`).

use std::cell::RefCell;
use std::rc::Rc;

use pyro_js::{JsError, JsResult, Object, Persistent, RealmOptions, Scope, Value};

use crate::lazy_module::LazyModule;
use crate::page::PageDom;

pub(crate) static MODULE: LazyModule = LazyModule {
    key: "lpPaintWorklet",
    source: include_str!("paint_worklet.js"),
    url: "pyro://internal/paint_worklet.js",
    natives: Some(natives),
};

/// The worklet global's own members: `registerPaint` (validated, recorded) and `devicePixelRatio`.
const PRELUDE: &str = r#"(function (global) {
  'use strict';
  const definitions = new Map();
  const TypeErrorC = TypeError;
  const toSequence = (value, what) => {
    if (value === undefined) return [];
    if (value === null || typeof value[Symbol.iterator] !== 'function') {
      throw new TypeErrorC(`Failed to execute 'registerPaint' on 'PaintWorkletGlobalScope': The '${what}' property is not iterable.`);
    }
    return Array.from(value, String);
  };
  const isConstructor = (f) => {
    if (typeof f !== 'function') return false;
    try { Reflect.construct(String, [], f); return true; } catch { return false; }
  };
  Object.defineProperty(global, 'registerPaint', {
    value: function registerPaint(name, paintCtor) {
      if (arguments.length < 2) throw new TypeErrorC(`Failed to execute 'registerPaint' on 'PaintWorkletGlobalScope': 2 arguments required, but only ${arguments.length} present.`);
      name = String(name);
      if (name === '') throw new TypeErrorC("Failed to execute 'registerPaint' on 'PaintWorkletGlobalScope': The empty string is not a valid name.");
      if (definitions.has(name)) {
        throw new DOMExceptionLike(`A class with name:'${name}' is already registered.`, 'InvalidModificationError');
      }
      if (typeof paintCtor !== 'function') throw new TypeErrorC("Failed to execute 'registerPaint' on 'PaintWorkletGlobalScope': The provided callback is not a function.");
      const inputProperties = toSequence(paintCtor.inputProperties, 'inputProperties');
      const inputArguments = toSequence(paintCtor.inputArguments, 'inputArguments');
      const contextOptions = paintCtor.contextOptions;
      const alpha = contextOptions == null || contextOptions.alpha === undefined ? true : !!contextOptions.alpha;
      if (!isConstructor(paintCtor)) throw new TypeErrorC("Failed to execute 'registerPaint' on 'PaintWorkletGlobalScope': The provided callback is not a constructor.");
      const prototype = paintCtor.prototype;
      if (prototype === null || (typeof prototype !== 'object' && typeof prototype !== 'function')) {
        throw new TypeErrorC("Failed to execute 'registerPaint' on 'PaintWorkletGlobalScope': The 'prototype' object on the class does not exist.");
      }
      if (typeof prototype.paint !== 'function') {
        throw new TypeErrorC("Failed to execute 'registerPaint' on 'PaintWorkletGlobalScope': The 'paint' function on the prototype does not exist.");
      }
      definitions.set(name, { paintCtor, inputProperties, inputArguments, alpha });
    },
    writable: true, configurable: true,
  });
  // The worklet global has no DOMException interface of its own here: an Error with its name.
  function DOMExceptionLike(message, name) { const e = new Error(message); e.name = name; return e; }
  Object.defineProperty(global, 'devicePixelRatio', { get() { return 1; }, configurable: true, enumerable: true });
})(globalThis);"#;

fn natives<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    let native = s.object();
    let active = s.function("active", |s, _, _| Ok(s.boolean(crate::target::page_of(s)?.is_fully_active())))?;
    native.set_atom("active", active.value())?;
    let evaluate = s.function("evaluate", |s, _, a| {
        let page = crate::target::page_of(s)?;
        let source = a.get(0).to_string()?;
        let url = a.get(1).to_string()?;
        let credentials = a.get(2).to_string()?;
        evaluate(s, &page, &source, &url, &credentials)
    })?;
    native.set_atom("evaluate", evaluate.value())?;
    Ok(native)
}

/// Evaluates `source` (from `url`) as a module in the page's paint worklet global, made on first
/// use, its imports fetched with the graph's `credentials` mode. Returns a promise of the calling
/// realm that settles as the module's evaluation does. paint_worklet.js runs one graph at a time, so
/// the fetcher installed here serves only this graph's modules.
fn evaluate<'s>(s: &Scope<'s>, page: &Rc<PageDom>, source: &str, url: &str, credentials: &str) -> JsResult<Value<'s>> {
    let deferred = s.deferred()?;
    let promise = deferred.promise.get(s);
    let realm = {
        let mut slot = page.paint_worklet.borrow_mut();
        if slot.is_none() {
            let realm = pyro_js::Engine::create_realm_in_current_thread(RealmOptions {
                name: "paint-worklet".into(),
                global_class: None,
                origin: crate::storage::tuple_origin(s, page),
            })?;
            {
                let ws = realm.scope()?;
                ws.eval(PRELUDE, Some("pyro://internal/paint-worklet-global.js"), 1)?;
            }
            *slot = Some(Rc::new(realm));
        }
        slot.as_ref().expect("worklet realm").clone()
    };
    // "Fetch a worklet script graph" with the document's fetch client: every module of the graph is
    // fetched with the graph's credentials mode, in the worklet's own module map (not the document's).
    realm.set_module_fetcher(s.realm().module_fetcher().and_then(|f| f.for_worklet_graph(credentials)));
    let ws = realm.scope()?;
    let caller = s.realm().downgrade();
    let deferred = Rc::new(RefCell::new(Some(deferred)));
    // Settles the caller's promise with `value` (fulfilled or rejected), in the caller's realm.
    let settle = move |deferred: &Rc<RefCell<Option<pyro_js::Deferred>>>, value: Persistent, ok: bool| {
        let Some(d) = deferred.borrow_mut().take() else { return };
        let Some(realm) = caller.upgrade() else { return };
        let Ok(cs) = realm.scope() else { return };
        let v = value.get(&cs);
        let _ = if ok { d.resolve(&cs, cs.undefined()) } else { d.reject(&cs, v) };
    };
    match ws.eval_module(source, url) {
        Ok(evaluation) => {
            let (d1, d2) = (deferred.clone(), deferred.clone());
            let (s1, s2) = (settle.clone(), settle);
            let on_ok = ws.function("fulfilled", move |ws, _, a| {
                s1(&d1, Persistent::new(ws, a.get(0)), true);
                Ok(ws.undefined())
            })?;
            let on_err = ws.function("rejected", move |ws, _, a| {
                s2(&d2, Persistent::new(ws, a.get(0)), false);
                Ok(ws.undefined())
            })?;
            let then = evaluation.as_object().map(|p| p.get_atom("then")).transpose()?.and_then(|t| t.as_object());
            match (then, evaluation.as_object()) {
                (Some(then), Some(p)) => {
                    then.call(Some(p), &[on_ok.value(), on_err.value()])?;
                }
                _ => {
                    let d = deferred.borrow_mut().take().ok_or_else(|| JsError::type_error("worklet"))?;
                    d.resolve(s, s.undefined())?;
                }
            }
        }
        Err(e) => {
            // A module that does not parse (or a failed import): the error, in the caller's realm.
            let d = deferred.borrow_mut().take().ok_or_else(|| JsError::type_error("worklet"))?;
            d.reject(s, e.to_value(s))?;
        }
    }
    Ok(promise)
}

/// `CSS.paintWorklet`: the realm's paint `Worklet` in a secure context, else none.
pub(crate) fn paint_worklet<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Option<Value<'s>>> {
    if !crate::secure_context::secure(s, page) {
        return Ok(None);
    }
    MODULE.module(s, page)?.get("paintWorklet").map(Some)
}

/// `Worklet` on a window global (absent outside a secure context).
pub(crate) fn define_globals<W>(def: &mut pyro_js::ClassDef<W>)
where
    W: pyro_js::JsClass,
    W::Data: crate::window::WindowHost,
{
    def.global_value_accessor(
        "Worklet",
        |s, w| {
            let page = crate::window::WindowHost::page(w);
            if !crate::secure_context::secure(s, page) {
                return Ok(s.undefined());
            }
            MODULE.global(s, page, "Worklet")
        },
        |s, w, value| crate::lazy_module::replace(s, crate::window::WindowHost::page(w), "Worklet", value),
    );
}
