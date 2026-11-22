const fs = require('node:fs');
const crypto = require('node:crypto');
const path = require('node:path');
const {chromium} = require('/private/tmp/pyro-r7-param/bench/node_modules/playwright-core');
const chrome = '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';
const candidate = process.argv[2];
const hash = source => crypto.createHash('sha256').update(source).digest('hex');
async function controls() {
  const results = [];
  for (const rate of [44100, 48000]) {
    async function render(mode, wet) {
      const frames = mode === 'pulse' ? 2048 : rate;
      const context = new OfflineAudioContext(2, frames, rate);
      const buffer = context.createBuffer(2, frames, rate);
      const left = buffer.getChannelData(0), right = buffer.getChannelData(1);
      if (mode === 'pulse') left[37] = right[37] = 0.001;
      else {
        left.fill(0.01); right.fill(-0.01);
        if (mode === 'left') left.fill(0.8, 0, rate / 2);
        if (mode === 'right') right.fill(-0.8, 0, rate / 2);
      }
      const source = context.createBufferSource(); source.buffer = buffer;
      const node = wet ? context.createDynamicsCompressor() : source;
      if (wet) source.connect(node);
      node.connect(context.destination); source.start();
      const output = await context.startRendering();
      return [output.getChannelData(0), output.getChannelData(1)];
    }
    const onset = samples => samples.findIndex(value => Math.abs(value) > 1e-8);
    const dry = await render('pulse', false), wet = await render('pulse', true);
    const latency = wet.map((channel,index) => onset(channel)-onset(dry[index]));
    const gain = (samples, time, input) => {
      const start = Math.round(rate*time) + latency[0];
      return samples.slice(start,start+128).reduce((sum,value) => sum+value/input,0)/128;
    };
    const quiet = await render('quiet', true);
    const quietGain = gain(quiet[0],0.45,0.01);
    const stereo = [];
    for (const mode of ['left','right']) {
      const out = await render(mode,true), loud = Number(mode === 'right'), probe = 1-loud;
      const probeInput = probe === 0 ? 0.01 : -0.01;
      const loudGain = gain(out[loud],0.45,loud ? -0.8 : 0.8);
      const probeGain = gain(out[probe],0.45,probeInput);
      const recovery = [0.51,0.65,0.95].map(time=>gain(out[probe],time,probeInput));
      stereo.push({mode,loudGain,probeGain,recovery});
    }
    const checks = {dry:dry.every(channel=>onset(channel)===37), delay:latency.every(value=>Math.abs(value-rate*0.006)<=1), gain:Number.isFinite(quietGain)&&quietGain>0, attenuation:stereo.every(row=>row.loudGain>0 && row.loudGain<1 && row.probeGain<quietGain*0.8), linked:stereo.every(row=>Math.abs(row.loudGain-row.probeGain)<0.005), release:stereo.every(({probeGain,recovery:[early,mid,late]})=>early>=probeGain*0.98 && early<mid && mid<late && late>quietGain*0.8 && late<=quietGain*1.05)};
    results.push({rate,latency,quietGain,stereo,checks});
  }
  return results;
}
(async()=>{
  const browser = await chromium.launch({executablePath:chrome,chromiumSandbox:true});
  try {
    const page = await browser.newPage();
    let candidateHash = null;
    if(candidate){const source = fs.readFileSync(candidate,'utf8'); candidateHash = hash(source); await page.evaluate(`Object.assign(globalThis, (${source})({activated:()=>false})); void 0`);}
    const results = await page.evaluate(controls);
    const receipt = {date:new Date().toISOString(), chrome, version:browser.version(), scriptHash:hash(fs.readFileSync(__filename)),candidateHash,results};
    if(candidate){const native=JSON.parse(fs.readFileSync(path.join(__dirname,'native.json'))); receipt.matchesNative=native.version===receipt.version && native.scriptHash===receipt.scriptHash && results.every((row,index)=>{const ref=native.results[index]; const gains=result=>[result.quietGain,...result.stereo.flatMap(side=>[side.loudGain,side.probeGain,...side.recovery])]; const baseline=gains(ref); return row.latency.every((value,i)=>value===ref.latency[i]) && gains(row).every((value,i)=>Math.abs(value-baseline[i])<=Math.max(0.005,0.05*Math.abs(baseline[i])));});}
    fs.writeFileSync(path.join(__dirname,candidate?'candidate-'+candidateHash+'.json':'native.json'),JSON.stringify(receipt,null,2)+'\n',{flag:'wx',mode:0o600});
    console.log(JSON.stringify(receipt)); if(receipt.matchesNative===false || results.some(row=>Object.values(row.checks).includes(false))) process.exitCode=1;
  } finally { await browser.close(); }
})().catch(error=>{console.error(error);process.exitCode=1});
