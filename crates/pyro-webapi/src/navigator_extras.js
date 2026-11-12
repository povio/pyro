// navigator.userAgentData, navigator.permissions, navigator.clipboard,
// ClipboardItem and Notification for one realm, made on first use. `native` reads the page's
// client hints, its context's permission table and clipboard:
//   uaData() -> JSON {brands, fullVersionList, mobile, platform, ...}
//   permissionState(name) -> 'granted' | 'denied' | 'prompt'
//   watchPermissions(refresh): refresh() runs in a task after a permission
//     change (once per realm, from the first status with a change listener)
//   clipboardRead() -> JSON [[type, data, binary]] or null (not granted)
//   clipboardWritable() -> false when writing is denied
//   clipboardWrite(json) stores [[type, data, binary]]
//   geolocationPosition() -> the override's coordinates, or null
// Binary data travels as a string of byte-valued characters. Shapes, order
// and messages follow Chrome 156.
(function (native) {
  'use strict';
  const token = Symbol('pyro');
  const O = Object;
  const freeze = O.freeze;
  const defineProperty = O.defineProperty;
  const parse = JSON.parse;
  const stringify = JSON.stringify;
  const PromiseC = Promise;
  const DateC = Date;
  const resolved = (v) => PromiseC.resolve(v);
  const rejected = (e) => PromiseC.reject(e);
  const DOMExceptionC = DOMException;
  const BlobC = Blob;
  const EventTargetC = EventTarget;
  const EventC = Event;
  const TypeErrorC = TypeError;
  const fromCharCode = String.fromCharCode;
  const illegal = (name) => new TypeErrorC(`Failed to construct '${name}': Illegal constructor`);
  const tag = (C, name) => defineProperty(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
  // Host EventTarget instances answer Symbol.toStringTag themselves; own
  // tags keep subclasses reporting their own name.
  const ownTag = (o, name) => defineProperty(o, Symbol.toStringTag, { value: name, configurable: true });
  const iterable = (v) => v !== null && typeof v === 'object' && typeof v[Symbol.iterator] === 'function';

  let uaJson = null;
  let uaParsed = null;
  const ua = () => {
    const json = native.uaData();
    if (json !== uaJson) {
      uaJson = json;
      uaParsed = parse(json);
    }
    return uaParsed;
  };
  // Chrome returns fresh brand objects each time: a frozen array from
  // `brands`, a plain one from getHighEntropyValues.
  const brandList = (list) => list.map((b) => ({ brand: b.brand, version: b.version }));

  class NavigatorUAData {
    constructor(t) {
      if (t !== token) throw illegal('NavigatorUAData');
    }
    get brands() {
      return freeze(brandList(ua().brands));
    }
    get mobile() {
      return ua().mobile;
    }
    get platform() {
      return ua().platform;
    }
    getHighEntropyValues(hints) {
      if (arguments.length === 0) {
        return rejected(new TypeErrorC("Failed to execute 'getHighEntropyValues' on 'NavigatorUAData': 1 argument required, but only 0 present."));
      }
      if (!iterable(hints)) {
        return rejected(new TypeErrorC("Failed to execute 'getHighEntropyValues' on 'NavigatorUAData': The provided value cannot be converted to a sequence."));
      }
      const data = ua();
      const values = { brands: this.brands, mobile: data.mobile, platform: data.platform };
      for (const hint of hints) {
        switch (String(hint)) {
          case 'architecture': values.architecture = data.architecture; break;
          case 'bitness': values.bitness = data.bitness; break;
          case 'formFactors': values.formFactors = data.formFactors.slice(); break;
          case 'fullVersionList': values.fullVersionList = brandList(data.fullVersionList); break;
          case 'model': values.model = data.model; break;
          case 'platformVersion': values.platformVersion = data.platformVersion; break;
          case 'uaFullVersion': values.uaFullVersion = data.uaFullVersion; break;
          case 'wow64': values.wow64 = data.wow64; break;
        }
      }
      // Chrome reports the keys in sorted order.
      const out = {};
      for (const key of O.keys(values).sort()) out[key] = values[key];
      return resolved(out);
    }
    toJSON() {
      return { brands: this.brands, mobile: this.mobile, platform: this.platform };
    }
  }
  tag(NavigatorUAData, 'NavigatorUAData');

  const NAMES = new Set([
    'accelerometer', 'background-fetch', 'background-sync', 'camera', 'captured-surface-control',
    'clipboard-read', 'clipboard-write', 'display-capture', 'geolocation', 'gyroscope', 'idle-detection',
    'keyboard-lock', 'local-fonts', 'magnetometer', 'microphone', 'midi', 'nfc', 'notifications',
    'payment-handler', 'periodic-background-sync', 'persistent-storage', 'pointer-lock', 'push',
    'screen-wake-lock', 'speaker-selection', 'storage-access', 'system-wake-lock',
    'top-level-storage-access', 'window-management',
  ]);

  const statusData = new WeakMap();
  // Statuses with change listeners stay alive (and are re-read after each
  // permission change) even when the page keeps no reference to them.
  const watched = new Set();
  let watching = false;
  const refresh = () => {
    for (const status of watched) {
      const d = statusData.get(status);
      const state = native.permissionState(d.name);
      if (state !== d.state) {
        d.state = state;
        status.dispatchEvent(new EventC('change'));
      }
    }
  };
  const watch = (status) => {
    watched.add(status);
    if (!watching) {
      watching = true;
      native.watchPermissions(refresh);
    }
  };
  const addListener = EventTargetC.prototype.addEventListener;
  class PermissionStatus extends EventTargetC {
    constructor(t, name) {
      if (t !== token) throw illegal('PermissionStatus');
      super();
      ownTag(this, 'PermissionStatus');
      const d = { name, onchange: null, state: native.permissionState(name), handler: false };
      statusData.set(this, d);
      defineProperty(this, 'addEventListener', {
        value: function addEventListener(type, ...rest) {
          if (String(type) === 'change') watch(this);
          return addListener.call(this, type, ...rest);
        },
        writable: true,
        configurable: true,
      });
    }
    get name() {
      return statusData.get(this).name;
    }
    get state() {
      return native.permissionState(statusData.get(this).name);
    }
    get onchange() {
      return statusData.get(this).onchange;
    }
    set onchange(v) {
      const d = statusData.get(this);
      d.onchange = typeof v === 'function' || (v !== null && typeof v === 'object') ? v : null;
      if (d.onchange !== null && !d.handler) {
        d.handler = true;
        watch(this);
        addListener.call(this, 'change', (e) => {
          if (typeof d.onchange === 'function') d.onchange.call(this, e);
        });
      }
    }
  }
  tag(PermissionStatus, 'PermissionStatus');

  const queryError = (rest) => new TypeErrorC(`Failed to execute 'query' on 'Permissions': ${rest}`);
  class Permissions {
    constructor(t) {
      if (t !== token) throw illegal('Permissions');
    }
    query(descriptor) {
      if (arguments.length === 0) return rejected(queryError('1 argument required, but only 0 present.'));
      if (descriptor === null || typeof descriptor !== 'object') {
        return rejected(queryError('parameter 1 is not of type \'object\'.'));
      }
      const name = descriptor.name;
      if (name === undefined) {
        return rejected(queryError("Failed to read the 'name' property from 'PermissionDescriptor': Required member is undefined."));
      }
      if (!NAMES.has(String(name))) {
        return rejected(queryError(`Failed to read the 'name' property from 'PermissionDescriptor': The provided value '${String(name)}' is not a valid enum value of type PermissionName.`));
      }
      if (String(name) === 'push' && !descriptor.userVisibleOnly) {
        return rejected(new DOMExceptionC("Failed to execute 'query' on 'Permissions': Push Permission without userVisibleOnly:true isn't supported yet.", 'NotSupportedError'));
      }
      return resolved(new PermissionStatus(token, String(name)));
    }
  }
  tag(Permissions, 'Permissions');

  const itemData = new WeakMap();
  const itemError = (rest) => new TypeErrorC(`Failed to construct 'ClipboardItem': ${rest}`);
  class ClipboardItem {
    constructor(items) {
      if (arguments.length === 0) throw itemError('1 argument required, but only 0 present.');
      if (items === null || typeof items !== 'object') {
        throw itemError('Only objects can be converted to record<K,V> types');
      }
      const entries = O.keys(items).map((type) => [type, items[type]]);
      if (entries.length === 0) throw itemError('Empty dictionary argument');
      itemData.set(this, { entries, types: freeze(entries.map((e) => e[0])) });
    }
    get types() {
      return itemData.get(this).types;
    }
    getType(type) {
      const entry = itemData.get(this).entries.find((e) => e[0] === String(type));
      if (!entry) {
        return rejected(new DOMExceptionC("Failed to execute 'getType' on 'ClipboardItem': The type was not found", 'NotFoundError'));
      }
      return resolved(entry[1]).then((v) => (v instanceof BlobC ? v : new BlobC([String(v)], { type: entry[0] })));
    }
    static supports(type) {
      type = String(type);
      return type === 'text/plain' || type === 'text/html' || type === 'image/png' || type === 'image/svg+xml' || type.startsWith('web ');
    }
  }
  tag(ClipboardItem, 'ClipboardItem');

  const bytesToString = (buffer) => {
    const bytes = new Uint8Array(buffer);
    let out = '';
    for (let i = 0; i < bytes.length; i += 8192) out += fromCharCode.apply(null, bytes.subarray(i, i + 8192));
    return out;
  };
  const stringToBytes = (s) => {
    const bytes = new Uint8Array(s.length);
    for (let i = 0; i < s.length; i++) bytes[i] = s.charCodeAt(i);
    return bytes;
  };
  const isText = (type) => type.startsWith('text/') || type === 'image/svg+xml';
  const denied = (method, what) => new DOMExceptionC(`Failed to execute '${method}' on 'Clipboard': ${what} permission denied.`, 'NotAllowedError');
  const store = (method, entries) => {
    if (!native.clipboardWritable()) return rejected(denied(method, 'Write'));
    native.clipboardWrite(stringify(entries));
    return undefined;
  };

  class Clipboard extends EventTargetC {
    constructor(t) {
      if (t !== token) throw illegal('Clipboard');
      super();
      ownTag(this, 'Clipboard');
    }
    readText() {
      const json = native.clipboardRead();
      if (json === null) return rejected(denied('readText', 'Read'));
      const entry = parse(json).find((e) => e[0] === 'text/plain');
      return resolved(entry ? entry[1] : '');
    }
    writeText(text) {
      if (arguments.length === 0) {
        return rejected(new TypeErrorC("Failed to execute 'writeText' on 'Clipboard': 1 argument required, but only 0 present."));
      }
      text = String(text);
      return resolved().then(() => store('writeText', [['text/plain', text, false]]));
    }
    read() {
      const json = native.clipboardRead();
      if (json === null) return rejected(denied('read', 'Read'));
      const items = {};
      for (const [type, data, binary] of parse(json)) {
        items[type] = new BlobC([binary ? stringToBytes(data) : data], { type });
      }
      return resolved(O.keys(items).length ? [new ClipboardItem(items)] : []);
    }
    write(items) {
      if (arguments.length === 0) {
        return rejected(new TypeErrorC("Failed to execute 'write' on 'Clipboard': 1 argument required, but only 0 present."));
      }
      if (!iterable(items)) {
        return rejected(new TypeErrorC("Failed to execute 'write' on 'Clipboard': The provided value cannot be converted to a sequence."));
      }
      const list = Array.from(items);
      if (list.some((item) => !itemData.has(item))) {
        return rejected(new TypeErrorC("Failed to execute 'write' on 'Clipboard': Failed to convert value to 'ClipboardItem'."));
      }
      const pending = [];
      for (const item of list) {
        for (const type of item.types) {
          if (!ClipboardItem.supports(type)) {
            return rejected(new DOMExceptionC(`Failed to execute 'write' on 'Clipboard': Type ${type} not supported on write.`, 'NotAllowedError'));
          }
          pending.push(item.getType(type).then((blob) => (isText(type)
            ? blob.text().then((data) => [type, data, false])
            : blob.arrayBuffer().then((buffer) => [type, bytesToString(buffer), true]))));
        }
      }
      return PromiseC.all(pending).then((entries) => store('write', entries));
    }
  }
  tag(Clipboard, 'Clipboard');

  // No location source of its own: without a granted permission the request
  // fails as a declined prompt would, without an override as unavailable.
  class GeolocationCoordinates {
    constructor(t, c) {
      if (t !== token) throw illegal('GeolocationCoordinates');
      for (const k of ['latitude', 'longitude', 'accuracy', 'altitude', 'altitudeAccuracy', 'heading', 'speed']) {
        defineProperty(this, k, { value: c[k], enumerable: true });
      }
    }
  }
  tag(GeolocationCoordinates, 'GeolocationCoordinates');
  class GeolocationPosition {
    constructor(t, coords) {
      if (t !== token) throw illegal('GeolocationPosition');
      defineProperty(this, 'coords', { value: coords, enumerable: true });
      defineProperty(this, 'timestamp', { value: DateC.now(), enumerable: true });
    }
  }
  tag(GeolocationPosition, 'GeolocationPosition');
  class GeolocationPositionError {
    constructor(t, code, message) {
      if (t !== token) throw illegal('GeolocationPositionError');
      defineProperty(this, 'code', { value: code, enumerable: true });
      defineProperty(this, 'message', { value: message, enumerable: true });
    }
  }
  for (const [i, name] of ['PERMISSION_DENIED', 'POSITION_UNAVAILABLE', 'TIMEOUT'].entries()) {
    defineProperty(GeolocationPositionError, name, { value: i + 1, enumerable: true });
    defineProperty(GeolocationPositionError.prototype, name, { value: i + 1, enumerable: true });
  }
  tag(GeolocationPositionError, 'GeolocationPositionError');
  class Geolocation {
    #next = 1;
    #watches = new Set();
    constructor(t) {
      if (t !== token) throw illegal('Geolocation');
    }
    #request(success, error, stillWanted) {
      if (typeof success !== 'function') {
        throw new TypeErrorC("Failed to execute 'getCurrentPosition' on 'Geolocation': parameter 1 is not a function.");
      }
      resolved().then(() => {
        if (!stillWanted()) return;
        const fail = (code, message) => {
          if (typeof error === 'function') error(new GeolocationPositionError(token, code, message));
        };
        if (native.permissionState('geolocation') !== 'granted') return fail(1, 'User denied Geolocation');
        const position = native.geolocationPosition();
        if (position === null) return fail(2, 'Position update is unavailable');
        success(new GeolocationPosition(token, new GeolocationCoordinates(token, position)));
      });
    }
    getCurrentPosition(success, error) {
      this.#request(success, error, () => true);
    }
    // One report per watch: an override changing later does not call back.
    watchPosition(success, error) {
      const id = this.#next++;
      this.#watches.add(id);
      this.#request(success, error, () => this.#watches.has(id));
      return id;
    }
    clearWatch(id) {
      this.#watches.delete(id);
    }
  }
  tag(Geolocation, 'Geolocation');

  // Notification (non-persistent). Nothing is displayed: a notification of
  // a document whose 'notifications' permission is granted reports 'show'
  // as Chrome's headless platform does, any other one 'error'. permission
  // reads the context's table ('prompt' is 'default'); requestPermission
  // has no user to ask, so it reports that state.
  const notificationPermission = () => {
    const state = native.permissionState('notifications');
    return state === 'prompt' ? 'default' : state;
  };
  const notificationData = new WeakMap();
  const notificationError = (rest) => new TypeErrorC(`Failed to construct 'Notification': ${rest}`);
  const DIRECTIONS = ['auto', 'ltr', 'rtl'];
  // Captured when present: a realm without timers (or structuredClone)
  // still gets the rest of this module.
  const setTimeoutC = globalThis.setTimeout;
  const structuredCloneC = globalThis.structuredClone;
  const queueTask = (f) => setTimeoutC(f, 0);
  const stringMember = (options, key) => (options[key] === undefined ? '' : String(options[key]));
  const urlMember = (options, key) => {
    if (options[key] === undefined) return '';
    try {
      return new URL(String(options[key]), document.baseURI).href;
    } catch (e) {
      return '';
    }
  };
  class Notification extends EventTargetC {
    constructor(title, options) {
      if (new.target === undefined) {
        throw new TypeErrorC("Failed to construct 'Notification': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
      }
      if (arguments.length === 0) throw notificationError('1 argument required, but only 0 present.');
      title = String(title);
      if (options === undefined || options === null) options = {};
      else if (typeof options !== 'object' && typeof options !== 'function') {
        throw notificationError("The provided value is not of type 'NotificationOptions'.");
      }
      const dir = options.dir === undefined ? 'auto' : String(options.dir);
      if (!DIRECTIONS.includes(dir)) {
        throw notificationError(`Failed to read the 'dir' property from 'NotificationOptions': The provided value '${dir}' is not a valid enum value of type NotificationDirection.`);
      }
      const actions = options.actions === undefined ? [] : Array.from(options.actions);
      const vibrate = options.vibrate === undefined ? [] : (options.vibrate !== null && typeof options.vibrate === 'object' ? Array.from(options.vibrate, Number) : [Number(options.vibrate)]);
      const silent = options.silent === undefined || options.silent === null ? null : !!options.silent;
      const renotify = !!options.renotify;
      const tagName = stringMember(options, 'tag');
      if (actions.length > 0) {
        throw notificationError('Actions are only supported for persistent notifications shown using ServiceWorkerRegistration.showNotification().');
      }
      if (silent === true && vibrate.length > 0) {
        throw notificationError('Silent notifications must not specify vibration patterns.');
      }
      if (renotify && tagName === '') {
        throw notificationError('Notifications which set the renotify flag must specify a non-empty tag.');
      }
      super();
      ownTag(this, 'Notification');
      const d = {
        title, dir, lang: stringMember(options, 'lang'), body: stringMember(options, 'body'), tag: tagName,
        image: urlMember(options, 'image'), icon: urlMember(options, 'icon'), badge: urlMember(options, 'badge'),
        vibrate: freeze(vibrate), timestamp: options.timestamp === undefined ? DateC.now() : Number(options.timestamp),
        renotify, silent, requireInteraction: !!options.requireInteraction,
        data: options.data === undefined ? null : structuredCloneC(options.data),
        handlers: { click: null, show: null, error: null, close: null },
        shown: false, closed: false,
      };
      notificationData.set(this, d);
      const granted = notificationPermission() === 'granted';
      queueTask(() => {
        if (d.closed) return;
        if (granted) d.shown = true;
        this.dispatchEvent(new EventC(granted ? 'show' : 'error'));
      });
    }
    static get permission() {
      return notificationPermission();
    }
    static get maxActions() {
      return 2;
    }
    static requestPermission(callback) {
      const state = notificationPermission();
      if (typeof callback === 'function') queueTask(() => callback(state));
      return resolved(state);
    }
    close() {
      const d = notificationData.get(this);
      if (d === undefined || d.closed) return;
      d.closed = true;
      if (d.shown) queueTask(() => this.dispatchEvent(new EventC('close')));
    }
    get actions() {
      return freeze([]);
    }
  }
  for (const key of ['title', 'dir', 'lang', 'body', 'tag', 'image', 'icon', 'badge', 'vibrate', 'timestamp', 'renotify', 'silent', 'requireInteraction', 'data']) {
    defineProperty(Notification.prototype, key, {
      get() {
        const d = notificationData.get(this);
        if (d === undefined) throw new TypeErrorC('Illegal invocation');
        return d[key];
      },
      enumerable: true,
      configurable: true,
    });
  }
  for (const type of ['click', 'show', 'error', 'close']) {
    defineProperty(Notification.prototype, 'on' + type, {
      get() {
        return notificationData.get(this)?.handlers[type] ?? null;
      },
      set(v) {
        const d = notificationData.get(this);
        if (d === undefined) return;
        const had = d.handlers[type] !== null;
        d.handlers[type] = typeof v === 'function' || (v !== null && typeof v === 'object') ? v : null;
        if (!had && d.handlers[type] !== null && !d['installed_' + type]) {
          d['installed_' + type] = true;
          addListener.call(this, type, (e) => {
            const h = d.handlers[type];
            if (typeof h === 'function') h.call(this, e);
          });
        }
      },
      enumerable: true,
      configurable: true,
    });
  }
  tag(Notification, 'Notification');

  return {
    geolocation: new Geolocation(token),
    userAgentData: new NavigatorUAData(token),
    permissions: new Permissions(token),
    clipboard: new Clipboard(token),
    ClipboardItem,
    Notification,
  };
})
