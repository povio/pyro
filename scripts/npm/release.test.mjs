import assert from 'node:assert/strict';
import test from 'node:test';
import { releaseInfo } from './release.mjs';

test('stable versions go to latest; release candidates go to next', () => {
  assert.deepEqual(releaseInfo('v0.0.17', '0.0.17', '0.0.17'), { version: '0.0.17', distTag: 'latest' });
  for (const version of ['0.0.18-rc', '0.0.18-rc.0', '0.0.18-rc.12']) {
    assert.deepEqual(releaseInfo(`v${version}`, version, version), { version, distTag: 'next' });
  }
});

test('malformed tags and other prereleases cannot publish to latest', () => {
  for (const tag of ['0.0.17', 'v01.0.0', 'v1.2', 'v1.2.3-beta.1', 'v1.2.3-rc.01', 'v1.2.3+build', 'v1.2.3\n']) {
    assert.throws(() => releaseInfo(tag, '1.2.3', '1.2.3'));
  }
});

test('a tag cannot publish a different package or browser version', () => {
  assert.throws(() => releaseInfo('v1.2.3', '1.2.2', '1.2.3'), /must match/);
  assert.throws(() => releaseInfo('v1.2.3-rc.1', '1.2.3-rc.1', '1.2.3'), /must match/);
});
