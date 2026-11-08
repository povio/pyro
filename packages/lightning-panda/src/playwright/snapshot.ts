// `expect(value).toMatchSnapshot()`: a string or Buffer compared with a file
// at Playwright's snapshot path (`snapshotPathTemplate`). PNGs compare by
// pixel (`threshold`, `maxDiffPixels`, `maxDiffPixelRatio`), the rest by
// bytes. A missing snapshot is written and fails the test, as in Playwright;
// `-u` / `updateSnapshots` rewrites what differs.
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { basename, dirname, extname, join, normalize, relative, resolve } from 'node:path';
import { inflateSync } from 'node:zlib';
import type { TestInfo } from './context.js';

export type SnapshotOptions = { threshold?: number; maxDiffPixels?: number; maxDiffPixelRatio?: number };
export type SnapshotKind = 'snapshot' | 'screenshot' | 'aria';

const LEGACY_TEMPLATE = '{snapshotDir}/{testFileDir}/{testFileName}-snapshots/{arg}{-projectName}{-snapshotSuffix}{ext}';
const ARIA_TEMPLATE = '{snapshotDir}/{testFileDir}/{testFileName}-snapshots/{arg}{ext}';
/** Playwright's `sanitizeForFilePath`. */
export const sanitize = (s: string) => s.replace(/[\x00-\x2C\x2E-\x2F\x3A-\x40\x5B-\x60\x7B-\x7F]+/g, '-');
/** Playwright's `trimLongString`: at most `length` chars, a sha1 fragment in the middle. */
export function trimLongString(s: string, length = 100): string {
  if (s.length <= length) return s;
  const middle = `-${createHash('sha1').update(s).digest('hex').substring(0, 5)}-`;
  const start = Math.floor((length - middle.length) / 2);
  const end = length - middle.length - start;
  return s.substring(0, start) + middle + s.slice(-end);
}
const extOf = (kind: SnapshotKind, p: string) => (kind === 'aria' && p.endsWith('.aria.yml') ? '.aria.yml' : extname(p));

/**
 * Playwright's snapshot path (`testInfo._resolveSnapshotPaths`): a string
 * name is sanitized before its extension, an array is joined as path
 * segments as is; the template expands per kind and resolves against the
 * config's directory.
 */
export function snapshotPath(info: TestInfo, name: string | string[], kind: SnapshotKind = 'snapshot'): string {
  const proj = info.project as typeof info.project & { snapshotDir?: string; snapshotPathTemplate?: string; configDir?: string; expect?: Record<string, any> };
  let sub: string, ext: string;
  if (Array.isArray(name)) {
    sub = join(...name);
    ext = extOf(kind, sub);
  } else {
    ext = extOf(kind, name);
    sub = sanitize(name.substring(0, name.length - ext.length)) + ext;
  }
  const arg = join(dirname(sub), basename(sub, ext));
  const expectCfg = (kind === 'screenshot' ? proj.expect?.toHaveScreenshot : kind === 'aria' ? proj.expect?.toMatchAriaSnapshot : undefined) ?? {};
  const template: string = (kind === 'snapshot' ? undefined : expectCfg.pathTemplate) || proj.snapshotPathTemplate || (kind === 'aria' ? ARIA_TEMPLATE : LEGACY_TEMPLATE);
  const testDir = info.project.testDir;
  const rel = relative(testDir, info.file);
  const parsed = { dir: dirname(rel) === '.' ? '' : dirname(rel), base: basename(rel) };
  const projectName = sanitize(info.project.name ?? '');
  const always = (v: string) => (_m: string, p?: string) => (p ?? '') + v;
  const ifSet = (v: string) => (_m: string, p?: string) => (v ? (p ?? '') + v : '');
  const expanded = template
    .replace(/\{(.)?testDir\}/g, always(testDir))
    .replace(/\{(.)?snapshotDir\}/g, always(proj.snapshotDir ?? testDir))
    .replace(/\{(.)?snapshotSuffix\}/g, ifSet(info.snapshotSuffix))
    .replace(/\{(.)?testFileDir\}/g, always(parsed.dir))
    .replace(/\{(.)?platform\}/g, always(process.platform))
    .replace(/\{(.)?projectName\}/g, ifSet(projectName))
    .replace(/\{(.)?testName\}/g, always(sanitize(trimLongString(info.titlePath.slice(1).join(' ')))))
    .replace(/\{(.)?testFileName\}/g, always(parsed.base))
    .replace(/\{(.)?testFilePath\}/g, always(rel))
    .replace(/\{(.)?arg\}/g, always(arg))
    .replace(/\{(.)?ext\}/g, ifSet(ext));
  return normalize(resolve(proj.configDir ?? process.cwd(), expanded));
}

type Image = { width: number; height: number; data: Uint8Array };

/** Any valid PNG (gray, RGB, palette, gray+alpha, RGBA; bit depths 1-16; Adam7) as 8-bit RGBA, as pngjs reads it; `undefined` if it is not one. */
export function decodePng(buf: Buffer): Image | undefined {
  try {
    return decode(buf);
  } catch {
    return undefined;
  }
}

function decode(buf: Buffer): Image | undefined {
  if (buf.length < 8 || buf.readUInt32BE(0) !== 0x89504e47 || buf.readUInt32BE(4) !== 0x0d0a1a0a) return undefined;
  let off = 8, width = 0, height = 0, depth = 0, type = -1, interlace = 0;
  let palette: Buffer | undefined, trns: Buffer | undefined;
  const idat: Buffer[] = [];
  while (off + 8 <= buf.length) {
    const len = buf.readUInt32BE(off);
    const t = buf.toString('latin1', off + 4, off + 8);
    const body = buf.subarray(off + 8, off + 8 + len);
    if (t === 'IHDR') [width, height, depth, type, interlace] = [body.readUInt32BE(0), body.readUInt32BE(4), body[8], body[9], body[12]];
    else if (t === 'PLTE') palette = body;
    else if (t === 'tRNS') trns = body;
    else if (t === 'IDAT') idat.push(body);
    else if (t === 'IEND') break;
    off += 12 + len;
  }
  const channels = ({ 0: 1, 2: 3, 3: 1, 4: 2, 6: 4 } as Record<number, number>)[type];
  const depths = ({ 0: [1, 2, 4, 8, 16], 2: [8, 16], 3: [1, 2, 4, 8], 4: [8, 16], 6: [8, 16] } as Record<number, number[]>)[type];
  if (!channels || !depths?.includes(depth) || !width || !height || interlace > 1 || (type === 3 && !palette)) return undefined;
  const raw = inflateSync(Buffer.concat(idat));
  const bitsPerPixel = channels * depth;
  const bpp = Math.max(1, bitsPerPixel >> 3);
  const data = new Uint8Array(width * height * 4);
  const max = 2 ** depth - 1;
  // tRNS for gray / RGB: the one fully transparent sample value(s), at the image's depth.
  const key = trns && (type === 0 || type === 2) ? Array.from({ length: trns.length >> 1 }, (_, i) => trns!.readUInt16BE(i * 2)) : undefined;
  let pos = 0;
  // Adam7 passes: [x0, y0, dx, dy]; one pass covering the image when not interlaced.
  const passes = interlace ? [[0, 0, 8, 8], [4, 0, 8, 8], [0, 4, 4, 8], [2, 0, 4, 4], [0, 2, 2, 4], [1, 0, 2, 2], [0, 1, 1, 2]] : [[0, 0, 1, 1]];
  for (const [x0, y0, dx, dy] of passes) {
    const pw = Math.ceil((width - x0) / dx), ph = Math.ceil((height - y0) / dy);
    if (pw <= 0 || ph <= 0) continue;
    const stride = Math.ceil((pw * bitsPerPixel) / 8);
    let prev = new Uint8Array(stride), cur = new Uint8Array(stride);
    /** Sample `ch` of pixel `x` on the current row, at the image's depth. */
    const s = (x: number, ch: number): number => {
      const i = x * channels + ch;
      if (depth === 8) return cur[i];
      if (depth === 16) return (cur[i * 2] << 8) | cur[i * 2 + 1];
      const bit = i * depth;
      return (cur[bit >> 3] >> (8 - depth - (bit & 7))) & max;
    };
    /** The same sample scaled to 8 bits, as pngjs does. */
    const s8 = (x: number, ch: number): number => (depth === 8 ? cur[x * channels + ch] : Math.floor((s(x, ch) * 255) / max + 0.5));
    for (let y = 0; y < ph; y++) {
      if (pos + 1 + stride > raw.length) throw new Error('truncated PNG');
      const f = raw[pos];
      const src = raw.subarray(pos + 1, pos + 1 + stride);
      pos += 1 + stride;
      for (let i = 0; i < stride; i++) {
        const a = i >= bpp ? cur[i - bpp] : 0, b = prev[i], c = i >= bpp ? prev[i - bpp] : 0;
        let p = src[i];
        if (f === 1) p += a;
        else if (f === 2) p += b;
        else if (f === 3) p += (a + b) >> 1;
        else if (f === 4) {
          const q = a + b - c, pa = Math.abs(q - a), pb = Math.abs(q - b), pc = Math.abs(q - c);
          p += pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
        } else if (f !== 0) throw new Error(`bad PNG filter ${f}`);
        cur[i] = p & 255;
      }
      for (let x = 0; x < pw; x++) {
        const d = (((y0 + y * dy) * width) + x0 + x * dx) * 4;
        if (type === 3) {
          const k = s(x, 0);
          data[d] = palette![k * 3] ?? 0;
          data[d + 1] = palette![k * 3 + 1] ?? 0;
          data[d + 2] = palette![k * 3 + 2] ?? 0;
          data[d + 3] = trns && k < trns.length ? trns[k] : 255;
        } else if (type === 0 || type === 4) {
          const g = s8(x, 0);
          data[d] = data[d + 1] = data[d + 2] = g;
          data[d + 3] = type === 4 ? s8(x, 1) : key && s(x, 0) === key[0] ? 0 : 255;
        } else {
          data[d] = s8(x, 0);
          data[d + 1] = s8(x, 1);
          data[d + 2] = s8(x, 2);
          data[d + 3] = type === 6 ? s8(x, 3) : key && key.length >= 3 && s(x, 0) === key[0] && s(x, 1) === key[1] && s(x, 2) === key[2] ? 0 : 255;
        }
      }
      [prev, cur] = [cur, prev];
    }
  }
  return { width, height, data };
}

/** Pixels that differ beyond `threshold` (pixelmatch's YIQ distance over white). */
function diffPixels(a: Image, b: Image, threshold: number): number {
  const max = 35215 * threshold * threshold;
  let n = 0;
  for (let i = 0; i < a.data.length; i += 4) {
    const blend = (d: Uint8Array, k: number) => 255 + ((d[i + k] - 255) * d[i + 3]) / 255;
    const r = blend(a.data, 0) - blend(b.data, 0), g = blend(a.data, 1) - blend(b.data, 1), bl = blend(a.data, 2) - blend(b.data, 2);
    const y = r * 0.29889531 + g * 0.58662247 + bl * 0.11448223;
    const iq = r * 0.59597799 - g * 0.2741761 - bl * 0.32180189;
    const q = r * 0.21147017 - g * 0.52261711 + bl * 0.31114694;
    if (0.5053 * y * y + 0.299 * iq * iq + 0.1957 * q * q > max) n++;
  }
  return n;
}

/** Why `actual` does not match `expected`, or `undefined` when it does. */
export function compare(actual: Buffer, expected: Buffer, o: SnapshotOptions): string | undefined {
  if (actual.equals(expected)) return undefined;
  const a = decodePng(actual), e = decodePng(expected);
  if (!a || !e) return 'the contents differ';
  if (a.width !== e.width || a.height !== e.height) return `expected an image ${e.width}px by ${e.height}px, received ${a.width}px by ${a.height}px`;
  const n = diffPixels(a, e, o.threshold ?? 0.2);
  // As Playwright's comparator: the stricter of the two limits when both are set.
  const total = e.width * e.height;
  const byRatio = o.maxDiffPixelRatio !== undefined ? total * o.maxDiffPixelRatio : undefined;
  const allowed = o.maxDiffPixels !== undefined && byRatio !== undefined ? Math.min(o.maxDiffPixels, byRatio) : (o.maxDiffPixels ?? byRatio ?? 0);
  return n > allowed ? `${n} pixels (ratio ${(Math.ceil((n / total) * 100) / 100).toFixed(2)} of all image pixels) are different` : undefined;
}

export type UpdateMode = 'all' | 'changed' | 'missing' | 'none';
export function updateSnapshots(info: TestInfo): UpdateMode {
  const env = process.env.LP_UPDATE_SNAPSHOTS;
  if (env) return (['1', 'true'].includes(env) ? 'changed' : env) as UpdateMode;
  // `bun test -u` without `lightningpanda test`: Playwright's `-u` default.
  if (process.argv.includes('-u') || process.argv.includes('--update-snapshots')) return 'changed';
  return (info.config as Record<string, any>).updateSnapshots ?? 'missing';
}

const counters = new WeakMap<TestInfo, Map<string, number>>();
/**
 * Playwright's anonymous snapshot name, `sanitize(trimLongString("<titles> <n>")) + ext`.
 * toMatchSnapshot and toHaveScreenshot share one counter, ARIA snapshots have their own.
 */
export function anonymousName(info: TestInfo, ext: string, kind: SnapshotKind | string, advance = true): string {
  const m = counters.get(info) ?? new Map<string, number>();
  counters.set(info, m);
  const bucket = kind === 'aria' ? 'aria' : 'snapshot';
  const n = (m.get(bucket) ?? 0) + 1;
  if (advance) m.set(bucket, n);
  return sanitize(trimLongString([...info.titlePath.slice(1), n].join(' '))) + ext;
}

/** `toMatchAriaSnapshot()`'s file: `name`, else the anonymous `.aria.yml`, or the legacy anonymous `.yml` when only that exists. */
export function ariaSnapshotFile(info: TestInfo, name?: string): string {
  if (name) return snapshotPath(info, name, 'aria');
  const legacy = snapshotPath(info, anonymousName(info, '.yml', 'aria', false), 'aria');
  const file = snapshotPath(info, anonymousName(info, '.aria.yml', 'aria'), 'aria');
  return !existsSync(file) && existsSync(legacy) ? legacy : file;
}

/** Playwright's "missing" mode: the baseline was written, the test fails at its end and is not retried. */
export function missingWritten(info: TestInfo, message: string): void {
  info._hasNonRetriableError = true;
  info._softErrors.push(new Error(message));
}

/** Compares `received` with its snapshot file; writes it when missing or updating. Returns a failure message. */
export function matchFile(info: TestInfo, file: string, received: Buffer, o: SnapshotOptions, isNot: boolean, label: string): string | undefined {
  const mode = updateSnapshots(info);
  const shown = relative(process.cwd(), file);
  if (!existsSync(file)) {
    if (isNot) return `A snapshot doesn't exist at ${shown}, matchers using ".not" won't write them automatically.`;
    if (mode === 'none') return `A snapshot doesn't exist at ${shown}.`;
    mkdirSync(dirname(file), { recursive: true });
    writeFileSync(file, received);
    if (mode === 'all' || mode === 'changed') return undefined;
    // As in Playwright: the test fails at its end (no retry), the body goes on.
    missingWritten(info, `A snapshot doesn't exist at ${shown}, writing actual.`);
    return undefined;
  }
  const expected = readFileSync(file);
  const why = compare(received, expected, o);
  if (isNot) return why ? undefined : `${label}: expected not to match ${shown}`;
  if (!why) {
    if (mode === 'all' && !received.equals(expected)) writeFileSync(file, received);
    return undefined;
  }
  if (mode === 'all' || mode === 'changed') {
    writeFileSync(file, received);
    return undefined;
  }
  const ext = extname(file);
  const stem = basename(file, ext);
  const actualPath = info.outputPath(`${stem}-actual${ext}`);
  writeFileSync(actualPath, received);
  writeFileSync(info.outputPath(`${stem}-expected${ext}`), expected);
  info.attachments.push({ name: `${stem}-actual${ext}`, contentType: ext === '.png' ? 'image/png' : 'text/plain', path: actualPath });
  const text = !decodePng(received) && !decodePng(expected);
  const diff = text ? `\n\n${lineDiff(expected.toString(), received.toString())}` : '';
  return `${label} failed: ${shown}: ${why}${diff}\n\nActual: ${relative(process.cwd(), actualPath)}`;
}

/** A short line diff: `- expected`, `+ received`. */
export function lineDiff(expected: string, received: string): string {
  const e = expected.split('\n'), r = received.split('\n');
  const out: string[] = [];
  let i = 0, j = 0;
  while (i < e.length || j < r.length) {
    if (i < e.length && j < r.length && e[i] === r[j]) {
      out.push(`  ${e[i]}`);
      i++, j++;
    } else if (j < r.length && (i >= e.length || !e.slice(i).includes(r[j]))) out.push(`+ ${r[j++]}`);
    else out.push(`- ${e[i++]}`);
  }
  return out.join('\n');
}
