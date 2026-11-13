// Screen capture for agents: the page's metrics, element boxes on the page
// (iframes included), `Page.captureScreenshot` at a clip scale, image sizes
// from the encoded bytes, and the size of a preview the model can take.
import { createHash } from 'node:crypto';
import type { CDPSession } from '../wright/cdp-session.js';
import type { Locator, Page } from '../wright/page.js';

export type Rect = { x: number; y: number; width: number; height: number };

/** The viewport and document in CSS px, the scroll offsets and the device scale factor. */
export type Metrics = {
  width: number;
  height: number;
  deviceScaleFactor: number;
  scrollX: number;
  scrollY: number;
  documentWidth: number;
  documentHeight: number;
};

const METRICS = `(() => {
  const d = document.documentElement, b = document.body;
  return {
    width: innerWidth, height: innerHeight, deviceScaleFactor: devicePixelRatio || 1, scrollX, scrollY,
    documentWidth: Math.max(innerWidth, d ? d.scrollWidth : 0, b ? b.scrollWidth : 0),
    documentHeight: Math.max(innerHeight, d ? d.scrollHeight : 0, b ? b.scrollHeight : 0),
  };
})()`;

export function pageMetrics(page: Page): Promise<Metrics> {
  return page.evaluate<Metrics>(METRICS);
}

/**
 * `getBoundingClientRect()` of the element, moved into the top page's
 * viewport when it is inside iframes (each frame's position, border and
 * padding added). `crossOrigin`: a frame on the way is cross-origin, so the
 * box is only known within it.
 */
const BOX = `(el) => {
  const r = el.getBoundingClientRect();
  let x = r.x, y = r.y, w = window, crossOrigin = false;
  try {
    while (w !== w.top) {
      const f = w.frameElement;
      if (!f) { crossOrigin = true; break; }
      const fr = f.getBoundingClientRect(), cs = w.parent.getComputedStyle(f);
      x += fr.x + f.clientLeft + (parseFloat(cs.paddingLeft) || 0);
      y += fr.y + f.clientTop + (parseFloat(cs.paddingTop) || 0);
      w = w.parent;
    }
  } catch { crossOrigin = true; }
  return { x, y, width: r.width, height: r.height, crossOrigin };
}`;

export function elementBox(locator: Locator, timeout: number): Promise<Rect & { crossOrigin: boolean }> {
  return locator.evaluate(BOX, undefined, { timeout });
}

/** Playwright's `enclosingIntRect`: the rect rounded out to whole CSS px. */
export function enclosing(r: Rect): Rect {
  const x = Math.floor(r.x + 1e-3);
  const y = Math.floor(r.y + 1e-3);
  const x2 = Math.ceil(r.x + r.width - 1e-3);
  const y2 = Math.ceil(r.y + r.height - 1e-3);
  return { x, y, width: x2 - x, height: y2 - y };
}

/** Playwright's `trimClipToSize`: `clip` within `size` (undefined when nothing is left). */
export function trim(clip: Rect, size: { width: number; height: number }): Rect | undefined {
  const x1 = Math.max(0, Math.min(clip.x, size.width));
  const y1 = Math.max(0, Math.min(clip.y, size.height));
  const x2 = Math.max(0, Math.min(clip.x + clip.width, size.width));
  const y2 = Math.max(0, Math.min(clip.y + clip.height, size.height));
  const out = { x: x1, y: y1, width: x2 - x1, height: y2 - y1 };
  return out.width > 0 && out.height > 0 ? out : undefined;
}

export type Format = 'png' | 'jpeg';
export type Captured = { buf: Buffer; width: number; height: number; format: Format };

/**
 * One `Page.captureScreenshot`, its bytes as the browser encoded them. With
 * `clip` (CSS px from the document's top-left) the image has
 * `deviceScaleFactor × clipScale` pixels per CSS px; `beyond` paints the
 * document from its top-left instead of at the scroll position.
 */
export async function capture(cdp: CDPSession, o: { format: Format; clip?: Rect; clipScale?: number; beyond?: boolean }): Promise<Captured> {
  const params: Record<string, unknown> = { format: o.format };
  if (o.format === 'jpeg') params.quality = 80;
  if (o.clip) params.clip = { ...o.clip, scale: o.clipScale ?? 1 };
  if (o.beyond) params.captureBeyondViewport = true;
  const r = await cdp.send<{ data: string }>('Page.captureScreenshot', params);
  const buf = Buffer.from(r.data, 'base64');
  return { buf, format: o.format, ...imageSize(buf) };
}

/** Pixel size of a PNG (IHDR) or JPEG (SOF marker) from its bytes. */
export function imageSize(buf: Buffer): { width: number; height: number } {
  if (buf.length > 24 && buf.readUInt32BE(0) === 0x89504e47) return { width: buf.readUInt32BE(16), height: buf.readUInt32BE(20) };
  if (buf[0] === 0xff && buf[1] === 0xd8) {
    let i = 2;
    while (i + 9 < buf.length) {
      if (buf[i] !== 0xff) {
        i++;
        continue;
      }
      const marker = buf[i + 1];
      if (marker === 0xff || marker === 0x01 || (marker >= 0xd0 && marker <= 0xd8)) {
        i += marker === 0xff ? 1 : 2;
        continue;
      }
      if (marker >= 0xc0 && marker <= 0xcf && marker !== 0xc4 && marker !== 0xc8 && marker !== 0xcc) {
        return { width: buf.readUInt16BE(i + 7), height: buf.readUInt16BE(i + 5) };
      }
      i += 2 + buf.readUInt16BE(i + 2);
    }
  }
  return { width: 0, height: 0 };
}

export const sha256 = (buf: Buffer): string => createHash('sha256').update(buf).digest('hex');

/** Largest side Pyro renders, in device px (pyro-render's MAX_WIDTH and MAX_HEIGHT). */
export const RENDER_MAX = 16_384;

/**
 * An image the model takes as it is: Claude's high-resolution limits (Claude
 * 4.7 and later: 2576 px on the long edge, 4784 visual tokens of 28x28 px;
 * older models scale such an image down themselves), under the 3000 px a side
 * of requests with many images, and at most 5 MB base64 (the smallest
 * per-image limit of the APIs that serve Claude).
 */
export const MODEL_IMAGE = { maxSide: 2576, maxTokens: 4784, maxBase64: 5 * 1024 * 1024 };

export const visualTokens = (w: number, h: number): number => Math.ceil(w / 28) * Math.ceil(h / 28);

export function fitsModel(c: { width: number; height: number; buf: Buffer }): boolean {
  return (
    Math.max(c.width, c.height) <= MODEL_IMAGE.maxSide &&
    visualTokens(c.width, c.height) <= MODEL_IMAGE.maxTokens &&
    Math.ceil(c.buf.length / 3) * 4 <= MODEL_IMAGE.maxBase64
  );
}

/**
 * Pixels per CSS px, at most `max`, at which a `width`x`height` CSS px area
 * fits the model's pixel limits (sizes rounded as the renderer rounds them).
 */
export function previewScale(width: number, height: number, max: number): number {
  let s = Math.min(max, MODEL_IMAGE.maxSide / Math.max(width, height), Math.sqrt((MODEL_IMAGE.maxTokens * 784) / (width * height)));
  for (let i = 0; i < 400 && visualTokens(Math.round(width * s), Math.round(height * s)) > MODEL_IMAGE.maxTokens; i++) s *= 0.995;
  return s;
}
