// Reduced modal variable-dimension control; no app/auth data.
import {createRequire} from 'node:module';
import {spawn} from 'node:child_process';
import http from 'node:http';
import net from 'node:net';
import fs from 'node:fs';
import crypto from 'node:crypto';
const require=createRequire('/private/tmp/pyro-tos-f9f-batch16-fx/node_modules/.bun/playwright@1.59.1/node_modules/playwright/package.json');
const {chromium}=require('playwright-core');
const [binary,output]=process.argv.slice(2);
if(!binary||!output)throw Error('binary and fresh output required');
if(fs.existsSync(output))throw Error('refusing to overwrite evidence');
let requests=0;
const server=http.createServer((q,r)=>{requests++;r.setHeader('Content-Type',q.url.endsWith('.js')?'text/javascript':'text/html');r.end(q.url.endsWith('.js')?'':'<!doctype html><html><head></head><body>link scan</body></html>');});
await new Promise(r=>server.listen(0,'127.0.0.1',r));
const url=`http://127.0.0.1:${server.address().port}/configuration/profile`;
const port=await new Promise(r=>{const s=net.createServer();s.listen(0,'127.0.0.1',()=>{const p=s.address().port;s.close(()=>r(p));});});
const child=binary==='chromium'?null:spawn(binary,['serve','--host','127.0.0.1','--port',String(port),'--log-level','error'],{cwd:'/private/tmp/pyro-reflected-url-cache',env:{...process.env,JSC_usePollingTraps:'true'},stdio:'ignore'});
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
  document.body.innerHTML='<style>.overlay{height:var(--visual-viewport-height);width:var(--w,120px);overflow:auto}</style><div class="overlay" id="overlay" style="--visual-viewport-height:900px"><input id="field"></div>';
  const el=document.getElementById('overlay');const snap=()=>({height:el.getBoundingClientRect().height,width:el.getBoundingClientRect().width,declared:getComputedStyle(el).height,custom:el.style.getPropertyValue('--visual-viewport-height')});
  const before=snap();el.style.setProperty('--visual-viewport-height','700px');return {before,after:snap()};
 }));

 fs.writeFileSync(output,JSON.stringify({binary,version:browser.version(),samples},null,2));
}finally{clearTimeout(deadline);if(browser)await browser.close();if(child)child.kill('SIGTERM');server.close();}
