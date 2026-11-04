const doc=(b)=>`<!doctype html><body style="margin:0;font:16px/20px Arial;">${b}`;
const d=(b,st='')=>`<div style="${st}">${b}</div>`;
const C={};
for(const [mt,pb] of [[20,10],[22,11],[5,2],[18,9],[12,6],[30,15]]) for(const top of [0,31,75,40]) {
 C[`w|${mt}/${pb}/${top}`]=doc(d(Array.from({length:34},(_,i)=>d(`a${i+1}`,`margin-top:${mt}px;padding-bottom:${pb}px`)).join(''),`padding-top:${top}px`));
 C[`m|${mt}/${pb}/${top}`]=doc(d(Array.from({length:34},(_,i)=>d(`a${i+1}`,`margin-top:${mt}px;margin-bottom:${pb}px`)).join(''),`padding-top:${top}px`));
}
export const cases=Object.entries(C).map(([name,html])=>({name,html}));
