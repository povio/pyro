(async () => {
  const rate = 32768, ctx = new OfflineAudioContext(1, 65539, rate);
  const trace = [], pauses = [];
  const record = label => trace.push([label, ctx.state, Math.round(ctx.currentTime * rate)]);
  let events = 0, eventBuffer;
  ctx.onstatechange = () => record('statechange');
  const complete = new Promise(resolve => {
    ctx.oncomplete = event => { events++; eventBuffer = event.renderedBuffer; record('complete'); resolve(); };
  });
  const early = ctx.suspend(257 / rate), late = ctx.suspend(32768 / rate);
  record('before');
  const render = ctx.startRendering().then(buffer => { record('render fulfilled'); return buffer; });
  record('start returned');
  Promise.resolve().then(() => record('microtask'));
  const timer = new Promise(resolve => setTimeout(() => { record('timer'); resolve(); }, 0));
  for (const pause of [early, late]) {
    await pause;
    pauses.push([ctx.state, Math.round(ctx.currentTime * rate)]);
    await ctx.resume();
    record('resume fulfilled');
  }
  const [output] = await Promise.all([render, complete, timer]);
  await new Promise(resolve => setTimeout(resolve, 0));
  return {pauses, events, sameBuffer: output === eventBuffer, length: output.length, final: ctx.state, trace};
})()
