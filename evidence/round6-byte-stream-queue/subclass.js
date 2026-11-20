(async () => {
  let controller;
  const reader = new ReadableStream({
    type: 'bytes',
    start(source) { controller = source; }
  }).getReader({mode: 'byob'});
  class Coordinates extends Uint16Array {}
  const coordinates = new Coordinates(2);
  const backing = coordinates.buffer;
  const first = reader.read(coordinates);
  const second = reader.read(new DataView(new ArrayBuffer(4), 1, 2));
  controller.enqueue(new Uint8Array([1, 0, 2, 0, 23, 42]));
  const [a,b] = await Promise.all([first,second]);
  console.log(JSON.stringify({
    intrinsic: Object.getPrototypeOf(a.value) === Uint16Array.prototype,
    inputTransferred: backing.byteLength === 0,
    coordinates: Array.from(a.value), second: [b.value.byteOffset,b.value.getUint8(0),b.value.getUint8(1)]
  }));
})().catch(e => console.log(String(e)));
