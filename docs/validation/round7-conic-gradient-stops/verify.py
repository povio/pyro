import gzip
import json
from pathlib import Path

root = Path(__file__).resolve().parent


def read(name):
    path = root / name
    with (gzip.open(path, 'rt') if name.endswith('.gz') else path.open()) as stream:
        return json.load(stream)


def outcomes(label):
    report = read(label + '.json.gz')
    assert report['wptRevision'] == 'a421f2fea5eb7f91e0db399e44b0fb0779ee1603'
    assert report['watchdogKills'] == report['restarts'] == 0
    assert sorted(read('urls.json')) == sorted(row['url'] for row in report['results'])
    result = {}
    for row in report['results']:
        assert row['status'] == 'complete' and row['harnessStatus'] == 0
        for sub in row['subtests']:
            key = row['url'], sub['name']
            assert key not in result
            result[key] = sub['status']
    return result


base, candidate = outcomes('base'), outcomes('candidate')
assert base == outcomes('base-repeat') and candidate == outcomes('candidate-repeat')
assert base.keys() == candidate.keys() and len(base) == 2785
chrome = read('chrome-wpt.json.gz')
changed = [key for key in base if base[key] != candidate[key]]
assert len(changed) == 6
for url, name in changed:
    assert url.endswith('/conic-gradient-calc-angle-percentage-valid.html')
    assert base[url, name] == 'Fail' and candidate[url, name] == 'Pass'
    assert chrome[url][:2] == ['complete', 0] and dict(chrome[url][2])[name] == 'Pass'
    print(name)
controls = read('candidate-cdp.json')
assert controls == read('chrome-controls.json') and len(controls) == 8
assert [(row[0], row[2]) for row in controls] == [(row[0], row[2]) for row in read('base-cdp.json')]
triage = [row for row in read('scroll-state-triage.json.gz')['results'] if '/scroll-state/' in row['url']]
assert len(triage) == 4
for row in triage:
    assert row['harnessStatus'] == 1 and [sub['name'] for sub in row['subtests']] == ['no test suite completion']
    assert sum(status == 'Pass' for _, status in chrome[row['url']][2]) == 5
print('6 Chrome-matched recoveries; no other status changes. All 8 direct controls match Chrome.')
