// CSS Font Loading 3 (https://drafts.csswg.org/css-font-loading/): `FontFace`, `FontFaceSet`,
// `FontFaceSetLoadEvent`, and the realm's set (`document.fonts`, `self.fonts`). Made on a realm's
// first use (font_loading.rs); the script evaluates to a function of the engine's helpers that
// returns { FontFace, FontFaceSet, FontFaceSetLoadEvent, fonts }.
(native => {
  'use strict';
  const { descriptor, sources, isFont, fontQuery, cssFaces, urlState, enginePending, register, loading } = native;
  const apply = Reflect.apply;
  const ET = EventTarget;
  const Ev = Event;
  const DE = DOMException;
  const addListener = ET.prototype.addEventListener;
  const dispatch = ET.prototype.dispatchEvent;
  const realmFetch = globalThis.fetch;
  // Captured now: a page's fake timers installed later do not stall loads. A realm without
  // timers (embedder tests) queues microtasks instead.
  const setTimer = typeof setTimeout === 'function' ? setTimeout : (fn) => { Promise.resolve().then(fn); };
  const PromiseCtor = Promise;
  const then = Promise.prototype.then;
  const isView = ArrayBuffer.isView;
  const sliceBuffer = ArrayBuffer.prototype.slice;
  const freeze = Object.freeze;
  const inWindow = typeof document !== 'undefined' && typeof Document !== 'undefined' && document instanceof Document;
  const baseURL = () => {
    try { return inWindow ? document.baseURI : String(self.location.href); } catch (e) { return 'about:blank'; }
  };
  const markHandled = (p) => { apply(then, p, [undefined, () => {}]); return p; };
  const deferred = () => {
    let resolve, reject;
    const promise = new PromiseCtor((res, rej) => { resolve = res; reject = rej; });
    return { promise, resolve, reject };
  };
  const queueTask = (fn) => setTimer(fn, 0);

  // The `FontFace` attributes and the descriptor each reflects (`stretch` and `width` both
  // reflect `font-width`), with their initial values; in the dictionary's member order.
  const ATTRS = [
    ['ascentOverride', 'ascent-override', 'normal'],
    ['descentOverride', 'descent-override', 'normal'],
    ['display', 'font-display', 'auto'],
    ['featureSettings', 'font-feature-settings', 'normal'],
    ['lineGapOverride', 'line-gap-override', 'normal'],
    ['sizeAdjust', 'size-adjust', '100%'],
    ['stretch', 'font-stretch', 'normal'],
    ['style', 'font-style', 'normal'],
    ['unicodeRange', 'unicode-range', 'U+0-10FFFF'],
    ['variationSettings', 'font-variation-settings', 'normal'],
    ['weight', 'font-weight', 'normal'],
    ['width', 'font-width', 'normal'],
  ];
  const SLOT = { width: 'stretch' };
  const slotOf = (attr) => SLOT[attr] || attr;

  // A family name as `FontFace.family` serializes it: as written when it is a valid
  // <family-name> of identifiers, else a CSS string.
  const GENERIC = /^(serif|sans-serif|cursive|fantasy|monospace|system-ui|math|emoji|fangsong|ui-serif|ui-sans-serif|ui-monospace|ui-rounded)$/i;
  const KEYWORD = /^(inherit|initial|unset|default|revert|revert-layer)$/i;
  const IDENT = /^-?[A-Za-z_\u0080-￿][A-Za-z0-9_\-\u0080-￿]*$/;
  const cssString = (s) => '"' + s.replace(/[\\"]/g, (c) => '\\' + c).replace(/[\0-\x1f\x7f]/g, (c) => c === '\0' ? '�' : '\\' + c.charCodeAt(0).toString(16) + ' ') + '"';
  const serializeFamily = (name) => {
    if (/^"(?:[^"\\]|\\.)*"$/.test(name)) return name;
    const parts = name.split(' ');
    const ok = name !== '' && !GENERIC.test(name) && parts.every((p) => IDENT.test(p) && !KEYWORD.test(p) && !/^--/.test(p));
    return ok ? name : cssString(name);
  };
  const familyKey = (name) => {
    const m = /^"((?:[^"\\]|\\.)*)"$/.exec(name);
    return (m ? m[1].replace(/\\(.)/g, '$1') : name).toLowerCase();
  };

  const syntaxError = (what) => new DE(`Failed to set '${what}' property on 'FontFace': Failed to parse '${what}'.`, 'SyntaxError');
  const networkError = () => new DE('A network error occurred.', 'NetworkError');
  const CSS_FACE = Symbol('css-connected');
  const WORKER_SET = Symbol('worker-set');

  const bufferBytes = (source) => {
    if (source instanceof ArrayBuffer) return apply(sliceBuffer, source, [0]);
    if (isView(source)) {
      const view = new Uint8Array(source.buffer, source.byteOffset, source.byteLength);
      return apply(sliceBuffer, view.slice().buffer, [0]);
    }
    return null;
  };

  // A set's hooks for its faces' loads (assigned in FontFaceSet's static block), and the faces'
  // internals font matching and the renderer read (FontFace's static block).
  let setLoading, setDone, setChanged;
  const FF_internal = {};

  // Resolves once the layout engine is not fetching any of `sources` (asking first starts its
  // loads for the faces the document's styles use), at most 10 s.
  const engineSettled = (sources) => new PromiseCtor((resolve) => {
    try { enginePending(); } catch (e) {}
    const until = Date.now() + 10000;
    const tick = () => {
      let pending = false;
      try { pending = (sources || []).some(([kind, url]) => kind === 'url' && urlState(url) === 'loading'); } catch (e) {}
      if (pending && Date.now() < until) setTimer(tick, 5); else resolve();
    };
    tick();
  });

  // Each face's sets (a face in a set reports its loads to it).
  const faceSets = new WeakMap();
  const setsOf = (face) => { let s = faceSets.get(face); if (!s) faceSets.set(face, s = new Set()); return s; };

  class FontFace {
    #family = '';
    #familyName = '';
    #values = {};
    #status = 'unloaded';
    #loaded = deferred();
    #sources = null;   // [[kind, url-or-name]...] of a source string, null for bytes or a bad string
    #data = null;      // the face's bytes once loaded (or given)
    #css = false;
    #started = false;

    constructor(family, source, descriptors = undefined) {
      if (family === CSS_FACE) {
        // A CSS-connected face: [family, descriptors, sources] of an @font-face rule.
        const [name, list, srcs] = source;
        this.#css = true;
        this.#familyName = name;
        this.#family = serializeFamily(name);
        for (const [attr, , initial] of ATTRS) this.#values[slotOf(attr)] = initial;
        for (const [name2, value] of list) {
          const entry = ATTRS.find((a) => a[1] === name2);
          if (entry) this.#values[slotOf(entry[0])] = value;
        }
        this.#sources = srcs;
        markHandled(this.#loaded.promise);
        return;
      }
      if (arguments.length < 2) {
        throw new TypeError(`Failed to construct 'FontFace': 2 arguments required, but only ${arguments.length} present.`);
      }
      this.#familyName = `${family}`;
      this.#family = serializeFamily(this.#familyName);
      const bytes = bufferBytes(source);
      const text = bytes === null ? `${source}` : null;
      if (descriptors !== undefined && descriptors !== null && typeof descriptors !== 'object' && typeof descriptors !== 'function') {
        throw new TypeError("Failed to construct 'FontFace': The provided value is not of type 'FontFaceDescriptors'.");
      }
      let failed = false;
      for (const [attr, name, initial] of ATTRS) {
        const slot = slotOf(attr);
        const given = descriptors == null ? undefined : descriptors[attr];
        if (given === undefined) {
          if (!(slot in this.#values)) this.#values[slot] = initial;
          continue;
        }
        const parsed = descriptor(name, `${given}`);
        if (parsed === null) { failed = true; this.#values[slot] = initial; } else this.#values[slot] = parsed;
      }
      markHandled(this.#loaded.promise);
      if (failed) {
        this.#fail(new DE("Failed to construct 'FontFace': Failed to parse a descriptor.", 'SyntaxError'));
        return;
      }
      if (text !== null) {
        this.#sources = sources(text, baseURL());
        if (this.#sources === null || this.#sources.length === 0) {
          this.#sources = null;
          this.#fail(new DE(`Failed to construct 'FontFace': The source provided ('${text}') could not be parsed as a value list.`, 'SyntaxError'));
        }
        return;
      }
      // Bytes: loading until parsed (in a task, as in parallel).
      this.#status = 'loading';
      this.#started = true;
      queueTask(() => {
        if (isFont(bytes)) { this.#data = bytes; this.#succeed(); }
        else this.#fail(new DE('Failed to load font: the font data is invalid.', 'SyntaxError'));
      });
    }

    #fail(error) {
      this.#status = 'error';
      this.#loaded.reject(error);
      for (const set of setsOf(this)) setDone(set, this, false);
    }

    #succeed() {
      this.#status = 'loaded';
      this.#loaded.resolve(this);
      for (const set of setsOf(this)) setDone(set, this, true);
    }

    get family() { FontFace.#check(this); return this.#family; }
    set family(value) {
      FontFace.#check(this);
      this.#familyName = `${value}`;
      this.#family = serializeFamily(this.#familyName);
      this.#changed();
    }

    get status() {
      FontFace.#check(this);
      if (this.#status === 'unloaded' && this.#sources && inWindow && (this.#css || setsOf(this).has(documentSet))) {
        // A face the layout engine loads when text uses it.
        for (const [kind, url] of this.#sources) {
          if (kind !== 'url') continue;
          const state = urlState(url);
          if (state !== null) return state;
        }
      }
      return this.#status;
    }
    get loaded() {
      if (!FontFace.#isFace(this)) return PromiseCtor.reject(new TypeError('Illegal invocation'));
      return this.#loaded.promise;
    }

    load() {
      if (!FontFace.#isFace(this)) return PromiseCtor.reject(new TypeError('Illegal invocation'));
      if (this.#started || this.#status !== 'unloaded') return this.#loaded.promise;
      this.#started = true;
      this.#status = 'loading';
      for (const set of setsOf(this)) setLoading(set, this);
      // A load the document's load event waits for, as Chromium's font loads.
      const counted = inWindow && !!loading;
      if (counted) { try { loading(1); } catch (e) {} }
      const done = () => { if (counted) { try { loading(-1); } catch (e) {} } };
      this.#loaded.promise.then(done, done);
      const list = this.#sources || [];
      const next = (i) => {
        if (i >= list.length) { this.#fail(networkError()); return; }
        const [kind, url] = list[i];
        // `local()` fonts are not looked up: the next source is tried, as for a missing one.
        if (kind !== 'url') { next(i + 1); return; }
        let request;
        // CSP: the fetch is a "font" request (font-src), not connect-src.
        native.fontFetch();
        try { request = apply(realmFetch, globalThis, [url, { mode: 'cors', credentials: 'same-origin' }]); }
        catch (e) { next(i + 1); return; }
        apply(then, request, [
          (response) => (response.ok ? response.arrayBuffer() : null),
          () => null,
        ]).then((buffer) => {
          if (buffer && isFont(buffer)) {
            this.#data = buffer;
            // A CSS-connected face is loaded once the layout engine has it too (its own fetch
            // of the same font): measurements after `load()` see the face, as in Chromium,
            // where there is only one load. (A script-added face goes to the engine as its
            // bytes.)
            if (inWindow && this.#css) engineSettled(this.#sources).then(() => this.#succeed());
            else this.#succeed();
          }
          else next(i + 1);
        }, () => next(i + 1));
      };
      next(0);
      return this.#loaded.promise;
    }

    // Internal: whether the face is CSS-connected; its family key, descriptor values, sources
    // and bytes (what font matching and the renderer read).
    static #render(face) {
      if (face.#status === 'error') return null;
      const list = [];
      for (const [attr, name, initial] of ATTRS) {
        if (attr === 'width') continue;
        const value = face.#values[slotOf(attr)];
        if (value !== initial) list.push([name, value]);
      }
      return [face.#familyName, list, face.#data, face.#sources || []];
    }
    static #isFace(o) { return o !== null && typeof o === 'object' && #status in o; }
    static #check(o) {
      if (!FontFace.#isFace(o)) throw new TypeError('Illegal invocation');
    }
    #changed() {
      for (const set of setsOf(this)) setChanged(set, this);
    }
    static #setValue(face, attr, value) {
      FontFace.#check(face);
      const entry = ATTRS.find((a) => a[0] === attr);
      const parsed = descriptor(entry[1], `${value}`);
      if (parsed === null) throw syntaxError(attr);
      face.#values[slotOf(attr)] = parsed;
      face.#changed();
    }
    static {
      FF_internal.css = (face) => face.#css;
      // Only `local()` sources: no fetch to wait for, a set loads it when it is added.
      FF_internal.localOnly = (face) => face.#status === 'unloaded' && !!face.#sources && face.#sources.every(([kind]) => kind === 'local');
      FF_internal.key = (face) => familyKey(face.#familyName);
      FF_internal.values = (face) => face.#values;
      FF_internal.render = (face) => FontFace.#render(face);
      FF_internal.isFace = (o) => FontFace.#isFace(o);
      FF_internal.setValue = (face, attr, value) => FontFace.#setValue(face, attr, value);
    }
  }
  for (const [attr] of ATTRS) {
    const accessors = {
      get [attr]() { if (!FF_internal.isFace(this)) throw new TypeError('Illegal invocation'); return FF_internal.values(this)[slotOf(attr)]; },
      set [attr](value) { FF_internal.setValue(this, attr, value); },
    };
    const desc = Object.getOwnPropertyDescriptor(accessors, attr);
    desc.enumerable = true;
    Object.defineProperty(FontFace.prototype, attr, desc);
  }

  class FontFaceSetLoadEvent extends Ev {
    #fontfaces;
    constructor(type, init = undefined) {
      if (arguments.length < 1) throw new TypeError("Failed to construct 'FontFaceSetLoadEvent': 1 argument required, but only 0 present.");
      super(type, init);
      const list = init == null || init.fontfaces === undefined ? [] : [...init.fontfaces];
      for (const f of list) {
        if (!FF_internal.isFace(f)) throw new TypeError("Failed to construct 'FontFaceSetLoadEvent': Failed to read the 'fontfaces' property from 'FontFaceSetLoadEventInit': Failed to convert value to 'FontFace'.");
      }
      this.#fontfaces = freeze(list);
    }
    get fontfaces() {
      if (!(this !== null && typeof this === 'object' && #fontfaces in this)) throw new TypeError('Illegal invocation');
      return this.#fontfaces;
    }
  }

  // Font matching (CSS Fonts 4 §5.2) over a family's faces, by width, then style, then weight.
  const STRETCH = { 'ultra-condensed': 50, 'extra-condensed': 62.5, condensed: 75, 'semi-condensed': 87.5, normal: 100,
    'semi-expanded': 112.5, expanded: 125, 'extra-expanded': 150, 'ultra-expanded': 200 };
  const num = (t, unit) => { const v = parseFloat(t); return Number.isFinite(v) ? v : null; };
  const weightRange = (t) => {
    const parts = t.split(/\s+/).map((p) => (p === 'normal' ? 400 : p === 'bold' ? 700 : p === 'bolder' ? 700 : p === 'lighter' ? 100 : num(p)));
    const a = parts[0] ?? 400, b = parts[1] ?? a;
    return [Math.min(a, b), Math.max(a, b)];
  };
  const stretchRange = (t) => {
    const parts = t.split(/\s+/).map((p) => (p in STRETCH ? STRETCH[p] : num(p)));
    const a = parts[0] ?? 100, b = parts[1] ?? a;
    return [Math.min(a, b), Math.max(a, b)];
  };
  // [kind, low, high]: kind 0 normal, 1 italic, 2 oblique (angles in degrees).
  const styleOf = (t) => {
    const p = t.trim().split(/\s+/);
    if (p[0] === 'italic') return [1, 0, 0];
    if (p[0] === 'oblique') {
      const a = p[1] ? num(p[1]) : 14, b = p[2] ? num(p[2]) : a;
      return [2, Math.min(a, b), Math.max(a, b)];
    }
    return [0, 0, 0];
  };
  const distanceTo = (range, v) => (v < range[0] ? range[0] - v : v > range[1] ? v - range[1] : 0);
  const pick = (faces, score) => {
    let best = Infinity, out = [];
    for (const f of faces) {
      const s = score(f);
      if (s < best) { best = s; out = [f]; } else if (s === best) out.push(f);
    }
    return out;
  };
  const matchStyle = (faces, query) => {
    const want = stretchRange(query.stretch)[0];
    faces = pick(faces, (f) => {
      const r = stretchRange(FF_internal.values(f).stretch);
      if (want >= r[0] && want <= r[1]) return 0;
      const below = r[1] < want;
      if (want <= 100) return below ? want - r[1] : 1000 + r[0] - want;
      return below ? 1000 + want - r[1] : r[0] - want;
    });
    const [kind] = styleOf(query.style);
    const order = kind === 1 ? [1, 2, 0] : kind === 2 ? [2, 1, 0] : [0, 2, 1];
    faces = pick(faces, (f) => order.indexOf(styleOf(FF_internal.values(f).style)[0]));
    const w = weightRange(query.weight)[0];
    faces = pick(faces, (f) => {
      const r = weightRange(FF_internal.values(f).weight);
      if (w >= r[0] && w <= r[1]) return 0;
      if (w >= 400 && w <= 500) {
        if (r[0] > w && r[0] <= 500) return r[0] - w;
        if (r[1] < w) return 1000 + w - r[1];
        return 2000 + r[0] - w;
      }
      if (w < 400) return r[1] < w ? w - r[1] : 1000 + r[0] - w;
      return r[0] > w ? r[0] - w : 1000 + w - r[1];
    });
    return faces;
  };
  const rangesOf = (t) => t.split(',').map((r) => {
    const m = /U\+([0-9A-F?]+)(?:-([0-9A-F]+))?/i.exec(r.trim());
    if (!m) return [0, 0x10ffff];
    if (m[1].includes('?')) return [parseInt(m[1].replace(/\?/g, '0'), 16), parseInt(m[1].replace(/\?/g, 'F'), 16)];
    const a = parseInt(m[1], 16);
    return [a, m[2] ? parseInt(m[2], 16) : a];
  });
  const covers = (face, text) => {
    if (text === '') return true;
    const ranges = rangesOf(FF_internal.values(face).unicodeRange);
    for (const ch of text) {
      const c = ch.codePointAt(0);
      if (ranges.some(([a, b]) => c >= a && c <= b)) return true;
    }
    return false;
  };

  let documentSet = null;

  class FontFaceSet extends ET {
    #faces = [];             // non-CSS-connected faces, in insertion order
    #document = false;
    #cssCache = new Map();
    #loading = new Set();
    #loadedFonts = [];
    #failedFonts = [];
    #ready = null;
    #readyPending = false;
    #handlers = { loading: null, loadingdone: null, loadingerror: null };
    #listening = { loading: false, loadingdone: false, loadingerror: false };

    // Not constructible by script (the constructor was removed from the spec): the realm makes
    // its one set.
    constructor() {
      const token = arguments[0];
      if (token !== CSS_FACE && token !== WORKER_SET) throw new TypeError('Illegal constructor');
      super();
      this.#document = token === CSS_FACE;
      this.#ready = deferred();
      this.#readyPending = true;
      this.#settleSoon();
    }

    static #check(o) {
      if (!(o !== null && typeof o === 'object' && #faces in o)) throw new TypeError('Illegal invocation');
    }

    // The CSS-connected faces of the document's sheets (the same object per rule while it stays).
    #cssFaces() {
      if (!this.#document) return [];
      let list;
      try { list = cssFaces(); } catch (e) { return []; }
      const next = new Map();
      const out = [];
      const seen = new Map();
      for (const entry of list) {
        const base = JSON.stringify(entry);
        const n = (seen.get(base) || 0) + 1;
        seen.set(base, n);
        const key = base + '#' + n;
        let face = this.#cssCache.get(key);
        if (!face) face = new FontFace(CSS_FACE, entry);
        next.set(key, face);
        setsOf(face).add(this);
        out.push(face);
      }
      for (const [key, face] of this.#cssCache) if (!next.has(key)) setsOf(face).delete(this);
      this.#cssCache = next;
      return out;
    }
    #entries() { return [...this.#cssFaces(), ...this.#faces]; }

    #insert(face) {
      if (this.#faces.includes(face)) return;
      this.#faces.push(face);
      setsOf(face).add(this);
      if (face.status === 'loading') this.#faceLoading(face);
      else if (FF_internal.localOnly(face)) face.load();
    }

    #sync() {
      if (!this.#document || !register) return;
      const entries = [];
      for (const face of this.#faces) {
        const entry = FF_internal.render(face);
        if (entry) entries.push(entry);
      }
      try { register(entries); } catch (e) {}
    }

    get size() { FontFaceSet.#check(this); return this.#entries().length; }
    has(face) {
      FontFaceSet.#check(this);
      if (arguments.length < 1) throw new TypeError("Failed to execute 'has' on 'FontFaceSet': 1 argument required, but only 0 present.");
      return this.#entries().includes(face); }
    add(face) {
      FontFaceSet.#check(this);
      if (arguments.length < 1) throw new TypeError("Failed to execute 'add' on 'FontFaceSet': 1 argument required, but only 0 present.");
      if (!FF_internal.isFace(face)) throw new TypeError("Failed to execute 'add' on 'FontFaceSet': parameter 1 is not of type 'FontFace'.");
      if (this.#faces.includes(face)) return this;
      if (FF_internal.css(face)) {
        if (this.#cssFaces().includes(face)) return this;
        throw new DE("Failed to execute 'add' on 'FontFaceSet': Cannot add a CSS-connected FontFace.", 'InvalidModificationError');
      }
      this.#insert(face);
      this.#sync();
      return this;
    }
    delete(face) {
      FontFaceSet.#check(this);
      if (arguments.length < 1) throw new TypeError("Failed to execute 'delete' on 'FontFaceSet': 1 argument required, but only 0 present.");
      if (!FF_internal.isFace(face)) return false;
      const i = this.#faces.indexOf(face);
      if (i < 0) return false;
      this.#faces.splice(i, 1);
      setsOf(face).delete(this);
      if (this.#loading.delete(face)) this.#maybeLoaded();
      this.#sync();
      return true;
    }
    clear() {
      FontFaceSet.#check(this);
      for (const face of this.#faces) { setsOf(face).delete(this); this.#loading.delete(face); }
      this.#faces = [];
      this.#maybeLoaded();
      this.#sync();
    }
    forEach(callback, thisArg = undefined) {
      FontFaceSet.#check(this);
      if (typeof callback !== 'function') throw new TypeError("Failed to execute 'forEach' on 'FontFaceSet': The callback provided as parameter 1 is not a function.");
      for (const face of this.#entries()) apply(callback, thisArg, [face, face, this]);
    }
    values() { FontFaceSet.#check(this); return this.#entries()[Symbol.iterator](); }
    keys() { FontFaceSet.#check(this); return this.#entries()[Symbol.iterator](); }
    entries() { FontFaceSet.#check(this); return this.#entries().map((f) => [f, f])[Symbol.iterator](); }

    get status() {
      FontFaceSet.#check(this);
      return this.#loading.size > 0 || this.#environmentPending() ? 'loading' : 'loaded';
    }
    get ready() {
      if (!(this !== null && typeof this === 'object' && #faces in this)) return PromiseCtor.reject(new TypeError('Illegal invocation'));
      if (!this.#readyPending && this.#environmentPending()) {
        this.#ready = deferred();
        this.#readyPending = true;
        this.#settleSoon();
      }
      return this.#ready.promise;
    }

    #environmentPending() {
      if (!this.#document) return false;
      try { return !!enginePending(); } catch (e) { return false; }
    }

    // Resolves `ready` once nothing loads (the layout engine's fetches polled, at most 10 s).
    #settleSoon() {
      const until = Date.now() + 10000;
      const tick = () => {
        if (!this.#readyPending) return;
        if (this.#loading.size > 0) return; // `#faceDone` settles it
        if (this.#environmentPending() && Date.now() < until) { setTimer(tick, 5); return; }
        this.#readyPending = false;
        this.#ready.resolve(this);
      };
      queueTask(tick);
    }

    #fire(type, faces) {
      const event = new FontFaceSetLoadEvent(type, { fontfaces: faces });
      queueTask(() => apply(dispatch, this, [event]));
    }

    // A face of this set started loading, finished, or changed a descriptor.
    #faceLoading(face) {
      if (this.#loading.has(face)) return;
      const was = this.#loading.size;
      this.#loading.add(face);
      if (was === 0) {
        if (!this.#readyPending) { this.#ready = deferred(); this.#readyPending = true; }
        this.#fire('loading', []);
      }
    }
    #faceDone(face, ok) {
      if (this.#loading.delete(face)) (ok ? this.#loadedFonts : this.#failedFonts).push(face);
      if (ok && this.#faces.includes(face)) this.#sync();
      this.#maybeLoaded();
    }
    #faceChanged(face) {
      if (this.#faces.includes(face)) this.#sync();
    }
    static {
      setLoading = (set, face) => set.#faceLoading(face);
      setDone = (set, face, ok) => set.#faceDone(face, ok);
      setChanged = (set, face) => set.#faceChanged(face);
    }
    #maybeLoaded() {
      if (this.#loading.size > 0) return;
      const loaded = this.#loadedFonts, failed = this.#failedFonts;
      if (loaded.length || failed.length) {
        this.#loadedFonts = [];
        this.#failedFonts = [];
        this.#fire('loadingdone', loaded);
        if (failed.length) this.#fire('loadingerror', failed);
      }
      if (this.#readyPending) this.#settleSoon();
    }

    #match(font, text, what) {
      const query = fontQuery(`${font}`);
      if (query === null) {
        throw new DE(`Failed to execute '${what}' on 'FontFaceSet': Could not resolve '${font}' as a font.`, 'SyntaxError');
      }
      const all = this.#entries();
      const found = [];
      for (const family of query.families) {
        const key = family.toLowerCase();
        const faces = all.filter((f) => FF_internal.key(f) === key);
        if (!faces.length) continue;
        for (const f of matchStyle(faces, query)) if (covers(f, text) && !found.includes(f)) found.push(f);
      }
      return found;
    }

    check(font, text = ' ') {
      FontFaceSet.#check(this);
      if (arguments.length < 1) throw new TypeError("Failed to execute 'check' on 'FontFaceSet': 1 argument required, but only 0 present.");
      const found = this.#match(font, `${text}`, 'check');
      return found.every((f) => f.status === 'loaded');
    }

    load(font, text = ' ') {
      try {
        FontFaceSet.#check(this);
        if (arguments.length < 1) throw new TypeError("Failed to execute 'load' on 'FontFaceSet': 1 argument required, but only 0 present.");
        const found = this.#match(font, `${text}`, 'load');
        return PromiseCtor.all(found.map((f) => f.load()));
      } catch (e) {
        return PromiseCtor.reject(e);
      }
    }

    #setHandler(type, value) {
      const handler = typeof value === 'function' || (value !== null && typeof value === 'object') ? value : null;
      this.#handlers[type] = handler;
      if (handler !== null && !this.#listening[type]) {
        this.#listening[type] = true;
        apply(addListener, this, [type, (event) => {
          const h = this.#handlers[type];
          if (typeof h === 'function') apply(h, this, [event]);
        }]);
      }
    }
    get onloading() { FontFaceSet.#check(this); return this.#handlers.loading; }
    set onloading(v) { FontFaceSet.#check(this); this.#setHandler('loading', v); }
    get onloadingdone() { FontFaceSet.#check(this); return this.#handlers.loadingdone; }
    set onloadingdone(v) { FontFaceSet.#check(this); this.#setHandler('loadingdone', v); }
    get onloadingerror() { FontFaceSet.#check(this); return this.#handlers.loadingerror; }
    set onloadingerror(v) { FontFaceSet.#check(this); this.#setHandler('loadingerror', v); }
  }
  // Setlike: `keys` and @@iterator are `values`.
  Object.defineProperty(FontFaceSet.prototype, Symbol.iterator, { value: FontFaceSet.prototype.values, writable: true, configurable: true });
  // Web IDL members are enumerable; the classes' prototypes carry their tags.
  const enumerate = (proto, names) => {
    for (const name of names) {
      const desc = Object.getOwnPropertyDescriptor(proto, name);
      if (!desc) continue;
      desc.enumerable = true;
      Object.defineProperty(proto, name, desc);
    }
  };
  enumerate(FontFace.prototype, ['family', 'status', 'loaded', 'load']);
  enumerate(FontFaceSet.prototype, ['size', 'has', 'add', 'delete', 'clear', 'forEach', 'values', 'keys', 'entries', 'status', 'ready', 'check', 'load', 'onloading', 'onloadingdone', 'onloadingerror']);
  enumerate(FontFaceSetLoadEvent.prototype, ['fontfaces']);
  for (const [cls, name] of [[FontFace, 'FontFace'], [FontFaceSet, 'FontFaceSet'], [FontFaceSetLoadEvent, 'FontFaceSetLoadEvent']]) {
    Object.defineProperty(cls.prototype, Symbol.toStringTag, { value: name, configurable: true });
  }

  const fonts = new FontFaceSet(inWindow ? CSS_FACE : WORKER_SET);
  if (inWindow) documentSet = fonts;
  return { FontFace, FontFaceSet, FontFaceSetLoadEvent, fonts };
});
