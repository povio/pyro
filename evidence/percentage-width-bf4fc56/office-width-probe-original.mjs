import {createRequire} from 'node:module';import{spawn}from'node:child_process';import http from'node:http';import net from'node:net';import fs from'node:fs';
const require=createRequire('/private/tmp/lp-w3-tos-scroll-readiness/apps/fe/package.json');const{chromium}=require('playwright');const[binary,output]=process.argv.slice(2);if(fs.existsSync(output))throw Error('fresh output required');
const html=`<!doctype html><html><head><style>
.cell{width:320px}.container{position:relative;display:flex;width:100%;min-width:0;align-items:center;gap:6px;overflow:hidden}.measurement{visibility:hidden;pointer-events:none;position:absolute;display:flex;width:max-content;gap:6px}.badge{flex-shrink:0;white-space:nowrap;border-radius:4px;padding:4px 6px;line-height:1;font:500 12px Arial}
</style></head><body><div class="cell"><div class="container"><div class="measurement"><span class="badge">Business Partner</span><span class="badge">Office</span><span class="badge">+1</span><span class="badge">+2</span></div></div></div></body></html>`;
const server=http.createServer((q,r)=>{r.setHeader('Content-Type','text/html');r.end(html)});await new Promise(r=>server.listen(0,'127.0.0.1',r));
const port=await new Promise(r=>{const s=net.createServer();s.listen(0,'127.0.0.1',()=>{const p=s.address().port;s.close(()=>r(p))})});
const child=binary==='chromium'?null:spawn(binary,['serve','--host','127.0.0.1','--port',String(port),'--log-level','error'],{env:{...process.env,JSC_usePollingTraps:'true'},stdio:'ignore'});let browser;const rows=[];const watchdog=setTimeout(()=>{child?.kill('SIGKILL');process.exit(2)},60000);
try{
 if(!child)browser=await chromium.launch();else{for(let i=0;i<200;i++){try{if((await fetch(`http://127.0.0.1:${port}/json/version`)).ok)break}catch{}await new Promise(r=>setTimeout(r,25))}browser=await chromium.connectOverCDP(`ws://127.0.0.1:${port}`)}
 const context=await browser.newContext({viewport:{width:1440,height:900}});const page=await context.newPage();
 try{await page.goto(`http://127.0.0.1:${server.address().port}/`);
 rows.push(await page.evaluate(()=>{const c=document.querySelector('.container'),m=document.querySelector('.measurement');const widths=Array.from(m.children,x=>x.getBoundingClientRect().width);const available=c.getBoundingClientRect().width;const gap=parseFloat(getComputedStyle(c).columnGap)||0;let used=0,count=0;for(let i=0;i<2;i++){used+=widths[i]+(i?gap:0);const remaining=1-i;const required=used+(remaining?gap+widths[2+remaining-1]:0);if(required<=available)count=i+1}return{available,gap,widths,visibleCount:count,measurementWidth:m.getBoundingClientRect().width,computed:{container:getComputedStyle(c).width,measurement:getComputedStyle(m).width,firstBadge:getComputedStyle(m.children[0]).width}}}));
 }finally{await context.close()}
 fs.writeFileSync(output,JSON.stringify({binary,version:browser.version(),rows},null,2));console.log(rows);
}finally{clearTimeout(watchdog);await browser?.close();child?.kill('SIGTERM');await new Promise(r=>server.close(r))}
