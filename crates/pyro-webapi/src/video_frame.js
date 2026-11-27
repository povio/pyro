// WebCodecs frames. Plane bytes are private and immutable;
// a cloned wrapper shares the resource and may be closed independently.
(function (limits) {
  'use strict';
  const frames = new WeakMap(),
    colors = new WeakMap();
  const U8 = Uint8Array,
    DE = DOMException,
    Rect = DOMRectReadOnly;
  const maxU32 = 0xffffffff;
  const rgb = new Set(['RGBA', 'RGBX', 'BGRA', 'BGRX']);
  const formats = new Set([
    ...rgb,
    'NV12',
    ...['I420', 'I422', 'I444'].flatMap((f) =>
      ['', 'A'].flatMap((a) => ['', 'P10', 'P12'].map((bits) => f + a + bits))
    )
  ]);
  const allowed = {
    primaries: ['bt709', 'bt470bg', 'smpte170m', 'bt2020', 'smpte432'],
    transfer: ['bt709', 'smpte170m', 'iec61966-2-1', 'linear', 'pq', 'hlg'],
    matrix: ['rgb', 'bt709', 'bt470bg', 'smpte170m', 'bt2020-ncl']
  };
  function dictionary(v) {
    if (v === undefined || v === null) return {};
    if (typeof v !== 'object' && typeof v !== 'function') throw new TypeError('Expected a dictionary.');
    return v;
  }
  function enumValue(v, options) {
    const s = String(v);
    if (!options.includes(s)) throw new TypeError('Invalid enumeration value: ' + s);
    return s;
  }
  function state(map, o) {
    const s = map.get(o);
    if (!s) throw new TypeError('Illegal invocation');
    return s;
  }
  class VideoColorSpace {
    constructor(init = {}) {
      init = dictionary(init);
      const s = {};
      const range = init.fullRange;
      s.fullRange = range == null ? null : Boolean(range);
      for (const k of ['matrix', 'primaries', 'transfer']) {
        const value = init[k];
        s[k] = value == null ? null : enumValue(value, allowed[k]);
      }
      colors.set(this, s);
    }
    get primaries() {
      return state(colors, this).primaries;
    }
    get transfer() {
      return state(colors, this).transfer;
    }
    get matrix() {
      return state(colors, this).matrix;
    }
    get fullRange() {
      return state(colors, this).fullRange;
    }
    toJSON() {
      return { ...state(colors, this) };
    }
  }
  function unsupported() {
    throw new DE('Unsupported frame format or conversion.', 'NotSupportedError');
  }
  function invalid() {
    throw new TypeError('Invalid VideoFrame dimensions or layout.');
  }
  function integer(v, min = 0, max = maxU32) {
    const n = Math.trunc(+v);
    if (!Number.isFinite(n) || n < min || n > max) invalid();
    return n;
  }
  const get = (proto, key) => Object.getOwnPropertyDescriptor(proto, key).get;
  const asBytes = (proto) => {
    const buffer = get(proto, 'buffer'),
      offset = get(proto, 'byteOffset'),
      length = get(proto, 'byteLength');
    return (v) => new U8(buffer.call(v), offset.call(v), length.call(v));
  };
  const typedBytes = asBytes(Object.getPrototypeOf(U8.prototype)),
    dataViewBytes = asBytes(DataView.prototype);
  // SharedArrayBuffer.prototype, also without the global (not cross-origin isolated): a shared
  // WebAssembly.Memory still hands out shared buffers (AllowSharedBufferSource).
  const sharedProto =
    typeof SharedArrayBuffer === 'function'
      ? SharedArrayBuffer.prototype
      : (() => {
          try {
            const proto = Object.getPrototypeOf(new WebAssembly.Memory({ initial: 0, maximum: 0, shared: true }).buffer);
            return proto === ArrayBuffer.prototype ? undefined : proto;
          } catch {
            return undefined;
          }
        })();
  const bufferGetters = [ArrayBuffer.prototype, sharedProto].filter(Boolean).map((p) => get(p, 'byteLength'));
  const cloneBuffers = globalThis.structuredClone;
  function view(v) {
    if (ArrayBuffer.isView(v)) {
      try {
        return typedBytes(v);
      } catch {
        return dataViewBytes(v);
      }
    }
    // The intrinsic brand check also works for other realms' buffers.
    for (const getter of bufferGetters) {
      try {
        getter.call(v);
        return new U8(v);
      } catch {}
    }
    throw new TypeError('A buffer source is required.');
  }
  function pixelFormat(v) {
    const f = String(v);
    if (!formats.has(f)) throw new TypeError('Unknown video pixel format.');
    return f;
  }
  function planes(f) {
    if (rgb.has(f)) return [[1, 1, 4]];
    if (f === 'NV12')
      return [
        [1, 1, 1],
        [2, 2, 2]
      ];
    if (f.includes('P')) unsupported(); // High-bit-depth planes.
    const x = f.startsWith('I444') ? 1 : 2,
      y = f.startsWith('I420') ? 2 : 1;
    const p = [
      [1, 1, 1],
      [x, y, 1],
      [x, y, 1]
    ];
    if (f.endsWith('A')) p.push([1, 1, 1]);
    return p;
  }
  function layoutOf(format, w, h, provided) {
    let size = 0;
    const given = provided === undefined ? null : Array.from(provided);
    const planesInfo = planes(format);
    if (given && given.length !== planesInfo.length) invalid();
    const regions = [],
      layout = [];
    for (const [i, [x, y, bytes]] of planesInfo.entries()) {
      const row = Math.ceil(w / x) * bytes,
        rows = Math.ceil(h / y);
      const offset = given ? integer(given[i].offset) : size;
      const stride = given ? integer(given[i].stride) : row;
      const end = offset + stride * rows;
      if (stride < row || end > maxU32 || regions.some(([a, b]) => a < end && offset < b)) invalid();
      regions.push([offset, end]);
      layout.push({ offset, stride });
      size = Math.max(size, end);
    }
    return { size, layout };
  }
  function rectOf(value, width, height, format) {
    if (+value.x < 0 || +value.y < 0) invalid();
    const x = integer(value.x ?? 0),
      y = integer(value.y ?? 0);
    const w = integer(value.width ?? 0, 1),
      h = integer(value.height ?? 0, 1);
    if (x + w > width || y + h > height || planes(format).some(([sx, sy]) => x % sx || y % sy)) invalid();
    return { x, y, width: w, height: h };
  }
  function requireLive(o) {
    const s = state(frames, o);
    if (!s.data) throw new DE('The frame is closed.', 'InvalidStateError');
    return s;
  }
  function colorInfo(format, supplied) {
    const packed = rgb.has(format);
    const defaults = {
      primaries: 'bt709',
      transfer: packed ? 'iec61966-2-1' : 'bt709',
      matrix: packed ? 'rgb' : 'bt709',
      fullRange: packed
    };
    const color = new VideoColorSpace(supplied === undefined ? defaults : supplied);
    const conversionColor = { ...defaults },
      explicit = colors.get(color);
    for (const key of Object.keys(defaults)) {
      if (explicit[key] !== null) conversionColor[key] = explicit[key];
    }
    return { color, conversionColor };
  }
  function copyPlanes(s, rect, to, dest) {
    for (const [i, [sx, sy, b]] of planes(s.format).entries()) {
      const src = s.layout[i],
        out = dest[i],
        row = Math.ceil(rect.width / sx) * b;
      const start = src.offset + (rect.y / sy) * src.stride + (rect.x / sx) * b;
      for (let y = 0; y < Math.ceil(rect.height / sy); y++) {
        const at = start + y * src.stride;
        to.set(s.data.subarray(at, at + row), out.offset + y * out.stride);
      }
    }
  }
  function optionsFor(s, o) {
    o = dictionary(o);
    const f = o.format === undefined ? s.format : pixelFormat(o.format);
    if (o.format !== undefined && !rgb.has(f)) unsupported();
    const rect = o.rect === undefined ? s.rect : rectOf(o.rect, s.width, s.height, s.format);
    const space = o.colorSpace === undefined ? 'srgb' : enumValue(o.colorSpace, ['srgb', 'display-p3']);

    return {
      format: f,
      rect,
      space,
      convert: o.format !== undefined,
      ...layoutOf(f, rect.width, rect.height, o.layout)
    };
  }
  // Decode encoded RGB into linear light, convert primaries through the
  // D65 XYZ connection space, and apply the sRGB/display-P3 transfer curve.
  const xyz = {
    bt709: [
      [0.412391, 0.357584, 0.180481],
      [0.212639, 0.715169, 0.072192],
      [0.019331, 0.119195, 0.950532]
    ],
    smpte170m: [
      [0.393521, 0.365258, 0.191677],
      [0.212376, 0.70106, 0.086564],
      [0.018739, 0.111934, 0.958385]
    ],
    bt470bg: [
      [0.430619, 0.341541, 0.178309],
      [0.222037, 0.706638, 0.071325],
      [0.020185, 0.129551, 0.939094]
    ],
    smpte432: [
      [0.486571, 0.265668, 0.198217],
      [0.228975, 0.691739, 0.079287],
      [0, 0.045113, 1.043944]
    ],
    bt2020: [
      [0.636958, 0.144617, 0.168881],
      [0.2627, 0.677998, 0.059302],
      [0, 0.028073, 1.060985]
    ]
  };
  const inv = {
    srgb: [
      [3.24097, -1.537383, -0.498611],
      [-0.969244, 1.875968, 0.041555],
      [0.05563, -0.203977, 1.056972]
    ],
    'display-p3': [
      [2.493497, -0.931384, -0.402711],
      [-0.829489, 1.762664, 0.023625],
      [0.035846, -0.076172, 0.956885]
    ]
  };
  function linear(x, t) {
    if (t === 'linear') return x;
    if (t === 'iec61966-2-1') return x <= 0.04045 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4;
    if (t === 'bt709' || t === 'smpte170m') return x < 0.081 ? x / 4.5 : ((x + 0.099) / 1.099) ** (1 / 0.45);
    unsupported(); // HDR requires a tone mapping policy.
  }
  const gamma = (x) => 255 * (x <= 0.0031308 ? 12.92 * x : 1.055 * x ** (1 / 2.4) - 0.055);
  const luma = {
    bt709: [0.2126, 0.0722],
    smpte170m: [0.299, 0.114],
    bt470bg: [0.299, 0.114],
    'bt2020-ncl': [0.2627, 0.0593]
  };
  function pixelReader(frame, pixel) {
    const { data, layout, format, conversionColor: color } = frame;
    if (rgb.has(format)) {
      const red = format.startsWith('BGR') ? 2 : 0,
        blue = 2 - red;
      const opaque = format.endsWith('X');
      return (x, y) => {
        const pos = layout[0].offset + y * layout[0].stride + 4 * x;
        pixel[0] = data[pos + red];
        pixel[1] = data[pos + 1];
        pixel[2] = data[pos + blue];
        pixel[3] = opaque ? 255 : data[pos + 3];
      };
    }
    const coeffs = luma[color.matrix];
    if (!coeffs) unsupported();
    const [kr, kb] = coeffs;
    const [sx, sy] = planes(format)[1],
      nv = format === 'NV12',
      alpha = format.endsWith('A');
    const yOffset = color.fullRange ? 0 : 16;
    const yScale = color.fullRange ? 255 : 219,
      uvScale = color.fullRange ? 255 : 224;
    const at = (p, x, y) => layout[p].offset + y * layout[p].stride + x;
    return (x, y) => {
      const xx = Math.floor(x / sx),
        yy = Math.floor(y / sy);
      const uPos = at(1, xx * (nv ? 2 : 1), yy),
        vPos = nv ? uPos + 1 : at(2, xx, yy);
      const Y = (data[at(0, x, y)] - yOffset) / yScale;
      const r = Y + (2 * (1 - kr) * (data[vPos] - 128)) / uvScale;
      const b = Y + (2 * (1 - kb) * (data[uPos] - 128)) / uvScale;
      pixel[0] = 255 * r;
      pixel[1] = (255 * (Y - kr * r - kb * b)) / (1 - kr - kb);
      pixel[2] = 255 * b;
      pixel[3] = alpha ? data[at(3, x, y)] : 255;
    };
  }
  const sdrCurves = new Map();
  function colorTransform(color, space) {
    if (color.primaries === (space === 'srgb' ? 'bt709' : 'smpte432')) {
      if (color.transfer === 'iec61966-2-1') return null;
      // For identical primaries each channel is independent. A 12-bit index
      // retains sub-byte precision for the 8-bit RGB output.
      let curve = sdrCurves.get(color.transfer);
      if (!curve) {
        curve = U8.from({ length: 4096 }, (_, i) => Math.round(gamma(linear(i / 4095, color.transfer))));
        sdrCurves.set(color.transfer, curve);
      }
      return (p) => {
        for (let i = 0; i < 3; i++)
          p[i] = curve[Math.max(0, Math.min(4095, Math.round(p[i] * (4095 / 255))))];
      };
    }
    const src = xyz[color.primaries],
      dst = inv[space];
    const matrix = dst.map((row) =>
      [0, 1, 2].map((i) => row[0] * src[0][i] + row[1] * src[1][i] + row[2] * src[2][i])
    );
    return (p) => {
      const r = linear(p[0] / 255, color.transfer),
        g = linear(p[1] / 255, color.transfer),
        b = linear(p[2] / 255, color.transfer);
      for (let i = 0; i < 3; i++) p[i] = gamma(matrix[i][0] * r + matrix[i][1] * g + matrix[i][2] * b);
    };
  }
  function copyPixels(s, o, to) {
    if (!o.convert) {
      copyPlanes(s, o.rect, to, o.layout);
      if (s.format.endsWith('X')) {
        for (let y = 0; y < o.rect.height; y++) {
          const start = o.layout[0].offset + y * o.layout[0].stride;
          for (let k = start + 3; k < start + o.rect.width * 4; k += 4) to[k] = 255;
        }
      }
      return;
    }
    const bgr = o.format.startsWith('BGR'),
      opaque = o.format.endsWith('X');
    const pixel = [0, 0, 0, 0],
      read = pixelReader(s, pixel),
      transform = colorTransform(s.conversionColor, o.space);
    const byte = (x) => Math.min(255, Math.max(0, Math.round(x)));
    for (let y = 0; y < o.rect.height; y++) {
      let k = o.layout[0].offset + y * o.layout[0].stride;
      for (let x = 0; x < o.rect.width; x++, k += 4) {
        read(o.rect.x + x, o.rect.y + y);
        if (transform) transform(pixel);
        to[k] = byte(pixel[bgr ? 2 : 0]);
        to[k + 1] = byte(pixel[1]);
        to[k + 2] = byte(pixel[bgr ? 0 : 2]);
        to[k + 3] = opaque ? 255 : pixel[3];
      }
    }
  }
  function to64(value, signed) {
    const number = +value;
    if (!Number.isFinite(number)) return 0;
    const bits = BigInt(Math.trunc(number));
    return Number(signed ? BigInt.asIntN(64, bits) : BigInt.asUintN(64, bits));
  }

  // Convert the image overload's dictionary before checking usability and
  // taking the bitmap snapshot. An author's getter can resize or draw to
  // the source; each member is read once and in Web IDL dictionary order.
  function canvasInit(init) {
    const alpha = init.alpha;
    const result = {__proto__: null, alpha: alpha === undefined ? 'keep' : enumValue(alpha, ['keep', 'discard'])};
    const displayHeight = init.displayHeight;
    if (displayHeight !== undefined) result.displayHeight = integer(displayHeight);
    const displayWidth = init.displayWidth;
    if (displayWidth !== undefined) result.displayWidth = integer(displayWidth);
    const duration = init.duration;
    if (duration !== undefined) result.duration = to64(duration, false);
    result.flip = !!init.flip;
    // The core VideoFrameMetadata dictionary has no defined members.
    dictionary(init.metadata);
    const rotation = init.rotation;
    result.rotation = rotation === undefined ? 0 : +rotation;
    if (!Number.isFinite(result.rotation)) invalid();
    const timestamp = init.timestamp;
    if (timestamp !== undefined) result.timestamp = to64(timestamp, true);
    const rect = init.visibleRect;
    if (rect !== undefined) {
      const d = dictionary(rect);
      result.visibleRect = {__proto__: null};
      for (const key of ['height', 'width', 'x', 'y']) {
        const member = d[key];
        result.visibleRect[key] = member === undefined ? 0 : +member;
      }
    }
    return result;
  }

  function applyGeometry(s, init) {
    const oldRect = s.rect;
    if (init.visibleRect !== undefined) s.rect = rectOf(init.visibleRect, s.width, s.height, s.format);
    if ((init.displayWidth === undefined) !== (init.displayHeight === undefined)) invalid();
    s.displayW =
      init.displayWidth === undefined
        ? Math.round((s.displayW * s.rect.width) / oldRect.width)
        : integer(init.displayWidth, 1);
    s.displayH =
      init.displayHeight === undefined
        ? Math.round((s.displayH * s.rect.height) / oldRect.height)
        : integer(init.displayHeight, 1);
    if (!s.displayW || !s.displayH) invalid();
    const angle = +(init.rotation ?? 0);
    if (!Number.isFinite(angle)) invalid();
    const turn = Math.floor(((((angle % 360) + 360) % 360) + 45) / 90) * 90;
    s.rotation = (s.rotation + (s.flip ? -turn : turn) + 360) % 360;
    s.flip = s.flip !== Boolean(init.flip);
  }

  function fromCanvas(target, source, init) {
    // Only branded, usable, origin-clean sources reach a pixel snapshot.
    // The native bridge owns the charge until construction succeeds.
    const bitmap = limits.canvasSnapshot(source);
    try {
      // An HTMLVideoElement's own position is the default timestamp (microseconds); no other source has one.
      if (init.timestamp === undefined) {
        if (!isVideoElement(source)) throw new TypeError('A timestamp is required for a bitmap source.');
        init.timestamp = to64(videoTime.call(source) * 1e6, true);
      }
      const {width, height} = bitmap;
      const format = bitmap.opaque || init.alpha === 'discard' ? 'RGBX' : 'RGBA';
      const colorSpace = bitmap.colorSpace === 'display-p3' ? {
        primaries: 'smpte432', transfer: 'iec61966-2-1', matrix: 'rgb', fullRange: true
      } : undefined;
      const s = {
        data: bitmap.data, imageLease: bitmap.lease, format, width, height,
        layout: [{offset: 0, stride: width * 4}],
        rect: {x: 0, y: 0, width, height}, displayW: width, displayH: height,
        rotation: 0, flip: false, timestamp: init.timestamp, duration: init.duration ?? null,
        ...colorInfo(format, colorSpace)
      };
      applyGeometry(s, init);
      frames.set(target, s);
    } catch (error) {
      bitmap.lease(false);
      throw error;
    }
  }

  // Brand checks through the intrinsic getters: they also accept another realm's element.
  const videoTime = typeof HTMLMediaElement === 'function' ? Object.getOwnPropertyDescriptor(HTMLMediaElement.prototype, 'currentTime')?.get : undefined;
  const videoBrand = typeof HTMLVideoElement === 'function' ? Object.getOwnPropertyDescriptor(HTMLVideoElement.prototype, 'videoWidth')?.get : undefined;
  function isVideoElement(o) {
    if (!videoBrand || !videoTime) return false;
    try {
      videoBrand.call(o);
      return true;
    } catch {
      return false;
    }
  }
  const decoderOwned = Symbol('decoder-owned frame bytes');
  class VideoFrame {
    constructor(source, init = {}, privateToken) {
      init = dictionary(init);
      if (privateToken !== decoderOwned && !frames.has(source) && limits.isCanvasSource(source)) {
        fromCanvas(this, source, canvasInit(init));
        return;
      }
      let s;
      let cropped_ = false;
      if (frames.has(source)) {
        s = { ...requireLive(source) };
        if (init.alpha !== undefined && enumValue(init.alpha, ['keep', 'discard']) === 'discard') {
          if (rgb.has(s.format)) s.format = s.format.slice(0, 3) + 'X';
          else if (s.format.endsWith('A')) s.format = s.format.slice(0, -1);
        }
      } else {
        const bytes = view(source),
          format = pixelFormat(init.format);
        const width = integer(init.codedWidth, 1),
          height = integer(init.codedHeight, 1);
        if (width > limits.maxSide || height > limits.maxSide || width * height > limits.maxArea)
          unsupported();
        const input = layoutOf(format, width, height, init.layout);
        if (input.size > bytes.length) invalid();
        const output = layoutOf(format, width, height);
        const rect = { x: 0, y: 0, width, height };
        // Only decodedImage receives an exclusive, tightly packed native
        // snapshot. Public buffer constructors always take their own copy.
        let data = privateToken === decoderOwned ? bytes : new U8(output.size);
        if (privateToken !== decoderOwned)
          copyPlanes({ format, layout: input.layout, data: bytes }, rect, data, output.layout);
        // Chrome crops a buffer's frame to init.visibleRect when it is made: the frame's coded size is the
        // visible size and the planes are the cropped region (a decoder's own frames keep their coded size).
        let cw = width,
          ch = height,
          layout = output.layout;
        if (privateToken !== decoderOwned && init.visibleRect !== undefined) {
          const crop = rectOf(dictionary(init.visibleRect), width, height, format);
          if (crop.x || crop.y || crop.width !== width || crop.height !== height) {
            const tight = layoutOf(format, crop.width, crop.height);
            const cropped = new U8(tight.size);
            copyPlanes({ format, layout: output.layout, data }, crop, cropped, tight.layout);
            data = cropped;
            layout = tight.layout;
            cw = crop.width;
            ch = crop.height;
            cropped_ = true;
          }
        }
        s = {
          data,
          imageLease: null,
          format,
          width: cw,
          height: ch,
          layout,
          rect: { x: 0, y: 0, width: cw, height: ch },
          displayW: cw,
          displayH: ch,
          rotation: 0,
          flip: false,
          timestamp: integer(init.timestamp, Number.MIN_SAFE_INTEGER, Number.MAX_SAFE_INTEGER),
          duration: null,
          ...colorInfo(format, init.colorSpace)
        };
      }
      if (frames.has(source)) {
        if (init.timestamp !== undefined) s.timestamp = to64(init.timestamp, true);
        if (init.duration !== undefined) s.duration = to64(init.duration, false);
      } else if (init.duration !== undefined) {
        s.duration = integer(init.duration, 0, Number.MAX_SAFE_INTEGER);
      }
      // A buffer frame cropped above has its visibleRect applied already.
      applyGeometry(s, cropped_ ? { __proto__: init, visibleRect: undefined } : init);
      if (init.displayWidth !== undefined && s.rotation % 180) {
        [s.displayW, s.displayH] = [s.displayH, s.displayW];
      }
      if (!frames.has(source) && init.transfer !== undefined) {
        const transfers = Array.from(init.transfer);
        for (const entry of transfers) bufferGetters[0].call(entry);
        if (transfers.length) cloneBuffers(null, { transfer: transfers });
      }
      if (s.imageLease) s.imageLease = s.imageLease(true);
      frames.set(this, s);
    }
    get timestamp() {
      return state(frames, this).timestamp;
    }
    get duration() {
      return state(frames, this).duration;
    }
    get codedWidth() {
      return state(frames, this).width;
    }
    get codedHeight() {
      return state(frames, this).height;
    }
    get codedRect() {
      const s = state(frames, this);
      return s.data ? new Rect(0, 0, s.width, s.height) : null;
    }
    get visibleRect() {
      const rect = state(frames, this).rect;
      return rect && new Rect(rect.x, rect.y, rect.width, rect.height);
    }
    get displayWidth() {
      const s = state(frames, this);
      return s.rotation % 180 ? s.displayH : s.displayW;
    }
    get displayHeight() {
      const s = state(frames, this);
      return s.rotation % 180 ? s.displayW : s.displayH;
    }
    get rotation() {
      return state(frames, this).rotation;
    }
    get flip() {
      return state(frames, this).flip;
    }
    get format() {
      return state(frames, this).format;
    }
    get colorSpace() {
      return state(frames, this).color;
    }
    metadata() {
      requireLive(this);
      return {};
    }
    allocationSize(options = {}) {
      return optionsFor(requireLive(this), options).size;
    }
    async copyTo(destination, options = {}) {
      const s = requireLive(this),
        out = view(destination),
        o = optionsFor(s, options);
      if (out.byteLength < o.size) invalid();
      copyPixels(s, o, out);
      return o.layout;
    }
    clone() {
      return new VideoFrame(this);
    }
    close() {
      const s = state(frames, this);
      if (s.imageLease) {
        s.imageLease(false);
        s.imageLease = null;
      }
      Object.assign(s, {
        data: null,
        layout: null,
        rect: null,
        width: 0,
        height: 0,
        displayW: 0,
        displayH: 0,
        rotation: 0,
        flip: false,
        format: null,
        conversionColor: null
      });
    }
  }
  for (const C of [VideoFrame, VideoColorSpace]) {
    for (const name of Object.getOwnPropertyNames(C.prototype)) {
      if (name !== 'constructor') Object.defineProperty(C.prototype, name, { enumerable: true });
    }
    Object.defineProperty(C.prototype, Symbol.toStringTag, { value: C.name, configurable: true });
  }
  function canvasSource(o, useOrientation = true) {
    if (!frames.has(o)) return null;
    const s = requireLive(o),
      opt = optionsFor(s, { format: 'RGBA' });
    const w = s.rect.width,
      h = s.rect.height,
      data = new U8(opt.size);
    copyPixels(s, opt, data);
    // A destination canvas can draw in stored pixel order without changing
    // the frame's public rotation, flip, or display dimensions.
    const rotation = useOrientation ? s.rotation : 0;
    const flip = useOrientation && s.flip;
    const rotated = rotation % 180 !== 0,
      outW = rotated ? h : w,
      outH = rotated ? w : h;
    let oriented = data;
    if (rotation || flip) {
      oriented = new U8(data.length);
      for (let y = 0; y < h; y++)
        for (let x = 0; x < w; x++) {
          let xx = x,
            yy = y;
          if (rotation === 90) {
            xx = h - 1 - y;
            yy = x;
          }
          if (rotation === 180) {
            xx = w - 1 - x;
            yy = h - 1 - y;
          }
          if (rotation === 270) {
            xx = y;
            yy = w - 1 - x;
          }
          if (flip) xx = outW - 1 - xx;
          const src = 4 * (y * w + x),
            dst = 4 * (yy * outW + xx);
          oriented.set(data.subarray(src, src + 4), dst);
        }
    }
    return [5, oriented, outW, outH, rotated ? s.displayH : s.displayW, rotated ? s.displayW : s.displayH];
  }
  function decodedImage(bytes, init, lease) {
    const frame = new VideoFrame(bytes, init, decoderOwned);
    frames.get(frame).imageLease = lease;
    return frame;
  }
  // VideoEncoder retains a separate wrapper while its control message is queued.
  const closeFrame = Function.prototype.call.bind(VideoFrame.prototype.close);
  function encoderFrame(op, o) {
    const s = frames.get(o);
    if (op === 'brand') return s !== undefined;
    if (!s || !s.data) throw new TypeError('Expected a live VideoFrame.');
    if (op === 'capture') return new VideoFrame(o, {__proto__: null});
    if (op === 'release') return closeFrame(o);
    if (op === 'orientation') return s.rotation / 90 + (s.flip ? 4 : 0);
    const { rect, conversionColor: color } = s;
    let data, kind, stride, offset;
    // Preserve the actual limited-range samples when they already match
    // the encoder's I420 matrix, transfer function and primaries.
    if ((s.format === 'I420' || s.format === 'I420A') && color.matrix === 'bt709' &&
        color.primaries === 'bt709' && color.transfer === 'bt709' && !color.fullRange) {
      kind = 'I420';
      const l = layoutOf(kind, rect.width, rect.height);
      data = new U8(l.size);
      copyPlanes({...s, format: kind}, rect, data, l.layout);
      stride = rect.width;
      offset = 0;
    } else if (rgb.has(s.format) && color.primaries === 'bt709' && color.transfer === 'iec61966-2-1' &&
               color.fullRange && color.matrix === 'rgb') {
      kind = s.format.startsWith('BGR') ? 'BGRA' : 'RGBA';
      data = s.data;
      stride = s.layout[0].stride;
      offset = s.layout[0].offset + rect.y * stride + rect.x * 4;
    } else {
      kind = 'RGBA';
      const opt = optionsFor(s, {__proto__: null, format: kind});
      data = new U8(opt.size);
      copyPixels(s, opt, data);
      stride = rect.width * 4;
      offset = 0;
    }
    return {__proto__: null, data, kind, stride, offset, width: rect.width, height: rect.height,
            rotation: s.rotation, flip: s.flip, timestamp: s.timestamp, duration: s.duration};
  }
  return { VideoFrame, VideoColorSpace, canvasSource, decodedImage, encoderFrame };
});
