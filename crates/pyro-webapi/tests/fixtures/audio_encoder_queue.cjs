const test = require('node:test');
const assert = require('node:assert/strict');
const {readFileSync} = require('node:fs');
const {resolve} = require('node:path');
const vm = require('node:vm');

function fixture() {
  class AudioData { close() { this.closed = true; } }
  const realm = vm.createContext({AudioData, EventTarget, DOMException});
  const factory = vm.runInContext(readFileSync(resolve(__dirname, '../../src/audio_encoder.js'), 'utf8'), realm);
  const tasks = [], captures = [];
  const {AudioEncoder} = factory({
    isAudioData: input => input instanceof AudioData,
    capture(input) {
      if (input.closed) throw new TypeError('closed');
      const copy = new AudioData();
      copy.value = input.value;
      captures.push(copy);
      return copy;
    },
    postTask: f => tasks.push(f),
    fireDequeue: encoder => encoder.dispatchEvent(new Event('dequeue')),
    supports: config => config.codec === 'opus',
    create: config => (op, data) => op === 'encode'
      ? [1, 2].map(i => ({chunk: data.value + i, metadata: {rate: config.sampleRate}})) : []
  });
  function drain() { while (tasks.length) tasks.shift()(); }
  return {AudioEncoder, AudioData, captures, drain};
}
const config = {codec: 'opus', numberOfChannels: 1, sampleRate: 48000};

test('encoder snapshots before input close and delivers each packet before flush', async () => {
  const {AudioEncoder, AudioData, drain, captures} = fixture();
  const output = [];
  const enc = new AudioEncoder({output: (chunk, metadata) => output.push([chunk, metadata.rate]), error: assert.fail});
  enc.configure(config);
  const data = new AudioData();
  data.value = 20;
  enc.encode(data);
  data.close();
  const flush = enc.flush();
  assert.equal(enc.encodeQueueSize, 1);
  drain();
  await flush;
  assert.deepEqual(output, [[21, 48000], [22, 48000]]);
  assert.equal(captures[0].closed, true);
  assert.equal(enc.encodeQueueSize, 0);
  enc.close();
});

test('reset from output releases queued samples, cancels remaining packets and rejects flush', async () => {
  const {AudioEncoder, AudioData, drain, captures} = fixture();
  let count = 0;
  const enc = new AudioEncoder({output() { count++; enc.reset(); }, error: assert.fail});
  enc.configure(config);
  enc.encode(new AudioData());
  enc.encode(new AudioData());
  const rejected = assert.rejects(enc.flush(), {name: 'AbortError'});
  drain();
  await rejected;
  assert.equal(count, 1);
  assert.ok(captures.every(input => input.closed));
  assert.equal(enc.state, 'unconfigured');
});

test('support converts config in member order and ignores unknown members', async () => {
  const {AudioEncoder, drain} = fixture();
  const reads = [];
  const pending = AudioEncoder.isConfigSupported(new Proxy({...config, opus: {}, unknown: 1}, {
    get(object, key) { reads.push(key); return object[key]; }
  }));
  drain();
  const result = await pending;
  assert.equal(result.supported, true);
  assert.deepEqual(reads, ['aac', 'bitrate', 'bitrateMode', 'codec', 'numberOfChannels', 'opus', 'sampleRate']);
  assert.equal(Object.hasOwn(result.config, 'unknown'), false);
  assert.equal(result.config.opus.frameDuration, 20000);
  await assert.rejects(AudioEncoder.isConfigSupported({...config, opus: {complexity: 11}}), {name: 'TypeError'});
});
