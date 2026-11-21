import collections
import json
from pathlib import Path


def outcomes(label):
    receipt = json.loads(Path(label + '.json').read_text())
    rows = receipt['results']
    assert sorted(row['url'] for row in rows) == json.loads(Path('urls.json').read_text())
    assert receipt['restarts'] == receipt['watchdogKills'] == 0
    passes, pages = set(), set()
    for row in rows:
        counts = collections.Counter()
        assert row['status'] == 'complete' and row['harnessStatus'] == 0
        for subtest in row['subtests']:
            name = subtest['name']
            counts[name] += 1
            if subtest['status'] == 'Pass':
                passes.add((row['url'], name, counts[name]))
        if counts and all(subtest['status'] == 'Pass' for subtest in row['subtests']):
            pages.add(row['url'])
    return passes, pages


base, candidate, chrome = map(outcomes, ['base', 'candidate', 'chrome'])
assert outcomes('repeat') == candidate
report = {}
for kind, before, after, reference in zip(['named', 'pages'], base, candidate, chrome):
    report[kind] = dict(base=len(before), candidate=len(after), chrome=len(reference),
                        gained=sorted(after - before), lost=sorted(before - after),
                        chrome_only=sorted(reference - after), candidate_only=sorted(after - reference))
print(json.dumps(report, indent=2))
