// Just enough PNG decoding for the capture tests: the header, and the color
// of one pixel (rows are unfiltered from the top down to it only).
import { inflateSync } from 'node:zlib';

export type PngInfo = { width: number; height: number; bitDepth: number; colorType: number };

function chunks(buf: Buffer): { type: string; data: Buffer }[] {
  if (buf.readUInt32BE(0) !== 0x89504e47) throw new Error('not a PNG');
  const out: { type: string; data: Buffer }[] = [];
  for (let i = 8; i < buf.length; ) {
    const len = buf.readUInt32BE(i);
    out.push({ type: buf.toString('ascii', i + 4, i + 8), data: buf.subarray(i + 8, i + 8 + len) });
    i += 12 + len;
  }
  return out;
}

export function pngInfo(buf: Buffer): PngInfo {
  const ihdr = chunks(buf).find((c) => c.type === 'IHDR')!.data;
  return { width: ihdr.readUInt32BE(0), height: ihdr.readUInt32BE(4), bitDepth: ihdr[8], colorType: ihdr[9] };
}

/** RGBA of pixel (x, y) of an 8-bit RGB or RGBA PNG (not interlaced). */
export function pngPixel(buf: Buffer, x: number, y: number): [number, number, number, number] {
  const info = pngInfo(buf);
  if (info.bitDepth !== 8 || (info.colorType !== 2 && info.colorType !== 6)) throw new Error(`unsupported PNG: depth ${info.bitDepth}, color type ${info.colorType}`);
  const bpp = info.colorType === 6 ? 4 : 3;
  const stride = info.width * bpp;
  const raw = inflateSync(Buffer.concat(chunks(buf).filter((c) => c.type === 'IDAT').map((c) => c.data)));
  let prev = Buffer.alloc(stride);
  let row = Buffer.alloc(stride);
  for (let r = 0; r <= y; r++) {
    const filter = raw[r * (stride + 1)];
    const line = raw.subarray(r * (stride + 1) + 1, (r + 1) * (stride + 1));
    for (let i = 0; i < stride; i++) {
      const a = i >= bpp ? row[i - bpp] : 0;
      const b = prev[i];
      const c = i >= bpp ? prev[i - bpp] : 0;
      let v = line[i];
      if (filter === 1) v += a;
      else if (filter === 2) v += b;
      else if (filter === 3) v += (a + b) >> 1;
      else if (filter === 4) {
        const p = a + b - c;
        const pa = Math.abs(p - a);
        const pb = Math.abs(p - b);
        const pc = Math.abs(p - c);
        v += pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
      }
      row[i] = v & 0xff;
    }
    [prev, row] = [row, prev];
  }
  const px = prev.subarray(x * bpp, x * bpp + bpp);
  return [px[0], px[1], px[2], bpp === 4 ? px[3] : 255];
}
