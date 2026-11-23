// Window MediaSource and SourceBuffer control algorithms. All segment state,
// codec validation and buffered timestamps belong to the coded-frame model.
(function (native) {
  'use strict';
  const secret = Symbol(), buffers = new WeakMap(), lists = new WeakMap();
  const define = Object.defineProperty, get = Object.getOwnPropertyDescriptor;
  const U8 = Uint8Array, apply = Reflect.apply;
  const bufferLength = get(ArrayBuffer.prototype, 'byteLength').get;
  const typed = Object.getPrototypeOf(U8.prototype);
  const viewInfo = proto => ['buffer','byteOffset','byteLength'].map(k => get(proto,k).get);
  const typedInfo = viewInfo(typed), dvInfo = viewInfo(DataView.prototype);
  function bytes(value) {
    let buffer = value, offset = 0, length;
    if (ArrayBuffer.isView(value)) {
      let info;
      try { buffer = apply(typedInfo[0],value,[]); info = typedInfo; }
      catch { buffer = apply(dvInfo[0],value,[]); info = dvInfo; }
      offset = apply(info[1],value,[]);
      length = apply(info[2],value,[]);
    }
    const size = apply(bufferLength,buffer,[]);
    return new U8(buffer,offset,length ?? size);
  }
  function invalid() { throw new DOMException('Invalid MediaSource state.', 'InvalidStateError'); }
  function unsupported() { throw new DOMException('Unsupported media type.', 'NotSupportedError'); }
  function finite(v) {
    const n = +v;
    if (!Number.isFinite(n)) throw new TypeError('Expected a finite double.');
    return n;
  }
  function slot(map, obj) {
    const s = map.get(obj);
    if (!s) throw new TypeError('Illegal invocation');
    return s;
  }
  function fire(target, type) { native.postTask(() => native.fire(target,type)); }
  function handler(proto, type) {
    const name = 'on' + type;
    const handlers = new WeakMap();
    define(proto, name, { enumerable:true, configurable:true,
      get() { return handlers.get(this)?.value ?? null; },
      set(v) {
        const old = handlers.get(this);
        if (typeof v !== 'function') {
          if (old) { this.removeEventListener(type,old.fn); handlers.delete(this); }
        } else if (old) old.value = v;
        else {
          const entry = {value:v, fn:e => apply(entry.value,this,[e])};
          handlers.set(this,entry);
          this.addEventListener(type,entry.fn);
        }
      }
    });
  }
  class SourceBufferList extends EventTarget {
    constructor(token) {
      super();
      if (token !== secret) throw new TypeError('Illegal constructor');
      lists.set(this,[]);
    }
    get length() { return slot(lists,this).length; }
    [Symbol.iterator]() { return slot(lists,this).values(); }
  }
  function listAdd(list, buffer, beforeIndex) {
    const items = lists.get(list), i = items.length;
    items.splice(beforeIndex ?? i,0,buffer);
    define(list,i,{enumerable:true,configurable:true,get:() => items[i]});
    fire(list,'addsourcebuffer');
  }
  function listRemove(list, buffer) {
    const items = lists.get(list), i = items.indexOf(buffer);
    if (i < 0) return;
    items.splice(i,1);
    delete list[items.length];
    fire(list,'removesourcebuffer');
  }
  // This presentation can decode one video and one audio track. Reserve
  // those slots before creating a buffer, including both slots for a mux.
  function overlaps(source, kinds, except) {
    return lists.get(source.sourceBuffers).some(buf => buf !== except && (buffers.get(buf).kinds & kinds));
  }
  function requireAttached(b) { if (!b.source) invalid(); }
  function idle(b) { requireAttached(b); if (b.updating) invalid(); }
  function reopen(s) {
    if (s.readyState === 'ended') { s.readyState = 'open'; fire(s.target,'sourceopen'); }
  }
  function abortUpdate(buffer,b) {
    if (!b.updating) return;
    b.generation++;
    b.updating = false;
    b.removing = false;
    fire(buffer,'abort');
    fire(buffer,'updateend');
  }
  function scheduleUpdate(buffer,b,removing,operation) {
    b.updating = true;
    b.removing = removing;
    const generation = ++b.generation;
    fire(buffer,'updatestart');
    native.postTask(() => {
      if (!b.source || b.generation !== generation) return;
      let success = false;
      try { operation(); success = true; }
      catch { /* Parser failures end the presentation with a decode error. */ }
      finally {
        b.removing = b.updating = false;
        fire(buffer,success ? 'update' : 'error');
        fire(buffer,'updateend');
        if (!success) {
          b.model('reset');
          finish(b.source,'decode');
        } else changed(b.source);
      }
    });
  }
  class SourceBuffer extends EventTarget {
    constructor(token,state) {
      super();
      if (token !== secret) throw new TypeError('Illegal constructor');
      buffers.set(this,state);
    }
    get updating() { return slot(buffers,this).updating; }
    get buffered() {
      const b = slot(buffers,this); requireAttached(b);
      return native.timeRanges(b.model('ranges',b.source.readyState === 'ended'));
    }
    get mode() { return slot(buffers,this).mode; }
    set mode(v) {
      const b = slot(buffers,this), mode = `${v}`;
      if (mode !== 'segments' && mode !== 'sequence') return;
      idle(b); reopen(b.source);
      if (b.model('partial')) invalid();
      b.model('mode',mode);
      b.mode = mode;
    }
    get timestampOffset() { const b = slot(buffers,this); return b.model ? b.model('offset') : b.lastOffset; }
    set timestampOffset(v) {
      const b = slot(buffers,this), offset = finite(v);
      idle(b); reopen(b.source);
      if (b.model('partial')) invalid();
      b.model('setOffset',offset);
    }
    get appendWindowStart() { return slot(buffers,this).start; }
    set appendWindowStart(v) {
      const b = slot(buffers,this), n = finite(v); idle(b);
      if (n < 0 || n >= b.end) throw new TypeError('Invalid append window start.');
      b.start = n; b.model('window',n,b.end);
    }
    get appendWindowEnd() { return slot(buffers,this).end; }
    set appendWindowEnd(v) {
      const b = slot(buffers,this), n = +v; idle(b);
      if (!(n > b.start)) throw new TypeError('Invalid append window end.');
      b.end = n; b.model('window',b.start,n);
    }
    appendBuffer(data) {
      const b = slot(buffers,this), input = bytes(data); idle(b);
      if (b.source.error) invalid();
      reopen(b.source);
      if (!b.model('canAppend',input.byteLength))
        throw new DOMException('SourceBuffer quota exceeded.', 'QuotaExceededError');
      const copy = new U8(input);
      scheduleUpdate(this,b,false,() => {
        const initialized = b.model('append',copy);
        // Subsequent init segments must preserve this buffer's track kinds.
        if (initialized) b.kinds = b.model('trackKinds');
        if (initialized && !lists.get(b.source.activeSourceBuffers).includes(this)) {
          const order = lists.get(b.source.sourceBuffers);
          const next = lists.get(b.source.activeSourceBuffers).findIndex(buf => order.indexOf(buf) > order.indexOf(this));
          listAdd(b.source.activeSourceBuffers,this,next < 0 ? undefined : next);
          if (Number.isNaN(b.source.duration)) b.source.duration = b.model('initDuration') ?? Infinity;
        }
        const highest = b.model('highestEnd');
        if (highest > b.source.duration) b.source.duration = highest;
      });
    }
    abort() {
      const b = slot(buffers,this); requireAttached(b);
      if (b.source.readyState !== 'open' || b.removing) invalid();
      abortUpdate(this,b);
      b.model('reset');
      b.start = 0; b.end = Infinity;
      b.model('window',0,Infinity);
    }
    changeType(value) {
      const b = slot(buffers,this);
      if (!arguments.length) throw new TypeError('Expected a media type.');
      const type = `${value}`;
      if (!type) throw new TypeError('Expected a non-empty media type.');
      idle(b);
      const kinds = native.typeKinds(type);
      const tracks = b.model('trackKinds');
      if (!kinds || (tracks & kinds) !== tracks || overlaps(b.source,tracks || kinds,this)) unsupported();
      reopen(b.source);
      b.model('change',type);
      b.kinds = tracks || kinds;
    }
    remove(start,end) {
      const b = slot(buffers,this), a = finite(start), z = +end;
      idle(b);
      if (Number.isNaN(b.source.duration) || a < 0 || a > b.source.duration || !(z > a))
        throw new TypeError('Invalid removal range.');
      reopen(b.source);
      scheduleUpdate(this,b,true,() => b.model('remove',a,z));
    }
  }
  for (const name of ['updatestart','update','updateend','error','abort']) handler(SourceBuffer.prototype,name);
  for (const name of ['addsourcebuffer','removesourcebuffer']) handler(SourceBufferList.prototype,name);
  for (const C of [SourceBuffer,SourceBufferList]) {
    define(C.prototype,Symbol.toStringTag,{value:C.name,configurable:true});
    define(C,'length',{value:0,configurable:true});
    for (const k of Object.getOwnPropertyNames(C.prototype))
      if (k !== 'constructor') define(C.prototype,k,{enumerable:true});
  }

  function removeBuffer(s,buffer) {
    const b = slot(buffers,buffer);
    if (b.source !== s) throw new DOMException('Unknown SourceBuffer.', 'NotFoundError');
    abortUpdate(buffer,b);
    listRemove(s.activeSourceBuffers,buffer);
    listRemove(s.sourceBuffers,buffer);
    b.lastOffset = b.model('offset'); b.model('release'); b.model = b.source = null;
  }
  function intersection(a,b) {
    const out = [];
    for (let i = 0,j = 0; i < a.length && j < b.length;) {
      const start = Math.max(a[i],b[j]), end = Math.min(a[i+1],b[j+1]);
      if (end > start) out.push(start,end);
      if (a[i+1] < b[j+1]) i += 2; else j += 2;
    }
    return out;
  }
  function buffered(s) {
    const ended = s.readyState === 'ended';
    const all = lists.get(s.activeSourceBuffers).map(buf => buffers.get(buf).model('ranges',ended));
    if (!all.length || all.some(r => !r.length)) return [];
    if (ended) {
      const end = Math.max(...all.map(r => r[r.length-1]));
      for (const r of all) r[r.length-1] = end;
    }
    return all.reduce(intersection);
  }
  function seekable(s,ranges) {
    if (Number.isFinite(s.duration)) return s.duration > 0 ? [0,s.duration] : [];
    if (Number.isNaN(s.duration)) return [];
    if (!s.live) return ranges.length ? [0,ranges[ranges.length-1]] : [];
    return ranges.length ? [Math.min(s.live[0],ranges[0]),Math.max(s.live[1],ranges[ranges.length-1])] : s.live.slice();
  }
  function changed(s) {
    if (!s.notify) return;
    const r = buffered(s);
    s.notify(s.error ?? 'update',s.duration,r,seekable(s,r),
      lists.get(s.sourceBuffers).length > 0 && lists.get(s.sourceBuffers).every(buf => buffers.get(buf).model('initialized')), s.readyState === 'ended');
  }
  function finish(s,error) {
    s.readyState = 'ended';
    s.error = error;
    if (error == null) s.duration = Math.max(0,...lists.get(s.sourceBuffers).map(buf => buffers.get(buf).model('highestEnd')));
    fire(s.target,'sourceended'); changed(s);
  }
  function control(target) {
    const s = {target, readyState:'closed',duration:NaN,notify:null,live:null,error:null,
      sourceBuffers:new SourceBufferList(secret),activeSourceBuffers:new SourceBufferList(secret)};
    return function (op,...args) {
      switch (op) {
        case 'readyState': case 'duration': case 'sourceBuffers': case 'activeSourceBuffers': return s[op];
        case 'attach':
          if (s.readyState !== 'closed') return false;
          s.notify = args[0]; s.readyState = 'open'; fire(target,'sourceopen'); return true;
        case 'detach':
          if (s.readyState === 'closed') return;
          s.notify = null; s.readyState = 'closed'; s.duration = NaN; s.error = s.live = null;
          for (const buffer of lists.get(s.sourceBuffers).slice()) removeBuffer(s,buffer);
          fire(target,'sourceclose'); return;
        case 'add': {
          const type = `${args[0]}`;
          if (!type) throw new TypeError('Expected a non-empty media type.');
          const kinds = native.typeKinds(type);
          if (!kinds) unsupported();
          if (overlaps(s,kinds))
            throw new DOMException('A SourceBuffer already uses this decoder kind.', 'QuotaExceededError');
          if (s.readyState !== 'open') invalid();
          const b = {model:native.makeBuffer(target,type),source:s,kinds,mode:'segments',
            start:0,end:Infinity,updating:false,removing:false,generation:0};
          const buffer = new SourceBuffer(secret,b);
          listAdd(s.sourceBuffers,buffer);
          return buffer;
        }
        case 'remove': removeBuffer(s,args[0]); changed(s); return;
        case 'setDuration': {
          const n = +args[0];
          if (!(n >= 0)) throw new TypeError('Invalid duration.');
          if (s.readyState !== 'open' || lists.get(s.sourceBuffers).some(buf => buffers.get(buf).updating)) invalid();
          const models = lists.get(s.sourceBuffers).map(buf => buffers.get(buf).model);
          if (models.some(m => (m('highestStart') ?? 0) > n)) invalid();
          s.duration = Math.max(n,...models.map(m => m('highestEnd'))); changed(s); return;
        }
        case 'end': {
          const error = args[0] === undefined ? null : `${args[0]}`;
          if (error !== null && !['network','decode'].includes(error)) throw new TypeError('Invalid endOfStream error.');
          if (s.readyState !== 'open' || lists.get(s.sourceBuffers).some(buf => buffers.get(buf).updating)) invalid();
          finish(s,error); return;
        }
        case 'setLive': {
          const a = finite(args[0]), z = finite(args[1]);
          if (s.readyState !== 'open') invalid();
          if (a < 0 || a > z) throw new TypeError('Invalid live seekable range.');
          s.live = [a,z]; changed(s); return;
        }
        case 'clearLive':
          if (s.readyState !== 'open') invalid();
          s.live = null; changed(s); return;
      }
    };
  }
  return {SourceBuffer,SourceBufferList,control};
})
