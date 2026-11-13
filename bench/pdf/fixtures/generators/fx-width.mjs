// What a paper-cut print lays out in: the same markers at 50% and 99.9% of each reference box, over papers, margins and scales.
const PAPERS = { A4: [8.27, 11.69], A4x: [8.2677165, 11.6929134], Letter: [8.5, 11], Legal: [8.5, 14], Tiny: [4.01, 5.03] };
const M = [0, 0.4, 0.39, 0.5, 1 / 2.54, 0.123];
const S = [1, 0.8, 1.5, 0.75];
const marks = `<div style="margin-left:50%">pct1</div><div style="text-align:right">rgt1</div><div style="margin-left:50vw">vw1</div>`
  + `<div style="width:50%;text-align:right">half1</div>`
  + `<div style="position:absolute;left:50%;top:200px">abs1</div><div style="position:fixed;left:50%;top:220px">fix1</div>`
  + `<div style="position:absolute;left:0;top:50%">abt1</div><div style="position:absolute;left:0;top:50vh">vht1</div>`
  + `<div style="display:flex"><div style="flex:1"></div><div style="flex:1">flx1</div></div>`
  + `<table style="width:100%;border-spacing:0"><tr><td style="padding:0;width:50%"></td><td style="padding:0">tbl1</td></tr></table>`;
const doc = (extra = '') => `<!doctype html><html style="margin:0"><body style="margin:0;font:10px/12px Arial">${marks}${extra}`;
export const cases = [];
for (const [pn, [w, h]] of Object.entries(PAPERS)) for (const m of M) for (const s of S) {
  const opts = { paperWidth: w, paperHeight: h, marginTop: m, marginBottom: m, marginLeft: m, marginRight: m, scale: s };
  cases.push({ name: `plain|${pn}|${m.toFixed(4)}|${s}`, html: doc(), opts });
}
// Unequal margins and overflow that shrinks.
for (const [l, r] of [[0.4, 0.1], [0.1, 0.4], [0.39, 0.41], [0.123, 0.5]]) for (const s of [1, 0.8, 1.5]) {
  cases.push({ name: `lr|A4|${l}/${r}|${s}`, html: doc(), opts: { paperWidth: 8.27, paperHeight: 11.69, marginTop: 0, marginBottom: 0, marginLeft: l, marginRight: r, scale: s } });
}
for (const [pn, [w, h]] of Object.entries(PAPERS)) for (const m of [0, 0.4]) for (const s of [1, 1.5]) for (const k of [1.1, 1.3, 3]) {
  const area = (Math.ceil(w * 72) - 2 * Math.floor(m * 72)) * 4 / 3 / s;
  cases.push({ name: `shrink|${pn}|${m}|${s}|${k}`, html: doc(`<div style="width:${Math.round(area * k)}px;height:1px"></div>`), opts: { paperWidth: w, paperHeight: h, marginTop: m, marginBottom: m, marginLeft: m, marginRight: m, scale: s } });
}
// Percentage heights and viewport units against what a page holds.
const line = '<div>end1</div>';
for (const [pn, [w, h]] of Object.entries(PAPERS)) for (const m of [0, 0.4, 0.5]) for (const s of [1, 0.8, 1.5]) {
  const o = { paperWidth: w, paperHeight: h, marginTop: m, marginBottom: m, marginLeft: m, marginRight: m, scale: s };
  const base = 'margin:0;font:10px/12px Arial';
  const n = `${pn}|${m}|${s}`;
  cases.push({ name: `h100|${n}`, opts: o, html: `<!doctype html><html style="height:100%"><body style="${base};height:100%">a1<div style="position:absolute;top:0;left:0;width:10px;height:100%;background:#000"><a href="https://x.test/h" style="display:block;height:100%"></a></div>${line}` });
  cases.push({ name: `box100|${n}`, opts: o, html: `<!doctype html><html style="height:100%"><body style="${base};height:100%"><div style="height:100%"><a href="https://x.test/b" style="display:block;height:100%"></a></div>${line}` });
  cases.push({ name: `vh100|${n}`, opts: o, html: `<!doctype html><body style="${base}"><div style="height:100vh"><a href="https://x.test/v" style="display:block;height:100%"></a></div>${line}` });
  cases.push({ name: `vw100|${n}`, opts: o, html: `<!doctype html><body style="${base}"><a href="https://x.test/w" style="display:block;width:100vw;height:10px"></a><a href="https://x.test/p" style="display:block;width:100%;height:10px"></a>${line}` });
}
// Link rects for pyro-render's tests: a block at 50% of the content, and an absolute box at 50% of the initial containing block.
for (const [pn, [w, h], m, s] of [['A4', [8.27, 11.69], 0, 1], ['A4', [8.27, 11.69], 0.4, 1], ['Letter', [8.5, 11], 0.4, 1], ['Letter', [8.5, 11], 0.4, 0.8], ['A4', [8.27, 11.69], 0, 1.5]]) {
  const opts = { paperWidth: w, paperHeight: h, marginTop: m, marginBottom: m, marginLeft: m, marginRight: m, scale: s };
  const link = (st) => `<!doctype html><body style="margin:0"><a href="https://x.test/l" style="display:block;height:10px;${st}"></a>`;
  cases.push({ name: `link-block|${pn}|${m}|${s}`, html: link('margin-left:50%'), opts });
  cases.push({ name: `link-abs|${pn}|${m}|${s}`, html: link('position:absolute;left:50%;width:10px'), opts });
  cases.push({ name: `link-vw|${pn}|${m}|${s}`, html: link('margin-left:50vw'), opts });
}
