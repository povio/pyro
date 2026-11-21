"""Report directional assertion differences for two LP baselines and pinned Chrome."""

import gzip
import json
from pathlib import Path


def read_run(root, label, urls):
    with gzip.open(root / f'{label}.json.gz') as handle:
        run = json.load(handle)
    assert set(run['plan']) == set(urls) and run['jobs'] == 2
    assert run['wptRevision'] == 'a421f2fea5eb7f91e0db399e44b0fb0779ee1603'
    assert sorted(row['url'] for row in run['results']) == sorted(urls)
    result = {}
    for row in run['results']:
        assert (row['status'], row['harnessStatus']) == ('complete', 0)
        names = {case['name']: case['status'] for case in row['subtests']}
        assert len(names) == len(row['subtests'])
        result[row['url']] = names
    return result


root = Path(__file__).resolve().parent
urls = json.loads((root / 'urls.json').read_text())
runs = {label: read_run(root, label, urls) for label in ('before', 'previous', 'split', 'chrome')}
report = {}
for url in urls:
    matrices = {label: run[url] for label, run in runs.items()}
    assert all(matrix.keys() == matrices['before'].keys() for matrix in matrices.values())
    passed = {label: {name for name, status in matrix.items() if status == 'Pass'}
              for label, matrix in matrices.items()}
    candidate, chrome = passed['split'], passed['chrome']
    page = {'observed': len(matrices['before']), 'passed': {key: len(value) for key, value in passed.items()}}
    for label in ('before', 'previous'):
        gains, losses = candidate - passed[label], passed[label] - candidate
        page[label] = {'gain_chrome': sorted(gains & chrome), 'gain_lp_only': sorted(gains - chrome),
                       'loss_chrome': sorted(losses & chrome), 'loss_lp_only': sorted(losses - chrome)}
    page['recovered_prior_losses'] = sorted((passed['before'] - passed['previous']) & candidate)
    page['split_only_vs_chrome'] = sorted(candidate - chrome)
    page['chrome_only_vs_split'] = sorted(chrome - candidate)
    report[url] = page
print(json.dumps(report, indent=2))
