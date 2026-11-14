// The paint matrix of bench/fidelity/paint.ts: each paint feature on a subject box, in each context that has broken
// one in pyro before (opacity on the box or on a parent, a transform, an overflow clip, a filter, a positioned box
// outside its parent, a stacking context). One small page per pair, with probe points: places in the subject's own
// coordinates (they follow the context's transform) whose colour both browsers should agree on.
//
// The pages are generated, not committed. `pages()` returns them with their HTML and probes; probes are 1x1
// elements (class "probe") inside the subject, measured in Chromium and read from both screenshots.
import { encodePng, type Image } from './image';

export const VIEWPORT = { width: 360, height: 260 };

export type Probe = [name: string, x: number, y: number];
export type Feature = { name: string; group: string; css: string; inner?: string; probes: Probe[] };
export type Context = { name: string; ctx?: string; self?: string; before?: string };
export type MatrixPage = { name: string; feature: string; group: string; context: string; html: string; probes: string[] };

function png(width: number, height: number, color: (x: number, y: number) => [number, number, number]): string {
  const data = new Uint8Array(width * height * 4);
  for (let y = 0; y < height; y++)
    for (let x = 0; x < width; x++) {
      const [r, g, b] = color(x, y);
      data.set([r, g, b, 255], (y * width + x) * 4);
    }
  const image: Image = { width, height, data };
  return `data:image/png;base64,${Buffer.from(encodePng(image, 9)).toString('base64')}`;
}
const BLUE: [number, number, number] = [43, 108, 176];
const ORANGE: [number, number, number] = [246, 173, 85];
// 100x60: blue left half, orange right half. 20x20 tile: blue and orange quarters.
const HALVES = png(100, 60, (x) => (x < 50 ? BLUE : ORANGE));
const TILE = png(20, 20, (x, y) => ((x < 10) !== (y < 10) ? BLUE : ORANGE));
// Two-tone content for the filter functions: blue box, orange right half.
const TWO_TONE = `<div style="position:absolute;left:50px;top:0;width:50px;height:60px;background:#f6ad55"></div>`;
const LR: Probe[] = [['left', 25, 30], ['right', 75, 30]];
const filter = (fn: string, extra: Probe[] = []): Feature => ({
  name: `filter: ${fn}`, group: 'filters', css: `background:#2b6cb0;filter:${fn}`, inner: TWO_TONE, probes: [...LR, ...extra],
});

export const FEATURES: Feature[] = [
  { name: 'box-shadow', group: 'shadows', css: 'background:#2b6cb0;box-shadow:12px 12px 0 4px rgba(9,15,27,.62)', probes: [['in', 50, 30], ['shadow', 110, 70], ['out', -20, -20]] },
  { name: 'box-shadow blur', group: 'shadows', css: 'background:#2b6cb0;box-shadow:0 0 14px 6px rgba(200,30,30,.9)', probes: [['in', 50, 30], ['halo', 104, 30], ['far', 140, 30]] },
  { name: 'box-shadow inset', group: 'shadows', css: 'background:#e2e8f0;box-shadow:inset 10px 10px 0 rgba(0,0,0,.6)', probes: [['shade', 5, 30], ['in', 60, 40]] },
  { name: 'box-shadow 6000px spread', group: 'shadows', css: 'background:#2b6cb0;box-shadow:0 0 0 6000px rgba(9,15,27,.62)', probes: [['in', 50, 30], ['far', -40, -30]] },
  { name: 'text-shadow', group: 'shadows', css: 'font:40px/1 sans-serif;color:transparent;text-shadow:0 0 0 rgb(200,30,30)', inner: '█', probes: [['glyph', 10, 20]] },
  { name: 'text-shadow offset blur', group: 'shadows', css: 'font:32px/1 sans-serif;color:#1a202c;text-shadow:6px 6px 2px rgb(200,30,30)', inner: 'Ag█', probes: [] },
  filter('blur(4px)', [['halo', 103, 30]]),
  filter('brightness(1.6)'),
  filter('contrast(2.5)'),
  filter('grayscale(1)'),
  filter('hue-rotate(120deg)'),
  filter('invert(1)'),
  filter('opacity(.4)'),
  filter('saturate(3)'),
  filter('sepia(1)'),
  filter('drop-shadow(10px 10px 0 rgba(0,0,0,.7))', [['shadow', 105, 65]]),
  { name: 'border-radius', group: 'borders', css: 'box-sizing:border-box;background:#fefcbf;border:6px solid #c05621;border-radius:18px', probes: [['corner', 1, 1], ['edge', 50, 2], ['in', 50, 30]] },
  { name: 'border styles', group: 'borders', css: 'box-sizing:border-box;background:#fefcbf;border:6px dashed #2c5282;border-left-style:dotted;border-right:6px double #2c5282', probes: [['in', 50, 30]] },
  { name: 'outline', group: 'outlines', css: 'background:#e9d8fd;outline:5px solid #805ad5;outline-offset:4px', probes: [['ring', -6, 30], ['in', 50, 30]] },
  { name: 'linear-gradient', group: 'backgrounds', css: 'background:linear-gradient(90deg,#e53e3e,#3182ce)', probes: [['left', 3, 30], ['mid', 50, 30], ['right', 97, 30]] },
  { name: 'radial-gradient', group: 'backgrounds', css: 'background:radial-gradient(circle at 30% 40%,#f6e05e,#dd6b20 40%,#2c5282)', probes: [['centre', 30, 24], ['edge', 95, 55]] },
  { name: 'background-image', group: 'backgrounds', css: `background:url(${TILE}) 0 0/20px 20px`, probes: [['a', 5, 5], ['b', 15, 5]] },
  { name: 'img', group: 'images', css: '', inner: `<img src="${HALVES}" style="display:block;width:100px;height:60px">`, probes: LR },
  { name: 'clip-path polygon', group: 'clip-path', css: 'background:#2b6cb0;clip-path:polygon(0 0,100% 0,0 100%)', probes: [['kept', 15, 15], ['cut', 85, 50]] },
  { name: 'clip-path circle', group: 'clip-path', css: 'background:#2b6cb0;clip-path:circle(28px at 50% 50%)', probes: [['kept', 50, 30], ['cut', 5, 5]] },
  { name: 'mask-image', group: 'masks', css: 'background:#2b6cb0;mask-image:linear-gradient(to right,#000 30%,transparent 70%)', probes: [['kept', 10, 30], ['cut', 95, 30]] },
  { name: 'mix-blend-mode', group: 'blending', css: 'background:#3182ce;mix-blend-mode:multiply', probes: [['in', 50, 30]] },
  { name: 'transform 2D', group: 'transforms', css: 'background:#2b6cb0;transform:rotate(20deg)', probes: [['in', 50, 30], ['corner', 2, 2]] },
  { name: 'transform 3D', group: 'transforms', css: 'background:#2b6cb0;transform:perspective(200px) rotateY(40deg)', probes: [['in', 50, 30]] },
];

// `ctx`: styles of the subject's parent (240x160 at 60,50); `self`: added to the subject's own; `before`: markup
// before the parent.
export const CONTEXTS: Context[] = [
  { name: 'none' },
  { name: 'opacity .5 (self)', self: 'opacity:.5' },
  { name: 'opacity .5 (parent)', ctx: 'opacity:.5' },
  { name: 'opacity 0 (parent)', ctx: 'opacity:0' },
  { name: 'transform', ctx: 'transform:rotate(-8deg) scale(.9)' },
  { name: 'overflow clip', ctx: 'overflow:hidden;width:120px;height:80px' },
  { name: 'filter', ctx: 'filter:hue-rotate(120deg)' },
  // A positioned box outside its parent's box (a zero-size parent), then the same parent with opacity: the case the
  // ToS feature videos hit (an opacity layer clipped to its own box drops the child).
  { name: 'outside the parent', ctx: 'width:0;height:0' },
  { name: 'outside an opacity parent', ctx: 'opacity:.6;width:0;height:0' },
  { name: 'stacking context', ctx: 'z-index:1', before: '<div style="position:absolute;left:150px;top:110px;width:120px;height:70px;background:#38a169;z-index:0"></div>' },
];

const slug = (s: string) => s.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '');

export function pages(): MatrixPage[] {
  const out: MatrixPage[] = [];
  for (const f of FEATURES)
    for (const c of CONTEXTS) {
      const probes = f.probes.map(([n, x, y]) => `<i class=probe data-name="${n}" style="left:${x}px;top:${y}px"></i>`).join('');
      const html = `<!doctype html><meta charset=utf-8><title>${f.name} / ${c.name}</title><style>
html,body{margin:0}body{background:#fdf2d0;width:${VIEWPORT.width}px;height:${VIEWPORT.height}px;position:relative;overflow:hidden}
.ctx{position:absolute;left:60px;top:50px;width:240px;height:160px}.s{position:absolute;left:50px;top:40px;width:100px;height:60px}
.probe{position:absolute;width:1px;height:1px}</style>
${c.before ?? ''}<div class=ctx style="${c.ctx ?? ''}"><div class=s style="${f.css}${c.self ? `;${c.self}` : ''}">${f.inner ?? ''}${probes}</div></div>`;
      out.push({ name: `${slug(f.name)}--${slug(c.name)}`, feature: f.name, group: f.group, context: c.name, html, probes: f.probes.map(([n]) => n) });
    }
  return out;
}
