const test = require('node:test');
const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');

const parse = eval(readFileSync(join(__dirname, '../../src/webvtt_file.js'), 'utf8'));
const transcript = readFileSync(join(__dirname, 'transcript.vtt'), 'utf8');

test('transcript yields cue metadata, source markup, region and stylesheet', () => {
  const { cues, regions, styles } = parse(transcript);
  assert.deepEqual(regions, [{
    id: 'captions', width: 80, lines: 2, regionAnchorX: 0, regionAnchorY: 100,
    viewportAnchorX: 10, viewportAnchorY: 95, scroll: '',
  }]);
  assert.deepEqual(styles, ['::cue { color: white; }']);
  assert.deepEqual(cues, [
    { id: 'intro', startTime: 0.2, endTime: 2.45, text: 'Welcome to the session.', settings: { region: 0 } },
    { id: 'speaker', startTime: 2.45, endTime: 5.5, text: "<v Ada>Let's begin.",
      settings: { line: 85, lineAlign: 'center', snapToLines: false, align: 'start' } },
  ]);
});

test('two cues referencing the same region resolve to one descriptor', () => {
  const sameRegion = transcript + '\nclosing\n00:05.500 --> 00:06.000 region:captions\nGoodbye.';
  const { cues, regions } = parse(sameRegion);
  assert.equal(cues.length, 3);
  assert.equal(cues[2].id, 'closing');
  assert.equal(cues[2].settings.region, 0);
  assert.strictEqual(regions[cues[0].settings.region], regions[cues[2].settings.region]);
});

test('complete files normalize a leading BOM and CRLF or CR line endings', () => {
  const expected = parse(transcript);
  for (const newline of ['\r\n', '\r']) {
    assert.deepEqual(parse('\uFEFF' + transcript.split('\n').join(newline)), expected);
  }
  assert.equal(parse('\uFEFF\uFEFF' + transcript), null);
  assert.equal(parse('WEBVTTX\n'), null);
});

test('adjacent timing lines split cues and preserve multiline payloads', () => {
  const { cues } = parse('WEBVTT\n\n00:00.000 --> 00:01.000\nfirst\nline two\n00:01.000 --> 00:02.000\nlast');
  assert.deepEqual(cues.map(cue => [cue.id, cue.startTime, cue.endTime, cue.text]), [
    ['', 0, 1, 'first\nline two'], ['', 1, 2, 'last'],
  ]);
});
