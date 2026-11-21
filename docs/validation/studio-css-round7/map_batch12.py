import gzip
import json
import runpy
import subprocess
from pathlib import Path


root = Path(__file__).resolve().parent
helpers = runpy.run_path(str(root / 'batch1/compare.py'))
read, sets, save = (helpers[key] for key in ('read', 'sets', 'save'))


def blob(revision, folder, name):
    raw = subprocess.check_output(['git', 'show', f'{revision}:docs/validation/{folder}/{name}.json.gz'])
    return json.loads(gzip.decompress(raw))


table = {}
for label in ('base', 'final', 'base-repeat', 'final-repeat'):
    report = blob('1c879c43', 'round7-table-part-boxes', label)
    table[label] = {row['url']: [row['status'], row['harnessStatus'], [[sub['name'], sub['status']] for sub in row['subtests']]] for row in report['results']}
assert table['base'] == table['base-repeat'] and table['final'] == table['final-repeat']
flex = blob('897d8cc0', 'studio-round7-flex-column-sharing', 'outcomes')
prior, current, chrome = [read(root / path) for path in ('batch11/outcomes-main.json.gz', 'batch12/outcomes-main.json.gz', 'frozen/outcomes-chrome-main.json.gz')]
mapping = {}
for key, baseline, candidate, count in [('table', table['base'], table['final'], 22), ('flex', flex['parent'], flex['candidate'], 7)]:
    assert baseline.keys() == candidate.keys() and baseline.keys() <= current.keys()
    before, after = sets(baseline), sets(candidate)
    assert not before['passes'] - after['passes'] and before['observed'] == after['observed']
    gains = sorted(after['passes'] - before['passes'])
    assert len(gains) == count
    mapping[key] = [[url, name, *[dict(data[url][2]).get(name) for data in (prior, current, chrome)]] for url, name in gains]
save(root / 'batch12/focused-map.json', {'columns': ['url', 'name', 'batch11', 'batch12', 'pinnedChrome'], 'gains': mapping})
