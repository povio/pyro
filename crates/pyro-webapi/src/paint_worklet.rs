//! CSS Painting API: `CSS.paintWorklet` and `Worklet` (made in JS, paint_worklet.js), and the page's
//! paint worklet global: a realm of its own (no window, no DOM: the JS built-ins, `registerPaint`
//! and `devicePixelRatio`), made on the first `addModule()`, where each module is evaluated as a
//! module script. `registerPaint` validates and records a paint definition as the spec's
//! "registerPaint(name, paintCtor)" steps do. pyro does not draw `paint()` images (the style engine
//! parses them; blitz-paint leaves them empty), so a registered painter is never invoked.
//! `[SecureContext]` (`crate::secure_context`).

use std::cell::RefCell;
use std::rc::Rc;

use pyro_js::{JsResult, Object, Persistent, RealmOptions, Scope, Value};

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
    // moduleState(url) -> 'fetched' | 'fetching' | 'none': the URL in the worklet's module map.
    let state = s.function("moduleState", |s, _, a| {
        let page = crate::target::page_of(s)?;
        let url = a.get(0).to_string()?;
        let state = match page.paint_worklet_modules.borrow().get(&url) {
            Some(true) => "fetched",
            Some(false) => "fetching",
            None => "none",
        };
        Ok(s.string(state))
    })?;
    native.set_atom("moduleState", state.value())?;
    // awaitGraph(url) -> promise of the graph of a module in the worklet's module map, or null when
    // this engine cannot wait for one.
    let await_graph_fn = s.function("awaitGraph", |s, _, a| {
        let page = crate::target::page_of(s)?;
        let url = a.get(0).to_string()?;
        await_graph(s, &page, &url)
    })?;
    native.set_atom("awaitGraph", await_graph_fn.value())?;
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
    realm.set_module_fetcher(
        s.realm().module_fetcher().and_then(|f| f.for_worklet_graph(credentials, page.paint_worklet_modules.clone())),
    );
    let ws = realm.scope()?;
    let settle = Settle::new(s, deferred);
    match ws.eval_module(source, url) {
        Ok(evaluation) => settle.when(s, &ws, evaluation, |_, _| None)?,
        // A module that does not parse (or a failed import): the error, in the caller's realm.
        Err(e) => settle.now(s, Err(e.to_value(s)))?,
    }
    Ok(promise)
}

/// The graph of `url`, a module of the worklet's module map (another graph's import, fetched or
/// being fetched): a promise of the calling realm that settles once the module and every module it
/// imports are fetched and evaluated (fulfilled), or one of them failed (rejected with the failure),
/// as "fetch a worklet script graph" ends; not when the module's own source arrived. `null` without
/// a worklet realm or an engine that loads graphs in phases (`Scope::load_registered_module_graph`).
fn await_graph<'s>(s: &Scope<'s>, page: &Rc<PageDom>, url: &str) -> JsResult<Value<'s>> {
    let Some(realm) = page.paint_worklet.borrow().clone() else { return Ok(s.null()) };
    let ws = realm.scope()?;
    let Some(loading) = ws.load_registered_module_graph(url)? else { return Ok(s.null()) };
    let deferred = s.deferred()?;
    let promise = deferred.promise.get(s);
    let settle = Settle::new(s, deferred);
    // Loaded: link and evaluate it (a no-op for a module that already ran), whose promise ends it.
    let url = url.to_owned();
    settle.when(s, &ws, loading, move |ws, _| Some(ws.link_and_evaluate_module(&url)))?;
    Ok(promise)
}

/// The calling realm's promise for a worklet operation, settled from the worklet realm's promises.
#[derive(Clone)]
struct Settle {
    deferred: Rc<RefCell<Option<pyro_js::Deferred>>>,
    caller: pyro_js::WeakRealm,
}

impl Settle {
    fn new(s: &Scope<'_>, deferred: pyro_js::Deferred) -> Settle {
        Settle { deferred: Rc::new(RefCell::new(Some(deferred))), caller: s.realm().downgrade() }
    }

    /// Settles it now, in `s` (the caller's realm): fulfilled with undefined, or rejected with the error.
    fn now(&self, s: &Scope<'_>, result: Result<(), Value<'_>>) -> JsResult<()> {
        let Some(d) = self.deferred.borrow_mut().take() else { return Ok(()) };
        match result {
            Ok(()) => d.resolve(s, s.undefined()),
            Err(e) => d.reject(s, e),
        }
    }

    /// Settles it from a value of the worklet realm (`Persistent`), in the caller's realm.
    fn from_worklet(&self, value: Persistent, ok: bool) {
        let Some(d) = self.deferred.borrow_mut().take() else { return };
        let Some(realm) = self.caller.upgrade() else { return };
        let Ok(cs) = realm.scope() else { return };
        let v = value.get(&cs);
        let _ = if ok { d.resolve(&cs, cs.undefined()) } else { d.reject(&cs, v) };
    }

    /// Settles it as `promise` (of the worklet realm `ws`) settles; when it fulfills, `next` may give
    /// the step after it (a value or promise it then settles as, or the error that rejects it).
    fn when<'s>(
        &self,
        s: &Scope<'s>,
        ws: &Scope<'_>,
        promise: Value<'_>,
        next: impl for<'w> Fn(&Scope<'w>, Value<'w>) -> Option<JsResult<Value<'w>>> + 'static,
    ) -> JsResult<()> {
        let (ok, err) = (self.clone(), self.clone());
        let next = Rc::new(next);
        let on_ok = ws.function("fulfilled", move |ws, _, a| {
            match next(ws, a.get(0)) {
                None => ok.from_worklet(Persistent::new(ws, a.get(0)), true),
                Some(Err(e)) => ok.from_worklet(Persistent::new(ws, e.to_value(ws)), false),
                Some(Ok(after)) => {
                    if let Err(e) = ok.when_worklet(ws, after) {
                        ok.from_worklet(Persistent::new(ws, e.to_value(ws)), false);
                    }
                }
            }
            Ok(ws.undefined())
        })?;
        let on_err = ws.function("rejected", move |ws, _, a| {
            err.from_worklet(Persistent::new(ws, a.get(0)), false);
            Ok(ws.undefined())
        })?;
        let then = promise.as_object().map(|p| p.get_atom("then")).transpose()?.and_then(|t| t.as_object());
        match (then, promise.as_object()) {
            (Some(then), Some(p)) => {
                then.call(Some(p), &[on_ok.value(), on_err.value()])?;
                Ok(())
            }
            // Not a promise: done.
            _ => self.now(s, Ok(())),
        }
    }

    /// Settles it as the worklet realm's `value` (a promise or not) settles.
    fn when_worklet(&self, ws: &Scope<'_>, value: Value<'_>) -> JsResult<()> {
        let then = value.as_object().map(|p| p.get_atom("then")).transpose()?.and_then(|t| t.as_object());
        let Some((then, p)) = then.zip(value.as_object()) else {
            self.from_worklet(Persistent::new(ws, ws.undefined()), true);
            return Ok(());
        };
        let (ok, err) = (self.clone(), self.clone());
        let on_ok = ws.function("fulfilled", move |ws, _, _| {
            ok.from_worklet(Persistent::new(ws, ws.undefined()), true);
            Ok(ws.undefined())
        })?;
        let on_err = ws.function("rejected", move |ws, _, a| {
            err.from_worklet(Persistent::new(ws, a.get(0)), false);
            Ok(ws.undefined())
        })?;
        then.call(Some(p), &[on_ok.value(), on_err.value()])?;
        Ok(())
    }
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
