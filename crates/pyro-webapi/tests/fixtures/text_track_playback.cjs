const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

class Element extends EventTarget { getAttribute() { return null; } }
class HTMLMediaElement extends Element {
  position = 0;
  get currentTime() { return this.position; }
}
function createRealm(peers, tasks) {
  const scope = vm.createContext({Element, HTMLMediaElement, EventTarget, Event, DOMException});
  const factory = name => vm.runInContext(fs.readFileSync(path.join(__dirname, '../../src', name + '.js'), 'utf8'), scope);
  const api = factory('text_tracks')(factory('webvtt_cue'), factory('webvtt_file'))({
    foreignState(kind, obj) {
      for (const peer of peers) {
        const state = peer.localState(kind, obj);
        if (state) return state;
      }
    },
    decodeCueText: s => s, children: () => [], postTask: task => tasks.push(task),
    fire: (target,event) => target.dispatchEvent(event)
  });
  peers.push(api);
  return api;
}
test('cue and region ownership across independent factories requires genuine brands', () => {
  const peers = [], a = createRealm(peers, []), b = createRealm(peers, []);
  const ta = a.addTextTrack(new HTMLMediaElement(), 'metadata');
  const tb = b.addTextTrack(new HTMLMediaElement(), 'metadata');
  const cue = new a.VTTCue(1, 2, 'cue'), region = new b.VTTRegion();
  cue.region = region;
  assert.equal(cue.region, region);
  tb.addCue(cue);
  assert.equal(cue.track, tb);
  ta.addCue(cue);
  assert.equal(ta.cues[0], cue);
  assert.equal(tb.cues.length, 0);
  ta.removeCue(cue);
  assert.equal(cue.track, null);
  assert.throws(() => tb.addCue(Object.create(a.VTTCue.prototype)), {name:'TypeError'});
  assert.throws(() => { cue.region = Object.create(b.VTTRegion.prototype); }, {name:'TypeError'});
});

test('normal cue crossings order both tracks before change and honor pause only on playback', () => {
  const tasks = [], api = createRealm([], tasks);
  const media = new HTMLMediaElement(), log = [];
  for (const name of ['A','B']) {
    const t = api.addTextTrack(media,'metadata');
    const cue = new api.VTTCue(.2,.55,name);
    cue.pauseOnExit = name === 'A';
    t.addCue(cue);
    cue.onenter = () => log.push(name + ' enter');
    cue.onexit = () => log.push(name + ' exit');
    t.oncuechange = () => log.push(name + ' change:' + t.activeCues.length);
  }
  const advance = (time,normal) => {
    media.position = time;
    const pause = api.advancePlayback(media,normal);
    while (tasks.length) tasks.shift()();
    return pause;
  };
  assert.equal(api.nextPause(media,0),.55);
  assert.equal(advance(.2,true),false);
  assert.equal(advance(.55,true),true);
  assert.deepEqual(log,['A enter','B enter','A change:1','B change:1','A exit','B exit','A change:0','B change:0']);
  assert.equal(advance(.3,false),false);
  assert.equal(advance(.8,false),false);
});
