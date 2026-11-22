const test = require('node:test');
const assert = require('node:assert/strict');
const {readFileSync} = require('node:fs');
const {resolve} = require('node:path');
const vm = require('node:vm');

class Frame {
  constructor(timestamp, orientation = 0) { Object.assign(this, {timestamp, orientation, duration: 40}); }
}
const config = {codec: 'vp8', width: 320, height: 180};

function fixture() {
  const realm = vm.createContext({EventTarget, DOMException});
  const factory = vm.runInContext(readFileSync(resolve(__dirname, '../../src/video_encoder.js'), 'utf8'), realm);
  const tasks = [], captures = [];
  const {VideoEncoder} = factory({
    isVideoFrame: frame => frame instanceof Frame,
    capture(frame) {
      if (frame.closed) throw new TypeError('Closed frame');
      const copy = {...frame};
      captures.push(copy);
      return copy;
    },
    release: frame => { assert.ok(!frame.closed); frame.closed = true; },
    orientation: frame => frame.orientation,
    postTask: fn => tasks.push(fn),
    fireDequeue: encoder => encoder.dispatchEvent(new Event('dequeue')),
    supports: config => config.codec === 'vp8',
    create(config) {
      const pending = [];
      return (op, frame, options) => {
        if (op === 'encode') pending.push({
          chunk: {timestamp: frame.timestamp, duration: frame.duration, type: options.keyFrame ? 'key' : 'delta'},
          metadata: {width: config.width, quantizer: options.vp9?.quantizer}
        });
        return op === 'flush' ? pending.splice(0) : [];
      };
    }
  });
  return {VideoEncoder, captures, drain() { while (tasks.length) tasks.shift()(); }};
}

test('snapshot encode options and drain packets in order across orientation reconfiguration', async () => {
  const {VideoEncoder, captures, drain} = fixture();
  const outputs = [];
  const encoder = new VideoEncoder({output: (chunk, metadata) => outputs.push([chunk, metadata]), error: assert.fail});
  const options = {keyFrame: true, vp9: {quantizer: 12}};
  const rotated = new Frame(7, 1);
  const inputConfig = {...config};
  encoder.configure(inputConfig);
  encoder.encode(rotated, options);
  rotated.closed = true;
  options.keyFrame = false;
  options.vp9.quantizer = 20;
  assert.throws(() => encoder.encode(new Frame(11)), {name: 'DataError'});
  inputConfig.width = 80;
  encoder.configure(inputConfig);
  encoder.encode(new Frame(12), options);
  assert.equal(encoder.encodeQueueSize, 2);
  const flushed = encoder.flush();
  drain();
  await flushed;
  assert.deepEqual(outputs, [
    [{timestamp: 7, duration: 40, type: 'key'}, {width: 320, quantizer: 12}],
    [{timestamp: 12, duration: 40, type: 'delta'}, {width: 80, quantizer: 20}]
  ]);
  assert.equal(encoder.encodeQueueSize, 0);
  assert.ok(captures.every(frame => frame.closed));
});

test('reset releases queued frames and rejects pending flush', async () => {
  const {VideoEncoder, captures, drain} = fixture();
  const encoder = new VideoEncoder({output: assert.fail, error: assert.fail});
  encoder.configure(config);
  encoder.encode(new Frame(5));
  const rejected = assert.rejects(encoder.flush(), {name: 'AbortError'});
  encoder.reset();
  drain();
  await rejected;
  assert.equal(captures[0].closed, true);
  assert.equal(encoder.state, 'unconfigured');
});

test('dictionary members convert in IDL order, retaining codec extensions and known defaults', async () => {
  const {VideoEncoder, drain} = fixture();
  const reads = [];
  const support = VideoEncoder.isConfigSupported(new Proxy({...config, hevc: {}, ignored: true}, {
    get(obj, key) { reads.push(key); return obj[key]; }
  }));
  drain();
  const result = await support;
  assert.deepEqual(reads, ['alpha', 'avc', 'bitrate', 'bitrateMode', 'codec', 'contentHint',
    'displayHeight', 'displayWidth', 'framerate', 'hardwareAcceleration', 'height', 'hevc',
    'latencyMode', 'scalabilityMode', 'width']);
  assert.equal(result.supported, true);
  assert.equal(result.config.hevc.format, 'hevc');
  assert.equal(Object.hasOwn(result.config, 'ignored'), false);
  await assert.rejects(VideoEncoder.isConfigSupported({...config, width: 2 ** 32}), {name: 'TypeError'});
});
