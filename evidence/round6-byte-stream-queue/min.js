(async () => {
  const reader = new ReadableStream({type:'bytes', start(c) { c.close(); }}).getReader({mode:'byob'});
  for (const min of [2 ** 53, 3, 1.8, 1n]) {
    try { console.log('done', (await reader.read(new Uint8Array(2), {min})).done); }
    catch (e) { console.log(e.name); }
  }
})();
