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
  Object.assign(realm, factory('video_frame.js')({maxSide: 4096, maxArea: 4096 * 4096}));
  const {VideoDecoder} = factory('video_decoder.js')({
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
