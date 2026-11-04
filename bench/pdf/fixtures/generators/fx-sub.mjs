// Fractional pagination groups. name = group|paper|case
const PAP={p6:{h:384,o:{}},A4m:{h:(842-56)*4/3,o:{paperWidth:8.27,paperHeight:11.69,marginTop:.4,marginBottom:.4}},
 A08:{h:Math.ceil(842*4/3/.8),o:{paperWidth:8.27,paperHeight:11.69,scale:.8}},L15:{h:704,o:{paperWidth:8.5,paperHeight:11,scale:1.5}}};
const L=(p,n,from=1)=>Array.from({length:n},(_,i)=>p+(i+from)).join('<br>');
const doc=(b,font='16px/20px Arial',st='')=>`<!doctype html><body style="margin:0;font:${font};${st}">${b}`;
const sp=h=>`<div style="height:${h}px"></div>`;
const d=(b,st='')=>`<div style="${st}">${b}</div>`;
export const cases=[];const add=(g,p,n,html)=>cases.push({name:`${g}|${p}|${n}`,opts:PAP[p].o,html});
for(const [p,{h}] of Object.entries(PAP)){
 const pages=2.6;
 // lh: fractional line-heights, one IFC with <br> and one block per line
 for(const lh of [21.92,18.4,19.2,20.25,20.5,20.75,20.33,17.6,1.37*16,22.4,19.99,20.01,20.015625,30.7]){
  const n=Math.ceil(h*pages/lh);
  add('lh',p,`br${lh.toFixed(3)}`,doc(d(L('a',n),'orphans:1;widows:1'),`16px/${lh}px Arial`));
  add('lhw',p,`br${lh.toFixed(3)}`,doc(d(L('a',n)),`16px/${lh}px Arial`));
  add('lhb',p,`bl${lh.toFixed(3)}`,doc(Array.from({length:n},(_,i)=>d('a'+(i+1))).join(''),`16px/${lh}px Arial`));
  for(const off of [.25,.5,.75,3.3]) add('lho',p,`${lh.toFixed(3)}+${off}`,doc(sp(off)+d(L('a',n),'orphans:1;widows:1'),`16px/${lh}px Arial`));
 }
 for(const fs of ['14.3px/1.53','13.7px/normal','16px/normal','11px/1.2','12.5px/1.6']) { const n=Math.ceil(h*pages/14);
  add('lhf',p,fs,doc(d(L('a',n)),`${fs} Arial`));add('lhf',p,fs+'+.4',doc(sp(.4)+d(L('a',n)),`${fs} Arial`)); }
 // acc: many blocks whose fractional heights accumulate
 for(const fh of [20.1,20.2,20.3,20.4,20.6,19.7,20.05,20.015625,20.125]){ const n=Math.ceil(h*pages/fh);
  add('acc',p,`h${fh}`,doc(Array.from({length:n},(_,i)=>d('a'+(i+1),`height:${fh}px`)).join('')));
  add('acca',p,`h${fh}`,doc(Array.from({length:n},(_,i)=>d('a'+(i+1),`height:${fh}px;break-inside:avoid`)).join('')));
 }
 // nest: nested fractional margins and padding
 for(const e of [1.3,1.37,.7,.33,1.1]){ const n=Math.ceil(h*pages/30);
  const para=(i)=>`<p style="margin:${e}em 0;padding:${(e/3).toFixed(3)}em 0">a${i}<br>b${i}</p>`;
  add('nest',p,`p${e}`,doc(Array.from({length:n},(_,i)=>para(i+1)).join('')));
  add('nest',p,`in${e}`,doc(d(d(Array.from({length:n},(_,i)=>d(`a${i+1}`,`margin-top:${e}em;padding-bottom:${e/2}em`)).join(''),`padding:${e}em;margin:${e}em`),`margin-top:${e*1.5}em;border-top:${e}px solid`)));
  add('nest',p,`h${e}`,doc(Array.from({length:n},(_,i)=>`<h3 style="margin:${e}em 0;font-size:${(1+e/5).toFixed(3)}em">a${i+1}</h3><p style="margin:${e}em 0">c${i+1}</p>`).join('')));
 }
 // later: breaks on later pages: fractional content pushed past pages 2, 3
 for(const off of [.1,.25,.4,.5,.6,.75,.9,1.3,5.5]){
  const n=Math.ceil(h*3.5/20);
  add('later',p,`br+${off}`,doc(sp(off)+d(L('a',n),'orphans:1;widows:1')));
  add('later',p,`pp+${off}`,doc(sp(h*2+off)+d(L('a',n),'orphans:1;widows:1')));
  // forced break, then a fractional start at a later page
  add('later',p,`fb+${off}`,doc(d('z1')+`<div style="break-before:page;height:${off}px"></div>`+d(L('a',n),'orphans:1;widows:1')));
  // a page starts at a fractional y: a tall fractional avoid box pushes the next
  add('later',p,`av+${off}`,doc(sp(h-10)+d('q1',`height:${50+off}px;break-inside:avoid`)+d(L('a',n),'orphans:1;widows:1')));
  add('later',p,`la+${off}`,doc(sp(h-30+off)+d(L('a',n),'orphans:1;widows:1')));
 }
 // over: blocks overflowing a page by under 0.5 px
 for(const e of [-1/64,0,1/64,1/32,.1,.25,.4,.49,.5,.51,.75,1]){
  const H=(h+e);
  add('over',p,`one${e}`,doc(d('a1',`height:${H}px`)));
  add('over',p,`bg${e}`,doc(d('a1',`height:${H}px;background:#ccc`)));
  add('over',p,`av${e}`,doc(d('a1',`height:${H}px;break-inside:avoid`)+d('b1')));
  add('over',p,`avl${e}`,doc(d('a1',`height:${H}px;break-inside:avoid`)));
  add('over',p,`two${e}`,doc(d('a1',`height:${H*2-e}px`)));
  add('over',p,`sp${e}`,doc(d('a1','height:100px')+d('b1',`height:${H-100}px;break-inside:avoid`)+d('c1')));
  add('over',p,`spt${e}`,doc(d('a1','height:100px')+d('b1',`height:${H-100}px;break-inside:avoid`)));
  add('over',p,`ln${e}`,doc(sp(H-20)+d('a1')));
  add('over',p,`ml${e}`,doc(d('a1',`margin-bottom:${H-20}px`)+d('')));
  add('over',p,`mb${e}`,doc(d('a1',`height:${H-10}px;margin-bottom:10px`)));
  add('over',p,`mbb${e}`,doc(d('a1',`height:${h-10}px;margin-bottom:${10+e}px`)+d('b1')));
  add('over',p,`pb${e}`,`<!doctype html><body style="margin:0;font:16px/20px Arial;padding-bottom:${10+e}px">`+d('a1',`height:${h-10}px`));
 }
 // bb: forced breaks at a fractional y
 for(const off of [.1,.25,.5,.75,.9,10.3]){
  add('bb',p,`before+${off}`,doc(sp(off)+d('a1','break-before:page')+d('b1')));
  add('bb',p,`after+${off}`,doc(d('a1',`height:${20+off}px;break-after:page`)+d('b1')));
  add('bb',p,`mid+${off}`,doc(d('a1',`height:${40+off}px`)+d('b1','break-before:page;margin-top:'+off+'px')+d('c1')));
  add('bb',p,`mm+${off}`,doc(d('a1',`margin-bottom:${off}px`)+d('b1',`break-before:page;margin-top:${off*2}px`)+d(L('c',Math.ceil(h/20)+2),'orphans:1;widows:1')));
  add('bb',p,`edge+${off}`,doc(sp(h-off)+d('b1','break-before:page')+d('c1')));
  add('bb',p,`past+${off}`,doc(sp(h+off)+d('b1','break-before:page')+d('c1')));
  add('bb',p,`lp+${off}`,doc(d('a1',`break-after:page;height:${off}px`)+d(L('c',Math.ceil(h/20)+3),'orphans:1;widows:1')));
 }
 // float: fractional positions and sizes
 for(const off of [0,.25,.5,.75,-.25,-.5,-.75]){
  const f=(c,st)=>`<div style="float:left;width:80px;${st}">${c}</div>`;
  add('float',p,`av+${off}`,doc(f('f1',`height:${h-100+off}px`)+f('g1','height:120px;break-inside:avoid')+d('')));
  add('float',p,`ln+${off}`,doc(sp(h-20+off)+f('f1','')+d('')+d('b1','clear:both')));
  add('float',p,`beside+${off}`,doc(sp(h-20+off)+f('f1','height:15px')+d(L('a',3),'orphans:1;widows:1')));
  add('float',p,`wrap+${off}`,doc(sp(off)+f('f1',`height:${h*.6}px`)+d(L('a',Math.ceil(h*1.6/20)),'orphans:1;widows:1')));
  add('float',p,`ifrac+${off}`,doc(f('f1',`height:${50.5+off}px`)+d(L('a',Math.ceil(h*1.6/20)),'orphans:1;widows:1')));
  add('float',p,`tall+${off}`,doc(f(L('f',Math.ceil(h/20)+2),`margin-top:${h-40+off}px`)+d('')));
  add('float',p,`clr+${off}`,doc(f('f1',`height:${h+off-20}px`)+d('b1','clear:both')+d('c1')));
 }
}
