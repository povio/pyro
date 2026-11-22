const { chromium } = require('node:module').createRequire('/private/tmp/pyro-r7-param/bench/package.json')('playwright-core');
const fs = require('node:fs');
(async () => {
  const browser = await chromium.launch({executablePath:'/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',args:['--autoplay-policy=no-user-gesture-required']});
  try {
    for (const permissions of [[], ['microphone']]) {
      const session = await browser.newContext(permissions.length ? {permissions} : {});
      if (process.argv[2]) await session.addInitScript({content:`(() => {const api=(${fs.readFileSync(process.argv[2],'utf8')})({activated:()=>true,timer:(delay,fn)=>setTimeout(fn,delay),clearTimer:id=>clearTimeout(id),canUpdatePlaybackStats:()=>document.visibilityState==='visible'});Object.assign(window,{AudioContext:api.AudioContext,AudioPlaybackStats:api.AudioPlaybackStats});window.qaFactory=api.AudioContext;})()`});
      await session.route('**/*', route => route.fulfill({contentType:'text/html',body:'<!doctype html><title>Audio QA</title>'}));
      const page = await session.newPage();
      await page.goto('https://audio.test/');
      const result = await page.evaluate(async () => {
        const sleep = ms => new Promise(resolve => setTimeout(resolve,ms));
        const audio = new AudioContext(), other = new AudioContext(), stats = audio.playbackStats;
        const prototype = Object.getPrototypeOf(stats), error = action => {try {action();return null} catch(reason){return reason.name}};
        const identity = {stable:stats===audio.playbackStats,separate:stats!==other.playbackStats,tag:Object.prototype.toString.call(stats),instance:stats instanceof AudioPlaybackStats,keys:Object.getOwnPropertyNames(prototype),badGetter:error(()=>Object.getOwnPropertyDescriptor(prototype,'totalDuration').get.call({})),badMethod:error(()=>stats.toJSON.call({})),badReset:error(()=>stats.resetLatency.call({}))};
        await other.close();
        const samples = [], snap = label => {const json=stats.toJSON(); samples.push({label,ms:performance.now(),state:audio.state,time:audio.currentTime,json,fieldsMatch:Object.keys(json).every(key=>json[key]===stats[key]),freshJSON:json!==stats.toJSON()})};
        const source=audio.createOscillator(), gain=audio.createGain();gain.gain.value=0.00001;source.connect(gain).connect(audio.destination);source.start();await audio.resume();snap('start'); await sleep(250);snap('running1');await sleep(500);snap('running2');await audio.suspend();await sleep(80);snap('suspended1');await sleep(350);snap('suspended2');const reset=stats.resetLatency();snap('reset');await audio.resume();await sleep(300);snap('resumed');await audio.close();await sleep(80);snap('closed1');await sleep(350);snap('closed2');
        const closedReset=stats.resetLatency();snap('closedReset');
        const long=new AudioContext();await long.resume();const growth=[];for(const delay of [0,1150,1150]){await sleep(delay);growth.push({ms:performance.now(),time:long.currentTime,json:long.playbackStats.toJSON()})}await long.close();identity.candidateFactory=window.qaFactory===audio.constructor;identity.visibility=document.visibilityState;identity.growth=growth;
        return {secure:isSecureContext,permission:(await navigator.permissions.query({name:'microphone'})).state,rate:audio.sampleRate,identity,resetUndefined:reset===undefined && closedReset===undefined,samples};
      });
      const filename=`${process.argv[2]?'candidate':'native'}-${result.permission}-${Date.now()}.json`;fs.writeFileSync(filename,JSON.stringify({chrome:browser.version(),...result},null,2)); console.log(filename,result.samples.slice(-2));await session.close();
    }
  } finally {await browser.close()}
})().catch(error=>{console.error(error);process.exitCode=1});
