// The HTML Sanitizer API's `Sanitizer` (https://wicg.github.io/sanitizer-api/), made on a window's
// first use (sanitizer.rs). A Sanitizer's only state is its canonical configuration, a plain
// dictionary in which every element, attribute and processing instruction is an object of its
// own: `elements` or `removeElements`, `attributes` or `removeAttributes`,
// `processingInstructions` or `removeProcessingInstructions`, `comments`, `javascriptURLs` and,
// with `attributes`, `dataAttributes`. `native.defaultConfig()` is the built-in safe default
// configuration as JSON (the same table the parser's `safe_default()` filter is made from) and
// `native.eventHandlers()` the event handler content attribute names the built-in safe baseline
// removes. The safe methods ask `resolve(value, safe)` for the canonical configuration of their
// `sanitizer` option; native code compiles that dictionary into the parser's filter.
(native => {
  'use strict';
  const XHTML = 'http://www.w3.org/1999/xhtml';
  const SVG = 'http://www.w3.org/2000/svg';
  const MATHML = 'http://www.w3.org/1998/Math/MathML';
  const defineProperty = Object.defineProperty;
  const apply = Reflect.apply;

  // The configuration of each Sanitizer ([[configuration]]).
  const slots = new WeakMap();

  const isObject = v => v !== null && (typeof v === 'object' || typeof v === 'function');
  // WebIDL DOMString conversion (a template literal throws on a Symbol, as ToString does).
  const str = v => `${v}`;

  // WebIDL sequence<T> conversion: the iterator protocol over an object.
  function sequence(value, what) {
    if (!isObject(value)) throw new TypeError(`${what} is not a sequence.`);
    const method = value[Symbol.iterator];
    if (typeof method !== 'function') throw new TypeError(`${what} is not iterable.`);
    const iterator = apply(method, value, []);
    if (!isObject(iterator)) throw new TypeError(`${what}'s iterator is not an object.`);
    const next = iterator.next;
    const items = [];
    for (;;) {
      const step = apply(next, iterator, []);
      if (!isObject(step)) throw new TypeError(`${what}'s iterator result is not an object.`);
      if (step.done) return items;
      items.push(step.value);
    }
  }

  // "canonicalize a sanitizer name": `""` is no namespace.
  function namespaceOf(value, absent) {
    if (value === undefined) return absent;
    if (value === null) return null;
    const ns = str(value);
    return ns === '' ? null : ns;
  }
  // SanitizerElement (a DOMString or {name, namespace = XHTML}) and, with `withAttributes`,
  // SanitizerElementWithAttributes ({attributes, removeAttributes} too).
  function element(item, withAttributes) {
    if (isObject(item)) {
      const name = item.name;
      if (name === undefined) throw new TypeError('A sanitizer element needs a name.');
      const out = { name: str(name), namespace: namespaceOf(item.namespace, XHTML) };
      if (withAttributes) {
        const attributes = item.attributes;
        if (attributes !== undefined) out.attributes = sequence(attributes, 'attributes').map(attribute);
        const removeAttributes = item.removeAttributes;
        if (removeAttributes !== undefined) out.removeAttributes = sequence(removeAttributes, 'removeAttributes').map(attribute);
      }
      return out;
    }
    if (item === null || item === undefined) throw new TypeError('A sanitizer element needs a name.');
    return { name: str(item), namespace: XHTML };
  }
  // SanitizerAttribute: a DOMString or {name, namespace = null}.
  function attribute(item) {
    if (isObject(item)) {
      const name = item.name;
      if (name === undefined) throw new TypeError('A sanitizer attribute needs a name.');
      return { name: str(name), namespace: namespaceOf(item.namespace, null) };
    }
    if (item === null || item === undefined) throw new TypeError('A sanitizer attribute needs a name.');
    return { name: str(item), namespace: null };
  }
  // SanitizerProcessingInstruction: a DOMString or {target}.
  function instruction(item) {
    if (isObject(item)) {
      const target = item.target;
      if (target === undefined) throw new TypeError('A sanitizer processing instruction needs a target.');
      return { target: str(target) };
    }
    if (item === null || item === undefined) throw new TypeError('A sanitizer processing instruction needs a target.');
    return { target: str(item) };
  }
  // An `elements` entry always says what it does with attributes.
  function withAttributesDefault(e) {
    if (e.attributes === undefined && e.removeAttributes === undefined) e.removeAttributes = [];
    return e;
  }

  const keyOf = n => (n.namespace === null ? 'N' : `S${n.namespace}`) + '|' + n.name;
  const same = (a, b) => keyOf(a) === keyOf(b);
  const has = (list, n) => list.some(o => same(o, n));
  const hasTarget = (list, pi) => list.some(o => o.target === pi.target);
  function removeFrom(list, n) {
    const i = list.findIndex(o => same(o, n));
    if (i < 0) return false;
    list.splice(i, 1);
    return true;
  }
  function dedupe(list) {
    const seen = new Set();
    return list.filter(n => {
      const k = keyOf(n);
      if (seen.has(k)) return false;
      seen.add(k);
      return true;
    });
  }
  const hasDuplicates = list => new Set(list.map(keyOf)).size !== list.length;
  const intersects = (a, b) => a !== undefined && b !== undefined && a.some(n => has(b, n));
  const isSubset = (a, b) => a === undefined || a.every(n => has(b, n));
  const isData = a => a.namespace === null && a.name.startsWith('data-');
  const nonReplaceable = e =>
    (e.namespace === XHTML && e.name === 'html') || (e.namespace === SVG && e.name === 'svg') || (e.namespace === MATHML && e.name === 'math');

  // Canonicalizes a SanitizerConfig dictionary. WebIDL reads the members in code point order.
  // `allow`: what a missing `comments`, `dataAttributes` and processing instruction list
  // default to (true for the constructor and the unsafe methods, false for the safe ones).
  function canonicalize(config, allow, seen) {
    const c = {};
    let dataAttributes;
    const bool = v => (v === undefined ? undefined : !!v);
    if (isObject(config)) {
      const member = key => {
        const v = config[key];
        if (v !== undefined) seen.any = true;
        return v;
      };
      const attributes = member('attributes');
      if (attributes !== undefined) c.attributes = sequence(attributes, 'attributes').map(attribute);
      const comments = member('comments');
      if (comments !== undefined) c.comments = !!comments;
      dataAttributes = bool(member('dataAttributes'));
      const elements = member('elements');
      if (elements !== undefined) c.elements = sequence(elements, 'elements').map(e => withAttributesDefault(element(e, true)));
      const javascriptURLs = member('javascriptURLs');
      if (javascriptURLs !== undefined) c.javascriptURLs = !!javascriptURLs;
      const processingInstructions = member('processingInstructions');
      if (processingInstructions !== undefined) c.processingInstructions = sequence(processingInstructions, 'processingInstructions').map(instruction);
      const removeAttributes = member('removeAttributes');
      if (removeAttributes !== undefined) c.removeAttributes = sequence(removeAttributes, 'removeAttributes').map(attribute);
      const removeElements = member('removeElements');
      if (removeElements !== undefined) c.removeElements = sequence(removeElements, 'removeElements').map(e => element(e, false));
      const removeProcessingInstructions = member('removeProcessingInstructions');
      if (removeProcessingInstructions !== undefined) {
        c.removeProcessingInstructions = sequence(removeProcessingInstructions, 'removeProcessingInstructions').map(instruction);
      }
      const replace = member('replaceWithChildrenElements');
      if (replace !== undefined) c.replaceWithChildrenElements = sequence(replace, 'replaceWithChildrenElements').map(e => element(e, false));
    }
    if (dataAttributes !== undefined) c.dataAttributes = dataAttributes;
    if (!('elements' in c) && !('removeElements' in c)) c.removeElements = [];
    if (!('processingInstructions' in c) && !('removeProcessingInstructions' in c)) {
      if (allow) c.removeProcessingInstructions = [];
      else c.processingInstructions = [];
    }
    if (!('attributes' in c) && !('removeAttributes' in c)) {
      c.removeAttributes = [];
    }
    if (!('comments' in c)) c.comments = allow;
    if ('attributes' in c && !('dataAttributes' in c)) c.dataAttributes = allow;
    if (!('javascriptURLs' in c)) c.javascriptURLs = true;
    return c;
  }

  // "valid" (the Sanitizer API's SanitizerConfig validity check).
  function isValid(c) {
    if ('elements' in c && 'removeElements' in c) return false;
    if ('processingInstructions' in c && 'removeProcessingInstructions' in c) return false;
    if ('attributes' in c && 'removeAttributes' in c) return false;
    if (hasDuplicates('elements' in c ? c.elements : c.removeElements)) return false;
    const replace = c.replaceWithChildrenElements;
    if (replace !== undefined) {
      if (hasDuplicates(replace)) return false;
      if (replace.some(nonReplaceable)) return false;
    }
    const targets = 'processingInstructions' in c ? c.processingInstructions : c.removeProcessingInstructions;
    if (new Set(targets.map(pi => pi.target)).size !== targets.length) return false;
    if (hasDuplicates('attributes' in c ? c.attributes : c.removeAttributes)) return false;
    if (replace !== undefined && intersects('elements' in c ? c.elements : c.removeElements, replace)) return false;
    const elements = c.elements ?? [];
    if ('attributes' in c) {
      if (!('dataAttributes' in c)) return false;
      for (const e of elements) {
        if (e.attributes !== undefined && hasDuplicates(e.attributes)) return false;
        if (e.removeAttributes !== undefined && hasDuplicates(e.removeAttributes)) return false;
        if (intersects(c.attributes, e.attributes)) return false;
        if (!isSubset(e.removeAttributes, c.attributes)) return false;
        if (c.dataAttributes && e.attributes !== undefined && e.attributes.some(isData)) return false;
      }
      if (c.dataAttributes && c.attributes.some(isData)) return false;
    } else {
      for (const e of elements) {
        if (e.attributes !== undefined && e.removeAttributes !== undefined) return false;
        if (e.attributes !== undefined && hasDuplicates(e.attributes)) return false;
        if (e.removeAttributes !== undefined && hasDuplicates(e.removeAttributes)) return false;
        if (intersects(c.removeAttributes, e.attributes)) return false;
        if (intersects(c.removeAttributes, e.removeAttributes)) return false;
      }
      if ('dataAttributes' in c) return false;
    }
    return true;
  }

  function fromConfig(config, allow, seen) {
    const c = canonicalize(config, allow, seen);
    if (!isValid(c)) throw new TypeError('The sanitizer configuration is invalid.');
    return c;
  }

  const copyList = list => list.map(n => ({ ...n }));
  function copyElement(e) {
    const out = { name: e.name, namespace: e.namespace };
    if (e.attributes !== undefined) out.attributes = copyList(e.attributes);
    if (e.removeAttributes !== undefined) out.removeAttributes = copyList(e.removeAttributes);
    return out;
  }
  // A deep copy, keys in the dictionary's order.
  function clone(c) {
    const out = {};
    if ('elements' in c) out.elements = c.elements.map(copyElement);
    if ('removeElements' in c) out.removeElements = copyList(c.removeElements);
    if ('replaceWithChildrenElements' in c) out.replaceWithChildrenElements = copyList(c.replaceWithChildrenElements);
    if ('attributes' in c) out.attributes = copyList(c.attributes);
    if ('removeAttributes' in c) out.removeAttributes = copyList(c.removeAttributes);
    if ('processingInstructions' in c) out.processingInstructions = copyList(c.processingInstructions);
    if ('removeProcessingInstructions' in c) out.removeProcessingInstructions = copyList(c.removeProcessingInstructions);
    out.comments = c.comments;
    if ('dataAttributes' in c) out.dataAttributes = c.dataAttributes;
    out.javascriptURLs = c.javascriptURLs;
    return out;
  }

  // The built-in safe default configuration: native code writes the table `safe_default()` (pyro-dom's
  // sanitizer_defaults.rs, WPT sanitizer-api/sanitizer-default-config.html) is made from as JSON.
  let defaultConfiguration;
  const parseJSON = JSON.parse;
  const defaults = () => (defaultConfiguration ??= parseJSON(native.defaultConfig()));

  // The configuration a `sanitizer` option or constructor argument stands for.
  // `seen.any` tells whether the value said anything: a Sanitizer, a preset or a dictionary with a member.
  function resolveValue(value, allow, instances, seen) {
    if (value === undefined) return clone(defaults());
    if (isObject(value)) {
      const slot = instances ? slots.get(value) : undefined;
      if (slot !== undefined) {
        seen.any = true;
        return clone(slot);
      }
      return fromConfig(value, allow, seen);
    }
    if (value === null) return fromConfig(null, allow, seen);
    const preset = str(value);
    if (preset !== 'default') throw new TypeError(`'${preset}' is not a valid SanitizerPresets value.`);
    seen.any = true;
    return clone(defaults());
  }

  // A copy native code reads: no prototype to inherit members from.
  function bare(x) {
    if (Array.isArray(x)) return x.map(bare);
    if (!isObject(x)) return x;
    const out = { __proto__: null };
    for (const key of Object.keys(x)) out[key] = bare(x[key]);
    return out;
  }

  // "get" sorts by namespace (none first), then name.
  const compareNames = (a, b) => {
    if (a.namespace !== b.namespace) {
      if (a.namespace === null) return -1;
      if (b.namespace === null) return 1;
      return a.namespace < b.namespace ? -1 : 1;
    }
    return a.name < b.name ? -1 : a.name > b.name ? 1 : 0;
  };
  const compareTargets = (a, b) => (a.target < b.target ? -1 : a.target > b.target ? 1 : 0);
  function sorted(c) {
    const out = clone(c);
    for (const key of ['removeElements', 'replaceWithChildrenElements', 'attributes', 'removeAttributes']) {
      if (key in out) out[key].sort(compareNames);
    }
    if ('elements' in out) {
      out.elements.sort(compareNames);
      for (const e of out.elements) {
        if (e.attributes !== undefined) e.attributes.sort(compareNames);
        if (e.removeAttributes !== undefined) e.removeAttributes.sort(compareNames);
      }
    }
    for (const key of ['processingInstructions', 'removeProcessingInstructions']) {
      if (key in out) out[key].sort(compareTargets);
    }
    return out;
  }

  // -- the modifiers: each returns whether the configuration changed -------------------------

  function allowAttribute(c, a) {
    if ('attributes' in c) {
      if (has(c.attributes, a)) return false;
      if (c.dataAttributes && isData(a)) return false;
      for (const e of c.elements ?? []) if (e.attributes !== undefined) removeFrom(e.attributes, a);
      c.attributes.push(a);
      return true;
    }
    return removeFrom(c.removeAttributes, a);
  }
  function removeAttribute(c, a) {
    const elements = c.elements ?? [];
    if ('attributes' in c) {
      let modified = false;
      if (removeFrom(c.attributes, a)) {
        modified = true;
        // An element's removeAttributes stay a subset of the global allow-list.
        for (const e of elements) if (e.removeAttributes !== undefined) removeFrom(e.removeAttributes, a);
      }
      for (const e of elements) if (e.attributes !== undefined && removeFrom(e.attributes, a)) modified = true;
      return modified;
    }
    if (has(c.removeAttributes, a)) return false;
    c.removeAttributes.push(a);
    for (const e of elements) {
      if (e.attributes !== undefined) removeFrom(e.attributes, a);
      if (e.removeAttributes !== undefined) removeFrom(e.removeAttributes, a);
    }
    return true;
  }
  // An `allowElement` entry made to satisfy the configuration's global attribute rules.
  function fitElement(c, e) {
    let attributes = e.attributes === undefined ? undefined : dedupe(e.attributes);
    let removeAttributes = e.removeAttributes === undefined ? undefined : dedupe(e.removeAttributes);
    if ('attributes' in c) {
      if (attributes !== undefined) {
        attributes = attributes.filter(a => !has(c.attributes, a));
        if (c.dataAttributes) attributes = attributes.filter(a => !isData(a));
      }
      if (removeAttributes !== undefined) removeAttributes = removeAttributes.filter(a => has(c.attributes, a));
    } else if (attributes !== undefined) {
      attributes = attributes.filter(a => !has(c.removeAttributes, a) && !(removeAttributes !== undefined && has(removeAttributes, a)));
      removeAttributes = undefined;
    } else if (removeAttributes !== undefined) {
      removeAttributes = removeAttributes.filter(a => !has(c.removeAttributes, a));
    }
    const out = { name: e.name, namespace: e.namespace };
    if (attributes !== undefined) out.attributes = attributes;
    if (removeAttributes !== undefined) out.removeAttributes = removeAttributes;
    return withAttributesDefault(out);
  }
  const sameSet = (a, b) => (a === undefined || b === undefined ? a === b : a.length === b.length && a.every(n => has(b, n)));
  const sameEntry = (a, b) => sameSet(a.attributes, b.attributes) && sameSet(a.removeAttributes, b.removeAttributes);

  function allowElement(c, input) {
    if ('elements' in c) {
      const entry = fitElement(c, input);
      let modified = false;
      if (c.replaceWithChildrenElements !== undefined && removeFrom(c.replaceWithChildrenElements, entry)) modified = true;
      const i = c.elements.findIndex(o => same(o, entry));
      if (i < 0) {
        c.elements.push(entry);
        modified = true;
      } else if (!sameEntry(c.elements[i], entry)) {
        c.elements[i] = entry;
        modified = true;
      }
      return modified;
    }
    // Everything not removed is allowed already: per-element attribute rules cannot be set.
    if (input.attributes !== undefined || (input.removeAttributes !== undefined && input.removeAttributes.length > 0)) return false;
    let modified = false;
    if (c.replaceWithChildrenElements !== undefined && removeFrom(c.replaceWithChildrenElements, input)) modified = true;
    if (removeFrom(c.removeElements, input)) modified = true;
    return modified;
  }
  function removeElement(c, e) {
    let modified = false;
    if (c.replaceWithChildrenElements !== undefined && removeFrom(c.replaceWithChildrenElements, e)) modified = true;
    if ('elements' in c) {
      if (removeFrom(c.elements, e)) modified = true;
    } else if (!has(c.removeElements, e)) {
      c.removeElements.push(e);
      modified = true;
    }
    return modified;
  }
  function replaceElementWithChildren(c, e) {
    if (nonReplaceable(e)) return false;
    if (c.replaceWithChildrenElements !== undefined && has(c.replaceWithChildrenElements, e)) return false;
    removeFrom('elements' in c ? c.elements : c.removeElements, e);
    (c.replaceWithChildrenElements ??= []).push({ name: e.name, namespace: e.namespace });
    return true;
  }
  function setBoolean(c, key, value) {
    if (c[key] === value) return false;
    c[key] = value;
    return true;
  }

  const BASELINE_ELEMENTS = [
    ['base', XHTML], ['embed', XHTML], ['frame', XHTML], ['iframe', XHTML], ['object', XHTML], ['script', XHTML], ['script', SVG], ['use', SVG],
  ];
  // "remove unsafe": merges the built-in safe baseline configuration.
  function removeUnsafe(c) {
    let modified = false;
    for (const [name, namespace] of BASELINE_ELEMENTS) if (removeElement(c, { name, namespace })) modified = true;
    for (const name of native.eventHandlers()) if (removeAttribute(c, { name, namespace: null })) modified = true;
    if (setBoolean(c, 'javascriptURLs', false)) modified = true;
    return modified;
  }

  function slotOf(self, what) {
    const c = slots.get(self);
    if (c === undefined) throw new TypeError(`Failed to execute '${what}' on 'Sanitizer': Illegal invocation`);
    return c;
  }
  function need(args, what) {
    if (args.length < 1) throw new TypeError(`Failed to execute '${what}' on 'Sanitizer': 1 argument required, but only 0 present.`);
  }

  class Sanitizer {
    constructor(configuration = 'default') {
      slots.set(this, resolveValue(configuration, true, false, { any: false }));
    }
    get() { return sorted(slotOf(this, 'get')); }
    allowElement(element_) {
      need(arguments, 'allowElement');
      const c = slotOf(this, 'allowElement');
      return allowElement(c, element(element_, true));
    }
    removeElement(element_) {
      need(arguments, 'removeElement');
      const c = slotOf(this, 'removeElement');
      return removeElement(c, element(element_, false));
    }
    replaceElementWithChildren(element_) {
      need(arguments, 'replaceElementWithChildren');
      const c = slotOf(this, 'replaceElementWithChildren');
      return replaceElementWithChildren(c, element(element_, false));
    }
    allowAttribute(attribute_) {
      need(arguments, 'allowAttribute');
      const c = slotOf(this, 'allowAttribute');
      return allowAttribute(c, attribute(attribute_));
    }
    removeAttribute(attribute_) {
      need(arguments, 'removeAttribute');
      const c = slotOf(this, 'removeAttribute');
      return removeAttribute(c, attribute(attribute_));
    }
    setComments(allow) {
      need(arguments, 'setComments');
      return setBoolean(slotOf(this, 'setComments'), 'comments', !!allow);
    }
    setDataAttributes(allow) {
      need(arguments, 'setDataAttributes');
      const c = slotOf(this, 'setDataAttributes');
      const value = !!allow;
      // Without an attribute allow-list every attribute not removed passes: nothing to set.
      if (!('attributes' in c) || c.dataAttributes === value) return false;
      c.dataAttributes = value;
      if (value) {
        // The allow-lists name no custom data attribute while all of them are allowed.
        c.attributes = c.attributes.filter(a => !isData(a));
        for (const e of c.elements ?? []) if (e.attributes !== undefined) e.attributes = e.attributes.filter(a => !isData(a));
      }
      return true;
    }
    setJavascriptURLs(allow) {
      need(arguments, 'setJavascriptURLs');
      return setBoolean(slotOf(this, 'setJavascriptURLs'), 'javascriptURLs', !!allow);
    }
    allowProcessingInstruction(pi) {
      need(arguments, 'allowProcessingInstruction');
      const c = slotOf(this, 'allowProcessingInstruction');
      const target = instruction(pi);
      if ('processingInstructions' in c) {
        if (hasTarget(c.processingInstructions, target)) return false;
        c.processingInstructions.push(target);
        return true;
      }
      const i = c.removeProcessingInstructions.findIndex(o => o.target === target.target);
      if (i < 0) return false;
      c.removeProcessingInstructions.splice(i, 1);
      return true;
    }
    removeProcessingInstruction(pi) {
      need(arguments, 'removeProcessingInstruction');
      const c = slotOf(this, 'removeProcessingInstruction');
      const target = instruction(pi);
      if ('processingInstructions' in c) {
        const i = c.processingInstructions.findIndex(o => o.target === target.target);
        if (i < 0) return false;
        c.processingInstructions.splice(i, 1);
        return true;
      }
      if (hasTarget(c.removeProcessingInstructions, target)) return false;
      c.removeProcessingInstructions.push(target);
      return true;
    }
    removeUnsafe() { return removeUnsafe(slotOf(this, 'removeUnsafe')); }
  }
  // Web IDL operations are enumerable.
  for (const name of Object.getOwnPropertyNames(Sanitizer.prototype)) {
    if (name !== 'constructor') defineProperty(Sanitizer.prototype, name, { enumerable: true });
  }
  defineProperty(Sanitizer.prototype, Symbol.toStringTag, { value: 'Sanitizer', configurable: true });

  return {
    Sanitizer,
    // The canonical configuration of a `sanitizer` option value, as `{ config, nonempty }` (a
    // prototype-less dictionary; `nonempty`: the value was no empty dictionary, so its use needs
    // vetting); `safe`: for a safe method (a dictionary's missing booleans and processing
    // instruction list default to false) rather than an unsafe one. Throws the constructor's TypeErrors.
    resolve(value, safe) {
      const seen = { any: false };
      const config = resolveValue(value, !safe, true, seen);
      return bare({ config, nonempty: seen.any });
    },
  };
})
