// where a line / box bottom can go past the page end
const doc=(b)=>`<!doctype html><body style="margin:0;font:16px/20px Arial">${b}`;
const PAP={L0:{paperWidth:8.5,paperHeight:11,h:1056},A0:{paperWidth:8.27,paperHeight:11.69,h:842*4/3},
 A4m:{paperWidth:8.27,paperHeight:11.69,marginTop:.4,marginBottom:.4,h:(842-56)*4/3},A5m:{paperWidth:8.27,paperHeight:11.69,marginTop:.5,marginBottom:.5,h:(842-72)*4/3},
 L4m:{paperWidth:8.5,paperHeight:11,marginTop:.4,marginBottom:.4,h:(792-56)*4/3},Am:{paperWidth:8.27,paperHeight:11.69,marginTop:.41,marginBottom:.41,h:(842-58)*4/3},
 A08:{paperWidth:8.27,paperHeight:11.69,scale:.8,h:842*4/3/.8},L15:{paperWidth:8.5,paperHeight:11,scale:1.5,h:1056/1.5},A15:{paperWidth:8.27,paperHeight:11.69,scale:1.5,h:842*4/3/1.5}};
export const cases=[];
for(const [pn,{h,...opts}] of Object.entries(PAP)) for(const d of [-1,-.5,-.25,-1/64,0,1/64,1/32,1/16,1/8,.2,.25,.33,.5,.75,.99,1,1.25,1.5,2]) {
 const t=h-20+d;
 cases.push({name:`${pn}|line|${d}`,opts,html:doc(`<div style="height:${t}px"></div><div>x1</div><div>x2</div>`)});
 cases.push({name:`${pn}|avoid|${d}`,opts,html:doc(`<div style="height:${t}px"></div><div style="height:20px;break-inside:avoid">x1</div><div>x2</div>`)});
}
