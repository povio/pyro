const L=(p,n,from=1)=>Array.from({length:n},(_,i)=>p+(i+from)).join('<br>');
const C={};const head=(b,st='')=>`<!doctype html><html style="${st}"><body style="margin:0;font:16px/20.5px Arial">${b}`;
for(const off of [0,.3,.5,.8]){
 const blk=`<div style="writing-mode:vertical-rl;height:300px;width:${100.5+off}px">${L('v',6)}</div>`;
 const blk2=`<div style="writing-mode:vertical-rl;height:600.4px;width:200.3px;margin-top:${off}px"><div style="margin-inline-start:${off+3.3}px;margin-block-start:7.7px;padding:1.1px">${L('v',8)}</div><div style="break-inside:avoid;inline-size:50.5px;block-size:30.25px">w1</div></div>`;
 C[`in|${off}`]=head(`<div style="height:${200+off}px"></div>`+blk+L('a',30));
 C[`in2|${off}`]=head(`<div style="height:${200+off}px"></div>`+blk2+L('a',30));
 C[`root|${off}`]=head(`<div style="height:${200+off}px;width:${300.5+off}px">${L('a',4)}</div><div style="width:${500+off}px">${L('b',4)}</div>`,'writing-mode:vertical-rl');
 C[`lr|${off}`]=head(`<div style="width:${300.5+off}px;height:${900.25+off}px">${L('a',60)}</div>`,'writing-mode:vertical-lr');
 C[`back|${off}`]=head(blk2+`<div style="writing-mode:horizontal-tb;margin:${off}px">${L('h',40)}</div>`);
 C[`nesth|${off}`]=head(`<div style="writing-mode:vertical-rl;height:900.7px"><div style="writing-mode:horizontal-tb;width:300.5px;margin-block-start:${off}px;">${L('h',60)}</div></div>`);
}
export const cases=Object.entries(C).map(([name,html])=>({name,html}));
