import gzip
import json
from pathlib import Path


def read(name):
    path = Path(__file__).with_name(name + '.json')
    if path.exists():
        return json.loads(path.read_text())
    with gzip.open(str(path) + '.gz') as stream:
        return json.load(stream)


urls = read('urls')
outcomes = {'chrome': read('chrome')}
for label in ('parent', 'candidate'):
    report = read(label)
    assert set(report['plan']) == set(urls) and len(report['results']) == len(urls)
    assert report['restarts'] == report['watchdogKills'] == 0
    outcomes[label] = {row['url']: [row['status'], row['harnessStatus'],
                       [[test['name'], test['status']] for test in row['subtests']]] for row in report['results']}


def sets(rows):
    assert set(rows) == set(urls)
    assert all(row[:2] == ['complete', 0] and row[2] for row in rows.values())
    return ({url for url, row in rows.items() if all(status == 'Pass' for _, status in row[2])},
            {(url, name) for url, row in rows.items() for name, status in row[2] if status == 'Pass'},
            {(url, name) for url, row in rows.items() for name, _ in row[2]})


candidate = sets(outcomes['candidate'])
result = {}
for label in ('parent', 'chrome'):
    reference = sets(outcomes[label])
    assert candidate[2] == reference[2]
    result[label] = {kind: {'gain': sorted(after - before), 'loss': sorted(before - after)}
                     for kind, after, before in zip(('pages', 'names'), candidate, reference)}
print(json.dumps(result, indent=2))
