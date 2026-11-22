const fs=require("fs"),path=require("path"),crypto=require("crypto");
const dir=process.argv[2], pat=new RegExp(process.argv[3]);
const out={version:"158.0.8127.37",wptRevision:"a421f2fea5eb7f91e0db399e44b0fb0779ee1603",receipts:{},results:[]};
for(const f of fs.readdirSync(dir).filter(f=>f.endsWith(".json"))){
  const buf=fs.readFileSync(path.join(dir,f)); let r; try{r=JSON.parse(buf)}catch{continue}
  if(!r.results) continue;
  const hit=r.results.filter(p=>pat.test(p.url));
  if(hit.length){ out.receipts[f]=crypto.createHash("sha256").update(buf).digest("hex"); for(const p of hit) out.results.push({...p,receipt:f}); }
}
console.log(JSON.stringify(out));
