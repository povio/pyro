// Reduced Vite preload scan: unchanged reverse live-link search and link
// reflection/setter/append shape; synthetic absolute .js dependencies only.
// Count wrappers run in a separate document; timed runs use original getters.
import {createRequire} from 'node:module';
import {spawn} from 'node:child_process';
import http from 'node:http';
import net from 'node:net';
import fs from 'node:fs';
import crypto from 'node:crypto';
const require=createRequire('/private/tmp/lp-tos-f9f-batch16-fx/node_modules/.bun/playwright@1.59.1/node_modules/playwright/package.json');
const {chromium}=require('playwright-core');
const [binary,output]=process.argv.slice(2);
if(!binary||!output)throw Error('binary and fresh output required');
if(fs.existsSync(output))throw Error('refusing to overwrite evidence');
let requests=0;
const server=http.createServer((q,r)=>{requests++;r.setHeader('Content-Type',q.url.endsWith('.js')?'text/javascript':'text/html');r.end(q.url.endsWith('.js')?'':'<!doctype html><html><head></head><body>link scan</body></html>');});
await new Promise(r=>server.listen(0,'127.0.0.1',r));
const url=`http://127.0.0.1:${server.address().port}/configuration/profile`;
const port=await new Promise(r=>{const s=net.createServer();s.listen(0,'127.0.0.1',()=>{const p=s.address().port;s.close(()=>r(p));});});
const child=binary==='chromium'?null:spawn(binary,['serve','--host','127.0.0.1','--port',String(port),'--log-level','error'],{cwd:'/private/tmp/lp-reflected-url-cache',env:{...process.env,JSC_usePollingTraps:'true'},stdio:'ignore'});
let browser;
const deadline=setTimeout(()=>{child?.kill('SIGKILL');process.exitCode=2;},45000);
try{
 if(binary==='chromium'){browser=await chromium.launch();}else{
 let ready=false;
 for(let i=0;i<200;i++){try{if((await fetch(`http://127.0.0.1:${port}/json/version`)).ok){ready=true;break;}}catch{}await new Promise(r=>setTimeout(r,25));}
 if(!ready)throw Error('browser startup timeout');
 browser=await chromium.connectOverCDP(`ws://127.0.0.1:${port}`);
 }
 const context=await browser.newContext({viewport:{width:1440,height:900}});const page=await context.newPage();
 const samples=[];
 for(const mode of ['default','prevent','allow']){
  await page.goto(url);
  samples.push(await page.evaluate(mode=>{
   document.body.innerHTML=Array.from({length:200},()=>'<div style="height:20px">filler</div>').join('')+'<button id="target">target</button><div style="height:900px">end</div>';
   const b=document.getElementById('target');const snap=()=>({y:b.getBoundingClientRect().y,height:b.getBoundingClientRect().height,scrollY,active:document.activeElement===b});
   const before=snap();const events=[];b.addEventListener('focus',()=>events.push({type:'focus',...snap()}));b.addEventListener('focusin',()=>events.push({type:'focusin',...snap()}));
   if(mode==='default')b.focus();else b.focus({preventScroll:mode==='prevent'});
   return {mode,before,after:snap(),events,viewport:[innerWidth,innerHeight]};
  },mode));
 }
 fs.writeFileSync(output,JSON.stringify({binary,version:browser.version(),samples},null,2));
}finally{clearTimeout(deadline);if(browser)await browser.close();if(child)child.kill('SIGTERM');server.close();}
