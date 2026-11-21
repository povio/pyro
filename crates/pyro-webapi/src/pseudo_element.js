(native => {
  'use strict';
  const token = Symbol(), children = new WeakMap();
  const cleanup = new FinalizationRegistry(({cache, key, ref}) => {
    const map = cache.deref();
    if (map && map.get(key) === ref) map.delete(key);
  });
  function handle(element, parent, type, argument) {
    const key = argument === null ? type : type + '(' + argument + ')';
    let cache = children.get(parent);
    if (!cache) children.set(parent, cache = new Map());
    const existing = cache.get(key)?.deref();
    if (existing) return existing;
    const object = new CSSPseudoElement(token, element, parent, type, argument, key);
    const ref = new WeakRef(object);
    cache.set(key, ref);
    cleanup.register(object, {cache:new WeakRef(cache), key, ref});
    return object;
  }
  function logicalButton(el, dir) {
    if (dir.includes('-')) return dir;
    const style = getComputedStyle(el), mode = style.writingMode, rtl = style.direction === 'rtl';
    const horizontal = !mode || mode === 'horizontal-tb';
    const blockStart = horizontal ? 'up' : mode.endsWith('-rl') ? 'right' : 'left';
    const inlineStart = horizontal ? (rtl ? 'right' : 'left') :
      (rtl !== (mode === 'sideways-lr') ? 'down' : 'up');
    const opposite = {up:'down', down:'up', left:'right', right:'left'};
    if (dir === blockStart) return 'block-start';
    if (dir === opposite[blockStart]) return 'block-end';
    return dir === inlineStart ? 'inline-start' : 'inline-end';
  }
  function fromElement(element, selector) {
    const parsed = native.parse(selector);
    if (!parsed) return null;
    const [kind, arg] = parsed;
    if (kind === '::scroll-button') return handle(element, element, kind, logicalButton(element, arg));
    let parent = element;
    if (kind.startsWith('::view-transition-')) {
      parent = handle(element, parent, '::view-transition', null);
      if (kind !== '::view-transition-group') parent = handle(element, parent, '::view-transition-group', arg);
      if (kind === '::view-transition-old' || kind === '::view-transition-new')
        parent = handle(element, parent, '::view-transition-image-pair', arg);
    }
    return handle(element, parent, kind, arg);
  }
  class CSSPseudoElement {
    #element; #parent; #type; #argument; #selector;
    constructor(key, element, parent, type, argument, selector) {
      if (key !== token) throw new TypeError('Illegal constructor');
      this.#element=element; this.#parent=parent; this.#type=type; this.#argument=argument; this.#selector=selector;
    }
    get type() { return this.#type; }
    get element() { return this.#element; }
    get parent() { return this.#parent; }
    get selectorText() { return this.#selector; }
    pseudo(type) {
      if (arguments.length === 0) throw new TypeError('A pseudo-element selector is required');
      const parsed = native.parse(type);
      if (!parsed) return null;
      const [kind, arg] = parsed, parent = this.#type;
      if ((parent === '::before' || parent === '::after') && kind === '::marker')
        return handle(this.#element, this, kind, arg);
      if (parent === '::view-transition' && kind === '::view-transition-group' ||
          arg === this.#argument && (parent === '::view-transition-group' && kind === '::view-transition-image-pair' ||
          parent === '::view-transition-image-pair' && (kind === '::view-transition-old' || kind === '::view-transition-new')))
        return handle(this.#element, this, kind, arg);
      return null;
    }
  }
  for (const key of ['type','element','parent','selectorText','pseudo'])
    Object.defineProperty(CSSPseudoElement.prototype,key,{enumerable:true});
  Object.defineProperty(CSSPseudoElement.prototype,Symbol.toStringTag,{value:'CSSPseudoElement',configurable:true});
  Object.defineProperty(CSSPseudoElement,'length',{value:0});
  return {CSSPseudoElement,fromElement};
})
