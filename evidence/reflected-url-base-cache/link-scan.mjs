// Reduced Vite preload scan: unchanged reverse live-link search and link
// reflection/setter/append shape; synthetic absolute .js dependencies only.
// Count wrappers run in a separate document; timed runs use original getters.
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
const child=spawn(binary,['serve','--host','127.0.0.1','--port',String(port),'--log-level','error'],{cwd:'/private/tmp/pyro-reflected-url-cache',env:{...process.env,JSC_usePollingTraps:'true'},stdio:'ignore'});
let browser;
const deadline=setTimeout(()=>{child.kill('SIGKILL');process.exitCode=2;},45000);
try{
 let ready=false;
 for(let i=0;i<200;i++){try{if((await fetch(`http://127.0.0.1:${port}/json/version`)).ok){ready=true;break;}}catch{}await new Promise(r=>setTimeout(r,25));}
 if(!ready)throw Error('browser startup timeout');
 browser=await chromium.connectOverCDP(`ws://127.0.0.1:${port}`);
 const context=await browser.newContext();const page=await context.newPage();
 const scan=({size,count})=>{
  const members=['href','rel','as','crossOrigin'];const descriptors={};const counters={};
  if(count)for(const name of members){const d=Object.getOwnPropertyDescriptor(HTMLLinkElement.prototype,name);descriptors[name]=d;counters[name]={get:0,set:0};Object.defineProperty(HTMLLinkElement.prototype,name,{...d,get(){counters[name].get++;return d.get.call(this)},set(v){counters[name].set++;return d.set.call(this,v)}});}
  const links=document.getElementsByTagName('link');const dependencies=Array.from({length:size},(_,i)=>new URL(`/assets/module-${i}.js`,document.URL).href);
  const start=performance.now();
  for(const n of dependencies){let present=false;const css=n.endsWith('.css');for(let i=links.length-1;i>=0;i--){const link=links[i];if(link.href===n&&(!css||link.rel==='stylesheet')){present=true;break;}}if(present)continue;const link=document.createElement('link');link.rel=css?'stylesheet':'modulepreload';if(!css)link.as='script';link.crossOrigin='';link.href=n;document.head.appendChild(link);}
  const elapsedMs=performance.now()-start;
  if(count)for(const name of members)Object.defineProperty(HTMLLinkElement.prototype,name,descriptors[name]);
  if(links.length!==size)throw Error('unexpected link count');
  return {size,elapsedMs,counters:count?counters:undefined,links:links.length};
 };
 await page.goto(url);const counts=await page.evaluate(scan,{size:294,count:true});
 if(counts.counters.href.get!==294*293/2)throw Error('unexpected href read count');
 const samples=[];
 for(const size of [294,600])for(let ordinal=0;ordinal<7;ordinal++){await page.goto(url);samples.push({...await page.evaluate(scan,{size,count:false}),ordinal,warmup:ordinal===0});}
 const out={binary,sha256:crypto.createHash('sha256').update(fs.readFileSync(binary)).digest('hex'),pid:child.pid,counts,samples,serverRequests:requests,scope:'one browser process, fresh document per scan; original getters during timing; wall time includes JS+native loop, no CPU or TOS navigation claim',polling:true};
 fs.writeFileSync(output,JSON.stringify(out,null,2));console.log(JSON.stringify({counts:counts.counters,samples:samples.length,output}));
}finally{clearTimeout(deadline);if(browser)await browser.close();child.kill('SIGTERM');server.close();}
