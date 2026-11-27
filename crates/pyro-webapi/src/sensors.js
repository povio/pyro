// The Generic Sensor API for one realm (https://w3c.github.io/sensors/): `Sensor`, `SensorErrorEvent`, and the
// sensors built on it: Accelerometer, LinearAccelerationSensor, GravitySensor, Gyroscope, AmbientLightSensor and
// the orientation sensors. A sensor object is a client of a *virtual sensor* of the browser context (sensors.rs):
// it holds the Sensor state machine (idle, activating, active), its options and its latest reading, and asks
// `native` for everything that is not per object:
//   active()            the document is fully active
//   permission(name)    'granted' | 'denied' | 'prompt'
//   allowed(feature)    the document's permissions policy allows the feature
//   post(fn)            runs fn in a task of the document's event loop
//   watch(fn)           fn(type) is called (in a task) when a reading was accepted for the virtual sensor `type`,
//                       or with '' when focus or visibility changed
//   connect(type, frequency)    a client id, or the name of the DOMException start() fails with
//   disconnect(type, id)
//   read(type)          undefined, or [counter, timestamp, ...values] of the latest reading, when this document
//                       may be given it now (visible, and focused or of the focused document's origin)
// Events fire from tasks: `start()` never fires one synchronously, and `activate` is a task of its own before the
// first `reading`.
(function (native) {
  'use strict';
  const O = Object;
  const TypeErrorC = TypeError;
  const DOMExceptionC = DOMException;
  const EventC = Event;
  const EventTargetC = EventTarget;
  const apply = Reflect.apply;
  const defineProperty = O.defineProperty;
  const freeze = O.freeze;
  const objectToString = O.prototype.toString;
  const dispatch = EventTargetC.prototype.dispatchEvent;
  const addListener = EventTargetC.prototype.addEventListener;
  const removeListener = EventTargetC.prototype.removeEventListener;
  const isFinite_ = Number.isFinite;
  const Float32ArrayC = Float32Array;
  const Float64ArrayC = Float64Array;
  const WeakMapC = WeakMap;
  const SetC = Set;

  const states = new WeakMapC();
  const started = new SetC();
  // `new Sensor()` is an illegal constructor: only a subclass constructor of this module may reach it.
  let constructing = false;

  const need = (self, what) => {
    const st = states.get(self);
    if (st === undefined) throw new TypeErrorC(`Failed to execute '${what}' on 'Sensor': Illegal invocation`);
    return st;
  };

  const fire = (self, type) => { apply(dispatch, self, [new EventC(type)]); };

  const screenAngle = () => {
    try { return +self.screen.orientation.angle || 0; } catch (_) { return 0; }
  };
  // The reading in the coordinate system of the screen: rotated by the screen's orientation angle about the
  // vertical axis (Generic Sensor "referenceFrame" "screen"; Chrome's remapping).
  const remapXyz = (v, angle) => {
    switch (((angle % 360) + 360) % 360) {
      case 0: return v;
      case 90: return [-v[1], v[0], v[2]];
      case 180: return [-v[0], -v[1], v[2]];
      case 270: return [v[1], -v[0], v[2]];
      default: {
        const r = angle * Math.PI / 180, c = Math.cos(r), s = Math.sin(r);
        return [v[0] * c - v[1] * s, v[0] * s + v[1] * c, v[2]];
      }
    }
  };
  // Orientation Sensor: the quaternion times the rotation about Z by minus the angle.
  const remapQuaternion = (q, angle) => {
    const half = -angle * Math.PI / 360, c = Math.cos(half), s = Math.sin(half);
    return [q[0] * c + q[1] * s, -q[0] * s + q[1] * c, q[3] * s + q[2] * c, q[3] * c - q[2] * s];
  };

  // The latest reading of the platform sensor becomes the sensor object's.
  const apply_ = (st, reading) => {
    st.seq = reading[0];
    st.timestamp = reading[1];
    let values = reading.slice(2);
    if (st.frame === 'screen') {
      const angle = screenAngle();
      if (st.kind === 'xyz') values = remapXyz(values, angle);
      else if (st.kind === 'quaternion') values = remapQuaternion(values, angle);
    }
    st.values = st.kind === 'quaternion' ? freeze(values) : values;
  };

  const clear = (st) => { st.values = null; st.timestamp = null; st.seq = -1; };

  const release = (st) => {
    st.gen++;
    if (st.id !== null) native.disconnect(st.type, st.id);
    st.id = null;
    st.pending = false;
    started.delete(st);
    st.state = 'idle';
    clear(st);
  };

  const fail = (st, name, message) => {
    release(st);
    fire_error(st.owner, new DOMExceptionC(message, name));
  };

  const fire_error = (self, error) => {
    apply(dispatch, self, [new SensorErrorEvent('error', { error })]);
  };

  // Generic Sensor "activate a sensor object" after the permission and connection checks: the sensor is
  // activated holding the platform sensor's current reading, `activate` fires, and `reading` follows in its own task.
  const begin = (st, gen) => {
    if (st.gen !== gen || st.state !== 'activating') return;
    if (!native.active()) { release(st); return; }
    for (const name of st.permissions) {
      if (native.permission(name) === 'denied') { fail(st, 'NotAllowedError', 'Permissions to access sensor are not granted'); return; }
    }
    const id = native.connect(st.type, st.frequency);
    if (typeof id !== 'number') { fail(st, id, 'Could not connect to a sensor'); return; }
    st.id = id;
    st.state = 'active';
    started.add(st);
    const reading = native.read(st.type);
    if (reading !== undefined) { apply_(st, reading); st.pending = true; }
    fire(st.owner, 'activate');
    if (st.pending && st.gen === gen && st.state === 'active') {
      native.post(() => {
        if (st.gen !== gen || st.state !== 'active' || !st.pending) return;
        st.pending = false;
        fire(st.owner, 'reading');
      });
    }
  };

  // A reading the platform sensor accepted, or a focus or visibility change: an active sensor object takes the
  // reading it has not got yet.
  const deliver = (st) => {
    if (st.state !== 'active') return;
    const reading = native.read(st.type);
    if (reading === undefined || reading[0] === st.seq) return;
    apply_(st, reading);
    st.pending = false;
    fire(st.owner, 'reading');
  };
  native.watch((type) => {
    for (const st of [...started]) {
      if (type === '' || st.type === type) deliver(st);
    }
  });

  // SensorOptions and the options of the subclasses: `frequency` is a double, `referenceFrame` an enum.
  const options = (init) => {
    if (init === undefined || init === null) return { frequency: undefined, frame: 'device' };
    if (typeof init !== 'object' && typeof init !== 'function') throw new TypeErrorC('The provided value is not of type \'SensorOptions\'.');
    let frequency = init.frequency;
    if (frequency !== undefined) {
      frequency = +frequency;
      if (!isFinite_(frequency)) throw new TypeErrorC("Failed to construct sensor: The provided double value is non-finite.");
    }
    let frame = init.referenceFrame;
    if (frame === undefined) frame = 'device';
    else {
      frame = `${frame}`;
      if (frame !== 'device' && frame !== 'screen') throw new TypeErrorC(`Failed to construct sensor: The provided value '${frame}' is not a valid enum value of type ReferenceFrame.`);
    }
    return { frequency, frame };
  };

  // Permissions Policy: every feature the sensor needs must be allowed in the document.
  const check = (features) => {
    for (const feature of features) {
      if (!native.allowed(feature)) throw new DOMExceptionC('Access to this sensor is disallowed by Permissions Policy', 'SecurityError');
    }
  };

  const setup = (self, type, kind, features, init) => {
    const opts = options(init);
    check(features);
    return { opts, type, kind, features };
  };

  class Sensor extends EventTargetC {
    constructor() {
      if (!constructing) throw new TypeErrorC("Failed to construct 'Sensor': Illegal constructor");
      constructing = false;
      super();
    }
    get activated() { return need(this, 'activated').state === 'active'; }
    get hasReading() { return need(this, 'hasReading').values !== null; }
    get timestamp() { return need(this, 'timestamp').timestamp; }
    start() {
      const st = need(this, 'start');
      if (st.state !== 'idle') return;
      // A document that is not fully active has no sensors to start.
      if (!native.active()) return;
      st.state = 'activating';
      const gen = ++st.gen;
      native.post(() => begin(st, gen));
    }
    stop() {
      const st = need(this, 'stop');
      if (st.state === 'idle') return;
      release(st);
    }
  }
  for (const name of ['reading', 'activate', 'error']) {
    defineProperty(Sensor.prototype, `on${name}`, {
      get() { return need(this, `on${name}`).handlers[name] ?? null; },
      set(f) {
        const st = need(this, `on${name}`);
        const old = st.handlers[name];
        if (old) apply(removeListener, this, [name, old]);
        if (typeof f === 'function' || (f !== null && typeof f === 'object')) {
          st.handlers[name] = f;
          apply(addListener, this, [name, f]);
        } else {
          st.handlers[name] = null;
        }
      },
      configurable: true,
    });
  }

  // Makes the state of a sensor object. `constructing` is armed by the subclass constructor just before
  // `super()`, which `Sensor`'s constructor spends.
  const make = (self, type, kind, features, permissions, opts) => {
    states.set(self, {
      owner: self, type, kind, features, permissions,
      frequency: opts.frequency, frame: opts.frame,
      state: 'idle', gen: 0, id: null, pending: false,
      values: null, timestamp: null, seq: -1, handlers: {},
    });
  };

  const xyz = (st, i) => (st.values === null ? null : st.values[i]);

  class Accelerometer extends Sensor {
    constructor(init = {}) {
      const type = new.target === Accelerometer || !(new.target.prototype instanceof Accelerometer) ? 'accelerometer'
        : new.target === LinearAccelerationSensor || new.target.prototype instanceof LinearAccelerationSensor ? 'linear-acceleration'
        : new.target === GravitySensor || new.target.prototype instanceof GravitySensor ? 'gravity' : 'accelerometer';
      const s = setup(null, type, 'xyz', ['accelerometer'], init);
      constructing = true;
      super();
      make(this, type, 'xyz', s.features, ['accelerometer'], s.opts);
    }
    get x() { return xyz(need(this, 'x'), 0); }
    get y() { return xyz(need(this, 'y'), 1); }
    get z() { return xyz(need(this, 'z'), 2); }
  }
  class LinearAccelerationSensor extends Accelerometer {
    constructor(init = {}) { super(init); }
  }
  class GravitySensor extends Accelerometer {
    constructor(init = {}) { super(init); }
  }

  class Gyroscope extends Sensor {
    constructor(init = {}) {
      const s = setup(null, 'gyroscope', 'xyz', ['gyroscope'], init);
      constructing = true;
      super();
      make(this, 'gyroscope', 'xyz', s.features, ['gyroscope'], s.opts);
    }
    get x() { return xyz(need(this, 'x'), 0); }
    get y() { return xyz(need(this, 'y'), 1); }
    get z() { return xyz(need(this, 'z'), 2); }
  }

  class AmbientLightSensor extends Sensor {
    constructor(init = {}) {
      const s = setup(null, 'ambient-light', 'single', ['ambient-light-sensor'], init);
      constructing = true;
      super();
      make(this, 'ambient-light', 'single', s.features, ['ambient-light-sensor'], s.opts);
    }
    get illuminance() { return xyz(need(this, 'illuminance'), 0); }
  }

  const matrixOf = (q) => {
    const x = q[0], y = q[1], z = q[2], w = q[3];
    return [
      1 - 2 * (y * y + z * z), 2 * (x * y - z * w), 2 * (x * z + y * w), 0,
      2 * (x * y + z * w), 1 - 2 * (x * x + z * z), 2 * (y * z - x * w), 0,
      2 * (x * z - y * w), 2 * (y * z + x * w), 1 - 2 * (x * x + y * y), 0,
      0, 0, 0, 1,
    ];
  };
  const MATRIX_MEMBERS = ['m11', 'm12', 'm13', 'm14', 'm21', 'm22', 'm23', 'm24', 'm31', 'm32', 'm33', 'm34', 'm41', 'm42', 'm43', 'm44'];

  class OrientationSensor extends Sensor {
    constructor() {
      // Only AbsoluteOrientationSensor and RelativeOrientationSensor construct one.
      super();
    }
    get quaternion() { return need(this, 'quaternion').values; }
    populateMatrix(targetMatrix) {
      const st = need(this, 'populateMatrix');
      const tag = apply(objectToString, targetMatrix, []);
      const isDOMMatrix = typeof DOMMatrix === 'function' && targetMatrix instanceof DOMMatrix;
      if (!isDOMMatrix) {
        if (tag !== '[object Float32Array]' && tag !== '[object Float64Array]') {
          throw new TypeErrorC("Failed to execute 'populateMatrix' on 'OrientationSensor': The provided value is not of type '(Float32Array or Float64Array or DOMMatrix)'.");
        }
        if (apply(objectToString, targetMatrix.buffer, []) === '[object SharedArrayBuffer]') {
          throw new TypeErrorC("Failed to execute 'populateMatrix' on 'OrientationSensor': The provided ArrayBufferView value must not be shared.");
        }
        if (targetMatrix.length < 16) {
          throw new TypeErrorC("Failed to execute 'populateMatrix' on 'OrientationSensor': The provided matrix cannot fit the data.");
        }
      }
      if (st.values === null) throw new DOMExceptionC('Could not populate rotation matrix, not enough reading data', 'NotReadableError');
      const m = matrixOf(st.values);
      if (isDOMMatrix) {
        for (let i = 0; i < 16; i++) targetMatrix[MATRIX_MEMBERS[i]] = m[i];
      } else {
        for (let i = 0; i < 16; i++) targetMatrix[i] = m[i];
      }
    }
  }

  const orientationSensor = (type, features) => {
    const C = {
      [type === 'absolute-orientation' ? 'AbsoluteOrientationSensor' : 'RelativeOrientationSensor']: class extends OrientationSensor {
        constructor(init = {}) {
          const s = setup(null, type, 'quaternion', features, init);
          constructing = true;
          super();
          make(this, type, 'quaternion', features, features, s.opts);
        }
      },
    };
    return O.values(C)[0];
  };
  const AbsoluteOrientationSensor = orientationSensor('absolute-orientation', ['accelerometer', 'gyroscope', 'magnetometer']);
  const RelativeOrientationSensor = orientationSensor('relative-orientation', ['accelerometer', 'gyroscope']);

  const errors = new WeakMapC();
  class SensorErrorEvent extends EventC {
    constructor(type, init) {
      if (arguments.length < 2) throw new TypeErrorC(`Failed to construct 'SensorErrorEvent': 2 arguments required, but only ${arguments.length} present.`);
      if (init === null || (typeof init !== 'object' && typeof init !== 'function')) throw new TypeErrorC("Failed to construct 'SensorErrorEvent': The provided value is not of type 'SensorErrorEventInit'.");
      const error = init.error;
      if (!(error instanceof DOMExceptionC)) throw new TypeErrorC("Failed to construct 'SensorErrorEvent': Failed to read the 'error' property from 'SensorErrorEventInit': Required member is undefined or not a DOMException.");
      super(type, init);
      errors.set(this, error);
    }
    get error() {
      const error = errors.get(this);
      if (error === undefined) throw new TypeErrorC("Failed to read the 'error' property from 'SensorErrorEvent': Illegal invocation");
      return error;
    }
  }

  // WebIDL: attributes and operations are enumerable, the interface prototype object is tagged.
  const finish = (C, tag) => {
    defineProperty(C.prototype, Symbol.toStringTag, { value: tag, configurable: true });
    for (const k of O.getOwnPropertyNames(C.prototype)) {
      if (k === 'constructor') continue;
      const d = O.getOwnPropertyDescriptor(C.prototype, k);
      d.enumerable = true;
      defineProperty(C.prototype, k, d);
    }
    defineProperty(C, 'name', { value: tag, configurable: true });
  };
  finish(Sensor, 'Sensor');
  finish(SensorErrorEvent, 'SensorErrorEvent');
  finish(Accelerometer, 'Accelerometer');
  finish(LinearAccelerationSensor, 'LinearAccelerationSensor');
  finish(GravitySensor, 'GravitySensor');
  finish(Gyroscope, 'Gyroscope');
  finish(AmbientLightSensor, 'AmbientLightSensor');
  finish(OrientationSensor, 'OrientationSensor');
  finish(AbsoluteOrientationSensor, 'AbsoluteOrientationSensor');
  finish(RelativeOrientationSensor, 'RelativeOrientationSensor');

  return {
    Sensor, SensorErrorEvent, Accelerometer, LinearAccelerationSensor, GravitySensor, Gyroscope,
    AmbientLightSensor, OrientationSensor, AbsoluteOrientationSensor, RelativeOrientationSensor,
  };
});
