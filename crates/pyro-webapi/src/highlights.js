// CSS Custom Highlight API 1: `Highlight` (a setlike of ranges) and `CSS.highlights` (the
// realm's `HighlightRegistry`, a maplike of names to highlights). Made on a realm's first use of
// either (highlights.rs). Every change that can alter what is painted hands the registry to
// `natives.sync` as `[name, priority, ranges]` entries in registration order.
(function (natives) {
  'use strict';
  const setAdd = Set.prototype.add;
  const setDelete = Set.prototype.delete;
  const setClear = Set.prototype.clear;
  const setValues = Set.prototype.values;
  const mapSet = Map.prototype.set;
  const mapDelete = Map.prototype.delete;
  const mapClear = Map.prototype.clear;
  const mapEntries = Map.prototype.entries;
  let registry = null;
  const priorities = new WeakMap();
  const types = new WeakMap();

  function sync() {
    if (registry === null) return;
    const entries = [];
    for (const [name, highlight] of mapEntries.call(registry)) {
      entries.push([name, priorities.get(highlight) ?? 0, [...setValues.call(highlight)]]);
    }
    natives.sync(entries);
  }

  function range(value) {
    if (!natives.isRange(value)) {
      throw new TypeError("Failed to execute 'add' on 'Highlight': parameter 1 is not of type 'AbstractRange'.");
    }
    return value;
  }

  class Highlight extends Set {
    constructor(...ranges) {
      super();
      priorities.set(this, 0);
      types.set(this, 'highlight');
      for (const r of ranges) setAdd.call(this, range(r));
    }
    get priority() { return priorities.get(this); }
    set priority(value) {
      priorities.set(this, Number(value) | 0);
      sync();
    }
    get type() { return types.get(this); }
    set type(value) {
      value = String(value);
      if (value === 'highlight' || value === 'spelling-error' || value === 'grammar-error') types.set(this, value);
    }
    add(value) {
      setAdd.call(this, range(value));
      sync();
      return this;
    }
    delete(value) {
      const deleted = setDelete.call(this, value);
      sync();
      return deleted;
    }
    clear() {
      setClear.call(this);
      sync();
    }
  }
  Object.defineProperty(Highlight.prototype, Symbol.toStringTag, { value: 'Highlight', configurable: true });

  class HighlightRegistry extends Map {
    constructor() {
      if (registry !== null) throw new TypeError('Illegal constructor');
      super();
    }
    set(name, highlight) {
      if (!(highlight instanceof Highlight)) {
        throw new TypeError("Failed to execute 'set' on 'HighlightRegistry': parameter 2 is not of type 'Highlight'.");
      }
      mapSet.call(this, String(name), highlight);
      sync();
      return this;
    }
    delete(name) {
      const deleted = mapDelete.call(this, String(name));
      sync();
      return deleted;
    }
    clear() {
      mapClear.call(this);
      sync();
    }
    highlightsFromPoint(x, y, options) {
      // WebIDL: `x`, `y` are doubles; `options.shadowRoots` is a sequence<ShadowRoot>.
      Number(x);
      Number(y);
      if (options !== undefined && options !== null) {
        if (typeof options !== 'object' && typeof options !== 'function') {
          throw new TypeError("Failed to execute 'highlightsFromPoint': options is not an object.");
        }
        const roots = options.shadowRoots;
        if (roots !== undefined) {
          if (roots === null || typeof roots[Symbol.iterator] !== 'function') {
            throw new TypeError("Failed to execute 'highlightsFromPoint': shadowRoots is not a sequence.");
          }
          const SR = globalThis.ShadowRoot;
          for (const root of roots) {
            if (typeof SR !== 'function' || !(root instanceof SR)) {
              throw new TypeError("Failed to execute 'highlightsFromPoint': shadowRoots holds a value that is not a ShadowRoot.");
            }
          }
        }
      }
      return [];
    }
  }
  Object.defineProperty(HighlightRegistry.prototype, Symbol.toStringTag, { value: 'HighlightRegistry', configurable: true });
  registry = new HighlightRegistry();

  return { Highlight, HighlightRegistry, highlights: registry };
});
