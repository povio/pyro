"""Print exact named differences and per-page counts from the three focused runs."""

import gzip
import json
from pathlib import Path

root = Path(__file__).resolve().parent
urls = json.loads((root / 'urls.json').read_text())
runs = {}
for label in ['before', 'after', 'chrome']:
    with gzip.open(root / f'{label}.json.gz', 'rt') as handle:
        run = json.load(handle)
    assert run['wptRevision'] == 'a421f2fea5eb7f91e0db399e44b0fb0779ee1603'
    assert sorted(run['plan']) == sorted(urls) and run['jobs'] == 2
    assert sorted(row['url'] for row in run['results']) == sorted(urls)
    runs[label] = {row['url']: row for row in run['results']}

report = {}
for url in urls:
    passed = []
    for row in [runs[label][url] for label in runs]:
        assert row['status'] == 'complete' and row['harnessStatus'] == 0
        names = {case['name'] for case in row['subtests']}
        assert len(names) == len(row['subtests'])
        if passed:
            assert names == previous_names
        previous_names = names
        passed.append({case['name'] for case in row['subtests'] if case['status'] == 'Pass'})
    before, after, chrome = passed
    report[url] = {
        'total': len(names), 'passes_before_after_chrome': [len(items) for items in passed],
        'gains_also_chrome': sorted((after - before) & chrome),
        'gains_lp_only': sorted(after - before - chrome),
        'losses_chrome_passes': sorted((before - after) & chrome),
        'losses_lp_only': sorted(before - after - chrome),
        'after_chrome_only': sorted(chrome - after), 'after_lp_only': sorted(after - chrome),
    }
print(json.dumps(report, indent=2))
