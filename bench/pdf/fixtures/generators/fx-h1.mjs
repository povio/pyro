// Which whole-px height a page holds, direct: a line placed to end exactly at N stays on page 1.
const PAPERS={Letter:[8.5,11],A4:[8.27,11.69],A4x:[8.2677165,11.6929134],Legal:[8.5,14],Tiny:[4.01,5.03]};
const M=[0.4,1/2.54,0.5,0,0.25,0.1,0.39,0.41,1,0.123];
const lrd=x=>Math.floor(x*64+.5)/64, ltr=x=>Math.floor(x*64)/64, f=Math.fround;
const mod=(P,mp,s)=>{const sc=f(1/f(s));const pg=lrd(f(f(P*96)/72));const m=f(f(mp*96)/72);return ltr(f(pg*sc))-2*ltr(f(m*sc));};
export const cases=[];
for(const [pn,[w,h]] of Object.entries(PAPERS)) for(const m of M) for(const s of [1,.8,1.5,.75,.5,1.2,2,.9]) {
  const P=Math.ceil(h*72),mp=Math.floor(m*72);const a=Math.ceil(mod(P,mp,s));
  if(a<40) continue;
  const o={paperWidth:w,paperHeight:h,marginTop:m,marginBottom:m,marginLeft:m,marginRight:m,scale:s};
  for(const d of [1]) cases.push({name:`${pn}|${m.toFixed(4)}|${s}|${d}|${mod(P,mp,s)}`,opts:o,html:`<!doctype html><body style="margin:0;font:16px/20px Arial"><div style="height:${a+d-20}px"></div><div>x1</div><div style="height:${a+d-20}px;break-before:page"></div>`});
}
