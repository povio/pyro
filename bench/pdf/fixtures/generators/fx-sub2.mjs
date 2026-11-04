// whole-px analogues: shows which remaining differences are not about fractions
const L=(p,n,from=1)=>Array.from({length:n},(_,i)=>p+(i+from)).join('<br>');
const doc=(b)=>`<!doctype html><body style="margin:0;font:16px/20px Arial;">${b}`;
const sp=h=>`<div style="height:${h}px"></div>`;const d=(b,st='')=>`<div style="${st}">${b}</div>`;
const f=(c,st)=>`<div style="float:left;width:80px;${st}">${c}</div>`;const h=384;
const C={};
for(const off of [1,5,10,19]){
 C[`w|tall+${off}`]=doc(f(L('f',21),`margin-top:${h-40+off}px`)+d(''));
 C[`w|beside+${off}`]=doc(sp(h-20+off)+f('f1','height:15px')+d(L('a',3),'orphans:1;widows:1'));
 C[`w|lp+${off}`]=doc(d('a1',`break-after:page;height:${off}px`)+d(L('c',22),'orphans:1;widows:1'));
 C[`w|ml+${off}`]=doc(d('a1',`margin-bottom:${h-20+off}px`)+d(''));
 C[`w|h${20-off}`]=doc(Array.from({length:60},(_,i)=>d('a'+(i+1),`height:${20-off}px`)).join(''));
}
// a fraction past a px of line overflow: lines over shorter boxes, parallel columns
for(const hh of [19.7,19.3,19,18.7,18.5,18,15.5]) C[`o|h${hh}`]=doc(Array.from({length:50},(_,i)=>d('a'+(i+1),`height:${hh}px`)).join(''));
for(const off of [0,5,5.5,10.25,15.75]) C[`col|${off}`]=doc(`<div style="display:flex"><div style="width:100px;">${L('a',30)}</div><div style="width:100px;padding-top:${off}px">${L('b',30)}</div></div>`);
// boxes with background at fractional cuts; a table row
for(const off of [.25,.5,.75]) C[`tb|${off}`]=doc(sp(off)+`<table style="border-collapse:collapse">`+Array.from({length:30},(_,i)=>`<tr><td style="padding:1.3px">a${i+1}</td><td>b${i+1}</td></tr>`).join('')+`</table>`);
for(const off of [.25,.5,.75]) C[`ib|${off}`]=doc(sp(off)+Array.from({length:30},(_,i)=>`<div><span style="display:inline-block;height:${20+off}px;vertical-align:top">a${i+1}</span></div>`).join(''));
export const cases=Object.entries(C).map(([name,html])=>({name,html}));
