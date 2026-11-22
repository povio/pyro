import { createRequire } from 'node:module';
import { readFile, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
const { chromium } = createRequire('/private/tmp/pyro-r7-param/bench/package.json')('playwright-core');
const mode = process.argv[2];
if (!['native','baseline','candidate'].includes(mode)) throw Error('Choose native, baseline or candidate');
const started = new Date().toISOString();
const clock = process.hrtime.bigint();
const source = mode === 'native' ? null : await readFile(new URL(`${mode}.js`,import.meta.url),'utf8');
const browser = await chromium.launch({ executablePath:'/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', headless:true, args:['--disable-background-networking'] });
const version = browser.version();
let results;
try {
  const page = await browser.newPage();
  results = await page.evaluate(async source => {
    const api = source === null ? window : (0,eval)(source)({ activated:()=>true, timer:(delay,callback)=>setTimeout(callback,delay), clearTimer:clearTimeout.bind(window) });
    const rows = [];
    for (const type of ['realtime','offline']) for (const label of ['0','-1','33','max+1','1','same']) {
      const context = type === 'offline' ? new api.OfflineAudioContext(2,256,48000) : new api.AudioContext();
      const dest = context.destination, before = dest.channelCount, max = dest.maxChannelCount;
      const value = label === 'same' ? before : label === 'max+1' ? max+1 : Number(label);
      let exception = null;
      try { dest.channelCount = value; } catch (error) { exception = error.name; }
      rows.push({type,label,before,max,value,exception,after:dest.channelCount});
      if (type === 'realtime') await context.close();
    }
    const context = new api.OfflineAudioContext(2,256,48000);
    const buf = context.createBuffer(2,256,48000);
    const levels = [0.25,-0.5];
    levels.forEach((level,index)=>buf.getChannelData(index).fill(level));
    const node = context.createBufferSource(); node.buffer=buf; node.connect(context.destination); node.start();
    const audio = await context.startRendering();
    const pcm = {channels:audio.numberOfChannels,frames:audio.length,rate:audio.sampleRate,levels,maxAbsError:levels.map((level,index)=>Math.max(...audio.getChannelData(index).map(sample=>Math.abs(sample-level))))};
    return {rows,pcm};
  },source);
} finally { await browser.close(); }
const ended = new Date().toISOString();
const result = {mode,version,started,ended,elapsedNs:String(process.hrtime.bigint()-clock),sha256:source && createHash('sha256').update(source).digest('hex'),...results};
await writeFile(new URL(`${mode}.json`,import.meta.url),JSON.stringify(result,null,2)+'\n',{flag:'wx'});
console.log(JSON.stringify(result));
