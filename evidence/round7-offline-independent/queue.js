(async () => {
  const ctx = new OfflineAudioContext(1, 901, 32768), trace = [], settled = [], events = [];
  ctx.oncomplete = event => { events.push(event.renderedBuffer); trace.push(['complete', ctx.state]); };
  const enqueue = (id, size) => ctx.startRendering(size).then(buffer => {settled.push(id); trace.push([id, ctx.state]); return buffer;});
  const first = await enqueue('first', 2);
  const idle = [ctx.state, Math.round(ctx.currentTime * 32768), events.length];
  const middle = enqueue('middle', 255), last = enqueue('last', 999);
  const exhausted = await ctx.startRendering(1).then(() => 'fulfilled', err => err.name);
  const chunks = [first, ...await Promise.all([middle, last])];
  await new Promise(resolve => setTimeout(resolve, 10));
  return {idle, lengths: chunks.map(chunk => chunk.length), settled, exhausted, events: events.length, finalBuffer: events[0] === chunks[2], final: ctx.state, trace};
})()
