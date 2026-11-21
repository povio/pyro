import gzip
import json
from collections import Counter
from pathlib import Path


def load(label):
    path = Path(__file__).with_name(label + '.json')
    if path.exists():
        return json.loads(path.read_text())
    with gzip.open(str(path) + '.gz') as stream:
        return json.load(stream)


urls = set(load('urls'))


def outcomes(label):
    raw = load(label)
    if 'results' in raw:
        assert set(raw['plan']) == urls and not raw['watchdogKills'] and not raw['restarts']
        raw = {r['url']: r for r in raw['results']}
    assert set(raw) == urls
    names, pages, observed = set(), set(), Counter()
    for url, row in raw.items():
        if isinstance(row, dict):
            row = [row['status'], row['harnessStatus'], [(s['name'], s['status']) for s in row['subtests']]]
        status, harness, subs = row
        assert status == 'complete' and harness == 0 and subs
        observed.update((url, name) for name, _ in subs)
        names.update((url, name) for name, result in subs if result == 'Pass')
        if all(result == 'Pass' for _, result in subs):
            pages.add(url)
    return pages, names, observed


candidate = outcomes('candidate')
directions = {}
for label in ['parent', 'batch14', 'batch15', 'chrome']:
    other = outcomes(label)
    assert candidate[2] == other[2]
    directions[label] = {kind: {'gain': sorted(new - old), 'loss': sorted(old - new)}
                         for kind, new, old in zip(['pages', 'names'], candidate, other)}
print(json.dumps(directions, indent=2))
