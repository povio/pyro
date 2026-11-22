import collections
import gzip
import json
from pathlib import Path

root = Path(__file__).resolve().parent


def read(name):
    with gzip.open(root / (name + '.json.gz'), 'rt') as stream:
        return json.load(stream)


urls = json.loads((root / 'urls.json').read_text())


def outcomes(label):
    report = read(label)
    assert report['wptRevision'] == 'a421f2fea5eb7f91e0db399e44b0fb0779ee1603'
    assert report['watchdogKills'] == report['restarts'] == 0
    assert sorted(row['url'] for row in report['results']) == urls
    names = {}
    for row in report['results']:
        assert row['status'] == 'complete' and row['harnessStatus'] == 0
        for sub in row['subtests']:
            key = row['url'], sub['name']
            assert key not in names
            names[key] = sub['status']
    return names


base, candidate = outcomes('base'), outcomes('candidate')
assert outcomes('base-repeat') == base and outcomes('candidate-repeat') == candidate
chrome = {}
for url, (status, tests) in read('chrome-wpt').items():
    assert status == 0
    for name, status, _ in tests:
        chrome[url, name] = ['Pass', 'Fail'][status]
assert base.keys() == candidate.keys() == chrome.keys()
changes = collections.Counter((base[k], candidate[k], chrome[k]) for k in base if base[k] != candidate[k])
print('baseline / candidate / signed Chrome:', changes)
assert changes == {('Fail', 'Pass', 'Pass'): 12}
assert all(url.endswith('/scrollWidthHeight-negative-margin-002.html') and name.startswith('scrollWidth ')
           for (url, name), value in base.items() if value != candidate[url, name])
for label, values in [('base', base), ('candidate', candidate)]:
    print(label, sum(v == 'Pass' for v in values.values()), '/', len(values), 'strict pages',
          sum(all(v == 'Pass' for (page, _), v in values.items() if page == url) for url in urls))
before, after, reference = [read(label + '-probe') for label in ['base', 'candidate', 'chrome']]
assert len(before) == len(after) == len(reference) == 90
assert all(b[:-2] == a[:-2] == c[:-2] for b, a, c in zip(before, after, reference))
print('direct geometry before/after exact agreement:', sum(b == c for b, c in zip(before, reference)),
      sum(a == c for a, c in zip(after, reference)))
assert all(a == b or a == c for b, a, c in zip(before, after, reference))
