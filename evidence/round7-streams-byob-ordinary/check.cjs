// Usage: run.cjs <streams.js> <wptRoot> <page> [allowlist.json]
// Runs only tests whose names are in the allowlist (others are never registered).
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const [src, wptRoot, page, allowFile] = process.argv.slice(2);
const allow = allowFile ? new Set(JSON.parse(fs.readFileSync(allowFile, 'utf8'))[page] || []) : null;
const run = (file) => vm.runInThisContext(fs.readFileSync(file, 'utf8'), { filename: file });
const factory = run(src);
const streams = factory({ slot() { const m = new WeakMap(); return { has: m.has.bind(m), get: m.get.bind(m), set: m.set.bind(m) }; } });
globalThis.self = globalThis;
for (const [n, v] of Object.entries(streams)) if (/^[A-Z]/.test(n)) globalThis[n] = v;
run(path.join(wptRoot, 'resources/testharness.js'));
if (allow) for (const k of ['test', 'promise_test', 'async_test']) {
  const orig = globalThis[k];
  globalThis[k] = function (fn, name, ...rest) { if (allow.has(name)) return orig.call(this, fn, name, ...rest); };
}
const timer = setTimeout(() => { console.error('Harness did not complete'); process.exit(2); }, 30000);
add_completion_callback((tests, harness) => {
  clearTimeout(timer);
  for (const t of tests) console.log(JSON.stringify({ page, name: t.name, status: t.format_status(), message: t.message }));
  if (harness.status !== 0) console.log(JSON.stringify({ page, harness: harness.status, message: harness.message }));
});
const file = path.join(wptRoot, page);
for (const m of fs.readFileSync(file, 'utf8').matchAll(/^\/\/ META: script=(.+)$/gm)) run(path.resolve(path.dirname(file), m[1]));
run(file);
