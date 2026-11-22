import gzip
import json
from pathlib import Path

root = Path(__file__).resolve().parent
prior = root.parent / 'round7-overflow-geometry'
urls = json.loads((prior / 'urls.json').read_text())


def read(label, directory=root):
    with gzip.open(directory / (label + '.json.gz'), 'rt') as stream:
        return json.load(stream)


def named(label):
    report = read(label)
    assert report['wptRevision'] == 'a421f2fea5eb7f91e0db399e44b0fb0779ee1603'
    assert report['watchdogKills'] == report['restarts'] == 0
    assert sorted(row['url'] for row in report['results']) == urls
    result = {}
    for row in report['results']:
        assert row['status'] == 'complete' and row['harnessStatus'] == 0
        for sub in row['subtests']:
            key = row['url'], sub['name']
            assert key not in result
            result[key] = sub['status']
    return result


base, candidate = named('base'), named('candidate')
assert named('base-repeat') == base and named('candidate-repeat') == candidate
chrome = read('chrome-wpt')
assert chrome == read('chrome-wpt', prior)
reference = {(url, name): status for url, (harness, tests) in chrome.items() if harness == 0
             for name, status, _ in tests}
assert len(base) == 958 and base.keys() == candidate.keys() == reference.keys()
changes = [key for key in base if base[key] != candidate[key]]
assert len(changes) == 24
for key in sorted(changes):
    page, name = key
    assert page.endswith('/scrollWidthHeight-negative-margin-002.html') and name.startswith('scrollWidth ')
    assert 'display: flex;' in name and ('overflow: visible;' in name or 'overflow: clip;' in name)
    assert (base[key], candidate[key], reference[key]) == ('Fail', 'Pass', 0)
    print(name)
before, after, chrome = [read(label + '-probe') for label in ['base', 'candidate', 'chrome']]
assert len(before) == len(after) == len(chrome) == 810
assert before[:90] == read('candidate-probe', prior) and chrome[:90] == read('chrome-probe', prior)
assert all(b[:-2] == a[:-2] == c[:-2] and b[-1] == a[-1] and (b == a or a == c)
           for b, a, c in zip(before, after, chrome))
assert sum(b != a for b, a in zip(before, after)) == 76
assert all(b == a for b, a in zip(before[90:], after[90:]) if b[4] == 'hidden' or b[5] == 'fits')
