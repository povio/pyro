import { spawn } from 'node:child_process';
import { mkdtempSync, rmSync, readFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { once } from 'node:events';

async function controls(api, injected) {
  const rate = 32768;
  async function render(quantum, scale, delay) {
    const length = 4 * quantum + 9;
    const started = performance.now();
    const context = new api.OfflineAudioContext({numberOfChannels:1, length, sampleRate:rate, renderSizeHint:quantum});
    const source = context.createBufferSource();
    const buffer = context.createBuffer(1, length, rate);
    const input = frame => ((frame % 13) - 6) / 8;
    const data = buffer.getChannelData(0);
    for (let frame = 0; frame < length; frame++) data[frame] = input(frame);
    source.buffer = buffer;
    const gain = context.createGain();
    gain.gain.automationRate = 'k-rate';
    gain.gain.setValueAtTime(scale, 0);
    gain.gain.setValueAtTime(-scale, (quantum - 2) / rate);
    gain.gain.setValueAtTime(scale / 2, (2 * quantum + 1) / rate);
    source.connect(gain);
    if (delay) {
      const node = context.createDelay(1);
      node.delayTime.value = delay / rate;
      gain.connect(node).connect(context.destination);
    } else gain.connect(context.destination);
    source.start();
    const output = await context.startRendering();
    const elapsedMs = performance.now() - started;
    const identity = [context.constructor === api.OfflineAudioContext, gain.constructor === api.GainNode, output.constructor === api.AudioBuffer, (api.OfflineAudioContext !== globalThis.OfflineAudioContext) === injected];
    const pcm = output.getChannelData(0);
    let mismatches = 0, first = null;
    for (let frame = 0; frame < length; frame++) {
      const index = frame - delay;
      const expected = index < 0 ? 0 : input(index) * (index < quantum ? scale : index < 3 * quantum ? -scale : scale / 2);
      if (!(Math.abs(pcm[frame] - expected) <= 1e-6)) {
        mismatches++; first ??= {frame, expected, actual:pcm[frame]};
      }
    }
    return {quantum, observed:context.renderQuantumSize, delay, length, returned:pcm.length, elapsedMs, identity, pcm:Array.from(pcm), mismatches, first, pass:identity.every(Boolean) && context.renderQuantumSize === quantum && length === pcm.length && !mismatches};
  }
  return Promise.all([render(127, 0.5, 0), render(129, 0.25, 0), render(128, 1, 32), render(127, 0.5, 131), render(129, 0.25, 32)]);
}

const profile = mkdtempSync('/private/tmp/pcm-chrome.');
const browser = spawn('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', ['--headless','--remote-debugging-pipe','--disable-background-networking','--no-first-run','--no-default-browser-check',`--user-data-dir=${profile}`,'about:blank'], {stdio:['ignore','ignore','ignore','pipe','pipe']});
const exited = once(browser,'exit');
const pending = new Map();
let id = 0, text = '';
browser.stdio[4].setEncoding('utf8').on('data', chunk => {
  const records = (text + chunk).split('\0'); text = records.pop();
  for (const record of records) {
    const msg = JSON.parse(record); pending.get(msg.id)?.(msg); pending.delete(msg.id);
  }
});
function call(method, params = {}, sessionId) {
  const key = ++id;
  return new Promise((resolve, reject) => {
    pending.set(key, msg => msg.error ? reject(Error(msg.error.message)) : resolve(msg.result));
    browser.stdio[3].write(JSON.stringify({id:key, method, params, sessionId}) + '\0');
  });
}
async function main() {
  const {product} = await call('Browser.getVersion');
  const {targetId} = await call('Target.createTarget',{url:'about:blank'});
  const {sessionId} = await call('Target.attachToTarget',{targetId,flatten:true});
  const path = process.argv[2];
  const source = path && readFileSync(path,'utf8');
  const sha256 = source && createHash('sha256').update(source).digest('hex');
  const factory = source ? `(${source}\n)({activated:()=>true,timer:setTimeout.bind(globalThis),clearTimer:clearTimeout.bind(globalThis)})` : 'globalThis';
  const started = performance.now();
  const out = await call('Runtime.evaluate',{expression:`(${controls})(${factory},${Boolean(path)})`,awaitPromise:true,returnByValue:true},sessionId);
  console.log(JSON.stringify({product,path:path ?? null,sha256:sha256 ?? null, evaluationMs:performance.now()-started, raw:out}));
  if (out.exceptionDetails) throw Error(out.exceptionDetails.exception?.description ?? out.exceptionDetails.text);
  if (!out.result.value.every(item => item.pass)) process.exitCode = 1;
}
const timer = setTimeout(() => browser.kill('SIGKILL'), 20000);
try { await Promise.race([main(), exited.then(() => {throw Error('Chrome exited before results');})]); }
finally { browser.kill('SIGTERM'); await exited; clearTimeout(timer); rmSync(profile,{recursive:true,force:true}); }
