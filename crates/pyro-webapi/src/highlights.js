// CSS Custom Highlight API 1: `Highlight` (a setlike of ranges) and `CSS.highlights` (the
// realm's `HighlightRegistry`, a maplike of names to highlights). Made on a realm's first use of
// either (highlights.rs). Every change that can alter what is painted hands the registry to
// `natives.sync` as `[name, priority, ranges]` entries in registration order.
//
// Both are plain interfaces (their prototypes inherit from Object.prototype, as WebIDL's setlike and
// maplike declarations give), holding their Set and Map in private fields and working through the
// realm's pristine Set and Map operations (`natives.primitives`, captured with the realm's
// intrinsics: page script may have replaced `Set.prototype.add` by the time this runs).
(function (natives) {
  'use strict';
  const P = natives.primitives;
  const defineProperty = P.defineProperty;
  const getOwnPropertyDescriptor = P.getOwnPropertyDescriptor;
  let registry = null;
  // Set below from inside the classes: the entries the registry hands to the engine.
  let snapshotOf = null;
  let isHighlight = null;
  let entriesOf = null;

  function sync() {
    if (registry === null) return;
    const entries = [];
    for (const [name, highlight] of entriesOf(registry)) {
      const [priority, ranges] = snapshotOf(highlight);
      entries.push([name, priority, ranges]);
    }
    natives.sync(entries);
  }

  function range(value) {
    if (!natives.isRange(value)) {
      throw new TypeError("Failed to execute 'add' on 'Highlight': parameter 1 is not of type 'AbstractRange'.");
    }
    return value;
  }

  function callable(value, name, interfaceName) {
    if (typeof value !== 'function') {
      throw new TypeError(`Failed to execute '${name}' on '${interfaceName}': parameter 1 is not of type 'Function'.`);
    }
    return value;
  }

  // WebIDL operations and attributes are enumerable (class members are not).
  function expose(proto, names) {
    for (const name of names) {
      const descriptor = getOwnPropertyDescriptor(proto, name);
      descriptor.enumerable = true;
      defineProperty(proto, name, descriptor);
    }
  }

  class Highlight {
    #ranges = P.newSet();
    #priority = 0;
    #type = 'highlight';

    static {
      snapshotOf = (highlight) => [highlight.#priority, [...P.setValues(highlight.#ranges)]];
      isHighlight = (value) => (typeof value === 'object' || typeof value === 'function') && value !== null && #ranges in value;
    }

    static #of(value) {
      if (!((typeof value === 'object' || typeof value === 'function') && value !== null && #ranges in value)) {
        throw new TypeError('Illegal invocation');
      }
      return value;
    }

    constructor(...initialRanges) {
      for (const r of initialRanges) P.setAdd(this.#ranges, range(r));
    }
    get priority() { return Highlight.#of(this).#priority; }
    set priority(value) {
      const self = Highlight.#of(this);
      self.#priority = Number(value) | 0;
      sync();
    }
    get type() { return Highlight.#of(this).#type; }
    set type(value) {
      const self = Highlight.#of(this);
      value = `${value}`;
      if (value === 'highlight' || value === 'spelling-error' || value === 'grammar-error') self.#type = value;
    }
    get size() { return P.setSize(Highlight.#of(this).#ranges); }
    has(value) { return P.setHas(Highlight.#of(this).#ranges, value); }
    add(value) {
      const self = Highlight.#of(this);
      P.setAdd(self.#ranges, range(value));
      sync();
      return this;
    }
    delete(value) {
      const deleted = P.setDelete(Highlight.#of(this).#ranges, value);
      sync();
      return deleted;
    }
    clear() {
      P.setClear(Highlight.#of(this).#ranges);
      sync();
    }
    entries() { return P.setEntries(Highlight.#of(this).#ranges); }
    keys() { return P.setValues(Highlight.#of(this).#ranges); }
    values() { return P.setValues(Highlight.#of(this).#ranges); }
    forEach(callback) {
      const self = Highlight.#of(this);
      callable(callback, 'forEach', 'Highlight');
      const thisArg = arguments[1];
      for (const value of P.setValues(self.#ranges)) callback.call(thisArg, value, value, this);
    }
  }
  expose(Highlight.prototype, ['priority', 'type', 'size', 'has', 'add', 'delete', 'clear', 'entries', 'keys', 'values', 'forEach']);
  defineProperty(Highlight.prototype, Symbol.iterator, {
    value: Highlight.prototype.values, writable: true, enumerable: false, configurable: true,
  });
  defineProperty(Highlight.prototype, Symbol.toStringTag, { value: 'Highlight', configurable: true });

  // Whether `value` (a Range, a StaticRange) with its endpoints in the document or in a shadow tree
  // of `allowed` is a live range to test, as a Range of this realm; `null` for one that is not
  // valid (a StaticRange whose container shrank, or that ends before it starts) or not shown.
  function rangeOf(value, document, allowed) {
    let live;
    if (typeof value.getClientRects === 'function') {
      live = value;
    } else {
      const sc = value.startContainer, ec = value.endContainer;
      if (!sc || !ec) return null;
      const length = (node) => (node.nodeType === 3 || node.nodeType === 4 || node.nodeType === 7 || node.nodeType === 8 ? node.length : node.childNodes.length);
      if (sc.nodeType === 10 || ec.nodeType === 10 || sc.nodeType === 2 || ec.nodeType === 2) return null;
      if (value.startOffset > length(sc) || value.endOffset > length(ec)) return null;
      if (sc.getRootNode() !== ec.getRootNode()) return null;
      try {
        live = document.createRange();
        live.setStart(sc, value.startOffset);
        live.setEnd(ec, value.endOffset);
      } catch (_) {
        return null;
      }
      // An end before the start collapsed the range to the end: not what the StaticRange holds.
      if (live.startContainer !== sc || live.startOffset !== value.startOffset) return null;
    }
    if (live.collapsed) return null;
    const root = live.startContainer.getRootNode();
    if (root !== document) {
      let shown = false;
      for (const shadow of allowed) if (shadow === root) shown = true;
      if (!shown) return null;
    }
    return live;
  }

  // Whether the point (`x`, `y`) is on the text `live` covers: in the box of one of its characters.
  function hitsText(live, document, x, y) {
    const holds = (node) => {
      const start = node === live.startContainer ? live.startOffset : 0;
      const end = node === live.endContainer ? live.endOffset : node.length;
      if (end <= start) return false;
      const part = document.createRange();
      part.setStart(node, start);
      part.setEnd(node, end);
      for (const r of part.getClientRects()) {
        if (x >= r.left && x <= r.right && y >= r.top && y <= r.bottom) return true;
      }
      return false;
    };
    const common = live.commonAncestorContainer;
    if (common.nodeType === 3 || common.nodeType === 4) return holds(common);
    const walker = document.createTreeWalker(common, 4);
    for (let node = walker.nextNode(); node !== null; node = walker.nextNode()) {
      if (live.intersectsNode(node) && holds(node)) return true;
    }
    return false;
  }

  function float(value, name) {
    const number = Number(value);
    if (!Number.isFinite(number)) {
      throw new TypeError(`Failed to execute 'highlightsFromPoint' on 'HighlightRegistry': The provided float value for ${name} is non-finite.`);
    }
    return number;
  }

  class HighlightRegistry {
    #map = P.newMap();

    static #of(value) {
      if (!((typeof value === 'object' || typeof value === 'function') && value !== null && #map in value)) {
        throw new TypeError('Illegal invocation');
      }
      return value;
    }

    static {
      // The registry's entries as `sync` reads them, in registration order.
      entriesOf = (r) => P.mapEntries(r.#map);
    }

    constructor() {
      if (registry !== null) throw new TypeError('Illegal constructor');
    }
    get size() { return P.mapSize(HighlightRegistry.#of(this).#map); }
    get(name) { return P.mapGet(HighlightRegistry.#of(this).#map, `${name}`); }
    has(name) { return P.mapHas(HighlightRegistry.#of(this).#map, `${name}`); }
    set(name, highlight) {
      const self = HighlightRegistry.#of(this);
      if (!isHighlight(highlight)) {
        throw new TypeError("Failed to execute 'set' on 'HighlightRegistry': parameter 2 is not of type 'Highlight'.");
      }
      P.mapSet(self.#map, `${name}`, highlight);
      sync();
      return this;
    }
    delete(name) {
      const deleted = P.mapDelete(HighlightRegistry.#of(this).#map, `${name}`);
      sync();
      return deleted;
    }
    clear() {
      P.mapClear(HighlightRegistry.#of(this).#map);
      sync();
    }
    entries() { return P.mapEntries(HighlightRegistry.#of(this).#map); }
    keys() { return P.mapKeys(HighlightRegistry.#of(this).#map); }
    values() { return P.mapValues(HighlightRegistry.#of(this).#map); }
    forEach(callback) {
      const self = HighlightRegistry.#of(this);
      callable(callback, 'forEach', 'HighlightRegistry');
      const thisArg = arguments[1];
      for (const [name, highlight] of P.mapEntries(self.#map)) callback.call(thisArg, highlight, name, this);
    }
    // The highlights (and the ranges of each) at a viewport point, topmost first: higher priority
    // first, and of equal priority the one registered last. A range holds the point when it is
    // on text the range covers. Ranges in a shadow tree count when the options list its root.
    highlightsFromPoint(x, y) {
      const self = HighlightRegistry.#of(this);
      if (arguments.length < 2) {
        throw new TypeError(`Failed to execute 'highlightsFromPoint' on 'HighlightRegistry': 2 arguments required, but only ${arguments.length} present.`);
      }
      x = float(x, 'x');
      y = float(y, 'y');
      const options = arguments[2];
      const allowed = [];
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
            allowed.push(root);
          }
        }
      }
      const document = globalThis.document;
      const html = document.documentElement;
      if (html === null || x < 0 || y < 0 || x >= html.clientWidth || y >= html.clientHeight) return [];
      const found = [];
      let order = 0;
      for (const [, highlight] of P.mapEntries(self.#map)) {
        const [priority, ranges] = snapshotOf(highlight);
        const hit = [];
        for (const r of ranges) {
          const live = rangeOf(r, document, allowed);
          if (live !== null && hitsText(live, document, x, y)) hit.push(r);
        }
        if (hit.length > 0) found.push({ highlight, ranges: hit, priority, order: order++ });
      }
      found.sort((a, b) => b.priority - a.priority || b.order - a.order);
      return found.map(({ highlight, ranges }) => ({ highlight, ranges }));
    }
  }
  expose(HighlightRegistry.prototype, ['size', 'get', 'has', 'set', 'delete', 'clear', 'entries', 'keys', 'values', 'forEach', 'highlightsFromPoint']);
  defineProperty(HighlightRegistry.prototype, Symbol.iterator, {
    value: HighlightRegistry.prototype.entries, writable: true, enumerable: false, configurable: true,
  });
  defineProperty(HighlightRegistry.prototype, Symbol.toStringTag, { value: 'HighlightRegistry', configurable: true });
  registry = new HighlightRegistry();

  return { Highlight, HighlightRegistry, highlights: registry };
});
