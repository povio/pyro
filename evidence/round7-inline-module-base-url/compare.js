// bun compare.js base.json cand.json chrome158.json [urlRegex]
const fs=require("fs");
const [b,c,ch]=process.argv.slice(2,5).map(f=>JSON.parse(fs.readFileSync(f)));
const re=new RegExp(process.argv[5]||".");
const idx=r=>{const m=new Map();for(const p of r.results){if(!re.test(p.url))continue;const n=new Map();for(const s of p.subtests||[]){const k=s.name;n.set(k,(n.get(k)||[]).concat(s.status==="Pass"));}m.set(p.url,{p,n});}return m};
const B=idx(b),C=idx(c),H=idx(ch);
let tot={named:0,base:0,cand:0,chrome:0,gained:0,lost:0,baseFull:0,candFull:0,chromeFull:0};
const perPage=[],losses=[];
for(const [url,{p:cp,n:cn}] of C){ const bp=B.get(url); if(!bp) continue; const hp=H.get(url);
  let g=0,l=0,cpass=0,bpass=0,hpass=0;
  for(const [name,arr] of cn){ const ba=bp.n.get(name)||[]; const ha=hp?.n.get(name)||[];
    arr.forEach((v,i)=>{ tot.named++; const bv=!!ba[i], hv=!!ha[i]; if(v)cpass++; if(bv)bpass++; if(hv)hpass++; if(v&&!bv)g++; if(!v&&bv){l++;losses.push(url+" :: "+name);} }); }
  for(const [name,arr] of bp.n) if(!cn.has(name)) arr.forEach(v=>{if(v){l++;losses.push(url+" :: (missing) "+name);}});
  const full=(x)=>x&&x.p.status==="complete"&&[...x.n.values()].flat().every(Boolean)&&x.n.size>0;
  tot.base+=bpass;tot.cand+=cpass;tot.chrome+=hpass;tot.gained+=g;tot.lost+=l;
  tot.baseFull+=full(bp);tot.candFull+=full(C.get(url));tot.chromeFull+=full(hp);
  if(g||l) perPage.push({url,base:bpass,cand:cpass,chrome:hp?hpass:null,total:[...cn.values()].flat().length,gained:g,lost:l});
}
console.log(JSON.stringify({totals:tot,pages:perPage,losses},null,1));
