import gzip
import json
from collections import Counter
from pathlib import Path


root = Path(__file__).parent
urls = set(json.loads((root / 'urls.json').read_text()))
targets = {url for url in urls if 'dashed-function-parsing' in url or 'dashed-function-named-arg' in url}
revision = 'a421f2fea5eb7f91e0db399e44b0fb0779ee1603'


def load(label):
    with gzip.open(root / f'{label}.json.gz') as source:
        return json.load(source)


def outcomes(label, manifest):
    report = load(label)
    assert report['wptRevision'] == revision
    assert report['jobs'] == 2 and report['testdriver'] and not report['reuseContext']
    rows = report['results']
    assert len(rows) == len(manifest) and {row['url'] for row in rows} == manifest
    assert all(row['status'] == 'complete' and row['harnessStatus'] == 0 for row in rows)
    result = {(row['url'], sub['name']): sub['status'] for row in rows for sub in row['subtests']}
    assert len(result) == sum(len(row['subtests']) for row in rows)
    return result


before = outcomes('integrated', urls)
after = outcomes('candidate', urls)
assert before == outcomes('baseline', urls) == outcomes('integrated-repeat', urls)
assert after == outcomes('candidate-repeat', urls)
assert before.keys() == after.keys()
parent = outcomes('pre', targets)
first = outcomes('first', targets)
assert first.keys() == parent.keys() == {key for key in before if key[0] in targets}
chrome = {(url, name): status for url, row in load('chrome').items() for name, status in row[2]}
changes = {key for key in before if before[key] != after[key]}
assert changes == {key for key in first if parent[key] == 'Pass' and first[key] == 'Fail'}
assert sorted(Counter(url for url, name in changes).values()) == [6, 10]
assert all(before[key] == 'Fail' and after[key] == chrome[key] == 'Pass' for key in changes)
assert all(after[key] == 'Pass' for key in parent)
for url, name in sorted(changes):
    print(f'{url}\t{name}\t{parent[url, name]} -> {first[url, name]} -> {after[url, name]} (Chrome Pass)')
print(f'{len(changes)} recoveries; {len(before) - len(changes)} unchanged named outcomes.')
