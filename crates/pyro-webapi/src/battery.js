// Battery Status (https://w3c.github.io/battery/) for one window realm: `BatteryManager` and the realm's
// `navigator.getBattery()` promise, made on first use. The machine has no battery to report on (a headless
// session reads none), and the spec's values for a UA in that case are the ones a desktop Chrome gives: charging,
// with no time to charge or discharge, at full level. They never change, so no `*change` event ever fires.
(function () {
  'use strict';
  const O = Object;
  const defineProperty = O.defineProperty;
  const PromiseC = Promise;
  const TypeErrorC = TypeError;
  const EventTargetC = EventTarget;
  const addListener = EventTargetC.prototype.addEventListener;
  const token = Symbol('pyro');
  const states = new WeakMap();

  const slotOf = (self, name) => {
    const d = states.get(self);
    if (d === undefined) throw new TypeErrorC(`Failed to read the '${name}' property from 'BatteryManager': Illegal invocation`);
    return d;
  };

  class BatteryManager extends EventTargetC {
    constructor(t) {
      if (t !== token) throw new TypeErrorC("Failed to construct 'BatteryManager': Illegal constructor");
      super();
      defineProperty(this, Symbol.toStringTag, { value: 'BatteryManager', configurable: true });
      states.set(this, { charging: true, chargingTime: 0, dischargingTime: Infinity, level: 1, handlers: {}, installed: {} });
    }
  }
  // WebIDL attributes are enumerable accessors on the prototype, their getters named `get <name>`.
  for (const name of ['charging', 'chargingTime', 'dischargingTime', 'level']) {
    const d = O.getOwnPropertyDescriptor({ get [name]() { return slotOf(this, name)[name]; } }, name);
    d.enumerable = true;
    defineProperty(BatteryManager.prototype, name, d);
  }
  // Event handler attributes: the first non-null assignment registers the one listener for the type.
  for (const type of ['chargingchange', 'chargingtimechange', 'dischargingtimechange', 'levelchange']) {
    const name = 'on' + type;
    // Accessor functions named `get on<type>` / `set on<type>`, as WebIDL gives them.
    defineProperty(BatteryManager.prototype, name, O.getOwnPropertyDescriptor({
      get [name]() {
        return slotOf(this, name).handlers[type] ?? null;
      },
      set [name](v) {
        const st = slotOf(this, name);
        st.handlers[type] = typeof v === 'function' || (v !== null && typeof v === 'object') ? v : null;
        if (st.handlers[type] !== null && !st.installed[type]) {
          st.installed[type] = true;
          addListener.call(this, type, (e) => {
            const h = st.handlers[type];
            if (typeof h === 'function') h.call(this, e);
          });
        }
      },
    }, name));
  }
  defineProperty(BatteryManager.prototype, Symbol.toStringTag, { value: 'BatteryManager', configurable: true });
  defineProperty(BatteryManager, 'length', { value: 0, configurable: true });

  // "The same promise every time": the realm's one BatteryManager, resolved.
  let promise = null;
  const getBattery = () => {
    if (promise === null) promise = PromiseC.resolve(new BatteryManager(token));
    return promise;
  };

  return { BatteryManager, getBattery };
})
