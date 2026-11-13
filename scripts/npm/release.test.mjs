import assert from 'node:assert/strict';
import test from 'node:test';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { execFileSync } from 'node:child_process';
import { releaseInfo, stampRelease } from './release.mjs';

test('stable versions go to latest; release candidates go to next', () => {
  assert.deepEqual(releaseInfo('v0.0.17'), { version: '0.0.17', distTag: 'latest' });
  for (const version of ['0.0.18-rc', '0.0.18-rc.0', '0.0.18-rc.12']) {
    assert.deepEqual(releaseInfo(`v${version}`), { version, distTag: 'next' });
  }
});

test('malformed tags and other prereleases cannot publish to latest', () => {
  for (const tag of ['0.0.17', 'v01.0.0', 'v1.2', 'v1.2.3-beta.1', 'v1.2.3-rc.01', 'v1.2.3+build', 'v1.2.3\n']) {
    assert.throws(() => releaseInfo(tag));
  }
});

function fixture() {
  const root = mkdtempSync(join(tmpdir(), 'pyro-release-'));
  mkdirSync(join(root, 'packages/pyro'), { recursive: true });
  mkdirSync(join(root, 'crates/browser/src'), { recursive: true });
  mkdirSync(join(root, 'vendor/dependency/src'), { recursive: true });
  writeFileSync(join(root, 'packages/pyro/package.json'), JSON.stringify({ name: '@povio/pyro', version: '0.0.1', exports: './dist/index.js' }));
  writeFileSync(join(root, 'Cargo.toml'), '[workspace]\nmembers = ["crates/*"]\nexclude = ["vendor/dependency"]\nresolver = "2"\n\n[workspace.package]\nversion = "0.0.2" # development version\nedition = "2021"\n\n[workspace.dependencies]\nexample-dependency = { path = "vendor/dependency", version = "0.0.2" }\n');
  writeFileSync(join(root, 'crates/browser/Cargo.toml'), '[package]\nname = "browser"\nversion.workspace = true\nedition.workspace = true\n\n[dependencies]\nexample-dependency.workspace = true\n');
  writeFileSync(join(root, 'vendor/dependency/Cargo.toml'), '[package]\nname = "example-dependency"\nversion = "0.0.2"\nedition = "2021"\n');
  writeFileSync(join(root, 'crates/browser/src/lib.rs'), '');
  writeFileSync(join(root, 'vendor/dependency/src/lib.rs'), '');
  return root;
}

for (const version of ['1.2.3', '2.0.0-rc.4']) {
  test(`stamps ${version} and updates the workspace lock without changing dependencies`, () => {
    const root = fixture();
    const cargo = args => execFileSync('cargo', args, { cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] });
    try {
      cargo(['generate-lockfile', '--offline']);
      const before = readFileSync(join(root, 'Cargo.lock'), 'utf8');
      stampRelease(`v${version}`, root);
      const pkg = JSON.parse(readFileSync(join(root, 'packages/pyro/package.json'), 'utf8'));
      assert.equal(pkg.version, version);
      assert.equal(pkg.exports, './dist/index.js');
      const manifest = readFileSync(join(root, 'Cargo.toml'), 'utf8');
      assert.ok(manifest.includes(`version = "${version}" # development version`));
      assert.ok(manifest.includes('version = "0.0.2" }'));
      cargo(['update', '--workspace', '--offline']);
      const lock = readFileSync(join(root, 'Cargo.lock'), 'utf8');
      assert.equal(lock, before.replace('name = "browser"\nversion = "0.0.2"', `name = "browser"\nversion = "${version}"`));
      const metadata = JSON.parse(cargo(['metadata', '--locked', '--offline', '--format-version', '1']));
      assert.equal(metadata.packages.find(pkg => pkg.name === 'browser').version, version);
      stampRelease(`v${version}`, root);
      assert.equal(readFileSync(join(root, 'Cargo.toml'), 'utf8'), manifest);
    } finally { rmSync(root, { recursive: true, force: true }); }
  });
}

test('invalid tags or missing workspace version fail before changing manifests', () => {
  const root = fixture();
  try {
    const packagePath = join(root, 'packages/pyro/package.json');
    const cargoPath = join(root, 'Cargo.toml');
    const pkg = readFileSync(packagePath, 'utf8');
    const cargo = readFileSync(cargoPath, 'utf8');
    assert.throws(() => stampRelease('v1.0.0-beta.1', root));
    assert.equal(readFileSync(packagePath, 'utf8'), pkg);
    assert.equal(readFileSync(cargoPath, 'utf8'), cargo);
    writeFileSync(cargoPath, '[workspace]\nmembers = []\n');
    assert.throws(() => stampRelease('v1.0.0', root), /exactly one version/);
    assert.equal(readFileSync(packagePath, 'utf8'), pkg);
  } finally { rmSync(root, { recursive: true, force: true }); }
});
