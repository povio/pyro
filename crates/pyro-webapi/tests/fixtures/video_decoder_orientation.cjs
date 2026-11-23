// Run with node --test crates/pyro-webapi/tests/fixtures/video_decoder_orientation.cjs.
// Exercise the real JS decoder and VideoFrame using deterministic native-format pixels.
const test = require('node:test');
const assert = require('node:assert/strict');
const {readFileSync} = require('node:fs');
const {resolve} = require('node:path');
const vm = require('node:vm');

test('decoded display aspect is applied before rotation', async () => {
  class EncodedVideoChunk { get type() { return 'key'; } }
  const realm = vm.createContext({EventTarget, DOMException, DOMRectReadOnly: class {}, EncodedVideoChunk});
  const factory = name => vm.runInContext(readFileSync(resolve(__dirname, '../../src', name), 'utf8'), realm);
  Object.assign(realm, factory('video_frame.js')({maxSide: 4096, maxArea: 4096 * 4096, isCanvasSource: () => false}));
  const {VideoDecoder} = factory('video_decoder.js')(factory('decoder_queue.js'))({
    supports: () => true,
    isKeyChunk: () => true,
    postTask: setImmediate,
    fireDequeue: decoder => decoder.dispatchEvent(new Event('dequeue')),
    create: () => op => op === 'decode' ? [{
      data: new Uint8Array(320 * 240 * 4), format: 'RGBA', codedWidth: 320, codedHeight: 240, timestamp: 10
    }] : [],
  });
  for (const [rotation, aspect, flip, width, height] of [
    [0, null, false, 320, 240],
    [90, null, false, 240, 320],
    [180, null, false, 320, 240],
    [-90, [16, 9], true, 240, 427],
    [45, [16, 9], false, 240, 427],
    [180, [16, 9], true, 427, 240],
  ]) {
    let frame;
    const decoder = new VideoDecoder({output: result => { frame = result; }, error: assert.fail});
    const config = {codec: 'fixture', rotation, flip};
    if (aspect) [config.displayAspectWidth, config.displayAspectHeight] = aspect;
    decoder.configure(config);
    decoder.decode(new EncodedVideoChunk());
    await decoder.flush();
    assert.deepEqual([frame.codedWidth, frame.codedHeight], [320, 240]);
    assert.deepEqual([frame.displayWidth, frame.displayHeight], [width, height]);
    assert.equal(frame.flip, flip);
    frame.close();
    decoder.close();
  }
});

test('canvas overload snapshots after dictionary conversion and shares its charge only with live clones', async () => {
  const source = {pixels: new Uint8Array([20, 40, 60, 128, 5, 10, 15, 255])};
  const realm = vm.createContext({DOMException, DOMRectReadOnly: class {}});
  const factory = vm.runInContext(readFileSync(resolve(__dirname, '../../src/video_frame.js'), 'utf8'), realm);
  let snapshots = 0;
  let leases = 0;
  function lease() {
    leases++;
    let live = true;
    return clone => {
      assert.ok(live);
      if (clone) return lease();
      live = false;
      leases--;
    };
  }
  const {VideoFrame} = factory({
    maxSide: 64, maxArea: 4096, isCanvasSource: candidate => candidate === source,
    canvasSnapshot: () => {
      snapshots++;
      return {width: 2, height: 1, data: source.pixels.slice(),
              opaque: false, colorSpace: 'srgb', lease: lease()};
    }
  });
  const failure = new Error('member conversion failed');
  for (const [key, value, error] of [
    ['displayHeight', 4294967297, null],
    ['displayWidth', 4294967297, null],
    ['displayWidth', -1, null],
    ['displayWidth', {valueOf() { throw failure; }}, failure],
  ]) {
    const reads = [];
    const init = new Proxy({displayHeight: 1, [key]: value, timestamp: 0}, {
      get(target, member) { reads.push(member); return target[member]; }
    });
    assert.throws(() => new VideoFrame(source, init), e => error ? e === error : e.name === 'TypeError');
    assert.deepEqual(reads, key === 'displayHeight' ? ['alpha', key] : ['alpha', 'displayHeight', key]);
    assert.equal(snapshots, 0);
  }
  const rotated = new VideoFrame(source, {
    displayWidth: 6.9, displayHeight: 4.9, timestamp: 0, rotation: 90, flip: true
  });
  assert.deepEqual([rotated.displayWidth, rotated.displayHeight], [4, 6]);
  rotated.close();
  const reads = [];
  const init = new Proxy({timestamp: -4, duration: 9, rotation: 90}, {
    get(target, key) {
      reads.push(key);
      if (key === 'timestamp') source.pixels[0] = 55;
      return target[key];
    }
  });
  const frame = new VideoFrame(source, init);
  assert.deepEqual(reads, ['alpha', 'displayHeight', 'displayWidth', 'duration', 'flip', 'metadata',
                           'rotation', 'timestamp', 'visibleRect']);
  assert.deepEqual([frame.timestamp, frame.duration, frame.codedWidth, frame.codedHeight,
                    frame.displayWidth, frame.displayHeight], [-4, 9, 2, 1, 1, 2]);
  source.pixels.fill(0);
  const clone = frame.clone();
  frame.close();
  assert.equal(leases, 1);
  const copy = new Uint8Array(8);
  await clone.copyTo(copy);
  assert.deepEqual([...copy], [55, 40, 60, 128, 5, 10, 15, 255]);
  assert.throws(() => new VideoFrame(source, {timestamp: 0, visibleRect: {width: 3, height: 1}}),
                e => e.name === 'TypeError');
  assert.equal(leases, 1);
  clone.close();
  assert.equal(leases, 0);
});
