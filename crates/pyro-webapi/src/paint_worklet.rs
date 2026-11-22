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
        let graph = a.get(3).to_number()? as u64;
        evaluate(s, &page, &source, &url, &credentials, graph)
    })?;
    native.set_atom("evaluate", evaluate.value())?;
    // moduleState(url) -> 'fetched' | 'fetching' | 'none': the URL in the worklet's module map.
    let state = s.function("moduleState", |s, _, a| {
        let page = crate::target::page_of(s)?;
        let url = a.get(0).to_string()?;
        let state = match page.paint_worklet_modules.borrow().get(&url) {
            Some(m) if m.fetched => "fetched",
            Some(_) => "fetching",
            None => "none",
        };
        Ok(s.string(state))
    })?;
    native.set_atom("moduleState", state.value())?;
    // moduleGraph(url) -> the id of the graph that fetched the URL (evaluate's `graph`), or -1.
    let module_graph = s.function("moduleGraph", |s, _, a| {
        let page = crate::target::page_of(s)?;
        let url = a.get(0).to_string()?;
        let graph = page.paint_worklet_modules.borrow().get(&url).map_or(-1.0, |m| m.graph as f64);
        Ok(s.number(graph))
    })?;
    native.set_atom("moduleGraph", module_graph.value())?;
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
/// use, its imports fetched with the graph's `credentials` mode and recorded as graph `graph`'s.
/// Returns a promise of the calling realm, as HTML's `addModule()` settles: rejected when the graph
/// fails to load (an import's fetch failed, a parse error) or to link; fulfilled once the module
/// ran, also when its evaluation threw (that error is reported, as "run a module script" does with
/// rethrow errors false). paint_worklet.js runs one graph at a time, so the fetcher installed here
/// serves only this graph's modules.
fn evaluate<'s>(
    s: &Scope<'s>,
    page: &Rc<PageDom>,
    source: &str,
    url: &str,
    credentials: &str,
    graph: u64,
) -> JsResult<Value<'s>> {
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
        s.realm().module_fetcher().and_then(|f| f.for_worklet_graph(credentials, page.paint_worklet_modules.clone(), graph)),
    );
    let ws = realm.scope()?;
    let settle = Settle::new(s, deferred);
    match ws.load_module_graph(source, url) {
        // Loaded (every import fetched, the graph parsed): link and run it.
        Ok(Some(loading)) => {
            let loaded = loaded_or_threw(&ws, loading)?;
            let url = url.to_owned();
            settle.when(s, &ws, loaded, move |ws, _| Some(run_module(ws, &url)))?
        }
        // An engine without module graph phases (JavaScriptCore without patch 0037): one promise for
        // the graph's load and its evaluation, which cannot tell an evaluation error from a failed
        // load, so both reject.
        Ok(None) => match ws.eval_module(source, url) {
            Ok(evaluation) => settle.when(s, &ws, evaluation, |_, _| None)?,
            // A module that does not parse (or a failed import): the error, in the caller's realm.
            Err(e) => settle.now(s, Err(e.to_value(s)))?,
        },
        Err(e) => settle.now(s, Err(e.to_value(s)))?,
    }
    Ok(promise)
}

/// HTML "run a module script" (rethrow errors false) for the loaded graph of `url` in the worklet
/// realm `ws`: a link error is `Err` (the script's error to rethrow, which rejects `addModule()`);
/// otherwise a promise of `ws` that fulfills once the evaluation settles, an evaluation error
/// reported rather than rejecting it. (Chrome 158 and WPT worklets/resources/import-tests.js:
/// "Importing a script which throws should still resolve the given promise".)
fn run_module<'w>(ws: &Scope<'w>, url: &str) -> JsResult<Value<'w>> {
    let evaluation = match ws.link_and_evaluate_module(url) {
        Ok(evaluation) => evaluation,
        // JavaScriptCore throws the [[EvaluationError]] of a graph without top-level await
        // synchronously (JSModuleLoader::linkAndEvaluateModule rethrows the entry's error), also when
        // a module of the graph threw before: the module ran, so report it and fulfill.
        Err(pyro_js::JsError::Thrown(e)) if !is_link_error(ws, e.get(ws)) => {
            ws.note_module_evaluation_error(e.get(ws));
            ws.report_exception(&pyro_js::JsError::Thrown(e));
            return Ok(ws.undefined());
        }
        Err(e) => return Err(e),
    };
    let deferred = Rc::new(ws.deferred()?);
    let promise = deferred.promise.get(ws);
    let (ran, threw) = (deferred.clone(), deferred);
    ws.then(
        evaluation,
        move |ws, _| {
            let _ = ran.resolve(ws, ws.undefined());
        },
        move |ws, error| {
            ws.note_module_evaluation_error(error);
            ws.report_exception(&pyro_js::JsError::thrown(Persistent::new(ws, error)));
            let _ = threw.resolve(ws, ws.undefined());
        },
    )?;
    Ok(promise)
}

/// Whether `error`, which linking and evaluating a graph threw, failed its link rather than its
/// evaluation: a `SyntaxError` (an unresolvable or ambiguous import) that is no module's recorded
/// evaluation error. A heuristic, as `module_graph_has_script` in pyro-browser's loader: a module
/// that throws a `SyntaxError` of its own reads as a link error.
fn is_link_error(ws: &Scope<'_>, error: Value<'_>) -> bool {
    let Some(object) = error.as_object() else { return false };
    !ws.is_module_evaluation_error(error)
        && object.get_atom("name").ok().and_then(|n| n.as_string()).as_deref() == Some("SyntaxError")
}

/// `loading`, a graph load of the worklet realm `ws`, as a promise of `ws` that fulfills when it
/// fulfills and also when it rejects with a module's recorded evaluation error: JavaScriptCore
/// rejects a load that reaches a module whose evaluation threw (JSModuleLoader::loadModule), but
/// that graph did load; `run_module` then reports the error again, as HTML's rerun does. Any other
/// rejection (a failed fetch, a parse error) rejects it.
fn loaded_or_threw<'w>(ws: &Scope<'w>, loading: Value<'_>) -> JsResult<Value<'w>> {
    let deferred = Rc::new(ws.deferred()?);
    let promise = deferred.promise.get(ws);
    let (loaded, failed) = (deferred.clone(), deferred);
    ws.then(
        loading,
        move |ws, value| {
            let _ = loaded.resolve(ws, value);
        },
        move |ws, error| {
            let _ = if ws.is_module_evaluation_error(error) {
                failed.resolve(ws, ws.undefined())
            } else {
                failed.reject(ws, error)
            };
        },
    )?;
    Ok(promise)
}

/// The graph of `url`, a module of the worklet's module map (another graph's import, fetched or
/// being fetched): a promise of the calling realm that settles once the module and every module it
/// imports are fetched and evaluated (fulfilled, an evaluation error reported: `run_module`), or one
/// of them failed to load or link (rejected with the failure),
/// as "fetch a worklet script graph" ends; not when the module's own source arrived. `null` without
/// a worklet realm or an engine that loads graphs in phases (`Scope::load_registered_module_graph`).
fn await_graph<'s>(s: &Scope<'s>, page: &Rc<PageDom>, url: &str) -> JsResult<Value<'s>> {
    #[cfg(feature = "test-hooks")]
    if FORCE_GRAPH_FALLBACK.get() {
        return Ok(s.null());
    }
    let Some(realm) = page.paint_worklet.borrow().clone() else { return Ok(s.null()) };
    let ws = realm.scope()?;
    let Some(loading) = ws.load_registered_module_graph(url)? else { return Ok(s.null()) };
    let deferred = s.deferred()?;
    let promise = deferred.promise.get(s);
    let settle = Settle::new(s, deferred);
    // Loaded (or reached a module that threw before): link and evaluate it (a no-op for a module that
    // already ran), whose promise ends it; an evaluation error (also one a module threw before, which
    // JavaScriptCore throws again) is reported and fulfills it.
    let loaded = loaded_or_threw(&ws, loading)?;
    let url = url.to_owned();
    settle.when(s, &ws, loaded, move |ws, _| Some(run_module(ws, &url)))?;
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

#[cfg(feature = "test-hooks")]
thread_local! {
    /// Tests: take paint_worklet.js's path for an engine without module graph phases.
    static FORCE_GRAPH_FALLBACK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Tests only (feature `test-hooks`): on this thread, `addModule()` of another graph's import waits
/// as it does with a JavaScriptCore that cannot load a registered module's graph (without patch
/// 0037), so that path is tested on a patched build too.
#[cfg(feature = "test-hooks")]
pub fn force_graph_fallback_for_tests(on: bool) {
    FORCE_GRAPH_FALLBACK.set(on);
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
