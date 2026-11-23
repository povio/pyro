const fs = require('node:fs');
const crypto = require('node:crypto');
const { chromium } = require('/private/tmp/pyro-r7-param/bench/node_modules/playwright-core');
const executablePath = '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';
const hash = bytes => crypto.createHash('sha256').update(bytes).digest('hex');
async function pcm() {
  const rate = 32768, frames = 160, start = 16, end = 32;
  const cases = [];
  for (const speed of [-1, 1]) for (const offset of [40, 23, 8]) {
    const ctx = new OfflineAudioContext(2, frames, rate);
    const buffer = ctx.createBuffer(2, 64, rate);
    for (let index = 0; index < buffer.length; index++) {
      buffer.getChannelData(0)[index] = (index + 1) / 128;
      buffer.getChannelData(1)[index] = -((index * 13) % 64 + 1) / 128;
    }
    const node = ctx.createBufferSource();
    node.buffer = buffer; node.loop = true; node.loopStart = start / rate; node.loopEnd = end / rate;
    node.playbackRate.value = speed; node.connect(ctx.destination); node.start(0, offset / rate);
    const output = await ctx.startRendering();
    cases.push({speed, offset, channels: [0,1].map(channel => Array.from(output.getChannelData(channel)))});
  }
  return {rate,frames,start,end,cases};
}
async function main() {
  const [label, modulePath] = process.argv.slice(2);
  const started = new Date().toISOString();
  const browser = await chromium.launch({executablePath, headless:true});
  try {
    const page = await browser.newPage();
    const source = modulePath && fs.readFileSync(modulePath, 'utf8');
    if (source) await page.evaluate(`globalThis.OfflineAudioContext = (${source})({activated:()=>false}).OfflineAudioContext; void 0`);
    const result = await page.evaluate(pcm);
    const finished = new Date().toISOString();
    const report = {label,started,finished,browser:browser.version(),executablePath,modulePath,moduleSha256:source && hash(source),probeSha256:hash(fs.readFileSync(__filename)),...result};
    if (source) {
      const native = JSON.parse(fs.readFileSync('native.json'));
      report.comparison = result.cases.map(({speed,offset,channels}, index) => ({speed,offset,mismatches: channels.map((samples,channel) => samples.filter((value,frame) => value !== native.cases[index].channels[channel][frame]).length)}));
    }
    fs.writeFileSync(`${label}.json`,JSON.stringify(report,null,2)+'\n',{flag:'wx'});
    console.log(label,started,finished,report.moduleSha256,report.comparison);
  } finally { await browser.close(); }
}
const deadline = setTimeout(()=>process.exit(124),30000);
main().catch(error=>{ console.error(error); process.exitCode=1; }).finally(()=>clearTimeout(deadline));
