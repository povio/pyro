import gzip
import json
import runpy
import subprocess
from pathlib import Path


root = Path(__file__).resolve().parent
helpers = runpy.run_path(str(root / 'batch1/compare.py'))
read, save, sets = (helpers[key] for key in ('read', 'save', 'sets'))
out = root / 'batch15'
prefix = '3961664ab48bd449c696ab56ac8fd0c2aa6b8416:docs/validation/round7-css-transition-reversal/'


def receipt(name):
    blob = subprocess.check_output(['git', 'show', prefix + name])
    return json.loads(gzip.decompress(blob) if name.endswith('.gz') else blob)


urls = receipt('urls.json')
focused = {'chrome': receipt('chrome.json.gz')}
for label in ('parent', 'candidate'):
    raw = receipt(label + '.json.gz')
    assert sorted(raw['plan']) == sorted(urls) == sorted(row['url'] for row in raw['results'])
    focused[label] = {row['url']: [row['status'], row['harnessStatus'], [[sub['name'], sub['status']] for sub in row['subtests']]] for row in raw['results']}


def statuses(outcomes):
    result = {(url, name): value for url, row in outcomes.items() for name, value in row[2]}
    assert len(result) == sum(len(row[2]) for row in outcomes.values())
    return result


full = [read(root / path) for path in ('batch14/outcomes-main.json.gz', 'batch15/outcomes-main.json.gz', 'frozen/outcomes-chrome-main.json.gz')]
for label, outcomes in zip(('parent', 'candidate', 'chrome'), full):
    subset = {url: outcomes[url] for url in urls}
    assert statuses(subset) == statuses(focused[label])
    assert all(subset[url][:2] == focused[label][url][:2] == ['complete', 0] for url in urls)
parent, candidate = (sets(focused[label]) for label in ('parent', 'candidate'))
delta = read(out / 'difference-main-vs-batch14.json.gz')
directions = receipt('directions.json')['parent']
for key, label in [('pages', 'pages'), ('passes', 'names')]:
    gains = json.loads(json.dumps(sorted(candidate[key] - parent[key])))
    assert len(gains) == 2 and gains == directions[label]['gain']
    assert not parent[key] - candidate[key] and not directions[label]['loss']
    assert all(item in delta[key]['candidateOnly'] for item in gains)
before, after, chrome = map(statuses, full)
expected = [tuple(item) for item in directions['names']['gain']]
assert all((before[item], after[item], chrome[item]) == ('Fail', 'Pass', 'Pass') for item in expected)
for cohort in ('main', 'supplement'):
    first, second = [read(root / f'batch{batch}/outcomes-{cohort}.json.gz') for batch in (14, 15)]
    left, right = statuses(first), statuses(second)
    changed = [list(item) + [left.get(item), right.get(item)] for item in sorted(left.keys() | right.keys()) if left.get(item) != right.get(item)]
    states = [[url, first[url][:2], second[url][:2]] for url in sorted(first) if first[url][:2] != second[url][:2]]
    save(out / f'status-changes-{cohort}.json', dict(assertions=changed, pages=states))
save(out / 'focused-map.json', dict(columns=['url', 'name', 'batch14', 'batch15', 'pinnedChrome'], gains=[[*item, before[item], after[item], chrome[item]] for item in expected]))
