// CookieStore and service-worker cookie events share the CookieListItem
// conversion, but have distinct prototypes, brands and native event bases.
((Base, slots, serviceWorker) => {
  'use strict';
  const TypeErrorC = TypeError;
  const freeze = Object.freeze, define = Object.defineProperty;
  const descriptor = Object.getOwnPropertyDescriptor;
  const wellFormed = Function.call.bind(String.prototype.toWellFormed);
  const isObject = v => v !== null && (typeof v === 'object' || typeof v === 'function');

  function dictionary(value) {
    if (value != null && !isObject(value)) throw new TypeErrorC('Expected a dictionary.');
    return value;
  }

  function items(sequence) {
    const list = [];
    if (sequence !== undefined) {
      if (!isObject(sequence)) throw new TypeErrorC('Expected a sequence of cookie list items.');
      for (const item of sequence) {
        const input = dictionary(item);
        const name = input?.name;
        const record = name === undefined ? {} : { name: wellFormed(`${name}`) };
        const value = input?.value;
        if (value !== undefined) define(record, 'value', {
          value: wellFormed(`${value}`), writable: true, enumerable: true, configurable: true
        });
        list.push(record);
      }
    }
    return freeze(list);
  }

  const name = serviceWorker ? 'ExtendableCookieChangeEvent' : 'CookieChangeEvent';
  const C = { [name]: class extends Base {
    constructor(type, init = {}) {
      if (!arguments.length) throw new TypeErrorC('An event type is required.');
      dictionary(init);
      super(type, init);
      slots.set(this, { changed: items(init?.changed), deleted: items(init?.deleted) });
    }
    get changed() {
      const data = slots.get(this);
      if (!data) throw new TypeErrorC('Illegal invocation');
      return data.changed;
    }
    get deleted() {
      const data = slots.get(this);
      if (!data) throw new TypeErrorC('Illegal invocation');
      return data.deleted;
    }
  }}[name];
  for (const key of ['changed', 'deleted']) {
    define(C.prototype, key, {...descriptor(C.prototype, key), enumerable: true});
  }
  define(C.prototype, Symbol.toStringTag, {value: name, configurable: true});
  return { [name]: C, make: (type, init) => new C(type, init) };
})
