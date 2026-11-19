import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

const wrapper = fileURLToPath(new URL('../../.github/actions/pgo-build/build.sh', import.meta.url));
const targetProfile = 'target/aarch64-apple-darwin/release-pgo';

function fixture(t) {
  const workspace = mkdtempSync(path.join(tmpdir(), 'pyro pgo-ci-'));
  t.after(() => rmSync(workspace, { recursive: true, force: true }));
  const scripts = path.join(workspace, 'scripts/pgo');
  mkdirSync(scripts, { recursive: true });
  mkdirSync(path.join(workspace, '.ci/llvm-pgo'), { recursive: true });
  const phases = {
    generate: `
[[ "$1" == --locked ]]
mkdir -p ${targetProfile}/build target/release-pgo/build "$PYRO_WEBKIT_SRC" \
  "$PYRO_PGO_DIR"/{webkit-build-generate,jsc-generate/key,jsc-generate/incomplete.tmp,profraw}
touch "$PYRO_PGO_DIR/jsc-generate/key/archive.a"
echo instrumented > ${targetProfile}/pyro
`,
    train: `
[[ -f ${targetProfile}/pyro && ! -e "$PYRO_PGO_DIR/webkit-build-generate" ]]
touch "$PYRO_PGO_DIR"/{merged.profdata,part-cdp-plain.profdata,train.log}
`,
    use: `
[[ "$1" == --locked && -f "$PYRO_PGO_DIR/merged.profdata" && -d "$PYRO_WEBKIT_SRC" ]]
[[ ! -e ${targetProfile}/pyro && ! -e ${targetProfile}/build ]]
[[ ! -e "$PYRO_PGO_DIR/profraw" && ! -e "$PYRO_PGO_DIR/part-cdp-plain.profdata" ]]
mkdir -p ${targetProfile}/deps "$PYRO_PGO_DIR"/{webkit-build-use-lto,jsc-use-lto}
touch "$PYRO_PGO_DIR/use-example.profdata"
case "$FAIL_USE" in fail) exit 42 ;; interrupt) kill -INT "$PPID"; exit 0 ;; esac
echo optimized > ${targetProfile}/pyro
`,
  };
  for (const [name, body] of Object.entries(phases)) {
    writeFileSync(path.join(scripts, `${name}.sh`), `#!/usr/bin/env bash\nset -eu\n${body}`, { mode: 0o755 });
  }
  return { workspace, run(arg, failUse) {
    return spawnSync('bash', [wrapper, arg], {
      encoding: 'utf8', env: { ...process.env, GITHUB_WORKSPACE: workspace, FAIL_USE: failUse },
    });
  } };
}

for (const [mode, expectedStatus] of [['', 0], ['fail', 42], ['interrupt', 130]]) {
  test(`PGO CI cleans job scratch on ${mode || 'success'} and preserves cache inputs`, (t) => {
    const { workspace, run } = fixture(t);
    const result = run('--locked', mode);
    assert.equal(result.status, expectedStatus, `${result.stdout}\n${result.stderr}`);
    const binary = path.join(workspace, targetProfile, 'pyro');
    for (const afterFallback of [false, true]) {
      if (afterFallback) assert.equal(run('--cleanup', mode).status, 0);
      for (const dir of ['.ci/llvm-pgo', 'target/pgo/jsc-generate/key/archive.a', 'target/pgo/train.log']) {
        assert.ok(existsSync(path.join(workspace, dir)), dir);
      }
      for (const scratch of ['webkit-src', 'webkit-build-use-lto', 'profraw', 'merged.profdata',
        'part-cdp-plain.profdata', 'use-example.profdata', 'jsc-use-lto', 'jsc-generate/incomplete.tmp']) {
        assert.ok(!existsSync(path.join(workspace, 'target/pgo', scratch)), scratch);
      }
      assert.ok(!existsSync(path.join(workspace, targetProfile, 'deps')));
      assert.ok(!existsSync(path.join(workspace, 'target/release-pgo/build')));
      if (expectedStatus === 0) assert.equal(readFileSync(binary, 'utf8'), 'optimized\n');
      else assert.ok(!existsSync(binary));
    }
  });
}
