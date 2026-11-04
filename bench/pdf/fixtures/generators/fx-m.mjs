// what is unbreakable: a tall box at y=300 of a 384 px page. moved = 'after1' at (300+h-384 vs h).
const L=(p,n,from=1)=>Array.from({length:n},(_,i)=>p+(i+from)).join('<br>');
const doc=(b,st='')=>`<!doctype html><body style="margin:0;font:16px/20px Arial;${st}">${b}`;
const sp=h=>`<div style="height:${h}px"></div>`;
const PNG='data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==';
const kinds={
 block:(h,b)=>`<div style="height:${h}px">${b}</div>`,
 ib:(h,b)=>`<div style="display:inline-block;width:200px;height:${h}px;vertical-align:top">${b}</div>`,
 ibauto:(h,b)=>`<div style="display:inline-block;width:200px;vertical-align:top">${b}</div>`,
 iflex:(h,b)=>`<div style="display:inline-flex;width:200px;height:${h}px;vertical-align:top"><div>${b}</div></div>`,
 igrid:(h,b)=>`<div style="display:inline-grid;width:200px;height:${h}px;vertical-align:top"><div>${b}</div></div>`,
 itbl:(h,b)=>`<table style="display:inline-table;width:200px;height:${h}px;vertical-align:top;border-spacing:0"><tr><td style="padding:0;vertical-align:top">${b}</td></tr></table>`,
 csize:(h,b)=>`<div style="contain:size;height:${h}px">${b}</div>`,
 cstrict:(h,b)=>`<div style="contain:strict;height:${h}px">${b}</div>`,
 cpaint:(h,b)=>`<div style="contain:paint;height:${h}px">${b}</div>`,
 clayout:(h,b)=>`<div style="contain:layout;height:${h}px">${b}</div>`,
 ccontent:(h,b)=>`<div style="contain:content;height:${h}px">${b}</div>`,
 cinline:(h,b)=>`<div style="contain:inline-size;height:${h}px">${b}</div>`,
 ctype:(h,b)=>`<div style="container-type:size;height:${h}px">${b}</div>`,
 ohidden:(h,b)=>`<div style="overflow:hidden;height:${h}px">${b}</div>`,
 oauto:(h,b)=>`<div style="overflow:auto;height:${h}px">${b}</div>`,
 oscroll:(h,b)=>`<div style="overflow:scroll;height:${h}px">${b}</div>`,
 oclip:(h,b)=>`<div style="overflow:clip;height:${h}px">${b}</div>`,
 lclamp:(h,b)=>`<div style="display:-webkit-box;-webkit-box-orient:vertical;-webkit-line-clamp:100;overflow:hidden">${b}</div>`,
 lclamp2:(h,b)=>`<div style="line-clamp:100;height:${h}px">${b}</div>`,
 canvas:(h,b)=>`<canvas width=100 height=${h} style="display:block"></canvas>`,
 canvasi:(h,b)=>`<canvas width=100 height=${h}></canvas>`,
 img:(h,b)=>`<img src="${PNG}" style="display:block;width:100px;height:${h}px">`,
 svg:(h,b)=>`<svg width=100 height=${h} style="display:block"></svg>`,
 video:(h,b)=>`<video style="display:block;width:100px;height:${h}px"></video>`,
 iframe:(h,b)=>`<iframe style="display:block;width:100px;height:${h}px;border:0"></iframe>`,
 object:(h,b)=>`<object style="display:block;width:100px;height:${h}px"></object>`,
 embed:(h,b)=>`<embed style="display:block;width:100px;height:${h}px">`,
 textarea:(h,b)=>`<textarea style="display:block;width:100px;height:${h}px;margin:0"></textarea>`,
 select:(h,b)=>`<select size=5 style="display:block;width:100px;height:${h}px;margin:0"></select>`,
 button:(h,b)=>`<button style="display:block;width:100px;height:${h}px;margin:0">x</button>`,
 input:(h,b)=>`<input style="display:block;width:100px;height:${h}px;margin:0">`,
 meter:(h,b)=>`<meter style="display:block;width:100px;height:${h}px"></meter>`,
 progress:(h,b)=>`<progress style="display:block;width:100px;height:${h}px"></progress>`,
 marquee:(h,b)=>`<marquee style="display:block;height:${h}px">${b}</marquee>`,
 vert:(h,b)=>`<div style="writing-mode:vertical-rl;height:${h}px;width:200px">${b}</div>`,
 fieldset:(h,b)=>`<fieldset style="margin:0;padding:0;border:0;height:${h}px">${b}</fieldset>`,
 details:(h,b)=>`<details open style="height:${h}px">${b}</details>`,
 flexb:(h,b)=>`<div style="display:flex;height:${h}px"><div>${b}</div></div>`,
 flexcol:(h,b)=>`<div style="display:flex;flex-direction:column;height:${h}px"><div>${b}</div></div>`,
 grid:(h,b)=>`<div style="display:grid;height:${h}px"><div>${b}</div></div>`,
 table:(h,b)=>`<table style="height:${h}px;border-spacing:0"><tr><td style="padding:0;vertical-align:top">${b}</td></tr></table>`,
 trow:(h,b)=>`<table style="border-spacing:0"><tr style="height:${h}px"><td style="padding:0;vertical-align:top">${b}</td></tr></table>`,
 col:(h,b)=>`<div style="columns:2;height:${h}px">${b}</div>`,
 flt:(h,b)=>`<div style="float:left;width:200px;height:${h}px">${b}</div>`,
 abs:(h,b)=>`<div style="position:absolute;width:200px;height:${h}px">${b}</div>`,
 fix:(h,b)=>`<div style="position:fixed;top:300px;width:200px;height:${h}px">${b}</div>`,
 rel:(h,b)=>`<div style="position:relative;height:${h}px">${b}</div>`,
 tf:(h,b)=>`<div style="transform:translateX(1px);height:${h}px">${b}</div>`,
 op:(h,b)=>`<div style="opacity:.9;height:${h}px">${b}</div>`,
 wc:(h,b)=>`<div style="will-change:transform;height:${h}px">${b}</div>`,
 cv:(h,b)=>`<div style="content-visibility:auto;contain-intrinsic-size:auto ${h}px">${b}</div>`,
 cvh:(h,b)=>`<div style="content-visibility:hidden;height:${h}px">${b}</div>`,
 math:(h,b)=>`<math style="display:block;height:${h}px"><mi>x</mi></math>`,
 inl:(h,b)=>`<span style="display:inline-block;height:${h}px;width:50px"></span>`,
 ruby:(h,b)=>`<ruby style="display:block;height:${h}px">x<rt>y</rt></ruby>`,
};
export const cases=[];
for(const [k,f] of Object.entries(kinds)) {
  const body=L('a',10);
  cases.push({name:`${k}|200`,html:doc(sp(300)+f(200,body)+'<div>after1</div>')});
  cases.push({name:`${k}|500`,html:doc(sp(300)+f(500,body)+'<div>after1</div>')});
  cases.push({name:`${k}|top500`,html:doc(f(500,body)+'<div>after1</div>')});
  cases.push({name:`${k}|lines`,html:doc(sp(300)+f(600,L('a',30))+'<div>after1</div>')});
}
