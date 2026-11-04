// Tall unbreakable boxes under a small margin, border or padding at the top of a page.
const PNG='data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==';
const doc=(b,st='margin:0')=>`<!doctype html><body style="${st};font:16px/20px Arial">${b}`;
const KIND={
 avoid:(s)=>`<div style="height:500px;break-inside:avoid;${s}">k1</div>`,
 fit:(s)=>`<div style="height:300px;break-inside:avoid;${s}">k1</div>`,
 img:(s)=>`<div style="${s}">k1</div><img src="${PNG}" style="display:block;width:50px;height:500px">`,
 imgm:(s)=>`<img src="${PNG}" style="display:block;width:50px;height:500px;${s}">`,
 vert:(s)=>`<div style="writing-mode:vertical-rl;height:500px;width:100px;${s}">k1</div>`,
 vertn:(s)=>`<div style="writing-mode:vertical-rl;height:300px;width:100px;${s}">k1</div>`,
 ctn:(s)=>`<div style="contain:size;height:500px;width:100px;${s}">k1</div>`,
 lines:(s)=>`<div style="${s}">${Array.from({length:30},(_,i)=>'k'+(i+1)).join('<br>')}</div>`,
};
export const cases=[];
for(const [k,f] of Object.entries(KIND)) for(const m of [0,.1,.3,.5,.8,1,5,20]) {
 cases.push({name:`mt|${k}|${m}`,html:doc(f(`margin-top:${m}px`)+'<div>z1</div>')});
 cases.push({name:`bt|${k}|${m}`,html:doc(f(`border-top:${m}px solid`)+'<div>z1</div>')});
 cases.push({name:`pt|${k}|${m}`,html:doc(f(`padding-top:${m}px`)+'<div>z1</div>')});
 cases.push({name:`par|${k}|${m}`,html:doc(`<div style="padding-top:${m}px">${f('')}</div><div>z1</div>`)});
 cases.push({name:`parm|${k}|${m}`,html:doc(`<div style="margin-top:${m}px">${f('')}</div><div>z1</div>`)});
 cases.push({name:`body|${k}|${m}`,html:doc(f('')+'<div>z1</div>',`margin:${m}px 0 0`)});
 // at the top of a later page, after a forced break and after a cut
 cases.push({name:`fb|${k}|${m}`,html:doc(`<div>y1</div><div style="break-before:page;height:0"></div>`+f(`margin-top:${m}px`)+'<div>z1</div>')});
 cases.push({name:`cut|${k}|${m}`,html:doc(`<div style="height:380px">y1</div>`+f(`margin-top:${m+4}px`)+'<div>z1</div>')});
}
