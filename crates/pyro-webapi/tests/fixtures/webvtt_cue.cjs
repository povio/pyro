const {test} = require('node:test');
const assert = require('node:assert/strict');
const {readFileSync} = require('node:fs');
const {resolve} = require('node:path');
const vm = require('node:vm');

function node(type, value) {
  return {type, value, children: [], attributes: {}, appendChild(child) { this.children.push(child); },
    setAttribute(key, v) { this.attributes[key] = v; }};
}
const document = {
  createDocumentFragment: () => node('fragment'),
  createElementNS: (ns, name) => node(name, ns),
  createTextNode: text => node('text', text),
  createProcessingInstruction: (name, value) => node(name, value)
};
const realm = vm.createContext({document, EventTarget, DOMException});
const factory = vm.runInContext(readFileSync(resolve(__dirname, '../../src/webvtt_cue.js'), 'utf8'), realm);
const api = factory(s => s.replaceAll('&amp;', '&'), () => false);

test('cue text tokens construct fresh nested fragments and normalized timestamps', () => {
  const cue = new api.VTTCue(2, Infinity,
    '<v.actor Alice &amp; Bob><lang fr><ruby>aller<rt>go</ruby></lang></v><01:02.003><rt>A</rt><x>B</x>');
  const fragment = cue.getCueAsHTML();
  const [voice, stamp, a, b] = fragment.children;
  assert.deepEqual(voice.attributes, {class: 'actor', title: 'Alice & Bob'});
  assert.deepEqual(voice.children[0].attributes, {lang: 'fr'});
  assert.equal(voice.children[0].children[0].children[1].type, 'rt');
  assert.equal(stamp.type, 'timestamp');
  assert.equal(stamp.value, '00:01:02.003');
  assert.equal(a.value + b.value, 'AB');
  assert.notEqual(fragment, cue.getCueAsHTML());
});

test('parsed cues retain region and track identity while script changes timing', () => {
  const region = api.parsedRegion({id: 'main', width: 56});
  const cue = api.parsedCue({id: 'line', startTime: 2, endTime: 9, text: 'HI',
    settings: {region: 0, align: 'start'}}, [region]);
  assert.equal(cue.region, region);
  const track = {};
  api.setCueOwner(cue, track);
  cue.startTime = 4;
  assert.equal(cue.track, track);
  assert.equal(api.cueTiming(cue)[0], 4);
  assert.throws(() => { region.width = 101; }, {name: 'IndexSizeError'});
  assert.equal(region.width, 56);
  cue.align = 'bogus';
  assert.equal(cue.align, 'start');
  cue.line = -20;
  assert.equal(cue.line, -20);
  assert.throws(() => { cue.endTime = NaN; }, {name: 'TypeError'});
});

test('cross-realm region assignment accepts a genuine foreign region and rejects forged instances', () => {
  const otherRealm = vm.createContext({document, EventTarget, DOMException});
  const otherFactory = vm.runInContext(readFileSync(resolve(__dirname, '../../src/webvtt_cue.js'), 'utf8'), otherRealm);
  const other = otherFactory(s => s, api.localState);
  const childCue = new other.VTTCue(0, 2, 'foreign');
  const parentRegion = new api.VTTRegion();
  childCue.region = parentRegion;
  assert.equal(childCue.region, parentRegion);
  const parentCue = new api.VTTCue(1, 3, 'parent');
  const track = {};
  other.setCueOwner(parentCue, track);
  assert.equal(parentCue.track, track);
  const id = Object.getOwnPropertyDescriptor(other.TextTrackCue.prototype, 'id');
  id.set.call(parentCue, 'updated');
  assert.equal(parentCue.id, 'updated');
  assert.throws(() => id.get.call(Object.create(api.VTTCue.prototype)), {name: 'TypeError'});
  const forged = Object.create(api.VTTRegion.prototype);
  assert.throws(() => { childCue.region = forged; }, {name: 'TypeError'});
  assert.equal(childCue.region, parentRegion);
  childCue.pauseOnExit = true;
  assert.equal(other.cueTiming(childCue)[3], true);
});
