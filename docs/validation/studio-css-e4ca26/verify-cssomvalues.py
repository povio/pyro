import gzip
import json
from pathlib import Path


here = Path(__file__).resolve().parent
values = here.parent / 'css-values-a7f8d09e'
first = json.loads((values / 'evidence.json').read_text())
second = json.loads((values / 'conditional-keyframes-fonts/differential.json').read_text())
targets = {tuple(entry) for entry in first['gained'] + second['matchedInventoryGains']}
assert len(targets) == 299

for label, expected in [('mixmath', 27), ('cssomnames', 27), ('cssomvalues', 299), ('csspriority', 299)]:
    with gzip.open(here / f'outcomes-{label}-main-candidate.json.gz', 'rt') as stream:
        outcomes = json.load(stream)
    assert len(outcomes) == 8082
    passing = {(url, name) for url, (_, _, tests) in outcomes.items() for name, status in tests if status == 'Pass'}
    recovered = targets & passing
    assert len(recovered) == expected, (label, len(recovered), sorted(targets - recovered))
    print(f'{label}: {len(recovered)}/{len(targets)} original Chrome-only names passing')
