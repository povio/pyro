// The script that makes a page's locale the default of Intl and the toLocale*String methods
// (profiles' and wright contexts' `locale`).

/**
 * `locale` for `Intl` and the `toLocale*String` methods: the engine's locale
 * override covers `navigator.language(s)` and `Accept-Language`, while
 * JavaScriptCore's default locale is the process's. This makes `locale` the
 * default wherever a page leaves the locales argument out. Constructors keep
 * their prototypes, names and statics (`instanceof` and subclasses work).
 */
export function localeScript(locale: string): string {
  return `(() => {
  'use strict';
  const locale = ${JSON.stringify(locale)};
  const define = (o, k, value) => Object.defineProperty(o, k, { value, writable: true, configurable: true, enumerable: false });
  const like = (f, orig) => {
    Object.defineProperty(f, 'name', { value: orig.name, configurable: true });
    Object.defineProperty(f, 'length', { value: orig.length, configurable: true });
    return f;
  };
  for (const name of ['Collator', 'DateTimeFormat', 'DisplayNames', 'DurationFormat', 'ListFormat', 'NumberFormat', 'PluralRules', 'RelativeTimeFormat', 'Segmenter']) {
    const Orig = Intl[name];
    if (typeof Orig !== 'function') continue;
    const Ctor = function (locales, options) {
      const args = [locales === undefined ? locale : locales, options];
      return new.target ? Reflect.construct(Orig, args, new.target === Ctor ? Orig : new.target) : Reflect.apply(Orig, this, args);
    };
    for (const key of Reflect.ownKeys(Orig)) if (key !== 'prototype' && key !== 'name' && key !== 'length') Object.defineProperty(Ctor, key, Object.getOwnPropertyDescriptor(Orig, key));
    Object.defineProperty(Ctor, 'prototype', { value: Orig.prototype, writable: false });
    define(Orig.prototype, 'constructor', like(Ctor, Orig));
    define(Intl, name, Ctor);
  }
  const method = (proto, key, index) => {
    const orig = proto && proto[key];
    if (typeof orig !== 'function') return;
    define(proto, key, like({ [key](...args) { if (args[index] === undefined) args[index] = locale; return Reflect.apply(orig, this, args); } }[key], orig));
  };
  for (const key of ['toLocaleString', 'toLocaleDateString', 'toLocaleTimeString']) method(Date.prototype, key, 0);
  method(Number.prototype, 'toLocaleString', 0);
  if (typeof BigInt === 'function') method(BigInt.prototype, 'toLocaleString', 0);
  method(String.prototype, 'localeCompare', 1);
  method(String.prototype, 'toLocaleLowerCase', 0);
  method(String.prototype, 'toLocaleUpperCase', 0);
})();`;
}

