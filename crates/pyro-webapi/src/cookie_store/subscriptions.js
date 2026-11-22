// Each registration wrapper retains its manager. The private operation closes
// over the registration's native list and the manager's relevant settings.
(function (slot) {
  'use strict';
  const token = Symbol();
  const TypeErrorC = TypeError;
  const usv = Function.call.bind(String.prototype.toWellFormed);
  const convert = value => usv(`${value}`);
  function options(sequence) {
    if (sequence == null || (typeof sequence !== 'object' && typeof sequence !== 'function')) {
      throw new TypeErrorC('Expected a sequence of cookie subscriptions.');
    }
    const entries = [];
    for (const item of sequence) {
      if (item != null && typeof item !== 'object' && typeof item !== 'function') {
        throw new TypeErrorC('Expected a cookie subscription dictionary.');
      }
      const name = item?.name;
      const convertedName = name === undefined ? null : convert(name);
      const url = item?.url;
      entries.push([convertedName, url === undefined ? null : convert(url)]);
    }
    return entries;
  }
  function operation(object) {
    const op = slot.get(object);
    if (!op) throw new TypeErrorC('Illegal invocation');
    return op;
  }
  class CookieStoreManager {
    constructor(...args) {
      if (args[0] !== token) throw new TypeErrorC('Illegal constructor');
      slot.set(this, args[1]);
    }
    async subscribe(subscriptions) { return operation(this)('subscribe', options(subscriptions)); }
    async getSubscriptions() { return operation(this)('list'); }
    async unsubscribe(subscriptions) { return operation(this)('unsubscribe', options(subscriptions)); }
  }
  for (const name of ['subscribe', 'getSubscriptions', 'unsubscribe']) {
    Object.defineProperty(CookieStoreManager.prototype, name, { enumerable: true });
  }
  Object.defineProperty(CookieStoreManager.prototype, Symbol.toStringTag, { value: 'CookieStoreManager', configurable: true });
  return { CookieStoreManager, manager: op => new CookieStoreManager(token, op) };
})
