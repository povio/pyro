<script setup lang="ts">
// One directory of the WPT status pages (/status/wpt.html, /status/wpt/<dir>.html), the directory taken from the
// address. Its data is a JSON file per directory under /status-data/, written at build time by
// bench/wpt/status-export.mjs from the wpt-results branch. The directory pages are copies of /status/wpt.html
// (VitePress knows only that page; copyPages maps each copy to it, so it hydrates as rendered), and a directory
// reached by a link shows this component where the theme would show "not found" (index.ts). The first render is
// the root's, as the HTML was rendered; the address takes over once mounted.
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { useData, useRoute, withBase } from 'vitepress';

const route = useRoute();
const { site } = useData();
const mounted = ref(false);
const dirPath = computed(() => {
  if (!mounted.value) return '';
  const base = site.value.base.replace(/\/$/, '');
  const p = decodeURIComponent(route.path.slice(base.length)).replace(/\.html$/, '');
  return p.startsWith('/status/wpt/') ? p.slice('/status/wpt/'.length) : '';
});
const props = computed(() => ({ path: dirPath.value }));

// WPT, run, passed, sub passed, subs, Chrome passed, Chrome ran, Chrome passed of the run pages, Chrome sub passed, Chrome subs
type Counts = [number, number, number, number, number, number, number, number, number, number];
type Test = { n: string; k: Kind; s: string | null; p?: number; t?: number; f?: string[]; c?: 0 | 1 };
type Page = {
  path: string; sha: string; date: string; wpt: string;
  chrome: { version: string; revision: string } | null;
  counts: Partial<Record<Kind, Counts>>;
  dirs: { name: string; counts: Partial<Record<Kind, Counts>> }[];
  tests: Test[];
  history: [string, string, Partial<Record<Kind, [number, number, number, number]>>][];
};
type Kind = 'testharness' | 'reftest' | 'crashtest';
const KINDS: { id: Kind; label: string }[] = [
  { id: 'testharness', label: 'Testharness' },
  { id: 'reftest', label: 'Reftests' },
  { id: 'crashtest', label: 'Crashtests' },
];

const page = ref<Page | null>(null);
const error = ref('');
const kind = ref<Kind>('testharness');
const range = ref(0);
const filter = ref<'all' | 'fail' | 'pass' | 'notrun'>('all');
const query = ref('');

const pageUrl = (d: string) => withBase(d ? `/status/wpt/${d}.html` : '/status/wpt.html');
const dataUrl = (d: string) => withBase(d ? `/status-data/wpt/${d}.json` : '/status-data/wpt.json');

async function load() {
  const d = props.value.path;
  page.value = null;
  error.value = '';
  query.value = '';
  filter.value = 'all';
  document.title = `${d ? `WPT: ${d}` : 'WPT status'} | ${site.value.title}`;
  try {
    const r = await fetch(dataUrl(d));
    if (!r.ok) throw new Error(r.status === 404 ? 'no such directory' : `HTTP ${r.status}`);
    const p = await r.json();
    if (d !== props.value.path) return;
    page.value = p;
    document.title = `${d ? `WPT: ${d}` : 'WPT status'} | ${site.value.title}`;
  } catch (e) {
    error.value = `Could not load the results: ${(e as Error).message}`;
    return;
  }
  const fromHash = location.hash.slice(1) as Kind;
  const present = kinds.value.map((k) => k.id);
  kind.value = present.includes(fromHash) ? fromHash : (present[0] ?? 'testharness');
}
// The chart is drawn at its box's width in CSS px, so its labels stay 11 px on a phone.
const chartBox = ref<HTMLElement | null>(null);
const W = ref(640);
let observer: ResizeObserver | null = null;
onMounted(() => {
  mounted.value = true;
  if (dirPath.value === '') load();
  observer = new ResizeObserver(([e]) => { W.value = Math.max(280, Math.round(e.contentRect.width)); });
  watch(chartBox, (el, old) => { if (old) observer!.unobserve(old); if (el) observer!.observe(el); }, { immediate: true });
});
onBeforeUnmount(() => observer?.disconnect());
watch(dirPath, load);
watch(kind, (k) => { if (page.value) history.replaceState(history.state, '', `#${k}`); });

const kinds = computed(() => KINDS.filter((k) => (page.value?.counts[k.id]?.[0] ?? 0) > 0));
const counts = computed<Counts>(() => page.value?.counts[kind.value] ?? [0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
const PASSING = { testharness: 'pages with every subtest passing', reftest: 'reftests passing', crashtest: 'crashtests passing' };
const crumbs = computed(() => {
  const parts = props.value.path ? props.value.path.split('/') : [];
  return [{ name: 'wpt', path: '' }, ...parts.map((p, i) => ({ name: p, path: parts.slice(0, i + 1).join('/') }))];
});

const pct = (a: number, b: number) => (b ? `${((100 * a) / b).toFixed(1)}%` : '–');
const num = (n: number) => n.toLocaleString('en-US');

const dirs = computed(() =>
  (page.value?.dirs ?? [])
    .filter((d) => (d.counts[kind.value]?.[0] ?? 0) > 0)
    .map((d) => ({ name: d.name, c: d.counts[kind.value]! }))
    .sort((a, b) => b.c[0] - a.c[0] || a.name.localeCompare(b.name)),
);

const passes = (t: Test) => t.s === 'complete' && t.p === t.t;
const tests = computed(() => {
  const q = query.value.trim().toLowerCase();
  return (page.value?.tests ?? []).filter((t) => {
    if (t.k !== kind.value) return false;
    if (q && !t.n.toLowerCase().includes(q)) return false;
    if (filter.value === 'notrun') return t.s === null;
    if (filter.value === 'pass') return passes(t);
    if (filter.value === 'fail') return t.s !== null && !passes(t);
    return true;
  });
});
const SHOWN = 500;
const shownTests = computed(() => tests.value.slice(0, SHOWN));
const kindTests = computed(() => (page.value?.tests ?? []).filter((t) => t.k === kind.value).length);

const result = (t: Test) => {
  if (t.s === null) return { text: 'no CI result', cls: 'notrun' };
  if (passes(t)) return { text: 'pass', cls: 'pass' };
  if (t.s !== 'complete') return { text: t.s, cls: 'fail' };
  return { text: 'fail', cls: 'fail' };
};
const testHref = (name: string) => `https://wpt.live/${props.value.path ? props.value.path + '/' : ''}${name}`;

// ---------- history chart ----------
const RANGES = [
  { label: '1 month', days: 31 },
  { label: '3 months', days: 92 },
  { label: '6 months', days: 183 },
  { label: '1 year', days: 366 },
  { label: 'All time', days: 0 },
];
const H = computed(() => (W.value < 500 ? 180 : 220));
const L = 40, R = 8, T = 12, B = 24;
const points = computed(() => {
  const h = (page.value?.history ?? []).filter((p) => p[2][kind.value]);
  if (!h.length) return [];
  const last = new Date(h[h.length - 1][0]).getTime();
  const days = RANGES[range.value].days;
  const from = days ? last - days * 864e5 : -Infinity;
  return h
    .map(([date, sha, m]) => ({ t: new Date(date).getTime(), date, sha, v: m[kind.value]! }))
    .filter((p) => p.t >= from)
    .map((p) => ({ ...p, y: p.v[1] ? (100 * p.v[0]) / p.v[1] : 0, ys: p.v[3] ? (100 * p.v[2]) / p.v[3] : null }));
});
const chart = computed(() => {
  const pts = points.value;
  if (!pts.length) return null;
  const W_ = W.value, H_ = H.value;
  const chromePct = counts.value[6] ? (100 * counts.value[5]) / counts.value[0] : null;
  const subs = kind.value === 'testharness' ? pts.filter((p) => p.ys !== null) : [];
  const ys = pts.map((p) => p.y).concat(chromePct ?? [], subs.map((p) => p.ys!));
  let lo = Math.max(0, Math.floor(Math.min(...ys) / 10) * 10 - (Math.min(...ys) % 10 < 2 ? 10 : 0));
  let hi = Math.min(100, Math.ceil(Math.max(...ys) / 10) * 10);
  if (hi - lo < 10) { lo = Math.max(0, hi - 10); hi = lo + 10; }
  const t0 = pts[0].t, t1 = pts[pts.length - 1].t;
  const x = (t: number) => (t1 === t0 ? L + (W_ - L - R) : L + ((t - t0) / (t1 - t0)) * (W_ - L - R));
  const y = (v: number) => T + (1 - (v - lo) / (hi - lo)) * (H_ - T - B);
  const step = hi - lo > 40 ? 20 : hi - lo > 20 ? 10 : 5;
  const ticks = [] as { y: number; label: string }[];
  for (let v = lo; v <= hi + 1e-9; v += step) ticks.push({ y: y(v), label: `${v}%` });
  const fmt = (t: number) => new Date(t).toLocaleDateString('en-GB', { day: 'numeric', month: 'short' });
  let d = pts.map((p, i) => `${i ? 'L' : 'M'}${x(p.t).toFixed(1)},${y(p.y).toFixed(1)}`).join(' ');
  if (pts.length === 1) d = `M${L},${y(pts[0].y).toFixed(1)} L${W_ - R},${y(pts[0].y).toFixed(1)}`;
  const ds = subs.map((p, i) => `${i ? 'L' : 'M'}${x(p.t).toFixed(1)},${y(p.ys!).toFixed(1)}`).join(' ');
  return {
    d, ticks, ds,
    dots: pts.map((p) => ({ cx: x(p.t), cy: y(p.y), title: `${p.date.slice(0, 10)} ${p.sha}: ${p.y.toFixed(2)}% (${num(p.v[0])} of ${num(p.v[1])})` })),
    chrome: chromePct === null ? null : { y: y(chromePct), label: `Chrome ${chromePct.toFixed(1)}%` },
    xLabels: t1 === t0 ? [{ x: W_ - R, label: fmt(t1), anchor: 'end' }] : [{ x: L, label: fmt(t0), anchor: 'start' }, { x: W_ - R, label: fmt(t1), anchor: 'end' }],
  };
});
</script>

<template>
  <div class="wpt">
    <nav class="crumbs" aria-label="Breadcrumb">
      <template v-for="(c, i) in crumbs" :key="c.path">
        <span v-if="i" class="sep">/</span>
        <a v-if="i < crumbs.length - 1" :href="pageUrl(c.path)">{{ c.name }}</a>
        <span v-else class="here">{{ c.name }}</span>
      </template>
    </nav>

    <p v-if="!props.path" class="round-progress">
      <a :href="withBase('/reference/round-six.html')">Round six compatibility progress →</a>
      <span>749 more CSS test pages pass in full on the 8,082-page comparison.</span>
    </p>

    <p v-if="error" class="error">{{ error }}</p>
    <p v-else-if="!page" class="muted">Loading…</p>
    <template v-else>
      <div class="tabs" role="tablist">
        <button v-for="k in kinds" :key="k.id" role="tab" :aria-selected="kind === k.id" :class="{ on: kind === k.id }" @click="kind = k.id">
          {{ k.label }} <span class="n">{{ num(page.counts[k.id]![0]) }}</span>
        </button>
      </div>

      <p class="meta">
        Main CI recorded results for {{ num(counts[1]) }} of {{ num(counts[0]) }} pages at
        <code>{{ page.sha.slice(0, 10) }}</code>. A missing result leaves the outcome unknown for this snapshot.
        Separate release-candidate measurements appear in the
        <a :href="withBase('/reference/round-seven.html')">round seven report</a>.
      </p>
      <div class="stats">
        <div class="stat">
          <div class="big">{{ pct(counts[2], counts[0]) }}</div>
          <div class="label">{{ PASSING[kind] }}: {{ num(counts[2]) }} of {{ num(counts[0]) }} in WPT</div>
        </div>
        <div class="stat">
          <div class="big">{{ num(counts[0] - counts[1]) }}</div>
          <div class="label">without a CI result ({{ pct(counts[0] - counts[1], counts[0]) }})</div>
        </div>
        <div v-if="kind === 'testharness'" class="stat">
          <div class="big">{{ pct(counts[3], counts[4]) }}</div>
          <div class="label">subtests passing: {{ num(counts[3]) }} of {{ num(counts[4]) }}, in the {{ num(counts[1]) }} pages run</div>
        </div>
        <div v-if="page.chrome && counts[6]" class="stat">
          <div class="big">{{ pct(counts[5], counts[0]) }}</div>
          <div class="label">Chrome {{ page.chrome.version.split('.')[0] }}: {{ num(counts[5]) }} of {{ num(counts[0]) }} in WPT; {{ num(counts[7]) }} of the {{ num(counts[1]) }} pyro runs (pyro {{ num(counts[2]) }})</div>
        </div>
      </div>

      <section ref="chartBox" class="history">
        <div class="history-head">
          <h2>Score history</h2>
          <div class="ranges">
            <button v-for="(r, i) in RANGES" :key="r.label" :class="{ on: range === i }" @click="range = i">{{ r.label }}</button>
          </div>
        </div>
        <svg v-if="chart" :viewBox="`0 0 ${W} ${H}`" class="chart" role="img" :aria-label="`Pass rate history, ${kind}`">
          <g class="grid">
            <template v-for="t in chart.ticks" :key="t.label">
              <line :x1="L" :x2="W - R" :y1="t.y" :y2="t.y" />
              <text :x="L - 6" :y="t.y + 4" text-anchor="end">{{ t.label }}</text>
            </template>
            <text v-for="xl in chart.xLabels" :key="xl.label + xl.anchor" :x="xl.x" :y="H - 8" :text-anchor="xl.anchor">{{ xl.label }}</text>
          </g>
          <g v-if="chart.chrome" class="chrome">
            <line :x1="L" :x2="W - R" :y1="chart.chrome.y" :y2="chart.chrome.y" />
            <text :x="W - R" :y="chart.chrome.y - 5" text-anchor="end">{{ chart.chrome.label }}</text>
          </g>
          <path v-if="chart.ds" class="line sub" :d="chart.ds" />
          <path class="line" :d="chart.d" />
          <circle v-for="(p, i) in chart.dots" :key="i" class="dot" :cx="p.cx" :cy="p.cy" r="3"><title>{{ p.title }}</title></circle>
        </svg>
        <p v-else class="muted">No history in this range.</p>
        <p class="legend"><span class="sw pyro" /> pyro, {{ PASSING[kind] }}, of all in WPT <template v-if="chart?.ds"><span class="sw sub" /> subtests passing, in the pages run</template> <template v-if="chart?.chrome"><span class="sw chrome" /> Chrome stable</template></p>
      </section>

      <p class="meta">
        Main CI snapshot at commit <code>{{ page.sha.slice(0, 10) }}</code> ({{ page.date.slice(0, 10) }}) · WPT <code>{{ page.wpt.slice(0, 10) }}</code>
        <template v-if="page.chrome"> · Chrome {{ page.chrome.version }} on wpt.fyi at WPT <code>{{ page.chrome.revision.slice(0, 10) }}</code></template>
      </p>

      <section v-if="dirs.length">
        <h2>Directories</h2>
        <div class="scroll">
          <table>
            <thead>
              <tr>
                <th>Area</th><th class="r">Passed / tests</th><th class="r">Test %</th><th class="r">No CI result</th>
                <th v-if="kind === 'testharness'" class="r">Subtests passed / total</th>
                <th v-if="kind === 'testharness'" class="r">Subtest %</th>
                <th v-if="page.chrome" class="r">Chrome</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="d in dirs" :key="d.name">
                <td><a :href="pageUrl(page.path ? `${page.path}/${d.name}` : d.name) + `#${kind}`">{{ d.name }}</a></td>
                <td class="r">{{ num(d.c[2]) }} / {{ num(d.c[0]) }}</td>
                <td class="r bar-cell"><span class="bar" :style="{ width: d.c[0] ? `${(100 * d.c[2]) / d.c[0]}%` : '0' }" /><span class="v">{{ pct(d.c[2], d.c[0]) }}</span></td>
                <td class="r" :class="{ muted: !(d.c[0] - d.c[1]) }">{{ num(d.c[0] - d.c[1]) }}</td>
                <td v-if="kind === 'testharness'" class="r">{{ num(d.c[3]) }} / {{ num(d.c[4]) }}</td>
                <td v-if="kind === 'testharness'" class="r">{{ pct(d.c[3], d.c[4]) }}</td>
                <td v-if="page.chrome" class="r">{{ d.c[6] ? pct(d.c[5], d.c[0]) : '–' }}</td>
              </tr>
            </tbody>
          </table>
        </div>
      </section>

      <section v-if="kindTests">
        <h2>Tests in this directory</h2>
        <div class="filters">
          <button v-for="f in (['all', 'fail', 'pass', 'notrun'] as const)" :key="f" :class="{ on: filter === f }" @click="filter = f">{{ { all: 'All', fail: 'Failing', pass: 'Passing', notrun: 'No CI result' }[f] }}</button>
          <input v-model="query" type="search" placeholder="Filter by name" aria-label="Filter tests by name" />
        </div>
        <div class="scroll">
          <table class="tests">
            <thead>
              <tr><th>Test</th><th>Result</th><th v-if="kind === 'testharness'" class="r">Subtests</th><th v-if="page.chrome">Chrome</th></tr>
            </thead>
            <tbody>
              <tr v-for="t in shownTests" :key="t.n">
                <td class="name">
                  <a :href="testHref(t.n)" rel="noopener" target="_blank">{{ t.n }}</a>
                  <details v-if="t.f?.length">
                    <summary>{{ t.f.length }} failing subtest{{ t.f.length === 1 ? '' : 's' }}</summary>
                    <ul><li v-for="s in t.f" :key="s">{{ s }}</li></ul>
                  </details>
                </td>
                <td><span class="badge" :class="result(t).cls">{{ result(t).text }}</span></td>
                <td v-if="kind === 'testharness'" class="r">{{ t.s === null ? '' : `${num(t.p!)} / ${num(t.t!)}` }}</td>
                <td v-if="page.chrome"><span v-if="t.c !== undefined" class="badge" :class="t.c ? 'pass' : 'fail'">{{ t.c ? 'pass' : 'fail' }}</span></td>
              </tr>
            </tbody>
          </table>
        </div>
        <p v-if="tests.length > SHOWN" class="muted">Showing {{ SHOWN }} of {{ num(tests.length) }}: filter to see the rest.</p>
        <p v-else-if="!tests.length" class="muted">No tests match.</p>
      </section>
    </template>
  </div>
</template>

<style scoped>
.wpt { --pass: #1a7f37; --fail: #cf222e; --notrun: var(--vp-c-text-3); font-variant-numeric: tabular-nums; }
.dark .wpt { --pass: #3fb950; --fail: #f85149; }
.crumbs { font-size: 1.5rem; font-weight: 600; margin: 0 0 1rem; word-break: break-word; }
.crumbs a { color: var(--vp-c-brand-1); text-decoration: none; }
.crumbs .sep { color: var(--vp-c-text-3); margin: 0 0.35em; }
.round-progress { display: flex; flex-wrap: wrap; gap: .25em 1em; border-left: 3px solid var(--vp-c-brand-1); padding: .45em .85em; }
.round-progress a { font-weight: 600; }
.round-progress span { color: var(--vp-c-text-2); }
.tabs, .ranges, .filters { display: flex; flex-wrap: wrap; gap: 6px; }
.tabs { margin-bottom: 1rem; }
button { border: 1px solid var(--vp-c-divider); border-radius: 6px; padding: 4px 10px; font-size: 0.875rem; color: var(--vp-c-text-2); background: var(--vp-c-bg); }
button:hover { border-color: var(--vp-c-brand-1); }
button.on { background: var(--vp-c-brand-soft); border-color: var(--vp-c-brand-1); color: var(--vp-c-text-1); }
.tabs button { font-size: 0.95rem; padding: 6px 12px; }
.tabs .n { color: var(--vp-c-text-3); margin-left: 4px; }
.stats { display: grid; grid-template-columns: repeat(auto-fit, minmax(150px, 1fr)); gap: 10px; margin-bottom: 1.25rem; }
.stat { border: 1px solid var(--vp-c-divider); border-radius: 8px; padding: 10px 12px; background: var(--vp-c-bg-soft); }
.big { font-size: 1.5rem; font-weight: 600; line-height: 1.3; }
.label { font-size: 0.8rem; color: var(--vp-c-text-2); }
.history-head { display: flex; flex-wrap: wrap; align-items: center; justify-content: space-between; gap: 8px; }
h2 { border-top: none !important; margin: 1.5rem 0 0.75rem !important; padding-top: 0 !important; font-size: 1.15rem !important; }
.history-head h2 { margin: 0 !important; }
.chart { width: 100%; height: auto; margin-top: 10px; display: block; }
.grid line { stroke: var(--vp-c-divider); stroke-width: 1; }
.grid text, .chrome text { fill: var(--vp-c-text-2); font-size: 11px; }
.chrome line { stroke: var(--vp-c-text-3); stroke-dasharray: 5 4; stroke-width: 1.5; }
.line { fill: none; stroke: var(--vp-c-brand-1); stroke-width: 2; stroke-linejoin: round; }
.dot { fill: var(--vp-c-bg); stroke: var(--vp-c-brand-1); stroke-width: 2; }
.legend { font-size: 0.8rem; color: var(--vp-c-text-2); margin: 4px 0 0; display: flex; align-items: center; gap: 6px; flex-wrap: wrap; }
.sw { display: inline-block; width: 18px; height: 0; border-top: 2px solid var(--vp-c-brand-1); }
.sw.sub { border-top: 2px dotted var(--vp-c-brand-3); margin-left: 10px; }
.line.sub { stroke: var(--vp-c-brand-3); stroke-dasharray: 2 3; stroke-width: 2; }
.sw.chrome { border-top: 2px dashed var(--vp-c-text-3); margin-left: 10px; }
.meta { font-size: 0.85rem; color: var(--vp-c-text-2); }
.scroll { overflow-x: auto; max-width: 100%; }
table { display: table; width: 100%; border-collapse: collapse; margin: 0; font-size: 0.875rem; }
th, td { border: none; border-bottom: 1px solid var(--vp-c-divider); padding: 6px 10px; white-space: nowrap; }
tr { background: transparent !important; border-top: none; }
th { font-weight: 600; color: var(--vp-c-text-2); text-align: left; }
.r { text-align: right; }
.bar-cell { position: relative; min-width: 90px; }
.bar { position: absolute; left: 0; top: 25%; height: 50%; background: var(--vp-c-brand-soft); border-radius: 2px; }
.bar-cell .v { position: relative; }
.muted { color: var(--vp-c-text-3); }
.filters { margin-bottom: 8px; align-items: center; }
.filters input { border: 1px solid var(--vp-c-divider); border-radius: 6px; padding: 4px 10px; font-size: 0.875rem; flex: 1 1 160px; min-width: 0; background: var(--vp-c-bg); }
.tests .name { white-space: normal; word-break: break-all; min-width: 220px; }
.tests details { font-size: 0.8rem; color: var(--vp-c-text-2); }
.tests details ul { margin: 4px 0; padding-left: 18px; max-height: 240px; overflow: auto; }
.tests details li { word-break: break-word; }
.badge { display: inline-block; font-size: 0.75rem; font-weight: 600; padding: 1px 8px; border-radius: 10px; border: 1px solid currentColor; }
.badge.pass { color: var(--pass); }
.badge.fail { color: var(--fail); }
.badge.notrun { color: var(--notrun); }
.error { color: var(--fail); }
</style>
