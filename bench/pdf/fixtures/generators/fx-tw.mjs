// T (width threshold) and H (page height) over papers, margins, scales
const words=Array.from({length:120},(_,i)=>'w'+i).join(' ');
const PAPERS={Letter:[8.5,11],A4:[8.27,11.69],A4x:[8.2677165,11.6929134],Legal:[8.5,14],Tiny:[4.01,5.03]};
const M=[0.4,1/2.54,0.5,0,0.25,0.1,0.39,0.41,1,0.123];
const rows=`<!doctype html><style>html,body{margin:0;padding:0}a{display:block;height:1px;background:#000}</style>`+Array.from({length:3000},(_,i)=>`<a href="https://x.test/${i}"></a>`).join('');
export const cases=[];
const add=(pn,w,h,m,s)=>{const o={paperWidth:w,paperHeight:h,marginTop:m,marginBottom:m,marginLeft:m,marginRight:m,scale:s};
  const pw=Math.ceil(w*72)-2*Math.floor(m*72); const cw=pw*4/3/s;
  cases.push({name:`W|${pn}|${m.toFixed(4)}|${s}|${pw}`,opts:o,html:`<!doctype html><body style="margin:0;font:16px/20px Arial"><div style="width:${Math.round(cw*1.2)}px">${words}</div>`});
  if(process.env.H) cases.push({name:`H|${pn}|${m.toFixed(4)}|${s}|${pw}`,opts:o,html:rows});};
for(const [pn,[w,h]] of Object.entries(PAPERS)) for(const m of M) for(const s of [1,.8,1.5]) add(pn,w,h,m,s);
for(const s of [1.25,.75,.5,2,.9,1.1,1.2,.6]) for(const m of [.4,.1,0]) {add('A4',8.27,11.69,m,s);add('Letter',8.5,11,m,s);}
