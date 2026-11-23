import { test } from 'node:test';
import assert from 'node:assert/strict';
import { plan } from './plan.mjs';

test('client, docs, baselines and ordinary Rust edits do not trigger release-toolchain builds', () => {
  for (const p of ['website/index.html', 'packages/pyro/src/index.ts', 'bench/wpt/baseline.json', 'crates/pyro-dom/src/lib.rs']) {
    assert.deepEqual(plan([p]), { 'linux-check': false, 'pgo-check': false });
  }
});
test('Linux integration changes retain both-architecture packaging and client coverage', () => {
  for (const p of ['scripts/linux/setup.sh', 'scripts/release/bundle-icu-linux.sh', 'crates/pyro-jsc-sys/build.rs']) {
    assert.deepEqual(plan([p]), { 'linux-check': true, 'pgo-check': false });
  }
});
test('JSC and PGO changes select optimized Linux builds as well as macOS PGO', () => {
  for (const p of ['crates/pyro-jsc-sys/webkit/patches/0001.patch', 'scripts/jsc/build.sh', 'scripts/pgo/train.sh',
    'crates/pyro-cdp/src/pgo_training.rs', '.github/actions/pgo-build/action.yml', '.github/workflows/jsc-linux.yml']) {
    assert.deepEqual(plan([p]), { 'linux-check': true, 'pgo-check': true });
  }
});
test('orchestration changes exercise the complete build graph', () => {
  for (const p of ['.github/workflows/wpt.yml', 'scripts/ci/plan.mjs', 'scripts/ci/plan.test.mjs']) {
    assert.deepEqual(plan([p]), { 'linux-check': true, 'pgo-check': true });
  }
});
