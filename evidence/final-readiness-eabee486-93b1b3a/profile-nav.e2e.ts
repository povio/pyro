import { writeFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { performance } from 'node:perf_hooks';
import { expect, test } from '/private/tmp/pyro-tos-f9f-batch16-fx/apps/fe/e2e/fixtures/test';
import { createLightpandaBrowser } from '/private/tmp/pyro-tos-f9f-batch16-fx/apps/fe/e2e/fixtures/lightpanda';
import { trackBrowserContexts } from '/private/tmp/pyro-tos-f9f-batch16-fx/apps/fe/e2e/fixtures/browser';
import { startWorkerSession, stopWorkerSession } from '/private/tmp/pyro-tos-f9f-batch16-fx/apps/fe/e2e/fixtures/worker-session';
import { e2eEnv } from '/private/tmp/pyro-tos-f9f-batch16-fx/apps/fe/e2e/fixtures/env';

const out='/private/tmp/pyro-final-readiness-93b1b3a';
const rows:any[]=[];
const save=()=>writeFileSync(`${out}/observations.json`,JSON.stringify(rows,null,2));
const browserPids=()=>execFileSync('/bin/ps',['-axo','pid=,comm='],{encoding:'utf8'}).split('\n').filter(l=>l.trim().endsWith(process.env.LIGHTPANDA_EXECUTABLE!)).map(l=>Number(l.trim().split(/\s+/)[0]));
const sample=(pid:number)=>{
 const t=performance.now();
 const values=execFileSync(`${out}/rss-sampler`,[String(pid),'-','--once'],{encoding:'utf8'}).trim().split(/\s+/).map(Number);
 return {hostStartMs:t,hostEndMs:performance.now(),epochMs:values[0],rssKiB:values[1],footprintKiB:values[2],userNs:values[3],systemNs:values[4],machMonotonicNs:values[5]};
};

test('diagnostic profile hard navigation matrix',async({playwright})=>{
 test.setTimeout(300000);
 const groupOrdinals={A:0,B:0};
 for (const [launchIndex,variant] of (['A','B','B','A','B','A','A','B'] as const).entries()) {
  const cacheMiB=64, processOrdinal=launchIndex+1, groupOrdinal=++groupOrdinals[variant];
  process.env.LIGHTPANDA_EXECUTABLE=`${out}/pyro-${variant}`;
  process.env.PYRO_SHARED_ASSET_CACHE_MB=String(cacheMiB);
  const prior=new Set(browserPids());
  const facade=trackBrowserContexts(createLightpandaBrowser(playwright));
  const setupStart=performance.now();
  let session:any;
  const processRow:any={hostTimeOriginMs:performance.timeOrigin,cacheMiB,processOrdinal,variant,launchOrdinal:processOrdinal,groupOrdinal,blockOrdinal:Math.floor(launchIndex/4)+1,setupStartMs:setupStart,navigations:[]}; rows.push(processRow);save();
  try {
   session=await startWorkerSession(facade);
   const pids=browserPids().filter(p=>!prior.has(p));
   if(pids.length!==1) throw new Error('diagnostic browser PID is ambiguous');
   const pid=pids[0];processRow.pid=pid;
   const page=await session.context.newPage();
   const cdp=await session.context.newCDPSession(page);await cdp.send('Network.enable');
   let current:any; const requests=new Map<string,{type:string,ordinal:number}>();let ordinal=0;
   const event=(kind:string,extra:any={})=>{if(current) current.network.push({kind,hostMs:performance.now(),...extra});};
   cdp.on('Network.requestWillBeSent',(e:any)=>{requests.set(e.requestId,{type:e.type??'Other',ordinal:++ordinal});event('request',{type:e.type??'Other',ordinal});});
   cdp.on('Network.responseReceived',(e:any)=>{const q=requests.get(e.requestId);event('response',{...q,type:e.type??q?.type??'Other',fromDiskCache:!!e.response.fromDiskCache,fromServiceWorker:!!e.response.fromServiceWorker,fromPrefetchCache:!!e.response.fromPrefetchCache,encodedDataLength:e.response.encodedDataLength??null});});
   cdp.on('Network.requestServedFromCache',(e:any)=>event('cache',{...requests.get(e.requestId)}));
   cdp.on('Network.dataReceived',(e:any)=>event('data',{...requests.get(e.requestId),dataLength:e.dataLength,encodedDataLength:e.encodedDataLength}));
   cdp.on('Network.loadingFinished',(e:any)=>event('finished',{...requests.get(e.requestId),encodedDataLength:e.encodedDataLength}));
   cdp.on('Network.loadingFailed',(e:any)=>event('failed',{...requests.get(e.requestId),canceled:!!e.canceled}));
   page.on('domcontentloaded',()=>{if(current)current.dclHostMs=performance.now();});
   page.on('load',()=>{if(current)current.loadHostMs=performance.now();});
   processRow.setupEndMs=performance.now();save();
   for(let navigationOrdinal=1;navigationOrdinal<=3;navigationOrdinal++){
    requests.clear();ordinal=0;
    current={navigationOrdinal,network:[],before:sample(pid)};
    processRow.navigations.push(current);
    current.startHostMs=performance.now();current.startEpochMs=performance.timeOrigin+current.startHostMs;save();
    await page.goto(`${e2eEnv.feUrl}/configuration/profile`,{waitUntil:'commit'});
    current.commitHostMs=performance.now();
    // Verbatim assertions from unchanged f9f profile-settings.e2e.ts.
    await expect(page).toHaveURL(/\/configuration\/profile\/?$/);
    await expect(page).toHaveTitle("My Profile");
    await expect(page.getByRole("heading", { name: "My Profile" })).toBeVisible();
    await expect(page.getByRole("textbox", { name: "First Name" })).toBeVisible();
    await expect(page.getByRole("textbox", { name: "Last Name" })).toBeVisible();
    await expect(page.getByRole("textbox", { name: "Email" })).toBeDisabled();
    await expect(page.getByText("Invoice signature", { exact: true })).toBeVisible();
    current.usableHostMs=performance.now();current.usableEpochMs=performance.timeOrigin+current.usableHostMs;current.usable=sample(pid);save();
    const remaining=current.usableHostMs+1000-performance.now();
    if(remaining>0)await new Promise(r=>setTimeout(r,remaining));
    current.observationEndHostMs=performance.now();current.observationEndEpochMs=performance.timeOrigin+current.observationEndHostMs;const completed=current;current=undefined;completed.end=sample(pid);
    save();
   }
   await cdp.detach();await page.close();processRow.completed=true;save();
  } catch(e) {processRow.failed=true;processRow.errorClass=e instanceof Error?e.name:'unknown';save();throw e;}
  finally {await stopWorkerSession(session);await facade.close();processRow.closedHostMs=performance.now();save();}
 }
});
