import {createRequire} from 'node:module';
import {spawn} from 'node:child_process';
import http from 'node:http';import net from 'node:net';import fs from 'node:fs';
import {activate} from '/private/tmp/lp-tos-f9f-batch16-fx/apps/fe/e2e/fixtures/activation.ts';
const require=createRequire('/private/tmp/lp-tos-f9f-batch16-fx/node_modules/.bun/playwright@1.59.1/node_modules/playwright/package.json');
const {chromium}=require('playwright-core');const [binary,output]=process.argv.slice(2);if(fs.existsSync(output))throw Error('fresh evidence required');
const server=http.createServer((q,r)=>{r.setHeader('Content-Type',q.url.startsWith('/main.js')?'text/javascript':'text/html');r.end(q.url.startsWith('/main.js')?fs.readFileSync(new URL('./dist/main.js',import.meta.url)):'<!doctype html><html><head><style>:root { --breakpoint-md: 48rem; }</style></head><body><div id="root"></div><script type="module" src="/main.js"></script></body></html>')});
await new Promise(r=>server.listen(0,'127.0.0.1',r));
const port=await new Promise(r=>{const s=net.createServer();s.listen(0,'127.0.0.1',()=>{const p=s.address().port;s.close(()=>r(p))})});
const child=binary==='chromium'?null:spawn(binary,['serve','--host','127.0.0.1','--port',String(port),'--log-level','error'],{cwd:'/private/tmp/lp-suite-current-5b46788',env:{...process.env,JSC_usePollingTraps:'true'},stdio:'ignore'});let browser;const rows=[];
const limit=setTimeout(()=>{child?.kill('SIGKILL');process.exit(2)},60000);
try{
 if(binary==='chromium')browser=await chromium.launch();else{for(let i=0;i<200;i++){try{if((await fetch(`http://127.0.0.1:${port}/json/version`)).ok)break}catch{}await new Promise(r=>setTimeout(r,25))}browser=await chromium.connectOverCDP(`ws://127.0.0.1:${port}`)}
 for(const fill of [0,400])for(const activation of ['scroll-delivered-then-dom']){
  const context=await browser.newContext({viewport:{width:1440,height:900}});const page=await context.newPage();const errors=[];page.on('pageerror',e=>errors.push(e.name+':'+e.message.slice(0,150)));
  try{
   await page.goto(`http://127.0.0.1:${server.address().port}/?fill=${fill}`);await page.getByPlaceholder('Add Party').waitFor({timeout:5000});
   await page.evaluate(()=>{window.samples=[];window.record=kind=>{const el=document.querySelector('input');const r=el?.getBoundingClientRect();if(window.samples.length<32)window.samples.push({kind,rect:r?[r.x,r.y,r.width,r.height]:null,scrollY,options:document.querySelectorAll('[role="option"]').length,expanded:el?.getAttribute('aria-expanded')})};for(const k of ['focusin','scroll','click'])window.addEventListener(k,()=>window.record(k),true);window.record('before')});
   await page.evaluate(() => new Promise(resolve=>{const before=scrollY;const fn=e=>{if(e.target===document){window.removeEventListener('scroll',fn,true);resolve()}};window.addEventListener('scroll',fn,true);document.querySelector('input').scrollIntoView({block:'center'});if(scrollY===before){window.removeEventListener('scroll',fn,true);resolve()}}));process.env.E2E_DOM_ACTIVATION='1';await activate(page.getByPlaceholder('Add Party'),{timeout:3000});await page.waitForTimeout(150);
   rows.push({fill,activation,errors,samples:await page.evaluate(()=>{window.record('after');return window.samples})});
  }catch(e){rows.push({fill,activation,error:e.name+':'+e.message.slice(0,250),errors})}finally{await context.close()}
 }
 fs.writeFileSync(output,JSON.stringify({binary,version:browser.version(),rows},null,2));console.log(rows.map(r=>({fill:r.fill,activation:r.activation,options:r.samples?.at(-1).options,error:r.error,errors:r.errors})));
}finally{clearTimeout(limit);await browser?.close();child?.kill('SIGTERM');server.close()}
