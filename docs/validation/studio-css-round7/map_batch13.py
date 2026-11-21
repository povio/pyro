import gzip
import json
import runpy
import subprocess
from pathlib import Path


root = Path(__file__).resolve().parent
helpers = runpy.run_path(str(root / 'batch1/compare.py'))
read, save = (helpers[key] for key in ('read', 'save'))


def focused(label):
    path = f'ce951bbf:docs/validation/round7-conic-gradient-stops/{label}.json.gz'
    report = json.loads(gzip.decompress(subprocess.check_output(['git', 'show', path])))
    result = {}
    for row in report['results']:
        assert row['status'] == 'complete' and row['harnessStatus'] == 0
        for sub in row['subtests']:
            key = row['url'], sub['name']
            assert key not in result
            result[key] = sub['status']
    return result


before, after = focused('base'), focused('candidate')
assert before == focused('base-repeat') and after == focused('candidate-repeat')
assert before.keys() == after.keys() and len(after) == 2785
changed = sorted(key for key in after if after[key] != before[key])
assert len(changed) == 6 and all((before[key], after[key]) == ('Fail', 'Pass') for key in changed)
full = [read(root / path) for path in ('batch12/outcomes-main.json.gz', 'batch13/outcomes-main.json.gz', 'frozen/outcomes-chrome-main.json.gz')]
rows = [[url, name, *[dict(outcomes[url][2]).get(name) for outcomes in full]] for url, name in changed]
assert all(row[2:] == ['Fail', 'Pass', 'Pass'] for row in rows)
delta = read(root / 'batch13/difference-main-vs-batch12.json.gz')
assert sorted(map(tuple, delta['passes']['candidateOnly'])) == changed
save(root / 'batch13/focused-map.json', {'columns': ['url', 'name', 'batch12', 'batch13', 'pinnedChrome'], 'gains': rows})
