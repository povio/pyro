// WebVTT cue text has its own tokenizer, restricted element vocabulary and tree rules.
(function (decodeReferences, foreignState) {
  'use strict';
  const cues = new WeakMap(), regions = new WeakMap(), construct = Symbol();
  const define = Object.defineProperty;
  const add = EventTarget.prototype.addEventListener, remove = EventTarget.prototype.removeEventListener;
  const string = value => `${value}`;
  function finite(value) {
    const n = +value;
    if (!Number.isFinite(n)) throw new TypeError('Expected a finite number.');
    return n;
  }
  function endTime(value) {
    const n = +value;
    if (Number.isNaN(n) || n === -Infinity) throw new TypeError('Invalid cue end time.');
    return n;
  }
  function range(n) {
    if (n < 0 || n > 100) throw new DOMException('Expected a percentage.', 'IndexSizeError');
    return n;
  }
  const percent = v => range(finite(v));
  function lineSetting(value) {
    if (typeof value === 'number') return finite(value);
    if (string(value) === 'auto') return 'auto';
    throw new TypeError('Expected a number or auto.');
  }
  function localState(kind, target) {
    if (kind === 'cue') return cues.get(target);
    if (kind === 'region') return regions.get(target);
  }
  function state(map, target) {
    const s = map.get(target) ?? foreignState(map === cues ? 'cue' : 'region', target);
    if (!s) throw new TypeError('Illegal invocation');
    return s;
  }
  function defineAttribute(proto, name, property) {
    define(property.get, 'name', { value: 'get ' + name, configurable: true });
    if (property.set) define(property.set, 'name', { value: 'set ' + name, configurable: true });
    define(proto, name, { enumerable: true, configurable: true, ...property });
  }
  function attribute(proto, map, name, convert) {
    const property = { get() { return state(map, this)[name]; } };
    if (convert) property.set = { set(v) { state(map, this)[name] = convert(v); } }.set;
    defineAttribute(proto, name, property);
  }
  function enumAttribute(proto, map, name, choices) {
    defineAttribute(proto, name, {
      get() { return state(map, this)[name]; },
      set(value) {
        const s = state(map, this), v = string(value);
        // A Web IDL attribute setter ignores an unrecognized enumeration value.
        if (choices.includes(v)) s[name] = v;
      }
    });
  }

  function cueHTML(text) {
    const doc = document, root = doc.createDocumentFragment();
    const stack = [{ name: '', node: root }];
    let pos = 0;
    while (pos < text.length) {
      const current = stack[stack.length - 1];
      if (text[pos] !== '<') {
        let end = text.indexOf('<', pos);
        if (end < 0) end = text.length;
        const value = text.slice(pos, end);
        current.node.appendChild(doc.createTextNode(value.includes('&') ? decodeReferences(value) : value));
        pos = end;
        continue;
      }
      let end = text.indexOf('>', pos + 1);
      if (end < 0) end = text.length;
      const tag = text.slice(pos + 1, end);
      pos = end + 1;
      if (tag.startsWith('/')) {
        const name = tag.slice(1);
        if (name === 'ruby' && current.name === 'rt') { stack.pop(); stack.pop(); }
        else if (name && current.name === name) stack.pop();
        continue;
      }
      if (/^[0-9]/.test(tag)) {
        const time = /^(?:(\d+):)?([0-5]\d):([0-5]\d)\.(\d{3})$/.exec(tag);
        if (time && time[0].length === tag.length) {
          const hours = (time[1] || '0').replace(/^0+(?=\d)/, '').padStart(2, '0');
          current.node.appendChild(doc.createProcessingInstruction('timestamp',
            hours + ':' + time[2] + ':' + time[3] + '.' + time[4]));
        }
        continue;
      }
      const split = tag.search(/[\t\n\f ]/);
      const names = (split < 0 ? tag : tag.slice(0, split)).split('.');
      const name = names.shift();
      if (!['c', 'i', 'b', 'u', 'ruby', 'rt', 'v', 'lang'].includes(name)) continue;
      if (name === 'rt' && current.name !== 'ruby') continue;
      const element = doc.createElementNS('http://www.w3.org/1999/xhtml',
        name === 'c' || name === 'v' || name === 'lang' ? 'span' : name);
      const classes = names.filter(Boolean);
      if (classes.length) element.setAttribute('class', classes.join(' '));
      if (name === 'v' || name === 'lang') {
        let annotation = split < 0 ? '' : tag.slice(split + 1);
        if (annotation.includes('&')) annotation = decodeReferences(annotation);
        annotation = annotation.replace(/[\t\r\n\f ]+/g, ' ').replace(/^ | $/g, '');
        element.setAttribute(name === 'v' ? 'title' : 'lang', annotation);
      }
      current.node.appendChild(element);
      stack.push({name, node: element});
    }
    return root;
  }

  class TextTrackCue extends EventTarget {
    constructor(...args) {
      super();
      if (args[0] !== construct) throw new TypeError('Illegal constructor');
    }
  }
  class VTTCue extends TextTrackCue {
    constructor(startTime, end, text) {
      super(construct);
      if (arguments.length < 3) throw new TypeError('Expected a start time, end time and text.');
      const start = finite(startTime), finish = +end, content = string(text);
      cues.set(this, { startTime: start, endTime: endTime(finish), text: content, id: '', track: null,
        pauseOnExit: false, onenter: null, onexit: null, region: null, vertical: '', snapToLines: true,
        line: 'auto', lineAlign: 'start', position: 'auto', positionAlign: 'auto', size: 100, align: 'center' });
    }
    getCueAsHTML() { return cueHTML(state(cues, this).text); }
  }
  class VTTRegion {
    constructor() {
      regions.set(this, {id: '', width: 100, lines: 3, regionAnchorX: 0, regionAnchorY: 100,
        viewportAnchorX: 0, viewportAnchorY: 100, scroll: ''});
    }
  }
  for (const proto of [TextTrackCue.prototype, VTTCue.prototype, VTTRegion.prototype])
    define(proto, Symbol.toStringTag, { value: proto.constructor.name, configurable: true });
  const base = TextTrackCue.prototype, cue = VTTCue.prototype, region = VTTRegion.prototype;
  define(cue, 'getCueAsHTML', {enumerable: true});
  attribute(base, cues, 'track');
  attribute(base, cues, 'id', string);
  attribute(base, cues, 'startTime', finite);
  attribute(base, cues, 'endTime', endTime);
  attribute(base, cues, 'pauseOnExit', Boolean);
  for (const type of ['enter', 'exit']) {
    const name = 'on' + type, listener = name + 'Listener';
    defineAttribute(base, name, {
      get() { return state(cues, this)[name]; },
      set(value) {
        const s = state(cues, this);
        s[name] = typeof value === 'function' ? value : null;
        if (s[name] && !s[listener]) {
          s[listener] = e => Reflect.apply(s[name], this, [e]);
          Reflect.apply(add, this, [type, s[listener]]);
        } else if (!s[name] && s[listener]) {
          Reflect.apply(remove, this, [type, s[listener]]);
          s[listener] = null;
        }
      }
    });
  }
  attribute(cue, cues, 'text', string);
  attribute(cue, cues, 'size', percent);
  attribute(cue, cues, 'line', lineSetting);
  attribute(cue, cues, 'position', v => { const n = lineSetting(v); return n === 'auto' ? n : range(n); });
  attribute(cue, cues, 'snapToLines', Boolean);
  attribute(cue, cues, 'region', value => {
    if (value == null) return null;
    state(regions, value);
    return value;
  });
  enumAttribute(cue, cues, 'vertical', ['', 'rl', 'lr']);
  enumAttribute(cue, cues, 'lineAlign', ['start', 'center', 'end']);
  enumAttribute(cue, cues, 'positionAlign', ['line-left', 'center', 'line-right', 'auto']);
  enumAttribute(cue, cues, 'align', ['start', 'center', 'end', 'left', 'right']);
  attribute(region, regions, 'id', string);
  attribute(region, regions, 'lines', v => v >>> 0);
  for (const name of ['width', 'regionAnchorX', 'regionAnchorY', 'viewportAnchorX', 'viewportAnchorY'])
    attribute(region, regions, name, percent);
  enumAttribute(region, regions, 'scroll', ['', 'up']);

  function parsedRegion(descriptor) {
    const region = new VTTRegion();
    Object.assign(regions.get(region), descriptor);
    return region;
  }
  function parsedCue(descriptor, regionList) {
    const cue = new VTTCue(descriptor.startTime, descriptor.endTime, descriptor.text);
    const s = cues.get(cue);
    s.id = descriptor.id;
    Object.assign(s, descriptor.settings);
    if (typeof s.region === 'number') s.region = regionList[s.region] ?? null;
    return cue;
  }
  function cueTiming(cue) {
    const s = state(cues, cue);
    return [s.startTime, s.endTime, s.id, s.pauseOnExit];
  }
  return {TextTrackCue, VTTCue, VTTRegion, parsedRegion, parsedCue, cueTiming,
    localState,
    cueOwner: cue => state(cues, cue).track, setCueOwner(cue, track) { state(cues, cue).track = track; }};
})
