(async () => {
  const rows = [];
  for (const length of [127, 128, 129, 4096, 65539]) {
    const ctx = new OfflineAudioContext(2, length, 32768);
    const buffer = ctx.createBuffer(2, length, ctx.sampleRate);
    const sample = (channel, frame) => ((frame * (channel ? 7 : 3) % 31) - 15) / 32;
    for (let channel = 0; channel < 2; channel++) {
      const input = buffer.getChannelData(channel);
      for (let frame = 0; frame < length; frame++) input[frame] = sample(channel, frame);
    }
    const source = ctx.createBufferSource(), gain = ctx.createGain();
    source.buffer = buffer;
    gain.gain.value = 0.5;
    gain.gain.setValueAtTime(0.25, 128 / ctx.sampleRate);
    source.connect(gain).connect(ctx.destination);
    source.start(3 / ctx.sampleRate);
    const sizes = [1, 129, 1025];
    const outputs = [];
    let committed = 0;
    for (const size of sizes) {
      if (committed >= length) break;
      outputs.push(ctx.startRendering(size));
      committed += Math.ceil(size / 128) * 128;
    }
    if (committed < length) outputs.push(ctx.startRendering());
    const chunks = await Promise.all(outputs);
    let mismatches = 0, first = null;
    for (let channel = 0; channel < 2; channel++) {
      const data = chunks.flatMap(chunk => Array.from(chunk.getChannelData(channel)));
      for (let frame = 0; frame < length; frame++) {
        const expected = frame < 3 ? 0 : sample(channel, frame - 3) * (frame < 128 ? 0.5 : 0.25);
        if (data[frame] !== expected) {
          mismatches++;
          first ??= {channel, frame, expected, actual: data[frame]};
        }
      }
    }
    rows.push({length, chunks: chunks.map(output => output.length), stereoAndRate: chunks.every(output => output.numberOfChannels === 2 && output.sampleRate === ctx.sampleRate), mismatches, first});
  }
  return rows;
})()
