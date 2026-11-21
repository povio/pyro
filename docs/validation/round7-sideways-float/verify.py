import gzip
import json
from pathlib import Path

root = Path(__file__).parent


def read(name):
    with gzip.open(root / (name + '.json.gz')) as stream:
        return json.load(stream)


def statuses(name):
    run = read(name)
    assert run['wptRevision'] == 'a421f2fea5eb7f91e0db399e44b0fb0779ee1603'
    assert run['restarts'] == 0 and len(run['results']) == 15
    assert all(row['status'] == 'complete' and row['harnessStatus'] == 0 for row in run['results'])
    return {(row['url'], test['name']): test['status'] for row in run['results'] for test in row['subtests']}


base, candidate = statuses('base'), statuses('candidate')
assert base == statuses('base-repeat') and candidate == statuses('candidate-repeat')
signed = {(url, name): value for url, (state, harness, tests) in json.loads((root / 'chrome.json').read_text()).items()
          if (state, harness) == ('complete', 0) for name, value in tests}
assert base.keys() == signed.keys() == candidate.keys() and len(base) == 91
changed = {key for key in base if base[key] != candidate[key]}
assert changed == {key for key in base if 'scrollIntoView-sideways-lr-' in key[0]} and len(changed) == 18
assert all(base[key] == 'Fail' and candidate[key] == signed[key] == 'Pass' for key in changed)
for url, name in sorted(changed):
    print('recovered', url, name)
before, after, chrome = (read(label + '-static') for label in ('base', 'candidate', 'chrome'))
assert len(before) == len(after) == 100 and after == chrome
assert sum(row == reference for row, reference in zip(before, chrome)) == 60
assert all(row == reference for row, reference in zip(before, chrome) if row['side'] == 'none')
direct = read('candidate-probe')
assert len(direct) == 160 and direct == read('../round7-scroll-logical/chrome-probe') == read('../round7-scroll-logical/candidate-probe')
print('18 gains; 0 losses; 73 unchanged. Static matches: 60 -> 100. Positioned controls: 160 unchanged.')
