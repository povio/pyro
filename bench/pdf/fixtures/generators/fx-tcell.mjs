// Column widths of auto-layout tables with percentage, fixed and auto cells, some empty. The text of each cell is its marker.
const doc = (b, st = '') => `<!doctype html><body style="margin:0;font:12px/16px Arial;${st}">${b}`;
const TABLES = { w100: 'width:100%', w400: 'width:400px', auto: '' };
const COLLAPSE = { sep: 'border-collapse:separate;border-spacing:0', coll: 'border-collapse:collapse', gap: 'border-collapse:separate;border-spacing:6px' };
const td = (w, t, st = '') => `<td style="padding:0;${w ? `width:${w};` : ''}${st}">${t}</td>`;
const S = {
  // Empty and whitespace cells.
  emptyPct: td('50%', '') + td('', 'a1'),
  wsPct: td('50%', ' \n ') + td('', 'a1'),
  nbspPct: td('50%', '&nbsp;') + td('', 'a1'),
  emptyFixed: td('100px', '') + td('', 'a1'),
  wsFixed: td('100px', ' ') + td('', 'a1'),
  emptyAuto: td('', '') + td('', 'a1'),
  emptyBoth: td('50%', '') + td('50%', ''),
  emptyPct30: td('30%', '') + td('', 'a1') + td('', 'b1'),
  emptyLast: td('', 'a1') + td('50%', ''),
  // With text.
  pct50: td('50%', 'a1') + td('', 'b1'),
  pct100: td('', 'a1', 'white-space:nowrap') + td('100%', 'b1'),
  pct100wide: td('', 'a1 long label text', 'white-space:nowrap') + td('100%', 'b1 words words words'),
  pctSum: td('30%', 'a1') + td('20%', 'b1') + td('', 'c1'),
  pctOver: td('70%', 'a1') + td('70%', 'b1') + td('', 'c1'),
  pctAll: td('25%', 'a1') + td('25%', 'b1'),
  pctFixed: td('50%', 'a1') + td('100px', 'b1') + td('', 'c1'),
  pctFixedEmpty: td('50%', '') + td('100px', 'b1') + td('', 'c1'),
  pctBig: td('10%', 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa1') + td('', 'b1'),
  pctPad: td('50%', 'a1', 'padding:0 10px') + td('', 'b1', 'padding:0 5px'),
  pctBorder: td('50%', 'a1', 'border:3px solid') + td('', 'b1', 'border:1px solid'),
  pctBox: td('50%', 'a1', 'padding:0 10px;box-sizing:border-box') + td('', 'b1'),
  fixedAuto: td('100px', 'a1') + td('', 'b1'),
  fixedBig: td('20px', 'aaaaaaaaaaaaaaaaaaaaaaa1') + td('', 'b1'),
  autoAuto: td('', 'a1') + td('', 'b1 long'),
};
const row = (cells) => `<tr>${cells}</tr>`;
export const cases = [];
for (const [tn, t] of Object.entries(TABLES)) for (const [cn, c] of Object.entries(COLLAPSE)) for (const [sn, cells] of Object.entries(S)) {
  cases.push({ name: `${sn}|${tn}|${cn}`, html: doc(`<table style="${t};${c}">${row(cells)}</table><div>end1</div>`) });
}
const M = (name, html) => cases.push({ name: `more|${name}`, html: doc(html) });
const sep = 'border-collapse:collapse;';
// Percent in a later row, on the larger of two rows, and through `<col>`.
M('row2', `<table style="width:100%;${sep}"><tr>${td('', 'a1')}${td('', 'b1')}</tr><tr>${td('', '')}${td('50%', '')}</tr></table>`);
M('rowsMax', `<table style="width:100%;${sep}"><tr>${td('30%', 'a1')}${td('', 'b1')}</tr><tr>${td('60%', '')}${td('', '')}</tr></table>`);
M('col', `<table style="width:100%;${sep}"><col style="width:50%"><col><tr>${td('', '')}${td('', 'b1')}</tr></table>`);
M('colpx', `<table style="width:100%;${sep}"><col style="width:100px"><col><tr>${td('', '')}${td('', 'b1')}</tr></table>`);
M('span', `<table style="width:100%;${sep}"><tr><td colspan=2 style="padding:0">a1 long text</td>${td('', 'c1')}</tr><tr>${td('50%', '')}${td('', 'b1')}${td('', 'd1')}</tr></table>`);
// The ToS header: a flex item, nowrap labels and a full-width value.
M('flex', `<div style="display:flex;width:600px"><div style="flex:1">left1</div><table style="flex:1;align-self:flex-start;${sep}"><tr>${td('', 'Datum1', 'white-space:nowrap;padding-right:22px')}${td('100%', 'value1')}</tr></table></div><div>end1</div>`);
M('nested', `<table style="width:100%;${sep}"><tr>${td('', `<table style="${sep}"><tr>${td('', 'in1')}${td('100%', 'in2')}</tr></table>`)}${td('', 'b1')}</tr></table>`);
M('nestedPct', `<table style="width:100%;${sep}"><tr>${td('50%', `<table style="width:100%;${sep}"><tr>${td('50%', '')}${td('', 'in2')}</tr></table>`)}${td('', 'b1')}</tr></table>`);
