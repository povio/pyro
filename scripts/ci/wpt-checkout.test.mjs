import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { test } from 'node:test';
import { pathToFileURL } from 'node:url';

function git(dir, ...args) {
  const result = spawnSync('git', ['-C', dir, ...args], { encoding: 'utf8' });
  assert.equal(result.status, 0, result.stderr);
  return result.stdout.trim();
}

function fixture(t) {
  const root = mkdtempSync(path.join(tmpdir(), 'pyro wpt-ci-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const upstream = path.join(root, 'upstream');
  const scripts = path.join(root, 'scripts');
  const runnerTemp = path.join(root, '_work', '_temp');
  const githubEnv = path.join(runnerTemp, 'env');
  for (const dir of ['first', 'second', 'tools/serve']) mkdirSync(path.join(upstream, dir), { recursive: true });
  mkdirSync(scripts);
  mkdirSync(runnerTemp, { recursive: true });
  writeFileSync(path.join(upstream, 'first/page.html'), 'first');
  writeFileSync(path.join(upstream, 'second/page.html'), 'second');
  writeFileSync(path.join(upstream, 'tools/serve/requirements.txt'), '');
  git(upstream, 'init', '-q');
  git(upstream, 'config', 'uploadpack.allowFilter', 'true');
  git(upstream, 'config', 'user.name', 'Test');
  git(upstream, 'config', 'user.email', 'test@example.com');
  git(upstream, 'add', '.');
  git(upstream, 'commit', '-qm', 'fixture');
  const rev = git(upstream, 'rev-parse', 'HEAD');
  const pin = path.join(scripts, 'wpt-revision.txt');
  const paths = path.join(scripts, 'wpt-paths.txt');
  writeFileSync(pin, `${rev}\n`);
  writeFileSync(paths, 'first\n');
  for (const name of ['fetch-wpt.sh', 'prepare-ci-wpt.sh']) {
    writeFileSync(path.join(scripts, name), readFileSync(new URL(`../../bench/wpt/${name}`, import.meta.url)), { mode: 0o755 });
  }
  function prepare({ mode = 'subset', runner = 'worker-1', source = upstream, fail = false } = {}) {
    writeFileSync(githubEnv, '');
    const result = spawnSync('sh', [path.join(scripts, 'prepare-ci-wpt.sh'), mode], {
      encoding: 'utf8',
      env: {
        ...process.env,
        RUNNER_TEMP: runnerTemp,
        RUNNER_NAME: runner,
        GITHUB_ENV: githubEnv,
        GITHUB_REPOSITORY: 'example/pyro',
        GIT_CONFIG_COUNT: '1',
        GIT_CONFIG_KEY_0: `url.${pathToFileURL(source)}.insteadOf`,
        GIT_CONFIG_VALUE_0: 'https://github.com/web-platform-tests/wpt.git',
        WPT_REVISION: 'not-the-pin',
        WPT_PYTHON: 'python3',
      },
    });
    const env = readFileSync(githubEnv, 'utf8').trimEnd();
    if (fail) {
      assert.notEqual(result.status, 0);
      assert.equal(env, '');
      return result.stdout;
    }
    assert.equal(result.status, 0, `${result.stdout}\n${result.stderr}`);
    assert.match(env, /^WPT_DIR=.+$/);
    return { dir: env.slice('WPT_DIR='.length), log: result.stdout };
  }
  return { prepare, rev, pin, paths, scripts, upstream, offline: path.join(root, 'missing-upstream') };
}

test('cold checkout is pinned and warm reuse works offline on the same registration', (t) => {
  const { prepare, rev, offline } = fixture(t);
  const cold = prepare();
  assert.match(cold.log, /Populating/);
  assert.equal(git(cold.dir, 'rev-parse', 'HEAD'), rev);
  assert.ok(existsSync(path.join(cold.dir, 'first/page.html')));
  assert.ok(!existsSync(path.join(cold.dir, 'second/page.html')));
  mkdirSync(path.join(cold.dir, 'leftovers'));
  const warm = prepare({ source: offline });
  assert.match(warm.log, /Reusing/);
  assert.equal(warm.dir, cold.dir);
  assert.ok(!existsSync(path.join(cold.dir, 'leftovers')));
  const other = prepare({ runner: 'worker-2' });
  assert.match(other.log, /Populating/);
  assert.notEqual(other.dir, warm.dir);
});

test('changes to sparse paths, fetch script, or revision invalidate reuse', (t) => {
  const { prepare, pin, paths, scripts, upstream } = fixture(t);
  const cold = prepare();
  writeFileSync(paths, 'second\n');
  const changed = prepare();
  assert.match(changed.log, /Populating/);
  assert.equal(changed.dir, cold.dir);
  assert.ok(!existsSync(path.join(changed.dir, 'first/page.html')));
  assert.ok(existsSync(path.join(changed.dir, 'second/page.html')));
  const fetch = path.join(scripts, 'fetch-wpt.sh');
  writeFileSync(fetch, `${readFileSync(fetch, 'utf8')}\n`);
  assert.match(prepare().log, /Populating/);
  git(upstream, 'commit', '--allow-empty', '-qm', 'second revision');
  const newRev = git(upstream, 'rev-parse', 'HEAD');
  writeFileSync(pin, `${newRev}\n`);
  assert.match(prepare().log, /Populating/);
  assert.equal(git(changed.dir, 'rev-parse', 'HEAD'), newRev);
});

test('failed refresh never exposes a checkout, and the next setup recovers', (t) => {
  const { prepare, offline } = fixture(t);
  const { dir } = prepare();
  writeFileSync(path.join(dir, 'first/page.html'), 'dirty');
  assert.match(prepare({ source: offline, fail: true }), /Populating/);
  assert.ok(!existsSync(dir));
  assert.ok(!existsSync(`${dir}.ready`));
  assert.match(prepare().log, /Populating/);
  assert.equal(readFileSync(path.join(dir, 'first/page.html'), 'utf8'), 'first');
});

test('full checkout retains a working venv at its final path', (t) => {
  const { prepare, offline } = fixture(t);
  const sparse = prepare();
  const full = prepare({ mode: 'full' });
  assert.notEqual(full.dir, sparse.dir);
  assert.ok(existsSync(path.join(full.dir, 'second/page.html')));
  const pip = spawnSync(path.join(full.dir, '.git/pyro-venv/bin/pip'), ['--version'], { encoding: 'utf8' });
  assert.equal(pip.status, 0, pip.stderr);
  assert.match(prepare({ mode: 'full', source: offline }).log, /Reusing/);
  rmSync(path.join(full.dir, '.git/pyro-venv/.pyro-req'));
  assert.match(prepare({ mode: 'full' }).log, /Populating/);
});
