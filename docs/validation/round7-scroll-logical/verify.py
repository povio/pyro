import gzip
import json
from pathlib import Path

root = Path(__file__).parent


def read(label):
    with gzip.open(root / f'{label}.json.gz') as stream:
        return json.load(stream)


def outcomes(label):
    run = read(label)
    assert run['wptRevision'] == 'a421f2fea5eb7f91e0db399e44b0fb0779ee1603'
    assert run['restarts'] == 0 and all(row['status'] == 'complete' and row['harnessStatus'] == 0 for row in run['results'])
    return {(row['url'], test['name']): test['status'] for row in run['results'] for test in row['subtests']}


before, after = outcomes('base'), outcomes('candidate')
assert before == outcomes('base-repeat') and after == outcomes('candidate-repeat')
chrome = json.loads((root / 'chrome.json').read_text())
reference = {(url, name): status for url, (state, harness, tests) in chrome.items() for name, status in tests if (state, harness) == ('complete', 0)}
assert before.keys() == after.keys() == reference.keys() and len(before) == 87
focus = {key for key in before if 'writing-mode' in key[0] and before[key] == 'Fail' and reference[key] == 'Pass'}
changed = {key for key in before if before[key] != after[key]}
assert len(focus) == 60 and len({url for url, name in focus}) == 8
assert len(changed) == 42 and changed <= focus and all(after[key] == 'Pass' for key in changed)
assert all('sideways-lr' in url for url, name in focus - changed)
for url, name in sorted(focus):
    print('recovered' if (url, name) in changed else 'remaining', url, name)
base, candidate, signed = (read(label + '-probe') for label in ('base', 'candidate', 'chrome'))
assert len(candidate) == 160 and candidate == signed
assert len(base) == len(candidate) and all(old[:-1] == new[:-1] for old, new in zip(base, candidate))
print('direct controls matching Chrome:', sum(old == ref for old, ref in zip(base, signed)), '->', len(candidate))
