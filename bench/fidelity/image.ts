// Image work for the fidelity harness, with no dependencies: a PNG decoder
// (8-bit, non-interlaced: what Chromium and pyro write) and encoder, SSIM,
// mean absolute difference, a diff heatmap and side-by-side composites.
import { deflateSync, inflateSync } from 'node:zlib';

/** 8-bit RGBA, rows top to bottom. */
export type Image = { width: number; height: number; data: Uint8Array };

const CRC_TABLE = (() => {
  const table = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    table[n] = c >>> 0;
  }
  return table;
})();

function crc32(bytes: Uint8Array, crc = 0xffffffff): number {
  for (let i = 0; i < bytes.length; i++) crc = CRC_TABLE[(crc ^ bytes[i]) & 0xff] ^ (crc >>> 8);
  return crc;
}

export function decodePng(file: Uint8Array): Image {
  const view = new DataView(file.buffer, file.byteOffset, file.byteLength);
  if (view.getUint32(0) !== 0x89504e47 || view.getUint32(4) !== 0x0d0a1a0a) throw new Error('not a PNG');
  let width = 0;
  let height = 0;
  let colorType = 0;
  let palette: Uint8Array | undefined;
  let transparency: Uint8Array | undefined;
  const chunks: Uint8Array[] = [];
  for (let pos = 8; pos < file.length; ) {
    const length = view.getUint32(pos);
    const type = String.fromCharCode(file[pos + 4], file[pos + 5], file[pos + 6], file[pos + 7]);
    const body = file.subarray(pos + 8, pos + 8 + length);
    if (type === 'IHDR') {
      width = view.getUint32(pos + 8);
      height = view.getUint32(pos + 12);
      const depth = body[8];
      colorType = body[9];
      if (depth !== 8 || body[12] !== 0) throw new Error(`unsupported PNG: depth ${depth}, interlace ${body[12]}`);
    } else if (type === 'PLTE') palette = body;
    else if (type === 'tRNS') transparency = body;
    else if (type === 'IDAT') chunks.push(body);
    else if (type === 'IEND') break;
    pos += 12 + length;
  }
  const channels = ({ 0: 1, 2: 3, 3: 1, 4: 2, 6: 4 } as Record<number, number>)[colorType];
  if (!channels) throw new Error(`unsupported PNG color type ${colorType}`);
  const raw = inflateSync(Buffer.concat(chunks));
  const stride = width * channels;
  const rows = new Uint8Array(stride * height);
  const bpp = channels;
  for (let y = 0; y < height; y++) {
    const filter = raw[y * (stride + 1)];
    const src = y * (stride + 1) + 1;
    const dst = y * stride;
    const up = dst - stride;
    if (filter === 0 || (filter === 2 && y === 0)) rows.set(raw.subarray(src, src + stride), dst);
    else if (filter === 1 || (filter === 4 && y === 0)) {
      for (let x = 0; x < bpp; x++) rows[dst + x] = raw[src + x];
      for (let x = bpp; x < stride; x++) rows[dst + x] = (raw[src + x] + rows[dst + x - bpp]) & 0xff;
    } else if (filter === 2) {
      for (let x = 0; x < stride; x++) rows[dst + x] = (raw[src + x] + rows[up + x]) & 0xff;
    } else if (filter === 3) {
      if (y === 0) {
        for (let x = 0; x < bpp; x++) rows[dst + x] = raw[src + x];
        for (let x = bpp; x < stride; x++) rows[dst + x] = (raw[src + x] + (rows[dst + x - bpp] >> 1)) & 0xff;
      } else {
        for (let x = 0; x < bpp; x++) rows[dst + x] = (raw[src + x] + (rows[up + x] >> 1)) & 0xff;
        for (let x = bpp; x < stride; x++) rows[dst + x] = (raw[src + x] + ((rows[dst + x - bpp] + rows[up + x]) >> 1)) & 0xff;
      }
    } else if (filter === 4) {
      for (let x = 0; x < bpp; x++) rows[dst + x] = (raw[src + x] + rows[up + x]) & 0xff;
      for (let x = bpp; x < stride; x++) {
        const left = rows[dst + x - bpp];
        const above = rows[up + x];
        const corner = rows[up + x - bpp];
        const pa = Math.abs(above - corner);
        const pb = Math.abs(left - corner);
        const pc = Math.abs(left + above - 2 * corner);
        rows[dst + x] = (raw[src + x] + (pa <= pb && pa <= pc ? left : pb <= pc ? above : corner)) & 0xff;
      }
    } else throw new Error(`bad PNG filter ${filter}`);
  }
  if (colorType === 6) return { width, height, data: rows };
  const data = new Uint8Array(width * height * 4);
  const n = width * height;
  if (colorType === 2)
    for (let i = 0, s = 0, o = 0; i < n; i++, s += 3, o += 4) {
      data[o] = rows[s];
      data[o + 1] = rows[s + 1];
      data[o + 2] = rows[s + 2];
      data[o + 3] = 255;
    }
  else
    for (let i = 0; i < n; i++) {
      const o = i * 4;
      if (colorType === 3) {
        const index = rows[i];
        data[o] = palette![index * 3];
        data[o + 1] = palette![index * 3 + 1];
        data[o + 2] = palette![index * 3 + 2];
        data[o + 3] = transparency && index < transparency.length ? transparency[index] : 255;
      } else {
        const gray = rows[i * channels];
        data[o] = data[o + 1] = data[o + 2] = gray;
        data[o + 3] = colorType === 4 ? rows[i * 2 + 1] : 255;
      }
    }
  return { width, height, data };
}

/** RGB PNG (alpha dropped: screenshots are opaque), Sub filter on every row. */
export function encodePng(image: Image, level = 6): Uint8Array {
  const { width, height, data } = image;
  const stride = width * 3;
  const raw = new Uint8Array((stride + 1) * height);
  for (let y = 0; y < height; y++) {
    const row = y * (stride + 1);
    raw[row] = 1;
    for (let x = 0; x < width; x++) {
      const s = (y * width + x) * 4;
      const d = row + 1 + x * 3;
      if (x === 0) {
        raw[d] = data[s];
        raw[d + 1] = data[s + 1];
        raw[d + 2] = data[s + 2];
      } else {
        raw[d] = (data[s] - data[s - 4]) & 0xff;
        raw[d + 1] = (data[s + 1] - data[s - 3]) & 0xff;
        raw[d + 2] = (data[s + 2] - data[s - 2]) & 0xff;
      }
    }
  }
  const chunk = (type: string, body: Uint8Array) => {
    const out = new Uint8Array(12 + body.length);
    const view = new DataView(out.buffer);
    view.setUint32(0, body.length);
    for (let i = 0; i < 4; i++) out[4 + i] = type.charCodeAt(i);
    out.set(body, 8);
    view.setUint32(8 + body.length, (crc32(out.subarray(4, 8 + body.length)) ^ 0xffffffff) >>> 0);
    return out;
  };
  const header = new Uint8Array(13);
  const hv = new DataView(header.buffer);
  hv.setUint32(0, width);
  hv.setUint32(4, height);
  header.set([8, 2, 0, 0, 0], 8);
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', header),
    chunk('IDAT', deflateSync(raw, { level })),
    chunk('IEND', new Uint8Array(0)),
  ]);
}

/** Luma (ITU-R BT.601, as MATLAB's rgb2gray in Wang et al.'s reference SSIM code), 0-255. */
export function luminance(image: Image): Float32Array {
  const { data } = image;
  const out = new Float32Array(image.width * image.height);
  for (let i = 0; i < out.length; i++) out[i] = 0.2989 * data[i * 4] + 0.587 * data[i * 4 + 1] + 0.114 * data[i * 4 + 2];
  return out;
}

/** Box-filter downscale of a luma plane by an integer factor (area average: what a 1/factor view shows). */
export function downscale(plane: Float32Array, width: number, height: number, factor: number) {
  const w = Math.floor(width / factor);
  const h = Math.floor(height / factor);
  const out = new Float32Array(w * h);
  const area = factor * factor;
  for (let y = 0; y < h; y++)
    for (let x = 0; x < w; x++) {
      let sum = 0;
      for (let dy = 0; dy < factor; dy++) {
        const row = (y * factor + dy) * width + x * factor;
        for (let dx = 0; dx < factor; dx++) sum += plane[row + dx];
      }
      out[y * w + x] = sum / area;
    }
  return { plane: out, width: w, height: h };
}

/** Box-filter downscale of an RGBA image (for the small composites). */
export function downscaleImage(image: Image, factor: number): Image {
  const w = Math.floor(image.width / factor);
  const h = Math.floor(image.height / factor);
  const data = new Uint8Array(w * h * 4);
  const area = factor * factor;
  for (let y = 0; y < h; y++)
    for (let x = 0; x < w; x++)
      for (let c = 0; c < 4; c++) {
        let sum = 0;
        for (let dy = 0; dy < factor; dy++)
          for (let dx = 0; dx < factor; dx++) sum += image.data[((y * factor + dy) * image.width + x * factor + dx) * 4 + c];
        data[(y * w + x) * 4 + c] = Math.round(sum / area);
      }
  return { width: w, height: h, data };
}

/**
 * Mean SSIM (Wang, Bovik, Sheikh, Simoncelli 2004) of two luma planes: 11x11
 * Gaussian window (sigma 1.5), K1 0.01, K2 0.03, L 255, over the windows that
 * fit the image (MATLAB `filter2(..., 'valid')`). Also returns the map.
 */
export function ssim(a: Float32Array, b: Float32Array, width: number, height: number) {
  const weights: number[] = [];
  for (let i = -5; i <= 5; i++) weights.push(Math.exp(-(i * i) / (2 * 1.5 * 1.5)));
  const total = weights.reduce((x, y) => x + y, 0);
  // Symmetric kernel: g0 is the centre tap, g5 the outermost pair.
  const [g5, g4, g3, g2, g1, g0] = weights.slice(0, 6).map((v) => v / total);
  const C1 = (0.01 * 255) ** 2;
  const C2 = (0.03 * 255) ** 2;
  const w = width - 10;
  const h = height - 10;
  const map = new Float32Array(w * h);
  // In strips of output rows: the horizontal pass of the five moments for the
  // strip's input rows, then the vertical pass straight into SSIM.
  const strip = 64;
  const hx = new Float32Array(w * (strip + 10));
  const hy = new Float32Array(w * (strip + 10));
  const hxx = new Float32Array(w * (strip + 10));
  const hyy = new Float32Array(w * (strip + 10));
  const hxy = new Float32Array(w * (strip + 10));
  let sum = 0;
  for (let y0 = 0; y0 < h; y0 += strip) {
    const outRows = Math.min(strip, h - y0);
    for (let r = 0; r < outRows + 10; r++) {
      const src = (y0 + r) * width;
      const dst = r * w;
      for (let x = 0; x < w; x++) {
        const i = src + x;
        const a0 = a[i], a1 = a[i + 1], a2 = a[i + 2], a3 = a[i + 3], a4 = a[i + 4], a5 = a[i + 5];
        const a6 = a[i + 6], a7 = a[i + 7], a8 = a[i + 8], a9 = a[i + 9], a10 = a[i + 10];
        const b0 = b[i], b1 = b[i + 1], b2 = b[i + 2], b3 = b[i + 3], b4 = b[i + 4], b5 = b[i + 5];
        const b6 = b[i + 6], b7 = b[i + 7], b8 = b[i + 8], b9 = b[i + 9], b10 = b[i + 10];
        const o = dst + x;
        hx[o] = g5 * (a0 + a10) + g4 * (a1 + a9) + g3 * (a2 + a8) + g2 * (a3 + a7) + g1 * (a4 + a6) + g0 * a5;
        hy[o] = g5 * (b0 + b10) + g4 * (b1 + b9) + g3 * (b2 + b8) + g2 * (b3 + b7) + g1 * (b4 + b6) + g0 * b5;
        hxx[o] =
          g5 * (a0 * a0 + a10 * a10) + g4 * (a1 * a1 + a9 * a9) + g3 * (a2 * a2 + a8 * a8) +
          g2 * (a3 * a3 + a7 * a7) + g1 * (a4 * a4 + a6 * a6) + g0 * a5 * a5;
        hyy[o] =
          g5 * (b0 * b0 + b10 * b10) + g4 * (b1 * b1 + b9 * b9) + g3 * (b2 * b2 + b8 * b8) +
          g2 * (b3 * b3 + b7 * b7) + g1 * (b4 * b4 + b6 * b6) + g0 * b5 * b5;
        hxy[o] =
          g5 * (a0 * b0 + a10 * b10) + g4 * (a1 * b1 + a9 * b9) + g3 * (a2 * b2 + a8 * b8) +
          g2 * (a3 * b3 + a7 * b7) + g1 * (a4 * b4 + a6 * b6) + g0 * a5 * b5;
      }
    }
    const w2 = 2 * w, w3 = 3 * w, w4 = 4 * w, w5 = 5 * w, w6 = 6 * w, w7 = 7 * w, w8 = 8 * w, w9 = 9 * w, w10 = 10 * w;
    for (let r = 0; r < outRows; r++)
      for (let x = 0; x < w; x++) {
        const i = r * w + x;
        const mx =
          g5 * (hx[i] + hx[i + w10]) + g4 * (hx[i + w] + hx[i + w9]) + g3 * (hx[i + w2] + hx[i + w8]) +
          g2 * (hx[i + w3] + hx[i + w7]) + g1 * (hx[i + w4] + hx[i + w6]) + g0 * hx[i + w5];
        const my =
          g5 * (hy[i] + hy[i + w10]) + g4 * (hy[i + w] + hy[i + w9]) + g3 * (hy[i + w2] + hy[i + w8]) +
          g2 * (hy[i + w3] + hy[i + w7]) + g1 * (hy[i + w4] + hy[i + w6]) + g0 * hy[i + w5];
        const mxx =
          g5 * (hxx[i] + hxx[i + w10]) + g4 * (hxx[i + w] + hxx[i + w9]) + g3 * (hxx[i + w2] + hxx[i + w8]) +
          g2 * (hxx[i + w3] + hxx[i + w7]) + g1 * (hxx[i + w4] + hxx[i + w6]) + g0 * hxx[i + w5];
        const myy =
          g5 * (hyy[i] + hyy[i + w10]) + g4 * (hyy[i + w] + hyy[i + w9]) + g3 * (hyy[i + w2] + hyy[i + w8]) +
          g2 * (hyy[i + w3] + hyy[i + w7]) + g1 * (hyy[i + w4] + hyy[i + w6]) + g0 * hyy[i + w5];
        const mxy =
          g5 * (hxy[i] + hxy[i + w10]) + g4 * (hxy[i + w] + hxy[i + w9]) + g3 * (hxy[i + w2] + hxy[i + w8]) +
          g2 * (hxy[i + w3] + hxy[i + w7]) + g1 * (hxy[i + w4] + hxy[i + w6]) + g0 * hxy[i + w5];
        const vx = mxx - mx * mx;
        const vy = myy - my * my;
        const cxy = mxy - mx * my;
        const s = ((2 * mx * my + C1) * (2 * cxy + C2)) / ((mx * mx + my * my + C1) * (vx + vy + C2));
        map[(y0 + r) * w + x] = s;
        sum += s;
      }
  }
  return { mean: sum / (w * h), map, width: w, height: h };
}

/** Mean absolute difference over R, G and B (0-255), and the share of pixels whose largest channel difference exceeds 16. */
export function meanAbsoluteDifference(a: Image, b: Image) {
  let sum = 0;
  let changed = 0;
  for (let i = 0, n = a.width * a.height; i < n; i++) {
    const o = i * 4;
    const dr = Math.abs(a.data[o] - b.data[o]);
    const dg = Math.abs(a.data[o + 1] - b.data[o + 1]);
    const db = Math.abs(a.data[o + 2] - b.data[o + 2]);
    sum += dr + dg + db;
    if (Math.max(dr, dg, db) > 16) changed++;
  }
  const n = a.width * a.height;
  return { mad: sum / (3 * n), changedShare: changed / n };
}

/**
 * Heatmap: the reference (Chromium) faded to light gray, and on top of it each
 * pixel's largest channel difference from yellow (small) to red (64 and more).
 */
export function diffHeatmap(a: Image, b: Image): Image {
  const data = new Uint8Array(a.width * a.height * 4);
  for (let i = 0, n = a.width * a.height; i < n; i++) {
    const o = i * 4;
    const gray = 0.2989 * a.data[o] + 0.587 * a.data[o + 1] + 0.114 * a.data[o + 2];
    const base = 175 + gray * 0.3;
    const d = Math.max(Math.abs(a.data[o] - b.data[o]), Math.abs(a.data[o + 1] - b.data[o + 1]), Math.abs(a.data[o + 2] - b.data[o + 2]));
    if (d <= 2) {
      data[o] = data[o + 1] = data[o + 2] = base;
    } else {
      const t = Math.min(1, d / 64);
      const alpha = 0.35 + 0.65 * t;
      const r = 255;
      const g = Math.round(220 * (1 - t));
      const bl = 0;
      data[o] = Math.round(base * (1 - alpha) + r * alpha);
      data[o + 1] = Math.round(base * (1 - alpha) + g * alpha);
      data[o + 2] = Math.round(base * (1 - alpha) + bl * alpha);
    }
    data[o + 3] = 255;
  }
  return { width: a.width, height: a.height, data };
}

/** `images` left to right with a gray gap between them; all of one height. */
export function sideBySide(images: Image[], gap = 24): Image {
  const height = Math.max(...images.map((i) => i.height));
  const width = images.reduce((sum, i) => sum + i.width, 0) + gap * (images.length - 1);
  const data = new Uint8Array(width * height * 4).fill(128);
  let left = 0;
  for (const image of images) {
    for (let y = 0; y < image.height; y++)
      data.set(image.data.subarray(y * image.width * 4, (y + 1) * image.width * 4), (y * width + left) * 4);
    left += image.width + gap;
  }
  return { width, height, data };
}

export function crop(image: Image, x: number, y: number, width: number, height: number): Image {
  const data = new Uint8Array(width * height * 4);
  for (let row = 0; row < height; row++)
    data.set(image.data.subarray(((y + row) * image.width + x) * 4, ((y + row) * image.width + x + width) * 4), row * width * 4);
  return { width, height, data };
}
