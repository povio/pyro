import { createRequire } from 'node:module';
import { execFileSync, spawn } from 'node:child_process';
import { writeFileSync } from 'node:fs'; import path from 'node:path';
const here = path.dirname(new URL(import.meta.url).pathname);
const require = createRequire(new URL('../../package.json', import.meta.url));
const { chromium } = require('playwright-core');
const words = Array.from({length:1200},(_,i)=>'word'+i).join(' ');
const FIX = {
 links: `<!doctype html><body style="margin:0;font:20px/30px Arial"><div style="height:40px"></div><p style="margin:0 0 0 40px"><a href="https://a.test/1">example</a> text <a href="https://a.test/2">more</a></p><a href="https://a.test/3" style="display:block;margin-left:40px;width:200px;height:50px"></a>`,
 para: `<!doctype html><body style="margin:0;font:16px/20px Arial"><div style="width:400px">${words}</div>`,
 breaks: `<!doctype html><body style="margin:8px;font:16px/20px Arial"><h1>One</h1><div style="width:400px">${words.slice(0,4000)}</div><h2 style="break-before:page">Two</h2><p>after</p><div style="height:300px;background:#eee;break-inside:avoid">box</div><div style="height:900px;break-inside:avoid">tall</div><p style="break-after:page">end</p><p>last</p>`,
};
const run = async (eng) => {
  let browser, kill=()=>{};
  if (eng==='chrome') browser = await chromium.launch({ executablePath:'/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', headless:true });
  else { const port=9900+Math.floor(Math.random()*90); const p=spawn(eng,['serve','--port',String(port)],{stdio:'ignore'}); kill=()=>p.kill(); for(let i=0;;i++){try{browser=await chromium.connectOverCDP(`http://127.0.0.1:${port}`);break}catch(e){if(i>50)throw e;await new Promise(r=>setTimeout(r,100))}} }
  const ctx=browser.contexts()[0]??await browser.newContext(); const page=await ctx.newPage(); const cdp=await ctx.newCDPSession(page);
  const res={};
  for (const [n,h] of Object.entries(FIX)) { await page.setContent(h);
   for (const [pn,w,hh] of [['Letter',8.5,11],['A4',8.27,11.69]]) for (const m of [.4,1/2.54,.5,0]) for (const s of [1,.8,1.5]) {
    const {data}=await cdp.send('Page.printToPDF',{paperWidth:w,paperHeight:hh,marginTop:m,marginBottom:m,marginLeft:m,marginRight:m,scale:s,printBackground:true});
    const f=path.join(process.env.TMPDIR||'/tmp','pyro-print-72.pdf'); writeFileSync(f,Buffer.from(data,'base64'));
    const pages=JSON.parse(execFileSync('python3',[path.join(here,'pdfrects.py'),f],{maxBuffer:1<<28}));
    const txt=execFileSync('pdftotext',['-layout',f,'-'],{maxBuffer:1<<28}).toString().split('\f').slice(0,-1).map(p=>p.split('\n').map(x=>x.trim()).filter(Boolean));
    const bbox=execFileSync('pdftotext',['-bbox-layout',f,'-'],{maxBuffer:1<<28}).toString(); const ys=[...bbox.matchAll(/<line xMin="[\d.]+" yMin="([\d.]+)"/g)].map(m=>+m[1]); res[[n,pn,m.toFixed(3),s].join('|')]={ys:ys.slice(0,4),txt,pages:pages.length,rects:pages[0].rects,lines:txt.map(p=>p.length),first:txt.map(p=>(p[0]||'').slice(0,20))};
   }}
  await browser.close(); kill(); return res;
};
const a=await run('chrome'), b=await run(process.argv[2]); 
let mis=0,rectbad=0,maxr=0;
for (const k of Object.keys(a)) { const x=a[k],y=b[k];
  if (x.pages!==y.pages||JSON.stringify(x.lines)!==JSON.stringify(y.lines)) { mis++; console.log('LINES',k,x.pages,y.pages,x.lines.join(),'|',y.lines.join()); }
  if (k.startsWith('links')) { if(x.rects.length!==y.rects.length) console.log('RECTN',k,x.rects.length,y.rects.length); else x.rects.forEach((r,i)=>r.forEach((v,j)=>{const d=Math.abs(v-y.rects[i][j]);maxr=Math.max(maxr,d);if(d>.25){rectbad++;console.log('RECT',k,i,r,y.rects[i]);}})); }
}
console.log('cases',Object.keys(a).length,'line/page mismatches',mis,'rect>0.25',rectbad,'max rect diff',maxr);
process.exit(0);
