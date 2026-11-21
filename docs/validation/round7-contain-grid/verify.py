import gzip
import json
from pathlib import Path

root = Path(__file__).resolve().parent
prior = root.parent / 'round7-flex-width'
url = 'css/cssom-view/scrollWidthHeight-contain-layout.html'


def read(label, directory=root):
    return json.loads(gzip.decompress((directory / (label + '.json.gz')).read_bytes()))


def names(report):
    assert report['restarts'] == report['watchdogKills'] == 0
    assert report['wptRevision'] == 'a421f2fea5eb7f91e0db399e44b0fb0779ee1603'
    assert len(report['results']) == 10
    assert all(r['harnessStatus'] == 0 and r['status'] == 'complete' for r in report['results'])
    return {(r['url'], t['name']): t['status'] for r in report['results'] for t in r['subtests']}


base = names(read('base'))
assert len(base) == 958 and base == names(read('candidate', prior))
chrome = read('chrome-wpt')
assert chrome == read('chrome-wpt', prior)
py, ch = [read(engine + '-trace') for engine in ['pyro', 'chrome']]
assert [(name, status) for name, status, _ in chrome[url][1]] == [tuple(row) for row in ch['ordered']['names']]
assert all(base[url, name] == ['Pass', 'Fail'][status] for name, status in py['ordered']['names'])
for mode in ['ordered','fresh','static']:
    assert len(py[mode]['rows']) == len(ch[mode]['rows']) == 128
    for index, (p, c) in enumerate(zip(py[mode]['rows'], ch[mode]['rows'])):
        assert all(p[k] == c[k] for k in ['display','padding','border','overflow'])
        if p['display'] != 'grid' or p['overflow'] == 'clip':
            continue
        pad = p['padding'] == '2px'
        assert p['before'] == ['visible'] * 2 and p['after'] == c['after'] == [p['overflow']] * 2
        assert c['before'] == ([p['overflow']] if pad and mode == 'ordered' else ['visible']) * 2
        assert p['visibleW'] == p['scrollableW'] == c['visibleW'] == c['scrollableW'] == (4 if pad else 0)
        assert (p['visibleH'],p['scrollableH']) == ((4,2004) if pad else (0,2000))
        assert (c['visibleH'],c['scrollableH']) == ((2004 if mode == 'ordered' else 4,2004) if pad else (0,0))
        assert py[mode]['names'][index][0] == ch[mode]['names'][index][0]
        if mode == 'ordered':
            print('carryover' if pad else 'fresh discrepancy', py[mode]['names'][index][0])
for engine, label in [('pyro','candidate'),('chrome','chrome')]:
    probe = read(engine + '-probe')
    assert len(probe) == 810 and probe == read(label + '-probe', prior)
edges, reference = read('pyro-edges'), read('chrome-edges')
assert len(edges) == len(reference) == 96
for p, c in zip(edges, reference):
    assert p[:-2] == c[:-2]
    _, content, width, pad, overflow = p[:-2]
    assert (p != c) == (content != 'wide-item' and width == 0 and pad != '0 1px' and overflow == 'hidden')
