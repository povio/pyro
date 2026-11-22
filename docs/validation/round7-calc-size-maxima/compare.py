import gzip
import json
import subprocess
from collections import Counter
from pathlib import Path


def retained(ref):
    return json.loads(gzip.decompress(subprocess.check_output(['git', 'show', ref])))


def index(rows):
    seen = Counter()
    output = {}
    for name, status in rows:
        seen[name] += 1
        output[name, seen[name]] = status
    return output


def pages(run):
    return {p['url']: index((r['name'], r['status']) for r in p['subtests']) for p in run['results']}


raw = json.loads(gzip.decompress(Path('candidate.json.gz').read_bytes()))
assert all(p['status'] == 'complete' and p['harnessStatus'] in (0, 'OK') for p in raw['results'])
candidate = pages(raw)
parent = pages(retained('b82e0cd35:docs/validation/round7-sampled-percent-endpoint/candidate.json.gz'))
reference = retained('5980e405b:docs/validation/studio-css-e4ca26/outcomes-stable.json.gz')
assert set(candidate) == set(json.loads(Path('urls.json').read_text()))
report = {}
for url, after in candidate.items():
    before, signed = parent[url], index(reference[url][2])
    assert before.keys() == after.keys() == signed.keys()
    changes = [dict(name=k[0], occurrence=k[1], before=before[k], after=v, signed=signed[k]) for k, v in after.items() if v != before[k]]
    report[url] = dict(total=len(after), before=sum(v=='Pass' for v in before.values()), after=sum(v=='Pass' for v in after.values()), signed=sum(v=='Pass' for v in signed.values()), changes=changes)
print(json.dumps(report, indent=1))
