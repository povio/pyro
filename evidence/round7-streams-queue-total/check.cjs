const fs = require('node:fs');
const assert = require('node:assert/strict');
const makeStreams = eval(fs.readFileSync('crates/pyro-webapi/src/streams.js','utf8'));
const {ReadableStream, WritableStream} = makeStreams({slot() {
  const m = new WeakMap();
  return {has:m.has.bind(m),get:m.get.bind(m),set:m.set.bind(m)};
}});
const assert_equals = assert.strictEqual;
const cases = [];
for (const kind of ['readable','writable']) {
  const promise_test = (fn,name) => cases.push({kind,name,fn});
  eval(fs.readFileSync(process.argv[2]+'/streams/'+kind+'-streams/floating-point-total-queue-size.any.js','utf8'));
}
(async () => { for (const {kind,name,fn} of cases) {
 try { await fn(); console.log(JSON.stringify({kind,name,status:'Pass'})); }
 catch(e) {console.log(JSON.stringify({kind,name,status:'Fail',error:e.message}));process.exitCode=1}
}})();
