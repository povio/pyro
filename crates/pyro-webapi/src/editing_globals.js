// ClipboardEvent and CaretPosition (https://w3c.github.io/clipboard-apis/,
// https://drafts.csswg.org/cssom-view/#caretposition), made on a realm's
// first use. The script evaluates to a function of the engine's helpers
// (editing_globals.rs) that returns the module object:
// { ClipboardEvent, CaretPosition, adopt, caretPosition }.
(native => {
  'use strict';
  const { clipboardData, isDataTransfer, caretRect } = native;
  const BaseEvent = Event;
  const defineProperty = Object.defineProperty;
  const setPrototypeOf = Object.setPrototypeOf;
  const WM = WeakMap;
  const wmGet = WM.prototype.get;
  const wmHas = WM.prototype.has;
  const wmSet = WM.prototype.set;
  const apply = Reflect.apply;

  // `clipboardData` of script-made events; trusted ones keep theirs natively.
  const synthetic = new WM();

  class ClipboardEvent extends BaseEvent {
    constructor(type, eventInitDict) {
      if (arguments.length < 1) {
        throw new TypeError("Failed to construct 'ClipboardEvent': 1 argument required, but only 0 present.");
      }
      const init = eventInitDict;
      if (init !== undefined && init !== null && typeof init !== 'object' && typeof init !== 'function') {
        throw new TypeError("Failed to construct 'ClipboardEvent': The provided value is not of type 'ClipboardEventInit'.");
      }
      super(type, init);
      let data = null;
      if (init !== undefined && init !== null) {
        const value = init.clipboardData;
        if (value !== undefined && value !== null) {
          if (!isDataTransfer(value)) {
            throw new TypeError("Failed to construct 'ClipboardEvent': Failed to read the 'clipboardData' property from 'ClipboardEventInit': Failed to convert value to 'DataTransfer'.");
          }
          data = value;
        }
      }
      apply(wmSet, synthetic, [this, data]);
    }
    get clipboardData() {
      if (apply(wmHas, synthetic, [this])) return apply(wmGet, synthetic, [this]);
      const value = clipboardData(this);
      if (value === undefined) throw new TypeError('Illegal invocation');
      return value;
    }
  }
  defineProperty(ClipboardEvent, 'length', { value: 1 });
  defineProperty(ClipboardEvent.prototype, 'clipboardData', { enumerable: true });
  defineProperty(ClipboardEvent.prototype, Symbol.toStringTag, { value: 'ClipboardEvent', configurable: true });

  const KEY = Symbol('CaretPosition');
  const carets = new WM();
  const caretOf = p => {
    if (!apply(wmHas, carets, [p])) throw new TypeError('Illegal invocation');
    return apply(wmGet, carets, [p]);
  };
  class CaretPosition {
    constructor(key) {
      if (key !== KEY) throw new TypeError('Illegal constructor');
    }
    get offsetNode() { return caretOf(this).node; }
    get offset() { return caretOf(this).offset; }
    getClientRect() {
      const c = caretOf(this);
      return caretRect(c.node, c.offset);
    }
  }
  defineProperty(CaretPosition.prototype, 'offsetNode', { enumerable: true });
  defineProperty(CaretPosition.prototype, 'offset', { enumerable: true });
  defineProperty(CaretPosition.prototype, 'getClientRect', { enumerable: true });
  defineProperty(CaretPosition.prototype, Symbol.toStringTag, { value: 'CaretPosition', configurable: true });

  return {
    ClipboardEvent,
    CaretPosition,
    // A natively dispatched ClipboardEvent's wrapper gets the prototype.
    adopt(event) { setPrototypeOf(event, ClipboardEvent.prototype); },
    caretPosition(node, offset) {
      const p = new CaretPosition(KEY);
      apply(wmSet, carets, [p, { node, offset }]);
      return p;
    },
  };
})
