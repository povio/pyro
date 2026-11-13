import {createRequire} from 'node:module';import{spawn}from'node:child_process';import http from'node:http';import net from'node:net';import fs from'node:fs';
const require=createRequire('/private/tmp/pyro-tos-calendar-6e7f7cf-fx/apps/fe/package.json');const{chromium}=require('playwright');const[binary,output]=process.argv.slice(2);if(fs.existsSync(output))throw Error('fresh output required');
const fillers=n=>Array.from({length:n},()=>'<div style="height:10px">row</div>').join('');
const html=`<!doctype html><html><body>${fillers(150)}<div id="outer" style="height:120px;width:240px;overflow:auto">${fillers(30)}<button id="button" style="display:block;width:160px;padding:0;border:0">${Array.from({length:8},()=>'<span style="display:block;height:10px">row</span>').join('')}<span id="label" style="display:block;height:10px">Click me</span></button>${fillers(80)}</div>${fillers(150)}</body></html>`;
const server=http.createServer((q,r)=>{r.setHeader('Content-Type','text/html');r.end(html)});await new Promise(r=>server.listen(0,'127.0.0.1',r));
const port=await new Promise(r=>{const s=net.createServer();s.listen(0,'127.0.0.1',()=>{const p=s.address().port;s.close(()=>r(p))})});
const child=binary==='chromium'?null:spawn(binary,['serve','--host','127.0.0.1','--port',String(port),'--log-level','error'],{env:{...process.env,JSC_usePollingTraps:'true'},stdio:'ignore'});let browser;const rows=[];const watchdog=setTimeout(()=>{child?.kill('SIGKILL');process.exit(2)},60000);
try{
 if(!child)browser=await chromium.launch();else{for(let i=0;i<200;i++){try{if((await fetch(`http://127.0.0.1:${port}/json/version`)).ok)break}catch{}await new Promise(r=>setTimeout(r,25))}browser=await chromium.connectOverCDP(`ws://127.0.0.1:${port}`)}
 for(const alignment of['label','button']){
 const context=await browser.newContext({viewport:{width:1440,height:900}});const page=await context.newPage();
 try{
 await page.goto(`http://127.0.0.1:${server.address().port}/`);
 await page.evaluate(alignment=>new Promise(resolve=>{
  const button=document.getElementById('button'),label=document.getElementById('label'),outer=document.getElementById('outer');window.clicked=0;window.events=[];button.addEventListener('click',()=>window.clicked++);
  window.snapshot=event=>{const r=label.getBoundingClientRect(),b=button.getBoundingClientRect();return{event:event?.type,target:event?.target?.id||event?.target?.tagName,active:document.activeElement?.id,outer:outer.scrollTop,scrollY,label:[r.x,r.y,r.width,r.height],button:[b.x,b.y,b.width,b.height],clicked:window.clicked}};
  for(const type of['pointerdown','mousedown','focusin','pointerup','mouseup','click','scroll'])document.addEventListener(type,e=>window.events.push(window.snapshot(e)),true);
  document.getElementById(alignment).scrollIntoView({block:'start',inline:'nearest',behavior:'instant'});setTimeout(()=>{window.events=[];resolve()},0);
 }),alignment);
 const before=await page.evaluate(()=>window.snapshot(null));const box=await page.locator('#label').boundingBox();await page.mouse.click(box.x+box.width/2,box.y+box.height/2);
 const after=await page.evaluate(()=>new Promise(resolve=>setTimeout(()=>resolve({state:window.snapshot(null),events:window.events}),0)));
 rows.push({alignment,before,after});
 }catch(e){rows.push({alignment,error:String(e)})}finally{await context.close()}
 }
 fs.writeFileSync(output,JSON.stringify({binary,version:browser.version(),rows},null,2));console.log(rows.map(r=>({alignment:r.alignment,clicked:r.after?.state.clicked,error:r.error})));
}finally{clearTimeout(watchdog);await browser?.close();child?.kill('SIGTERM');await new Promise(r=>server.close(r))}
