// CSS.paintWorklet (CSS Painting API) and the Worklet interface for one realm. addModule() fetches
// the module and evaluates it in the page's paint worklet global (paint_worklet.rs), where
// registerPaint() validates and records paint definitions. pyro does not draw paint() images: a
// registered painter is never invoked. natives:
//   active() -> whether the document is fully active
//   evaluate(source, url, credentials) -> promise of the module's evaluation in the worklet global,
//     its imports fetched with `credentials`; one graph at a time (see `graphs`)
(function (native) {
  'use strict';
  const token = Symbol('pyro');
  const O = Object;
  const PromiseC = Promise;
  const DOMExceptionC = DOMException;
  const TypeErrorC = TypeError;
  const URLC = URL;
  const StringC = String;
  const fetchC = fetch;
  const reject = (e) => PromiseC.reject(e);
  const worklets = new WeakSet();
  // The module graphs being fetched and evaluated run one after another: each installs a fetcher
  // carrying its own credentials mode, which must not serve another graph's imports.
  let graphs = PromiseC.resolve();
  class Worklet {
    constructor(t) {
      if (t !== token) throw new TypeErrorC("Failed to construct 'Worklet': Illegal constructor");
      worklets.add(this);
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
        // "fetch a worklet script graph": a failed fetch (network error, not ok) rejects with AbortError.
        const aborted = () => new DOMExceptionC('Unable to load a worklet\'s module.', 'AbortError');
        // Evaluated under the response's URL (after redirects), which import.meta.url and the module's
        // relative imports use.
        return fetchC(url, { credentials, mode: 'cors' }).then(
          (response) => {
            if (!response.ok) throw aborted();
            return response.text().then((source) => [source, response.url || url]);
          },
          () => { throw aborted(); },
        ).then(([source, finalUrl]) => {
          const run = graphs.then(() => native.evaluate(source, finalUrl, credentials));
          graphs = run.then(() => {}, () => {});
          return run;
        });
      } catch (e) {
        return reject(e);
      }
    }
  }
  O.defineProperty(Worklet.prototype, Symbol.toStringTag, { value: 'Worklet', configurable: true });
  O.defineProperty(Worklet.prototype, 'addModule', { enumerable: true });
  return { Worklet, paintWorklet: new Worklet(token) };
});
