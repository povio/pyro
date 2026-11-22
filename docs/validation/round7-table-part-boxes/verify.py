import gzip
import json
from pathlib import Path


root = Path(__file__).resolve().parent
urls = json.loads((root / 'urls.json').read_text())
revision = 'a421f2fea5eb7f91e0db399e44b0fb0779ee1603'


def load(label):
    with gzip.open(root / (label + '.json.gz'), 'rt') as stream:
        return json.load(stream)


def outcomes(label):
    report = load(label)
    assert report['wptRevision'] == revision and report['jobs'] == 2
    assert report['restarts'] == report['watchdogKills'] == 0
    assert sorted(report['plan']) == sorted(urls) == sorted(row['url'] for row in report['results'])
    by_url = {}
    for row in report['results']:
        assert row['status'] == 'complete' and row['harnessStatus'] == 0
        names = {sub['name']: sub['status'] for sub in row['subtests']}
        assert len(names) == len(row['subtests'])
        by_url[row['url']] = names
    return by_url


base, final = outcomes('base'), outcomes('final')
assert base == outcomes('base-repeat') and final == outcomes('final-repeat')
chrome = load('chrome')
assert len(base) == len(set(urls)) == 138 and set(chrome) == set(base)
counts = []
for run in (base, final):
    counts.append((sum(all(status == 'Pass' for status in names.values()) for names in run.values()),
                   sum(status == 'Pass' for names in run.values() for status in names.values())))
assert counts == [(94, 638), (97, 660)] and sum(map(len, base.values())) == 822
confirmed = 0
for url in urls:
    assert base[url].keys() == final[url].keys()
    for name, status in base[url].items():
        if final[url][name] != status:
            assert status == 'Fail' and final[url][name] == 'Pass'
            assert chrome[url][:2] == ['complete', 0]
            reference = dict(chrome[url][2])[name]
            confirmed += reference == 'Pass'
            print(f'{url}\t{name}\tFail → Pass\tChrome {reference}')
assert confirmed == 21
print(f'{counts}; 21 Chrome-confirmed gains, 1 additional WPT gain; no losses')
