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
    'accelerometer', 'ambient-light-sensor', 'background-fetch', 'background-sync', 'camera', 'captured-surface-control',
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

  // Same-origin realms share their ClipboardItems (a native slot, not a per-realm WeakMap).
  const itemData = native.itemSlot;
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
      const data = itemData.get(this);
      // An item read from the clipboard is stale once the clipboard changed, even for a type read before.
      if (data.generation !== undefined && data.generation !== native.clipboardGeneration()) {
        return rejected(new DOMExceptionC("Failed to execute 'getType' on 'ClipboardItem': The clipboard changed since this item was read.", 'InvalidStateError'));
      }
      const entry = data.entries.find((e) => e[0] === String(type));
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
  // PNG signature, then an IHDR chunk of 13 bytes.
  const isPng = (buffer) => {
    const b = new Uint8Array(buffer);
    const magic = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13, 0x49, 0x48, 0x44, 0x52];
    return b.length >= 33 && magic.every((v, i) => b[i] === v);
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
      if (native.detached()) return resolved(undefined);
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
      if (native.detached()) return resolved();
      return resolved().then(() => store('writeText', [['text/plain', text, false]]));
    }
    read(options) {
      // ClipboardUnsanitizedFormats: `unsanitized` is a sequence (null is not one); only text/html
      // may be asked for unsanitized.
      try {
        if (options !== undefined && options !== null && typeof options !== 'object' && typeof options !== 'function') {
          throw new TypeErrorC("Failed to execute 'read' on 'Clipboard': The provided value is not of type 'ClipboardUnsanitizedFormats'.");
        }
        const unsanitized = options == null ? undefined : options.unsanitized;
        if (unsanitized !== undefined) {
          if (unsanitized === null || typeof unsanitized[Symbol.iterator] !== 'function') {
            throw new TypeErrorC("Failed to execute 'read' on 'Clipboard': The provided value cannot be converted to a sequence.");
          }
          for (const format of unsanitized) {
            if (String(format) !== 'text/html') {
              return rejected(new DOMExceptionC(`Failed to execute 'read' on 'Clipboard': Unsanitized ${String(format)} is not supported.`, 'NotAllowedError'));
            }
          }
        }
      } catch (e) {
        return rejected(e);
      }
      if (native.detached()) return resolved(undefined);
      const json = native.clipboardRead();
      if (json === null) return rejected(denied('read', 'Read'));
      const items = {};
      for (const [type, data, binary] of parse(json)) {
        items[type] = new BlobC([binary ? stringToBytes(data) : data], { type });
      }
      if (!O.keys(items).length) return resolved([]);
      const item = new ClipboardItem(items);
      itemData.get(item).generation = native.clipboardGeneration();
      return resolved([item]);
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
      if (native.detached()) return resolved();
      const pending = [];
      for (const item of list) {
        for (const type of item.types) {
          if (!ClipboardItem.supports(type)) {
            return rejected(new DOMExceptionC(`Failed to execute 'write' on 'Clipboard': Type ${type} not supported on write.`, 'NotAllowedError'));
          }
          pending.push(item.getType(type).then((blob) => (isText(type)
            ? blob.text().then((data) => [type, data, false])
            : blob.arrayBuffer().then((buffer) => {
              // An image is decoded before it is stored: bytes that are not one are a DataError.
              if (type === 'image/png' && !isPng(buffer)) {
                throw new DOMExceptionC("Failed to execute 'write' on 'Clipboard': Failed to decode the image/png data.", 'DataError');
              }
              return [type, bytesToString(buffer), true];
            }))));
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
      // Permissions Policy `geolocation`: asked (and a refusal reported) in the call, where the caller is on the stack.
      const policyAllows = native.policy('geolocation');
      resolved().then(() => {
        if (!stillWanted()) return;
        const fail = (code, message) => {
          if (typeof error === 'function') error(new GeolocationPositionError(token, code, message));
        };
        if (!policyAllows) return fail(1, 'Geolocation has been disabled in this document by permissions policy.');
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
  // A realm without timers still gets the rest of this module.
  const setTimeoutC = globalThis.setTimeout;
  // A task of a window whose document is gone (a closed popup's Notification) is never run; queueing it
  // is not an error (the realm's setTimeout throws there).
  const queueTask = (f) => {
    try { setTimeoutC(f, 0); } catch (e) { /* the document is no longer fully active */ }
  };
  const stringMember = (value) => value === undefined ? '' : `${value}`;
  const urlMember = (value) => {
    if (value === undefined) return '';
    try {
      return new URL(value, native.worker ? globalThis.location.href : document.baseURI).href;
    } catch (e) {
      return '';
    }
  };
  const optionalString = value => value === undefined ? undefined : `${value}`;
  const notificationObject = v => v !== null && (typeof v === 'object' || typeof v === 'function');
  const apply = Reflect.apply, iteratorKey = Symbol.iterator;
  // WebIDL sequence conversion propagates a member conversion error directly.
  // A non-persistent notification needs only the resulting list's emptiness.
  const hasActions = value => {
    if (value === undefined) return false;
    if (!notificationObject(value)) throw notificationError('NotificationOptions.actions must be a sequence.');
    const iterator = apply(value[iteratorKey], value, []);
    if (!notificationObject(iterator)) throw notificationError('Invalid notification action iterator.');
    const next = iterator.next;
    let any = false;
    for (;;) {
      const step = apply(next, iterator, []);
      if (!notificationObject(step)) throw notificationError('Invalid notification action iterator result.');
      if (step.done) return any;
      const item = step.value;
      if (!notificationObject(item)) throw notificationError('Invalid notification action.');
      if (optionalString(item.action) === undefined) throw notificationError('NotificationAction.action is required.');
      optionalString(item.icon);
      optionalString(item.navigate);
      if (optionalString(item.title) === undefined) throw notificationError('NotificationAction.title is required.');
      any = true;
    }
  };
  class Notification extends EventTargetC {
    constructor(title, options) {
      if (new.target === undefined) {
        throw new TypeErrorC("Failed to construct 'Notification': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
      }
      if (arguments.length === 0) throw notificationError('1 argument required, but only 0 present.');
      title = `${title}`;
      if (options === undefined || options === null) options = {};
      else if (typeof options !== 'object' && typeof options !== 'function') {
        throw notificationError("The provided value is not of type 'NotificationOptions'.");
      }
      // WebIDL converts dictionary members in lexicographic order before
      // the constructor steps, including the service-worker restriction.
      const actionsPresent = hasActions(options.actions);
      const badge = optionalString(options.badge), body = stringMember(options.body), data = options.data;
      const rawDir = options.dir, dir = rawDir === undefined ? 'auto' : `${rawDir}`;
      if (!DIRECTIONS.includes(dir)) {
        throw notificationError(`Failed to read the 'dir' property from 'NotificationOptions': The provided value '${dir}' is not a valid enum value of type NotificationDirection.`);
      }
      const icon = optionalString(options.icon), image = optionalString(options.image), lang = stringMember(options.lang);
      optionalString(options.navigate);
      const renotify = !!options.renotify, requireInteraction = !!options.requireInteraction;
      const rawSilent = options.silent, silent = rawSilent == null ? null : !!rawSilent;
      const tagName = stringMember(options.tag), rawTimestamp = options.timestamp;
      const timestamp = rawTimestamp === undefined ? undefined : +rawTimestamp;
      const rawVibrate = options.vibrate;
      const vibrate = rawVibrate === undefined ? [] :
        (rawVibrate !== null && typeof rawVibrate === 'object' ? Array.from(rawVibrate, Number) : [Number(rawVibrate)]);
      if (native.serviceWorker) {
        throw notificationError("Use ServiceWorkerRegistration.showNotification() in a service worker.");
      }
      if (actionsPresent) {
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
        title, dir, lang, body, tag: tagName,
        image: urlMember(image), icon: urlMember(icon), badge: urlMember(badge),
        vibrate: freeze(vibrate), timestamp: timestamp === undefined ? DateC.now() : timestamp,
        renotify, silent, requireInteraction, actions: freeze([]),
        data: data === undefined ? null : native.cloneNotificationData(data),
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
      const d = notificationData.get(this);
      if (!d) throw new TypeErrorC('Illegal invocation');
      return d.actions;
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
  if (native.worker) delete Notification.requestPermission;
  tag(Notification, 'Notification');

  // WebIDL: operations and attributes of an interface are enumerable properties of its prototype, and an interface
  // without a constructor operation has an interface object of length 0 (these classes take a private token).
  const enumerate = (C) => {
    for (const k of O.getOwnPropertyNames(C.prototype)) {
      if (k === 'constructor') continue;
      const d = O.getOwnPropertyDescriptor(C.prototype, k);
      d.enumerable = true;
      defineProperty(C.prototype, k, d);
    }
    defineProperty(C, 'length', { value: 0, configurable: true });
  };
  // An `on<type>` EventHandler attribute of an EventTarget of this module; `records` maps each instance to
  // its `{ handler, installed }`. The accessors are made with computed names so they read `get on<type>`.
  const defineHandler = (C, type, records) => {
    const name = 'on' + type;
    const bad = (what) => new TypeErrorC(`Failed to ${what} the '${name}' property on '${C.name}': Illegal invocation`);
    const d = O.getOwnPropertyDescriptor({
      get [name]() {
        const r = records.get(this);
        if (r === undefined) throw bad('read');
        return r.handler;
      },
      set [name](v) {
        const r = records.get(this);
        if (r === undefined) throw bad('set');
        // EventHandler is a callback function type: anything that is not callable becomes null.
        r.handler = typeof v === 'function' ? v : null;
        if (r.handler !== null && !r.installed) {
          r.installed = true;
          addListener.call(this, type, (e) => {
            const h = r.handler;
            if (h !== null) h.call(this, e);
          });
        }
      },
    }, name);
    d.enumerable = true;
    defineProperty(C.prototype, name, d);
  };
  const illegalOp = (what, iface) => new TypeErrorC(`Failed to execute '${what}' on '${iface}': Illegal invocation`);

  // Keyboard Lock (https://wicg.github.io/keyboard-lock/) and Keyboard Map (https://wicg.github.io/keyboard-map/).
  // A headless Pyro has no shell that claims keys (Escape, F11, Ctrl+W, ...): every key already reaches the page, so a
  // lock request is the promise state machine the spec defines plus the set of locked codes it records, exactly what
  // headless Chrome reports. The layout is the US layout Pyro's own key dispatcher types (pyro-cdp engine/keyboard.rs),
  // printable keys only, as Chrome reports them.
  const LAYOUT = (() => {
    const entries = [];
    for (const c of 'abcdefghijklmnopqrstuvwxyz') entries.push(['Key' + c.toUpperCase(), c]);
    for (const c of '0123456789') entries.push(['Digit' + c, c]);
    entries.push(['Backquote', '`'], ['Minus', '-'], ['Equal', '='], ['BracketLeft', '['], ['BracketRight', ']'],
      ['Backslash', '\\'], ['Semicolon', ';'], ['Quote', "'"], ['Comma', ','], ['Period', '.'], ['Slash', '/']);
    return entries;
  })();
  const MapC = Map;
  const mapData = new WeakMap();
  const mapOf = (self, what) => {
    const m = mapData.get(self);
    if (m === undefined) throw illegalOp(what, 'KeyboardLayoutMap');
    return m;
  };
  class KeyboardLayoutMap {
    constructor(t, entries) {
      if (t !== token) throw illegal('KeyboardLayoutMap');
      mapData.set(this, new MapC(entries));
    }
    get size() {
      return mapOf(this, 'size').size;
    }
    entries() {
      return mapOf(this, 'entries').entries();
    }
    keys() {
      return mapOf(this, 'keys').keys();
    }
    values() {
      return mapOf(this, 'values').values();
    }
    forEach(callback, thisArg = undefined) {
      const m = mapOf(this, 'forEach');
      if (arguments.length === 0) throw new TypeErrorC("Failed to execute 'forEach' on 'KeyboardLayoutMap': 1 argument required, but only 0 present.");
      if (typeof callback !== 'function') throw new TypeErrorC("Failed to execute 'forEach' on 'KeyboardLayoutMap': The callback provided as parameter 1 is not a function.");
      for (const [k, v] of m) callback.call(thisArg, v, k, this);
    }
    get(key) {
      const m = mapOf(this, 'get');
      if (arguments.length === 0) throw new TypeErrorC("Failed to execute 'get' on 'KeyboardLayoutMap': 1 argument required, but only 0 present.");
      return m.get(`${key}`);
    }
    has(key) {
      const m = mapOf(this, 'has');
      if (arguments.length === 0) throw new TypeErrorC("Failed to execute 'has' on 'KeyboardLayoutMap': 1 argument required, but only 0 present.");
      return m.has(`${key}`);
    }
  }
  defineProperty(KeyboardLayoutMap.prototype, Symbol.iterator, {
    value: KeyboardLayoutMap.prototype.entries, writable: true, configurable: true,
  });
  enumerate(KeyboardLayoutMap);
  tag(KeyboardLayoutMap, 'KeyboardLayoutMap');

  const keyboardData = new WeakMap();
  // WebIDL `sequence<DOMString>` (a string, null or a non-iterable object is no sequence).
  const keyCodesOf = (v) => {
    if (v === null || typeof v !== 'object' || typeof v[Symbol.iterator] !== 'function') {
      throw new TypeErrorC("Failed to execute 'lock' on 'Keyboard': The provided value cannot be converted to a sequence.");
    }
    const out = [];
    for (const code of v) out.push(`${code}`);
    return out;
  };
  class Keyboard extends EventTargetC {
    constructor(t) {
      if (t !== token) throw illegal('Keyboard');
      super();
      ownTag(this, 'Keyboard');
      // locked: the codes of the settled request (null: none; an empty set: every key); request: the pending
      // lock() ({ reject, cancelled }), map: the getLayoutMap() promise while it is pending.
      keyboardData.set(this, { handler: null, installed: false, locked: null, request: null, map: null });
    }
    // Keyboard Lock: "lock keyboard" and the outermost-frame rule (the request may only come from the document of a
    // top-level browsing context that is fully active).
    lock(keyCodes = []) {
      const d = keyboardData.get(this);
      if (d === undefined) return rejected(illegalOp('lock', 'Keyboard'));
      let codes;
      try {
        codes = keyCodesOf(keyCodes);
      } catch (e) {
        return rejected(e);
      }
      if (!native.active() || !native.topLevel()) {
        return rejected(new DOMExceptionC("Failed to execute 'lock' on 'Keyboard': lock() may only be called from the outermost main frame.", 'InvalidStateError'));
      }
      // A newer request supersedes the pending one (which is rejected, as Chrome does).
      const previous = d.request;
      d.request = null;
      if (previous !== null) previous.reject(new DOMExceptionC('This request was superseded by a subsequent lock() call.', 'AbortError'));
      return new PromiseC((resolve, reject) => {
        const request = { reject, cancelled: false };
        d.request = request;
        // The request is applied after the calling script, so a second lock() in the same task finds the first
        // one still pending.
        resolved().then(() => {
          if (d.request !== request) return;
          d.request = null;
          if (!request.cancelled) d.locked = new Set(codes);
          resolve(undefined);
        });
      });
    }
    // "Cancel keyboard lock": the locked codes go; a request still pending resolves but is not applied.
    unlock() {
      const d = keyboardData.get(this);
      if (d === undefined) throw illegalOp('unlock', 'Keyboard');
      if (d.request !== null) d.request.cancelled = true;
      d.locked = null;
    }
    // Keyboard Map: every document that may use `keyboard-map` (the default allowlist is 'self': a cross-origin frame
    // needs `allow="keyboard-map"`); concurrent calls get the same pending promise.
    getLayoutMap() {
      const d = keyboardData.get(this);
      if (d === undefined) return rejected(illegalOp('getLayoutMap', 'Keyboard'));
      if (!native.active()) {
        return rejected(new DOMExceptionC("Failed to execute 'getLayoutMap' on 'Keyboard': The document is not fully active.", 'InvalidStateError'));
      }
      if (!native.policy('keyboard-map')) {
        return rejected(new DOMExceptionC("Failed to execute 'getLayoutMap' on 'Keyboard': getLayoutMap() must be called from a top-level browsing context or allowed by the permissions policy.", 'SecurityError'));
      }
      if (d.map === null) {
        const promise = new PromiseC((resolve) => {
          resolved().then(() => {
            d.map = null;
            resolve(new KeyboardLayoutMap(token, LAYOUT));
          });
        });
        d.map = promise;
      }
      return d.map;
    }
  }
  defineHandler(Keyboard, 'layoutchange', keyboardData);
  enumerate(Keyboard);
  tag(Keyboard, 'Keyboard');

  // Virtual Keyboard API (https://w3c.github.io/virtual-keyboard/): there is no on-screen keyboard on a headless
  // desktop, so (as in headless Chrome) its geometry is the empty rect at the origin and show()/hide() have no keyboard
  // to move. `overlaysContent` is the page's opt-in; without a keyboard the env(keyboard-inset-*) values stay 0.
  const virtualData = new WeakMap();
  class VirtualKeyboard extends EventTargetC {
    constructor(t) {
      if (t !== token) throw illegal('VirtualKeyboard');
      super();
      ownTag(this, 'VirtualKeyboard');
      virtualData.set(this, { handler: null, installed: false, overlays: false });
    }
    get boundingRect() {
      if (!virtualData.has(this)) throw new TypeErrorC("Failed to read the 'boundingRect' property from 'VirtualKeyboard': Illegal invocation");
      return new DOMRect(0, 0, 0, 0);
    }
    get overlaysContent() {
      const d = virtualData.get(this);
      if (d === undefined) throw new TypeErrorC("Failed to read the 'overlaysContent' property from 'VirtualKeyboard': Illegal invocation");
      return d.overlays;
    }
    set overlaysContent(v) {
      const d = virtualData.get(this);
      if (d === undefined) throw new TypeErrorC("Failed to set the 'overlaysContent' property on 'VirtualKeyboard': Illegal invocation");
      d.overlays = !!v;
    }
    show() {
      if (!virtualData.has(this)) throw illegalOp('show', 'VirtualKeyboard');
    }
    hide() {
      if (!virtualData.has(this)) throw illegalOp('hide', 'VirtualKeyboard');
    }
  }
  defineHandler(VirtualKeyboard, 'geometrychange', virtualData);
  enumerate(VirtualKeyboard);
  tag(VirtualKeyboard, 'VirtualKeyboard');

  // The navigator's [SameObject] members, made when first read.
  let keyboard = null;
  let virtualKeyboard = null;
  return {
    geolocation: new Geolocation(token),
    userAgentData: new NavigatorUAData(token),
    permissions: new Permissions(token),
    clipboard: new Clipboard(token),
    get keyboard() {
      return keyboard ??= new Keyboard(token);
    },
    get virtualKeyboard() {
      return virtualKeyboard ??= new VirtualKeyboard(token);
    },
    ClipboardItem,
    Notification,
    Clipboard,
    Permissions,
    PermissionStatus,
    Keyboard,
    KeyboardLayoutMap,
    VirtualKeyboard,
  };
})
