// CSS.paintWorklet (CSS Painting API) and the Worklet interface for one realm. addModule() fetches
// the module and evaluates it in the page's paint worklet global (paint_worklet.rs), where
// registerPaint() validates and records paint definitions. pyro does not draw paint() images: a
// registered painter is never invoked. natives:
//   active() -> whether the document is fully active
//   moduleState(url) -> 'fetched' | 'fetching' | 'none': the URL in the worklet global's module map,
//     which records every module its graphs fetched (imports included); 'fetched' is its source only
//   moduleGraph(url) -> id of the graph that fetched it (evaluate's `graph`), -1 for none
//   awaitGraph(url) -> promise of such a module's graph (it and its imports fetched and evaluated, or
//     failed), or null when the engine cannot wait for one
//   fetch(url, init) -> fetch() of a module, as a "paintworklet" request (CSP: script-src-elem)
//   evaluate(source, url, credentials, graph) -> promise of the module's evaluation in the worklet
//     global as graph `graph` (its imports recorded as that graph's),
//     its imports fetched with `credentials`; one graph at a time (see `graphs`). It rejects when the
//     graph fails to load or link, and fulfills once the module ran, also when evaluation threw
//     (reported, not rethrown)
(function (native) {
  'use strict';
  const token = Symbol('pyro');
  const O = Object;
  const PromiseC = Promise;
  const DOMExceptionC = DOMException;
  const TypeErrorC = TypeError;
  const URLC = URL;
  const StringC = String;
  const reject = (e) => PromiseC.reject(e);
  const worklets = new WeakSet();
  // Each worklet's module map ("module responses map"), by requested URL: a module being fetched or
  // evaluated, or one that loaded, answers a later addModule() of that URL without a new fetch. A
  // failed one leaves the map, so a later call fetches it again.
  const moduleMaps = new WeakMap();
  // The module graphs being fetched and evaluated run one after another: each installs a fetcher
  // carrying its own credentials mode, which must not serve another graph's imports.
  let graphs = PromiseC.resolve();
  // Each graph's evaluation, by id (moduleGraph's): what a module it fetched waits for when the engine
  // cannot wait for the module's own graph.
  const runs = new Map();
  let nextGraph = 1;
  class Worklet {
    constructor(t) {
      if (t !== token) throw new TypeErrorC("Failed to construct 'Worklet': Illegal constructor");
      worklets.add(this);
      moduleMaps.set(this, new Map());
    }
    addModule(moduleURL, options = undefined) {
      try {
        if (!worklets.has(this)) throw new TypeErrorC("Failed to execute 'addModule' on 'Worklet': Illegal invocation");
        if (arguments.length < 1) throw new TypeErrorC("Failed to execute 'addModule' on 'Worklet': 1 argument required, but only 0 present.");
        if (!native.active()) throw new DOMExceptionC('This frame is not fully active.', 'InvalidStateError');
        let url;
        try {
          url = new URLC(StringC(moduleURL), document.baseURI).href;
        } catch {
          throw new DOMExceptionC(`Failed to execute 'addModule' on 'Worklet': The URL '${StringC(moduleURL)}' is invalid.`, 'SyntaxError');
        }
        let credentials = options == null ? undefined : options.credentials;
        credentials = credentials === undefined ? 'same-origin' : StringC(credentials);
        if (credentials !== 'omit' && credentials !== 'same-origin' && credentials !== 'include') {
          throw new TypeErrorC(`Failed to execute 'addModule' on 'Worklet': Failed to read the 'credentials' property from 'WorkletOptions': The provided value '${credentials}' is not a valid enum value of type RequestCredentials.`);
        }
        const map = moduleMaps.get(this);
        const known = map.get(url);
        if (known) return known.then(() => undefined);
        // A module another graph imported (fetched, or being fetched): no new fetch; it settles as its
        // own graph completes (it and its imports fetched and evaluated, or one failed), not when its
        // source arrived. A module whose fetch failed left the map and is fetched as an entry.
        if (native.moduleState(url) !== 'none') {
          const graph = native.awaitGraph(url);
          // An engine that cannot wait for a module's own graph: the graph that fetched it, which
          // contains its graph, rejection included.
          const later = graph === null ? (runs.get(native.moduleGraph(url)) ?? graphs).then(() => undefined) : graph;
          map.set(url, later);
          later.then(undefined, () => { if (map.get(url) === later) map.delete(url); });
          return later;
        }
        return fetchEntry();
        function fetchEntry() {
          // "fetch a worklet script graph": a failed fetch (network error, not ok) rejects with AbortError.
          const aborted = () => new DOMExceptionC('Unable to load a worklet\'s module.', 'AbortError');
          // Evaluated under the response's URL (after redirects), which import.meta.url and the module's
          // relative imports use.
          const loading = native.fetch(url, { credentials, mode: 'cors' }).then(
            (response) => {
              if (!response.ok) throw aborted();
              return response.text().then((source) => [source, response.url || url]);
            },
            () => { throw aborted(); },
          ).then(([source, finalUrl]) => {
            const id = nextGraph++;
            const run = graphs.then(() => native.evaluate(source, finalUrl, credentials, id));
            runs.set(id, run);
            run.then(undefined, () => {});
            graphs = run.then(() => {}, () => {});
            return run;
          });
          map.set(url, loading);
          loading.then(undefined, () => { if (map.get(url) === loading) map.delete(url); });
          return loading;
        }
      } catch (e) {
        return reject(e);
      }
    }
  }
  O.defineProperty(Worklet.prototype, Symbol.toStringTag, { value: 'Worklet', configurable: true });
  O.defineProperty(Worklet.prototype, 'addModule', { enumerable: true });
  return { Worklet, paintWorklet: new Worklet(token) };
});
