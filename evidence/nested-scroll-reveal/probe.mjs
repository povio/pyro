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
 await page.goto(url);
 samples.push(await page.evaluate(()=>{
  document.body.innerHTML=Array.from({length:100},()=>'<div style="height:20px">before</div>').join('')+'<div id="outer" style="overflow:auto;width:180px;height:80px">'+Array.from({length:20},()=>'<div style="height:20px">outer</div>').join('')+'<div id="inner" style="overflow:auto;width:120px;height:60px">'+Array.from({length:20},()=>'<div style="height:20px">inner</div>').join('')+'<button id="target" style="height:20px;width:20px">target</button><div style="height:100px">tail</div></div><div style="height:100px">tail</div></div><div style="height:900px">end</div>';
  const outer=document.getElementById('outer'),inner=document.getElementById('inner'),target=document.getElementById('target');
  const snap=()=>({outer:outer.scrollTop,inner:inner.scrollTop,viewport:scrollY,targetY:target.getBoundingClientRect().y,outerY:outer.getBoundingClientRect().y,innerY:inner.getBoundingClientRect().y});
  window.scrollEvents=[];for(const [name,node] of [['outer',outer],['inner',inner],['document',document]])node.addEventListener('scroll',e=>{if(e.target===node)window.scrollEvents.push(name)});
  const before=snap();target.scrollIntoView();return {before,after:snap()};
 }));
 await page.waitForTimeout(50);const events=await page.evaluate(()=>window.scrollEvents);samples.push({events});

 fs.writeFileSync(output,JSON.stringify({binary,version:browser.version(),samples},null,2));
}finally{clearTimeout(deadline);if(browser)await browser.close();if(child)child.kill('SIGTERM');server.close();}
