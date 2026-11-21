import gzip
import json
import runpy
import subprocess
from pathlib import Path


root = Path(__file__).resolve().parent
helpers = runpy.run_path(str(root / 'batch1/compare.py'))
read, save = helpers['read'], helpers['save']
receipt = 'f3ba721f:docs/validation/studio-round7-flex-column-synthesis/outcomes.json.gz'
focused = json.loads(gzip.decompress(subprocess.check_output(['git', 'show', receipt])))


def changes(before, after):
    assert before.keys() == after.keys()
    result = []
    for url, (status, harness, assertions) in before.items():
        assert after[url][:2] == [status, harness]
        left, right = dict(assertions), dict(after[url][2])
        assert left.keys() == right.keys() and len(left) == len(assertions) and len(right) == len(after[url][2])
        result.extend([url, name, value, right[name]] for name, value in left.items() if value != right[name])
    return sorted(result)


expected = changes(focused['parent'], focused['candidate'])
assert len(expected) == 4 and len({row[0] for row in expected}) == 4
for cohort in ('main', 'supplement'):
    actual = changes(*[read(root / f'batch{batch}/outcomes-{cohort}.json.gz') for batch in (13, 14)])
    assert actual == (expected if cohort == 'main' else [])
chrome = read(root / 'frozen/outcomes-chrome-main.json.gz')
for url, name, before, after in expected:
    assert before == 'Fail' and after == dict(focused['chrome'][url][2])[name] == dict(chrome[url][2])[name] == 'Pass'
save(root / 'batch14/focused-map.json', {'columns': ['url', 'name', 'batch13', 'batch14', 'pinnedChrome'],
                                         'gains': [row + ['Pass'] for row in expected]})
