const test = require('node:test');
const assert = require('node:assert/strict');
const {readFileSync} = require('node:fs');
const {resolve} = require('node:path');
const vm = require('node:vm');

function fixture(makeBuffer) {
  const tasks = [], notices = [];
  const realm = vm.createContext({EventTarget,DOMException});
  const factory = vm.runInContext(readFileSync(resolve(__dirname,'../../src/media_source.js'),'utf8'),realm);
  const {control} = factory({
    typeKinds: type => ({'video/mp4':2,'audio/mp4':1,'mux':3})[type] ?? 0,
    postTask: fn => tasks.push(fn),
    fire: (target,type) => target.dispatchEvent(new Event(type)),
    timeRanges: values => [...values],
    makeBuffer: makeBuffer ?? function (_,type) {
      let end = 0, offset = 0, initial = false;
      return (op,arg) => {
        switch (op) {
          case 'append':
            if (arg[0] === 255) throw new Error('decode');
            initial = true;
            end = arg[0];
            return true;
          case 'initDuration': return 6;
          case 'ranges': return initial ? [0,end] : [];
          case 'highestEnd': return end;
          case 'highestStart': return null;
          case 'canAppend': return true;
          case 'initialized': return initial;
          case 'trackKinds': return initial ? ({'video/mp4':2,'audio/mp4':1,'mux':3})[type] : 0;
          case 'offset': return offset;
          case 'setOffset': offset = arg; break;
          case 'remove': end = Math.min(end,arg); break;
        }
      };
    }
  });
  const target = new EventTarget(), call = control(target);
  const attach = () => call('attach',(...args) => notices.push(args));
  return {call,attach,target,notices,drain() { while (tasks.length) tasks.shift()(); }};
}

test('append snapshots input, updates lists and publishes ranges before updateend', () => {
  const {call,attach,target,notices,drain} = fixture();
  const events = [];
  target.addEventListener('sourceopen',() => events.push('open'));
  assert.equal(call('readyState'),'closed');
  assert.ok(attach());
  assert.equal(call('readyState'),'open');
  assert.equal(attach(),false);
  const list = call('sourceBuffers'), active = call('activeSourceBuffers');
  const a = call('add','video/mp4'), b = call('add','audio/mp4');
  assert.equal(list[0],a);
  const data = Uint8Array.of(4);
  for (const type of ['updatestart','update','updateend']) b.addEventListener(type,() => events.push(type));
  b.appendBuffer(data); data[0] = 5;
  assert.equal(b.updating,true);
  assert.throws(() => call('end'),{name:'InvalidStateError'});
  drain();
  assert.deepEqual(events,['open','updatestart','update','updateend']);
  assert.deepEqual(b.buffered,[0,4]);
  assert.equal(active[0],b);
  a.appendBuffer(Uint8Array.of(3)); drain();
  assert.equal(active[0],a);
  assert.equal(active[1],b);
  assert.deepEqual([...notices.at(-1)[2]],[0,3]);
  assert.equal(call('duration'),6);
  call('end'); assert.equal(call('duration'),4);
  a.timestampOffset = 1;
  assert.equal(call('readyState'),'open');
  call('detach'); drain();
  assert.equal(list.length,0);
  assert.equal(active.length,0);
  assert.throws(() => a.buffered,{name:'InvalidStateError'});
  assert.ok(Number.isNaN(call('duration')));
});

test('abort invalidates a queued append and decode failure ends the source', () => {
  const {call,attach,notices,drain} = fixture();
  attach();
  const b = call('add','video/mp4'), events = [];
  for (const t of ['updatestart','update','updateend','abort','error']) b.addEventListener(t,() => events.push(t));
  b.appendWindowEnd = 5;
  b.appendBuffer(Uint8Array.of(3));
  b.abort(); drain();
  assert.deepEqual(events,['updatestart','abort','updateend']);
  assert.equal(b.appendWindowEnd,Infinity);
  assert.equal(call('activeSourceBuffers').length,0);
  b.appendBuffer(Uint8Array.of(255)); drain();
  assert.deepEqual(events.slice(3),['updatestart','error','updateend']);
  assert.equal(call('readyState'),'ended');
  assert.equal(notices.at(-1)[0],'decode');
  assert.equal(notices.at(-1)[5],true);
});

test('only the first accepted initialization sets an unset presentation duration', () => {
  for (const initial of [7, null]) {
    for (const chunks of [[[1,2]], [[1],[2]]]) {
      let reads = 0;
      const {call,attach,drain} = fixture((_,type) => {
        let initialized = false, firstDuration;
        return (op,bytes) => {
          switch (op) {
            case 'canAppend': return true;
            case 'append':
              for (const id of bytes) {
                if (!initialized) firstDuration = id === 1 ? initial : 70;
                initialized = true;
              }
              return true;
            case 'initDuration': reads++; return firstDuration;
            case 'initialized': return initialized;
            case 'trackKinds': return type === 'audio/mp4' ? 1 : 2;
            case 'highestEnd': return 0;
            case 'ranges': return [];
          }
        };
      });
      attach();
      const buffer = call('add','video/mp4');
      for (const chunk of chunks) {
        buffer.appendBuffer(Uint8Array.from(chunk));
        drain();
      }
      const second = call('add','audio/mp4');
      second.appendBuffer(Uint8Array.of(2)); drain();
      assert.equal(call('duration'),initial ?? Infinity);
      assert.equal(reads,1);
    }
  }
});

test('reserve one decoder per kind without changing existing buffers on a quota failure', () => {
  for (const kind of ['video/mp4','audio/mp4','mux']) {
    const {call,attach,drain} = fixture();
    attach();
    const first = call('add',kind);
    first.appendBuffer(Uint8Array.of(3)); drain();
    assert.throws(() => call('add',kind),{name:'QuotaExceededError'});
    assert.equal(call('sourceBuffers').length,1);
    assert.equal(call('activeSourceBuffers')[0],first);
    assert.deepEqual(first.buffered,[0,3]);
    if (kind === 'mux') {
      assert.throws(() => call('add','audio/mp4'),{name:'QuotaExceededError'});
      assert.throws(() => call('add','video/mp4'),{name:'QuotaExceededError'});
    } else {
      const other = call('add',kind === 'video/mp4' ? 'audio/mp4' : 'video/mp4');
      assert.throws(() => other.changeType(kind),{name:'NotSupportedError'});
      assert.equal(call('sourceBuffers').length,2);
      call('remove',first);
      other.changeType(kind);
      assert.throws(() => call('add',kind),{name:'QuotaExceededError'});
    }
  }
});
