const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const [wptRoot, page] = process.argv.slice(2);
function run(file) {
  return vm.runInThisContext(fs.readFileSync(file, 'utf8'), { filename: file });
}

// Use the same production Streams factory and per-slot WeakMaps as the queue lane.
const factory = run('crates/pyro-webapi/src/streams.js');
const streams = factory({
  slot() {
    const map = new WeakMap();
    return { has: map.has.bind(map), get: map.get.bind(map), set: map.set.bind(map) };
  },
});
globalThis.self = globalThis;
for (const [name, value] of Object.entries(streams)) {
  if (/^[A-Z]/.test(name)) globalThis[name] = value;
}
run(path.join(wptRoot, 'resources/testharness.js'));
const timer = setTimeout(() => { throw new Error('Harness did not complete'); }, 30000);
add_completion_callback((tests, harness) => {
  clearTimeout(timer);
  if (harness.status !== 0 || tests.length === 0) throw new Error(harness.message || 'Missing tests');
  for (const test of tests) {
    const status = test.format_status();
    console.log(JSON.stringify({ page, name: test.name, status, message: test.message }));
    if (status !== 'Pass') process.exitCode = 1;
  }
});
const file = path.join(wptRoot, page);
for (const match of fs.readFileSync(file, 'utf8').matchAll(/^\/\/ META: script=(.+)$/gm)) {
  run(path.resolve(path.dirname(file), match[1]));
}
run(file);
